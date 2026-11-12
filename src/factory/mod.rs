//! Factories: conveyors that move items between containers, and machines on an electric grid.
//!
//! **What there is.**
//! - **Conveyors** (`Block::Conveyor`) carry items the way their arrow points. Whatever is behind
//!   one (a chest, a furnace's or a machine's output) is emptied onto it, and whatever is in front
//!   of it (another conveyor, a chest, a furnace, a machine) takes what reaches its end. So one
//!   conveyor between two containers moves items from one to the other, and a line of them is a
//!   belt. A conveyor only takes what the end of its line can use (fuel for a generator, ore for a
//!   furnace), and a conveyor running into the side of another feeds onto its middle.
//! - **Power:** a coal generator burns fuel. Cables join poles and machines into networks, and the
//!   machines on a network share what its generators make: all of them slow down alike when there
//!   isn't enough, and generators only burn what is used. An electric furnace smelts twice as fast
//!   as a furnace, and gets twice as much out of the coal.
//!
//! **Scale.** A factory lives in flat arrays, apart from the world's chunks: it works wherever it
//! is, loaded or not, and on the world you aren't on too. How the parts connect is worked out when
//! something is built or taken away (`relink`), never per tick. Conveyors are stepped downstream
//! first (an item can go on the tick it arrives), and one that is empty with nothing behind it to
//! take from costs a single check. Ticks are `TICK` long; under time warp they grow to `MAX_TICK`
//! and run within a time budget per frame (`BUDGET`): past that the factory falls behind (at most
//! `MOST_BEHIND`) rather than slow the game down.
//!
//! **Who decides** (multiplayer): only where `authority` is set (offline, and the host) do items go
//! into and out of containers. Everywhere else conveyors only carry items along, and the host sends
//! what is on the conveyors near each player (`belts_near`) and the containers it has changed
//! (`changed`). Machines run everywhere, as furnaces do.

mod draw;
pub mod recipes;

use std::time::{Duration, Instant};

use glam::DVec3;
use serde::{Deserialize, Serialize};

pub use draw::Art;

use crate::{
    render::models,
    survival::{
        furnace::Furnace,
        inventory::{self, Slot},
        items::{Item, ItemStack},
    },
    ui::screen::CHEST_SLOTS,
    util::FastMap,
    world::{block::Block, chunk::BlockCell, planet::Planet},
};

/// Seconds per factory step, and the longest step taken when catching up (time warp).
pub const TICK: f64 = 0.05;
const MAX_TICK: f64 = 0.1;
/// CPU time a factory may spend catching up in one frame, and how far it may fall behind (s).
const BUDGET: Duration = Duration::from_micros(2500);
const MOST_BEHIND: f64 = 60.0;

/// Conveyor lengths an item travels per second, and how many items fit on one conveyor.
pub const BELT_SPEED: f32 = 1.6;
pub const BELT_ITEMS: usize = 4;
const SPACING: f32 = 1.0 / BELT_ITEMS as f32;

/// What a generator makes while it has fuel, used or not (W).
pub const GENERATOR_POWER: f32 = 180_000.0;
/// Energy in fuel (J) per second it would burn in a furnace: coal makes 7.2 MJ.
pub const FUEL_ENERGY: f32 = 90_000.0;
/// Seconds an electric furnace takes per item (a furnace takes 10).
pub const SMELT_TIME: f32 = 5.0;
/// What an accumulator holds (J), and how fast it takes or gives power (W).
pub const ACCUMULATOR_ENERGY: f32 = 5_000_000.0;
pub const ACCUMULATOR_POWER: f32 = 300_000.0;
/// Seconds a miner takes per item, and an oil pump per barrel.
pub const MINE_TIME: f32 = 2.5;
pub const PUMP_TIME: f32 = 3.0;
/// How far below an oil pump it finds oil shale (layers), and around it (columns each way).
pub const PUMP_DEPTH: i32 = 24;
/// How far below a miner it digs (layers), and around it (columns each way).
pub const MINER_DEPTH: i32 = 8;
pub const MINE_REACH: i32 = 1;
/// Seconds of power history a network keeps (one sample a second).
pub const HISTORY: usize = 60;
/// The longest a cable may be (m).
pub const CABLE_REACH: f64 = 24.0;

/// The way a conveyor facing 0..3 runs, in grid steps (u, v): the model's front.
pub const AHEAD: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// An item riding on a conveyor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeltItem {
    pub stack: ItemStack,
    /// How far along (0 at the back edge, 1 at the front edge).
    pub at: f32,
}

/// Where a conveyor is, for drawing and the network (planet-relative).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    center: DVec3,
    /// Where items come on (the middle of the back edge, or of the side a bend comes round
    /// from) and go off (the middle of the front edge), at belt height, and up.
    start: DVec3,
    end: DVec3,
    up: DVec3,
    /// A bend's corner: the path is the curve from `start` to `end` that this pulls towards
    /// (a quadratic Bezier, close to a quarter circle).
    corner: Option<DVec3>,
}

impl Place {
    /// Where an item `t` along (0..1) is, and which way it is going.
    fn at(&self, t: f64) -> (DVec3, DVec3) {
        match self.corner {
            None => (
                self.start + (self.end - self.start) * t,
                self.end - self.start,
            ),
            Some(c) => {
                let (s, e) = (self.start, self.end);
                let point = s * (1.0 - t) * (1.0 - t) + c * 2.0 * (1.0 - t) * t + e * t * t;
                let along = (c - s) * 2.0 * (1.0 - t) + (e - c) * 2.0 * t;
                (point, along)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conveyor {
    pub cell: BlockCell,
    /// The model's facing (`ModelState::facing`): items run towards its front.
    pub facing: u8,
    pub front: BlockCell,
    pub back: BlockCell,
    /// A bend (`ModelState::bend`): 0 straight, 1 or 2 coming round from the model's +x or -x
    /// side, when the only conveyor feeding it is there (worked out in `relink`).
    #[serde(skip)]
    pub bend: u8,
    /// The cells on its model's +x and -x sides (once placed in space: `locate`).
    #[serde(skip)]
    sides: Option<[BlockCell; 2]>,
    /// What it carries, furthest along first, at least `SPACING` apart.
    pub items: [Option<BeltItem>; BELT_ITEMS],
    #[serde(skip)]
    next: Link,
    #[serde(skip)]
    source: Link,
    #[serde(skip)]
    wants: Want,
    /// Its items moved in the last tick (drawing carries them on between ticks).
    #[serde(skip)]
    moving: bool,
    #[serde(skip)]
    place: Option<Place>,
    /// The sky light estimate at it (costly: terrain sampling).
    #[serde(skip)]
    sky: Option<u32>,
}

impl Conveyor {
    fn new(planet: &Planet, cell: BlockCell, facing: u8) -> Self {
        let facing = facing % 4;
        Self {
            cell,
            facing,
            front: neighbour(planet, cell, facing, 1),
            back: neighbour(planet, cell, facing, -1),
            bend: 0,
            sides: Some(sides(planet, cell, facing)),
            items: [None; BELT_ITEMS],
            next: Link::None,
            source: Link::None,
            wants: Want::Anything,
            moving: false,
            place: None,
            sky: None,
        }
    }

    pub fn len(&self) -> usize {
        self.items.iter().take_while(|i| i.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.items[0].is_none()
    }

    pub fn carried(&self) -> impl Iterator<Item = &BeltItem> {
        self.items.iter().flatten()
    }

    /// Puts an item on at `at`, if there is room for it there.
    fn insert(&mut self, stack: ItemStack, at: f32) -> bool {
        let n = self.len();
        if n == BELT_ITEMS {
            return false;
        }
        let k = self.items[..n]
            .iter()
            .position(|i| i.is_some_and(|i| i.at < at))
            .unwrap_or(n);
        let ahead = k > 0 && self.items[k - 1].is_some_and(|i| i.at < at + SPACING);
        let behind = k < n && self.items[k].is_some_and(|i| i.at > at - SPACING);
        if ahead || behind {
            return false;
        }
        self.items[k..=n].rotate_right(1);
        self.items[k] = Some(BeltItem {
            stack: ItemStack { count: 1, ..stack },
            at,
        });
        true
    }

    /// Puts an item on at the back, `at` along or as far as the last item allows.
    fn insert_behind(&mut self, stack: ItemStack, at: f32) -> bool {
        let n = self.len();
        let room = match n {
            0 => at,
            BELT_ITEMS => return false,
            _ => self.items[n - 1].map_or(at, |last| at.min(last.at - SPACING - 1e-5)),
        };
        room >= 0.0 && self.insert(stack, room)
    }

    /// Where items come onto it from: the cell behind, or the side a bend comes round from.
    fn entry(&self) -> BlockCell {
        match (self.bend, self.sides) {
            (1, Some([a, _])) => a,
            (2, Some([_, b])) => b,
            _ => self.back,
        }
    }

    fn pop_front(&mut self) {
        self.items.rotate_left(1);
        self.items[BELT_ITEMS - 1] = None;
    }

    /// Whether a new item fits at the back.
    fn room_at_back(&self) -> bool {
        match self.len() {
            0 => true,
            BELT_ITEMS => false,
            n => self.items[n - 1].is_some_and(|i| i.at >= SPACING),
        }
    }
}

/// The cells on the model's +x and -x sides of a conveyor facing `facing`.
fn sides(planet: &Planet, cell: BlockCell, facing: u8) -> [BlockCell; 2] {
    [1, 3].map(|turn| {
        let (du, dv) = AHEAD[(facing as usize + turn) % 4];
        planet
            .cell(cell.face, cell.u + du, cell.v + dv, cell.k)
            .unwrap_or(cell)
    })
}

/// The cell next to `cell` along a conveyor's facing (`sign` 1: in front, -1: behind).
fn neighbour(planet: &Planet, cell: BlockCell, facing: u8, sign: i32) -> BlockCell {
    let (du, dv) = AHEAD[facing as usize % 4];
    planet
        .cell(cell.face, cell.u + du * sign, cell.v + dv * sign, cell.k)
        .unwrap_or(cell)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineKind {
    Generator,
    ElectricFurnace,
    Miner,
    OilPump,
    Refinery,
    ArcFurnace,
    Assembler,
    ChipFab,
    Accumulator,
}

impl MachineKind {
    pub fn of(block: Block) -> Option<MachineKind> {
        Some(match block {
            Block::CoalGenerator => MachineKind::Generator,
            Block::ElectricFurnace => MachineKind::ElectricFurnace,
            Block::Miner => MachineKind::Miner,
            Block::OilPump => MachineKind::OilPump,
            Block::Refinery => MachineKind::Refinery,
            Block::ArcFurnace => MachineKind::ArcFurnace,
            Block::Assembler => MachineKind::Assembler,
            Block::ChipFab => MachineKind::ChipFab,
            Block::Accumulator => MachineKind::Accumulator,
            _ => return None,
        })
    }

    /// Lowercase, for screens.
    pub fn name(self) -> &'static str {
        match self {
            MachineKind::Generator => "coal generator",
            MachineKind::ElectricFurnace => "electric furnace",
            MachineKind::Miner => "miner",
            MachineKind::OilPump => "oil pump",
            MachineKind::Refinery => "refinery",
            MachineKind::ArcFurnace => "arc furnace",
            MachineKind::Assembler => "assembler",
            MachineKind::ChipFab => "chip fab",
            MachineKind::Accumulator => "accumulator",
        }
    }

    /// What it uses while it works (W); 0 for generators and accumulators.
    pub fn power(self) -> f32 {
        match self {
            MachineKind::Generator | MachineKind::Accumulator => 0.0,
            MachineKind::ElectricFurnace => 90_000.0,
            MachineKind::Miner => 60_000.0,
            MachineKind::OilPump => 50_000.0,
            MachineKind::Refinery => 120_000.0,
            MachineKind::ArcFurnace => 150_000.0,
            MachineKind::Assembler => 60_000.0,
            MachineKind::ChipFab => 200_000.0,
        }
    }

    /// Makes what is picked on its screen (`recipes`).
    pub fn has_recipes(self) -> bool {
        matches!(
            self,
            MachineKind::Refinery
                | MachineKind::ArcFurnace
                | MachineKind::Assembler
                | MachineKind::ChipFab
        )
    }

    /// Has an output slot (conveyors behind it take from there).
    pub fn has_output(self) -> bool {
        !matches!(self, MachineKind::Generator | MachineKind::Accumulator)
    }

    /// Where cables meet it (model units above the cell's floor).
    pub fn cable_height(self) -> f32 {
        match self {
            MachineKind::Miner => models::MINER_TOP,
            MachineKind::Refinery => models::REFINERY_TOP,
            _ => models::MACHINE_TOP,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub cell: BlockCell,
    pub kind: MachineKind,
    /// The first ingredient (what an electric furnace smelts); the others in `more`.
    pub input: Slot,
    #[serde(default)]
    pub more: [Slot; 3],
    pub fuel: Slot,
    pub output: Slot,
    /// Seconds into the current item or batch.
    pub progress: f32,
    /// Generator: energy left in the fuel burning now, and what it held. Accumulator: what it
    /// holds, and what it can (J).
    pub energy: f32,
    pub energy_total: f32,
    /// Watts made (generator), used (machines) or going in or out (accumulator) in the last
    /// tick.
    pub power: f32,
    /// How much of what it asks for its network gives it (0..1).
    pub supply: f32,
    /// What a recipe machine makes (the recipe's product, see `recipes`).
    #[serde(default)]
    pub recipe: Option<Item>,
    /// What a miner or oil pump brings up (decided by what is under it when it is placed).
    #[serde(default)]
    pub resource: Option<Item>,
}

impl Machine {
    fn new(cell: BlockCell, kind: MachineKind) -> Self {
        Self {
            cell,
            kind,
            input: None,
            more: [None; 3],
            fuel: None,
            output: None,
            progress: 0.0,
            energy: 0.0,
            energy_total: if kind == MachineKind::Accumulator {
                ACCUMULATOR_ENERGY
            } else {
                0.0
            },
            power: 0.0,
            supply: 0.0,
            recipe: None,
            resource: None,
        }
    }

    pub fn contents(&self) -> impl Iterator<Item = ItemStack> {
        [
            self.input,
            self.more[0],
            self.more[1],
            self.more[2],
            self.fuel,
            self.output,
        ]
        .into_iter()
        .flatten()
    }

    /// The ingredient slots, in recipe order.
    pub fn inputs(&self) -> [Slot; 4] {
        [self.input, self.more[0], self.more[1], self.more[2]]
    }

    pub fn inputs_mut(&mut self) -> [&mut Slot; 4] {
        let [a, b, c] = &mut self.more;
        [&mut self.input, a, b, c]
    }

    /// The recipe it is set to.
    pub fn current_recipe(&self) -> Option<&'static recipes::Recipe> {
        recipes::find(self.kind, self.recipe?)
    }

    /// Making or using power right now (its lamp is lit, a furnace's window glows). An
    /// accumulator: more than half full.
    pub fn lit(&self) -> bool {
        if self.kind == MachineKind::Accumulator {
            self.energy > self.energy_total * 0.5
        } else {
            self.power > 0.0
        }
    }

    /// Room in the output for `count` more of `item`.
    fn room_for(&self, item: Item, count: u8) -> bool {
        match self.output {
            None => true,
            Some(out) => {
                out.item == item && out.count as u32 + count as u32 <= item.max_stack() as u32
            }
        }
    }

    /// What it would make next and how many, if it has everything for it and room for them.
    fn product(&self) -> Option<(Item, u8)> {
        let made = match self.kind {
            MachineKind::ElectricFurnace => (self.input?.item.smelted()?, 1),
            MachineKind::Miner | MachineKind::OilPump => (self.resource?, 1),
            k if k.has_recipes() => {
                let recipe = self.current_recipe()?;
                let have = self.inputs();
                let ready =
                    recipe.inputs.iter().zip(have).all(|(&(item, n), slot)| {
                        slot.is_some_and(|s| s.item == item && s.count >= n)
                    });
                if !ready {
                    return None;
                }
                (recipe.output, recipe.count)
            }
            _ => return None,
        };
        self.room_for(made.0, made.1).then_some(made)
    }

    /// Seconds per item or batch.
    fn work_time(&self) -> f32 {
        match self.kind {
            MachineKind::Miner => MINE_TIME,
            MachineKind::OilPump => PUMP_TIME,
            k if k.has_recipes() => self.current_recipe().map_or(1.0, |r| r.time),
            _ => SMELT_TIME,
        }
    }

    /// Wants power now: a machine with something to do.
    fn working(&self) -> bool {
        self.kind.power() > 0.0 && self.product().is_some()
    }

    /// How far through the current item or batch (0..1).
    pub fn progress_share(&self) -> f32 {
        (self.progress / self.work_time()).clamp(0.0, 1.0)
    }

    fn can_supply(&self) -> bool {
        self.kind == MachineKind::Generator
            && (self.energy > 0.0 || self.fuel.is_some_and(|f| f.item.fuel().is_some()))
    }

    /// Burns fuel for `need` joules; returns how much it could give.
    fn burn(&mut self, mut need: f32) -> f32 {
        let mut given = 0.0;
        while need > 0.0 {
            if self.energy <= 0.0 {
                let Some(seconds) = self.fuel.and_then(|f| f.item.fuel()) else {
                    break;
                };
                inventory::take(&mut self.fuel, 1);
                self.energy = seconds * FUEL_ENERGY;
                self.energy_total = self.energy;
            }
            let e = need.min(self.energy);
            self.energy -= e;
            need -= e;
            given += e;
        }
        given
    }

    /// Works for `dt` seconds' worth of full power: smelts, mines, pumps or makes.
    fn work(&mut self, dt: f32) {
        let Some((item, count)) = self.product() else {
            self.progress = 0.0;
            return;
        };
        self.progress += dt;
        if self.progress < self.work_time() {
            return;
        }
        self.progress = 0.0;
        match self.kind {
            MachineKind::ElectricFurnace => {
                inventory::take(&mut self.input, 1);
            }
            k if k.has_recipes() => {
                if let Some(recipe) = self.current_recipe() {
                    for (slot, &(_, n)) in self.inputs_mut().into_iter().zip(recipe.inputs) {
                        inventory::take(slot, n);
                    }
                }
            }
            _ => {}
        }
        match &mut self.output {
            Some(out) => out.count += count,
            None => self.output = Some(ItemStack::new(item, count)),
        }
    }

    /// Which input slot `item` belongs in, and how many that slot takes from a conveyor.
    fn slot_for(&self, item: Item) -> Option<(usize, u8)> {
        match self.kind {
            MachineKind::ElectricFurnace => item.smelted().map(|_| (0, item.max_stack())),
            k if k.has_recipes() => {
                let recipe = self.current_recipe()?;
                let i = recipe.inputs.iter().position(|&(it, _)| it == item)?;
                // Enough for a few batches: a conveyor shouldn't pile everything in.
                let most = (recipe.inputs[i].1 as u32 * 4).clamp(4, 64) as u8;
                Some((i, most))
            }
            _ => None,
        }
    }

    /// Whether a conveyor would hand it `item` now.
    fn wants(&self, item: Item) -> bool {
        self.wants_more(item, 0)
    }

    /// Whether it wants another `item` with `coming` more already on their way to it. (A line
    /// feeding it more than it takes would fill up with what it won't take, and nothing else
    /// would get through: coal stuck in front of the sand an arc furnace is waiting for.)
    fn wants_more(&self, item: Item, coming: usize) -> bool {
        let fits = |slot: Slot, most: u8| {
            slot.is_none_or(|s| s.item == item)
                && slot.map_or(0, |s| s.count as usize) + coming < most as usize
        };
        if self.kind == MachineKind::Generator {
            return item.fuel().is_some() && fits(self.fuel, item.max_stack());
        }
        self.slot_for(item)
            .is_some_and(|(i, most)| fits(self.inputs()[i], most))
    }

    /// Takes one item from a conveyor, into the slot it belongs in.
    fn accept(&mut self, stack: ItemStack) -> bool {
        if !self.wants(stack.item) {
            return false;
        }
        if self.kind == MachineKind::Generator {
            return put_one(&mut self.fuel, stack);
        }
        match self.slot_for(stack.item) {
            Some((i, _)) => put_one(self.inputs_mut()[i], stack),
            None => false,
        }
    }

    /// Sets what a recipe machine makes. Whatever is in its input slots that the new recipe
    /// doesn't want there comes back out.
    pub fn set_recipe(&mut self, output: Option<Item>) -> Vec<ItemStack> {
        self.recipe = output;
        self.progress = 0.0;
        let wanted: Vec<Option<Item>> = (0..4)
            .map(|i| {
                self.current_recipe()
                    .and_then(|r| r.inputs.get(i))
                    .map(|&(item, _)| item)
            })
            .collect();
        let mut out = Vec::new();
        for (slot, want) in self.inputs_mut().into_iter().zip(wanted) {
            if slot.is_some_and(|s| Some(s.item) != want) {
                out.extend(slot.take());
            }
        }
        out
    }

    /// A few words on what it is doing (the HUD, its screen).
    pub fn status(&self) -> String {
        let kw = |w: f32| format!("{:.0} kw", w / 1000.0);
        let percent = |x: f32| format!("{:.0}%", x * 100.0);
        match self.kind {
            MachineKind::Generator if self.power > 0.0 => format!("making {}", kw(self.power)),
            MachineKind::Generator => "no fuel".to_string(),
            MachineKind::Accumulator => {
                let charge = percent(self.energy / self.energy_total.max(1.0));
                if self.power > 0.0 {
                    format!("{charge}, charging {}", kw(self.power))
                } else if self.power < 0.0 {
                    format!("{charge}, giving {}", kw(-self.power))
                } else {
                    format!("{charge} charged")
                }
            }
            MachineKind::Miner if self.resource.is_none() => "nothing to dig under it".to_string(),
            MachineKind::OilPump if self.resource.is_none() => "no oil shale under it".to_string(),
            k if k.has_recipes() && self.recipe.is_none() => "pick what to make".to_string(),
            _ if !self.working() => {
                let full = self.output.is_some_and(|s| s.count >= s.item.max_stack())
                    || (self.output.is_some() && self.product().is_none() && self.has_inputs());
                if full {
                    "output full".to_string()
                } else if self.kind.has_recipes() || self.kind == MachineKind::ElectricFurnace {
                    "waiting for ingredients".to_string()
                } else {
                    "idle".to_string()
                }
            }
            _ if self.supply <= 0.0 => "no power".to_string(),
            _ => format!("{} power, {}", percent(self.supply), kw(self.power)),
        }
    }

    /// Has everything its recipe (or smelting) takes, whether or not there is room for it.
    fn has_inputs(&self) -> bool {
        match self.current_recipe() {
            Some(r) => r
                .inputs
                .iter()
                .zip(self.inputs())
                .all(|(&(item, n), s)| s.is_some_and(|s| s.item == item && s.count >= n)),
            None => self.input.is_some(),
        }
    }

    /// Whether `stack` may go into ingredient slot `i` (by hand).
    pub fn input_takes(&self, i: usize, item: Item) -> bool {
        match self.kind {
            MachineKind::ElectricFurnace => i == 0 && item.smelted().is_some(),
            k if k.has_recipes() => self
                .current_recipe()
                .and_then(|r| r.inputs.get(i))
                .is_some_and(|&(it, _)| it == item),
            _ => false,
        }
    }
}

/// What a miner over `block` brings up (None: nothing it can dig).
pub fn mined_from(block: Block) -> Option<Item> {
    use crate::survival::items::{Tier, ToolKind, drops};
    if !block.is_solid() || block.is_model() || matches!(block, Block::Lava | Block::OilShale) {
        return None;
    }
    drops(block, Some((ToolKind::Pickaxe, Tier::Diamond)), 0.5)
        .or_else(|| drops(block, Some((ToolKind::Shovel, Tier::Diamond)), 0.5))
        .map(|s| s.item)
}

/// How much a miner prefers an ore (higher first); 0 for ordinary ground.
pub fn ore_rank(block: Block) -> u8 {
    match block {
        Block::DiamondOre => 7,
        Block::GoldOre => 6,
        Block::UraniumOre => 5,
        Block::CopperOre => 4,
        Block::IronOre => 3,
        Block::BauxiteOre => 2,
        Block::CoalOre => 1,
        _ => 0,
    }
}

/// Puts one of `stack` into `slot` if it is empty or holds the same with room to spare.
fn put_one(slot: &mut Slot, stack: ItemStack) -> bool {
    match slot {
        None => {
            *slot = Some(ItemStack { count: 1, ..stack });
            true
        }
        Some(s) if s.stacks_with(&stack) && s.count < s.item.max_stack() => {
            s.count += 1;
            true
        }
        Some(_) => false,
    }
}

/// What a conveyor hands its front item to, or takes from behind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Link {
    #[default]
    None,
    Belt(u32),
    Chest(BlockCell),
    Furnace(BlockCell),
    Machine(u32),
}

/// What the end of a line of conveyors can use (so nothing else gets onto it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Want {
    #[default]
    Anything,
    SmeltableOrFuel,
    /// What that machine takes now (fuel, what it smelts, its recipe's ingredients).
    Machine(u32),
}

impl Want {
    fn accepts(self, item: Item, machines: &[Machine]) -> bool {
        match self {
            Want::Anything => true,
            Want::SmeltableOrFuel => item.smelted().is_some() || item.fuel().is_some(),
            Want::Machine(m) => machines.get(m as usize).is_some_and(|m| m.wants(item)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Node {
    Conveyor(u32),
    Machine(u32),
    Pole(u32),
}

/// Machines joined by cables, and how its power went in the last tick (W).
#[derive(Clone, Debug, Default)]
pub struct Network {
    machines: Vec<u32>,
    /// It goes by its first member (the history is kept under it, across relinks).
    key: Option<BlockCell>,
    /// What its generators made, what its machines asked for, and what they got.
    pub capacity: f32,
    pub demand: f32,
    pub used: f32,
    /// What went into and came out of its accumulators, what they hold and can.
    pub charging: f32,
    pub discharging: f32,
    pub stored: f32,
    pub storage: f32,
    /// How many generators (with fuel, of all), machines (working, of all) and accumulators.
    pub generators: (u32, u32),
    pub consumers: (u32, u32),
    pub accumulators: u32,
    pub poles: u32,
}

/// A network's power over the last `HISTORY` seconds: (made, used) per second, oldest first.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub samples: std::collections::VecDeque<(f32, f32)>,
    /// The second being added up: joules made, used, and seconds so far.
    made: f32,
    used: f32,
    time: f32,
}

/// A network seen from one of its poles or machines (the power screen).
#[derive(Clone, Debug)]
pub struct PowerView {
    pub network: Network,
    pub history: History,
}

/// The containers outside the factory that conveyors reach (`Survival`'s).
pub struct Stores<'a> {
    pub chests: &'a mut FastMap<BlockCell, [Slot; CHEST_SLOTS]>,
    pub furnaces: &'a mut FastMap<BlockCell, Furnace>,
}

/// Conveyors are sent to other players in groups of 4 x 4 x 2 cells: the group, and the cell in it.
pub type GroupKey = (u8, i32, i32, i32);

pub fn group_key(cell: BlockCell) -> GroupKey {
    (
        cell.face.0,
        cell.u.div_euclid(4),
        cell.v.div_euclid(4),
        cell.k.div_euclid(2),
    )
}

fn group_slot(cell: BlockCell) -> u8 {
    (cell.u.rem_euclid(4) + 4 * cell.v.rem_euclid(4) + 16 * cell.k.rem_euclid(2)) as u8
}

/// What is on the conveyors of one group (`group_key`), as sent to other players: every
/// conveyor of the group not listed is empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetBelts {
    pub key: GroupKey,
    /// (cell in the group, items: what, and how far along in 1/255ths).
    pub belts: Vec<(u8, Vec<(Item, u8)>)>,
}

/// What a factory keeps in a saved game.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedFactory {
    conveyors: Vec<Conveyor>,
    machines: Vec<Machine>,
    poles: Vec<BlockCell>,
    cables: Vec<(BlockCell, BlockCell)>,
}

/// What came off a part of a factory that was taken away.
pub struct Removed {
    pub contents: Vec<ItemStack>,
    pub cables: u32,
}

/// One world's factories: every conveyor, machine, pole and cable on it.
#[derive(Default)]
pub struct Factory {
    conveyors: Vec<Conveyor>,
    machines: Vec<Machine>,
    poles: Vec<BlockCell>,
    cables: Vec<(BlockCell, BlockCell)>,
    /// Where each part is in the arrays above.
    index: FastMap<BlockCell, Node>,
    indexed: bool,
    /// Whether the links, the order, the groups and the networks are up to date.
    linked: bool,
    /// Conveyors downstream first.
    order: Vec<u32>,
    groups: FastMap<GroupKey, Vec<u32>>,
    networks: Vec<Network>,
    /// The network of each machine and pole.
    machine_network: Vec<u32>,
    pole_network: Vec<u32>,
    /// Some conveyor hasn't been placed in space yet (`locate`).
    unlocated: bool,
    /// Game time not yet stepped (s).
    behind: f64,
    /// May move items into and out of containers (offline, the host).
    pub authority: bool,
    /// Containers the conveyors have changed (for the other players), and machines that lit up
    /// or went out.
    pub changed: Vec<BlockCell>,
    pub lit_changed: Vec<(BlockCell, bool)>,
    /// Conveyors whose bend changed (`ModelState::bend`, for their models).
    pub bends_changed: Vec<(BlockCell, u8)>,
    /// Each network's power over the last minute, by its key.
    history: FastMap<BlockCell, History>,
    pub art: Art,
}

impl Factory {
    pub fn saved(&self) -> SavedFactory {
        SavedFactory {
            conveyors: self.conveyors.clone(),
            machines: self.machines.clone(),
            poles: self.poles.clone(),
            cables: self.cables.clone(),
        }
    }

    pub fn restored(saved: SavedFactory) -> Self {
        Self {
            conveyors: saved.conveyors,
            machines: saved.machines,
            poles: saved.poles,
            cables: saved.cables,
            unlocated: true,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.conveyors.is_empty() && self.machines.is_empty() && self.poles.is_empty()
    }

    /// (conveyors, machines, poles, cables, networks)
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.conveyors.len(),
            self.machines.len(),
            self.poles.len(),
            self.cables.len(),
            self.networks.len(),
        )
    }

    pub fn machines(&self) -> &[Machine] {
        &self.machines
    }

    pub fn networks(&self) -> &[Network] {
        &self.networks
    }

    /// Items riding on conveyors, all told.
    pub fn carried(&self) -> usize {
        self.conveyors.iter().map(Conveyor::len).sum()
    }

    /// The chests and furnaces conveyors take from or hand to (once linked).
    pub fn stores(&self) -> Vec<BlockCell> {
        let mut cells: Vec<BlockCell> = Vec::new();
        for c in &self.conveyors {
            for link in [c.next, c.source] {
                if let Link::Chest(cell) | Link::Furnace(cell) = link
                    && !cells.contains(&cell)
                {
                    cells.push(cell);
                }
            }
        }
        cells
    }

    fn ensure_index(&mut self) {
        if self.indexed {
            return;
        }
        self.index.clear();
        for (i, c) in self.conveyors.iter().enumerate() {
            self.index.insert(c.cell, Node::Conveyor(i as u32));
        }
        for (i, m) in self.machines.iter().enumerate() {
            self.index.insert(m.cell, Node::Machine(i as u32));
        }
        for (i, p) in self.poles.iter().enumerate() {
            self.index.insert(*p, Node::Pole(i as u32));
        }
        self.indexed = true;
    }

    /// Something next to the factory changed (a chest or furnace came or went).
    pub fn touch(&mut self) {
        self.linked = false;
    }

    /// A factory block was placed in `cell` (`facing`: its model's).
    pub fn add(&mut self, planet: &Planet, cell: BlockCell, block: Block, facing: u8) {
        self.ensure_index();
        if self.index.contains_key(&cell) {
            self.remove(cell);
        }
        let node = match block {
            Block::Conveyor => {
                self.conveyors.push(Conveyor::new(planet, cell, facing));
                self.unlocated = true;
                Node::Conveyor(self.conveyors.len() as u32 - 1)
            }
            Block::PowerPole => {
                self.poles.push(cell);
                Node::Pole(self.poles.len() as u32 - 1)
            }
            _ => {
                let Some(kind) = MachineKind::of(block) else {
                    return;
                };
                self.machines.push(Machine::new(cell, kind));
                Node::Machine(self.machines.len() as u32 - 1)
            }
        };
        self.index.insert(cell, node);
        self.linked = false;
    }

    /// Takes away the part in `cell`, with its cables: what it held, and how many cables.
    pub fn remove(&mut self, cell: BlockCell) -> Option<Removed> {
        self.ensure_index();
        let node = self.index.remove(&cell)?;
        let mut contents = Vec::new();
        match node {
            Node::Conveyor(i) => {
                let c = self.conveyors.swap_remove(i as usize);
                contents.extend(c.carried().map(|i| i.stack));
                if let Some(moved) = self.conveyors.get(i as usize) {
                    self.index.insert(moved.cell, Node::Conveyor(i));
                }
            }
            Node::Machine(i) => {
                let m = self.machines.swap_remove(i as usize);
                contents.extend(m.contents());
                if let Some(moved) = self.machines.get(i as usize) {
                    self.index.insert(moved.cell, Node::Machine(i));
                }
            }
            Node::Pole(i) => {
                self.poles.swap_remove(i as usize);
                if let Some(&moved) = self.poles.get(i as usize) {
                    self.index.insert(moved, Node::Pole(i));
                }
            }
        }
        let before = self.cables.len();
        self.cables.retain(|&(a, b)| a != cell && b != cell);
        self.linked = false;
        Some(Removed {
            contents,
            cables: (before - self.cables.len()) as u32,
        })
    }

    /// Turns a conveyor to face `facing`.
    pub fn turn(&mut self, planet: &Planet, cell: BlockCell, facing: u8) {
        self.ensure_index();
        if let Some(Node::Conveyor(i)) = self.index.get(&cell) {
            let c = &mut self.conveyors[*i as usize];
            let items = c.items;
            *c = Conveyor::new(planet, cell, facing);
            c.items = items;
            self.unlocated = true;
            self.linked = false;
        }
    }

    #[cfg(test)]
    pub fn conveyor(&self, cell: BlockCell) -> Option<&Conveyor> {
        match self.node(cell)? {
            Node::Conveyor(i) => self.conveyors.get(i as usize),
            _ => None,
        }
    }

    pub fn machine(&self, cell: BlockCell) -> Option<&Machine> {
        match self.node(cell)? {
            Node::Machine(i) => self.machines.get(i as usize),
            _ => None,
        }
    }

    pub fn machine_mut(&mut self, cell: BlockCell) -> Option<&mut Machine> {
        self.ensure_index();
        match self.index.get(&cell)? {
            Node::Machine(i) => self.machines.get_mut(*i as usize),
            _ => None,
        }
    }

    fn node(&self, cell: BlockCell) -> Option<Node> {
        if self.indexed {
            return self.index.get(&cell).copied();
        }
        // (Before the index is built: a saved game just loaded.)
        if let Some(i) = self.conveyors.iter().position(|c| c.cell == cell) {
            return Some(Node::Conveyor(i as u32));
        }
        if let Some(i) = self.machines.iter().position(|m| m.cell == cell) {
            return Some(Node::Machine(i as u32));
        }
        self.poles
            .iter()
            .position(|p| *p == cell)
            .map(|i| Node::Pole(i as u32))
    }

    // ---- Cables -------------------------------------------------------------------------

    /// Whether cables can run to `cell` (a pole or a machine).
    pub fn is_electric(&self, cell: BlockCell) -> bool {
        matches!(self.node(cell), Some(Node::Machine(_) | Node::Pole(_)))
    }

    pub fn connected(&self, a: BlockCell, b: BlockCell) -> bool {
        let pair = ordered(a, b);
        self.cables.contains(&pair)
    }

    /// Runs a cable between two poles or machines; false if there already is one (or they
    /// can't take one).
    pub fn connect(&mut self, a: BlockCell, b: BlockCell) -> bool {
        if a == b || self.connected(a, b) || !self.is_electric(a) || !self.is_electric(b) {
            return false;
        }
        self.cables.push(ordered(a, b));
        self.linked = false;
        true
    }

    pub fn disconnect(&mut self, a: BlockCell, b: BlockCell) -> bool {
        let pair = ordered(a, b);
        let before = self.cables.len();
        self.cables.retain(|c| *c != pair);
        self.linked = false;
        self.cables.len() != before
    }

    #[cfg(test)]
    pub fn cables(&self) -> &[(BlockCell, BlockCell)] {
        &self.cables
    }

    // ---- Links --------------------------------------------------------------------------

    /// Works out what every conveyor hands on to and takes from, the order they are stepped
    /// in, what each line can use, the network groups, and the power networks.
    fn relink(&mut self, stores: &Stores) {
        self.ensure_index();
        let n = self.conveyors.len();
        let store = |cell: BlockCell| {
            if stores.chests.contains_key(&cell) {
                Link::Chest(cell)
            } else if stores.furnaces.contains_key(&cell) {
                Link::Furnace(cell)
            } else {
                Link::None
            }
        };
        for i in 0..n {
            let (cell, front, back) = {
                let c = &self.conveyors[i];
                (c.cell, c.front, c.back)
            };
            let next = match self.index.get(&front) {
                // (Two conveyors pointing at each other hand nothing on.)
                Some(Node::Conveyor(j)) if self.conveyors[*j as usize].front != cell => {
                    Link::Belt(*j)
                }
                Some(Node::Machine(m)) => Link::Machine(*m),
                Some(_) => Link::None,
                None => store(front),
            };
            let source = match self.index.get(&back) {
                Some(Node::Machine(m)) if self.machines[*m as usize].kind.has_output() => {
                    Link::Machine(*m)
                }
                Some(_) => Link::None,
                None => store(back),
            };
            let c = &mut self.conveyors[i];
            c.next = next;
            c.source = source;
        }

        // Bends: fed only from one side (nothing behind it), a conveyor comes round from there.
        for i in 0..n {
            let c = &self.conveyors[i];
            let feeds = |from: BlockCell| {
                matches!(self.index.get(&from), Some(Node::Conveyor(j))
                    if self.conveyors[*j as usize].front == c.cell)
            };
            let straight = feeds(c.back) || c.source != Link::None;
            let bend = match c.sides {
                Some([a, b]) if !straight => match (feeds(a), feeds(b)) {
                    (true, false) => 1,
                    (false, true) => 2,
                    _ => 0,
                },
                _ => 0,
            };
            let c = &mut self.conveyors[i];
            if bend != c.bend {
                c.bend = bend;
                c.place = None;
                self.unlocated = true;
            }
            // (Every one: the model of one just turned or loaded may not say what this does.)
            self.bends_changed.push((c.cell, bend));
        }

        // Downstream first: by how many conveyors there are still to go.
        let mut depth = vec![0u32; n];
        let mut state = vec![0u8; n]; // 0 new, 1 on the current walk, 2 done
        let mut path = Vec::new();
        for start in 0..n {
            if state[start] != 0 {
                continue;
            }
            let mut j = start;
            let base = loop {
                match state[j] {
                    2 => break depth[j] + 1,
                    1 => break 0, // a loop
                    _ => {}
                }
                state[j] = 1;
                path.push(j);
                match self.conveyors[j].next {
                    Link::Belt(k) => j = k as usize,
                    _ => break 0,
                }
            };
            for (d, &p) in (base..).zip(path.iter().rev()) {
                depth[p] = d;
                state[p] = 2;
            }
            path.clear();
        }
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by_key(|&i| depth[i as usize]);

        // What each line can use, from its end back.
        for &i in &order {
            let wants = match self.conveyors[i as usize].next {
                Link::Belt(j) if depth[j as usize] < depth[i as usize] => {
                    self.conveyors[j as usize].wants
                }
                Link::Furnace(_) => Want::SmeltableOrFuel,
                Link::Machine(m) => Want::Machine(m),
                _ => Want::Anything,
            };
            self.conveyors[i as usize].wants = wants;
        }
        self.order = order;

        self.groups.clear();
        for (i, c) in self.conveyors.iter().enumerate() {
            self.groups
                .entry(group_key(c.cell))
                .or_default()
                .push(i as u32);
        }

        // Power networks: machines and poles joined by cables.
        let machines = self.machines.len();
        let mut parent: Vec<usize> = (0..machines + self.poles.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let slot = |node: Option<&Node>| match node {
            Some(Node::Machine(m)) => Some(*m as usize),
            Some(Node::Pole(p)) => Some(machines + *p as usize),
            _ => None,
        };
        for (a, b) in &self.cables {
            if let (Some(a), Some(b)) = (slot(self.index.get(a)), slot(self.index.get(b))) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
        let mut network_of: FastMap<usize, u32> = FastMap::default();
        self.networks.clear();
        let mut assign = |i: usize, networks: &mut Vec<Network>| {
            let r = root(&mut parent, i);
            *network_of.entry(r).or_insert_with(|| {
                networks.push(Network::default());
                networks.len() as u32 - 1
            })
        };
        self.machine_network = (0..machines)
            .map(|m| assign(m, &mut self.networks))
            .collect();
        self.pole_network = (0..self.poles.len())
            .map(|p| assign(machines + p, &mut self.networks))
            .collect();
        for (m, &net) in self.machine_network.iter().enumerate() {
            self.networks[net as usize].machines.push(m as u32);
        }
        // Each network's key: its least member, so its history carries on as it grows.
        let order = |c: &BlockCell| (c.face.0, c.u, c.v, c.k);
        let members = (self.machines.iter().map(|m| m.cell))
            .zip(self.machine_network.iter().copied())
            .chain(
                self.poles
                    .iter()
                    .copied()
                    .zip(self.pole_network.iter().copied()),
            );
        for (cell, net) in members {
            let net = &mut self.networks[net as usize];
            if net.key.is_none_or(|k| order(&cell) < order(&k)) {
                net.key = Some(cell);
            }
        }
        for &net in &self.pole_network {
            self.networks[net as usize].poles += 1;
        }
        let keys: Vec<BlockCell> = self.networks.iter().filter_map(|n| n.key).collect();
        self.history.retain(|k, _| keys.contains(k));
        self.linked = true;
    }

    /// Places conveyors in space (for drawing and the network), those that aren't yet.
    pub fn locate(&mut self, planet: &Planet) {
        if !self.unlocated {
            return;
        }
        for c in &mut self.conveyors {
            if c.place.is_some() {
                continue;
            }
            if c.sides.is_none() {
                c.sides = Some(sides(planet, c.cell, c.facing));
            }
            let point = |p: [f32; 3]| models::model_point(planet, c.cell, c.facing, p);
            let belt = models::BELT_TOP;
            let (start, corner) = match c.bend {
                1 => (point([16.0, 8.0, belt]), Some(point([8.0, 8.0, belt]))),
                2 => (point([0.0, 8.0, belt]), Some(point([8.0, 8.0, belt]))),
                _ => (point([8.0, 16.0, belt]), None),
            };
            let end = point([8.0, 0.0, belt]);
            let bottom = point([8.0, 8.0, 0.0]);
            let top = point([8.0, 8.0, 16.0]);
            c.place = Some(Place {
                center: (bottom + top) * 0.5,
                start,
                end,
                up: (top - bottom).normalize_or(DVec3::Y),
                corner,
            });
        }
        self.unlocated = false;
    }

    // ---- Time passing -------------------------------------------------------------------

    /// Runs for `dt` seconds of game time (in steps, within a time budget).
    pub fn run(&mut self, dt: f64, stores: &mut Stores) {
        if self.is_empty() {
            self.behind = 0.0;
            return;
        }
        if !self.linked {
            self.relink(stores);
        }
        self.behind = (self.behind + dt).min(MOST_BEHIND);
        let start = Instant::now();
        while self.behind >= TICK {
            let step = if self.behind > 1.0 { MAX_TICK } else { TICK }.min(self.behind);
            self.tick(step as f32, stores);
            self.behind -= step;
            if start.elapsed() > BUDGET {
                break;
            }
        }
    }

    fn tick(&mut self, dt: f32, stores: &mut Stores) {
        self.step_power(dt);
        self.step_conveyors(dt, stores);
    }

    /// Each network: its generators make power whenever they have fuel, used or not. Machines
    /// that have something to do get it first (all slowed alike when there isn't enough), what
    /// is left charges the accumulators, and when the generators fall short the accumulators
    /// make up what they can.
    fn step_power(&mut self, dt: f32) {
        for net in &mut self.networks {
            let (mut capacity, mut demand, mut stored, mut storage) = (0.0, 0.0, 0.0, 0.0);
            let (mut gens, mut fuelled, mut users, mut busy, mut accumulators) = (0, 0, 0, 0, 0);
            // What accumulators could take in, and give out, this tick (W).
            let (mut can_take, mut can_give) = (0.0, 0.0);
            for &m in &net.machines {
                let m = &self.machines[m as usize];
                match m.kind {
                    MachineKind::Generator => {
                        gens += 1;
                        if m.can_supply() {
                            fuelled += 1;
                            capacity += GENERATOR_POWER;
                        }
                    }
                    MachineKind::Accumulator => {
                        accumulators += 1;
                        stored += m.energy;
                        storage += m.energy_total;
                        can_take += ACCUMULATOR_POWER.min((m.energy_total - m.energy) / dt);
                        can_give += ACCUMULATOR_POWER.min(m.energy / dt);
                    }
                    kind => {
                        users += 1;
                        if m.working() {
                            busy += 1;
                            demand += kind.power();
                        }
                    }
                }
            }
            let (used, charge, discharge) = if capacity >= demand {
                (demand, (capacity - demand).min(can_take), 0.0)
            } else {
                let discharge = (demand - capacity).min(can_give);
                (capacity + discharge, 0.0, discharge)
            };
            let share = if demand > 0.0 { used / demand } else { 0.0 };
            for &i in &net.machines {
                let m = &mut self.machines[i as usize];
                let was = m.lit();
                match m.kind {
                    MachineKind::Generator => {
                        // Full power while there is fuel, whether anything uses it or not.
                        let want = if m.can_supply() { GENERATOR_POWER } else { 0.0 };
                        m.power = m.burn(want * dt) / dt;
                        m.supply = if capacity > 0.0 {
                            (used + charge) / capacity
                        } else {
                            0.0
                        };
                    }
                    MachineKind::Accumulator => {
                        // Shared out by how much each could take or give.
                        let room = ACCUMULATOR_POWER.min((m.energy_total - m.energy) / dt);
                        let have = ACCUMULATOR_POWER.min(m.energy / dt);
                        let flow = if charge > 0.0 && can_take > 0.0 {
                            charge * room / can_take
                        } else if discharge > 0.0 && can_give > 0.0 {
                            -discharge * have / can_give
                        } else {
                            0.0
                        };
                        m.energy = (m.energy + flow * dt).clamp(0.0, m.energy_total);
                        m.power = flow;
                        m.supply = m.energy / m.energy_total.max(1.0);
                    }
                    kind => {
                        let working = m.working();
                        m.supply = if working { share } else { 0.0 };
                        m.power = if working { kind.power() * share } else { 0.0 };
                        if working && share > 0.0 {
                            m.work(dt * share);
                        } else if !working {
                            m.progress = 0.0;
                        }
                    }
                }
                if m.lit() != was {
                    self.lit_changed.push((m.cell, m.lit()));
                }
            }
            net.capacity = capacity;
            net.demand = demand;
            net.used = used;
            net.charging = charge;
            net.discharging = discharge;
            net.stored = (stored + (charge - discharge) * dt).clamp(0.0, storage);
            net.storage = storage;
            net.generators = (fuelled, gens);
            net.consumers = (busy, users);
            net.accumulators = accumulators;
            // A second's worth at a time, for the power screen's graph.
            if let Some(key) = net.key {
                let h = self.history.entry(key).or_default();
                h.made += capacity * dt;
                h.used += (used + charge) * dt;
                h.time += dt;
                if h.time >= 1.0 {
                    let sample = (h.made / h.time, h.used / h.time);
                    h.samples.push_back(sample);
                    if h.samples.len() > HISTORY {
                        h.samples.pop_front();
                    }
                    (h.made, h.used, h.time) = (0.0, 0.0, 0.0);
                }
            }
        }
    }

    /// Conveyors, downstream first: hand the front item on, move along, take from behind.
    fn step_conveyors(&mut self, dt: f32, stores: &mut Stores) {
        let step = BELT_SPEED * dt;
        let authority = self.authority;
        for k in 0..self.order.len() {
            let i = self.order[k] as usize;
            let c = &self.conveyors[i];
            if c.is_empty() && !(authority && c.source != Link::None) {
                self.conveyors[i].moving = false;
                continue;
            }

            // Along the belt, each item up to the one ahead of it. What the front one would
            // have gone past the end carries on onto the next conveyor.
            let c = &mut self.conveyors[i];
            let mut limit = 1.0f32;
            let mut moved = false;
            let excess = c.items[0].map_or(0.0, |f| (f.at + step - 1.0).max(0.0));
            for item in c.items.iter_mut().flatten() {
                let to = (item.at + step).min(limit);
                if to > item.at {
                    item.at = to;
                    moved = true;
                }
                limit = item.at - SPACING;
            }
            c.moving = moved;

            // The front item off the end.
            let c = &self.conveyors[i];
            if let Some(front) = c.items[0]
                && front.at >= 1.0
            {
                let (cell, next) = (c.cell, c.next);
                let handed = match next {
                    Link::Belt(j) => {
                        let next = &mut self.conveyors[j as usize];
                        // Straight on (or round a bend) at its start; from the side onto its
                        // middle.
                        if next.entry() == cell {
                            next.insert_behind(front.stack, excess)
                        } else {
                            next.insert(front.stack, 0.5)
                        }
                    }
                    Link::Chest(to) if authority => {
                        let done = stores.chests.get_mut(&to).is_some_and(|chest| {
                            // Onto a stack of the same, else into the first empty slot.
                            let room = |s: &Slot| {
                                s.is_some_and(|s| {
                                    s.stacks_with(&front.stack) && s.count < s.item.max_stack()
                                })
                            };
                            let slot = chest
                                .iter()
                                .position(room)
                                .or_else(|| chest.iter().position(Option::is_none));
                            slot.is_some_and(|s| put_one(&mut chest[s], front.stack))
                        });
                        if done {
                            self.changed.push(to);
                        }
                        done
                    }
                    Link::Furnace(to) if authority => {
                        let item = front.stack.item;
                        let done = stores.furnaces.get_mut(&to).is_some_and(|f| {
                            (item.smelted().is_some() && put_one(&mut f.input, front.stack))
                                || (item.fuel().is_some() && put_one(&mut f.fuel, front.stack))
                        });
                        if done {
                            self.changed.push(to);
                        }
                        done
                    }
                    Link::Machine(m) if authority => {
                        let machine = &mut self.machines[m as usize];
                        let done = machine.accept(front.stack);
                        if done {
                            self.changed.push(machine.cell);
                        }
                        done
                    }
                    _ => false,
                };
                if handed {
                    self.conveyors[i].pop_front();
                }
            }

            // From whatever is behind it.
            let c = &mut self.conveyors[i];
            if authority && c.source != Link::None && c.room_at_back() {
                let (wants, source) = (c.wants, c.source);
                // What is already on its way down the line to the machine at its end.
                let mut coming: Vec<Item> = Vec::new();
                if let Want::Machine(_) = wants {
                    let mut j = i;
                    for _ in 0..256 {
                        coming.extend(self.conveyors[j].carried().map(|it| it.stack.item));
                        match self.conveyors[j].next {
                            Link::Belt(k) => j = k as usize,
                            _ => break,
                        }
                    }
                }
                let machines = &self.machines;
                let ok = |item: Item| match wants {
                    Want::Machine(m) => machines.get(m as usize).is_some_and(|m| {
                        m.wants_more(item, coming.iter().filter(|&&x| x == item).count())
                    }),
                    _ => wants.accepts(item, machines),
                };
                let taken = match source {
                    Link::Chest(from) => stores.chests.get_mut(&from).and_then(|chest| {
                        let slot = chest
                            .iter_mut()
                            .rev()
                            .find(|s| s.is_some_and(|s| ok(s.item)))?;
                        inventory::take(slot, 1).map(|s| (s, from))
                    }),
                    Link::Furnace(from) => stores.furnaces.get_mut(&from).and_then(|f| {
                        f.output
                            .filter(|s| ok(s.item))
                            .and_then(|_| inventory::take(&mut f.output, 1))
                            .map(|s| (s, from))
                    }),
                    Link::Machine(m) => {
                        let wanted = machines[m as usize].output.is_some_and(|s| ok(s.item));
                        let machine = &mut self.machines[m as usize];
                        let cell = machine.cell;
                        wanted
                            .then(|| inventory::take(&mut machine.output, 1))
                            .flatten()
                            .map(|s| (s, cell))
                    }
                    _ => None,
                };
                if let Some((stack, from)) = taken {
                    self.conveyors[i].insert(stack, 0.0);
                    self.changed.push(from);
                }
            }
        }
    }

    // ---- Seeing it ------------------------------------------------------------------------

    /// A few words on a part, for the HUD (what it is doing).
    pub fn describe(&self, cell: BlockCell) -> Option<String> {
        let kw = |w: f32| format!("{:.0} kw", w / 1000.0);
        let network = |i: Option<&u32>| i.and_then(|&n| self.networks.get(n as usize));
        match self.node(cell)? {
            Node::Conveyor(i) => {
                let n = self.conveyors[i as usize].len();
                (n > 0).then(|| format!("carrying {n}"))
            }
            Node::Machine(i) => Some(self.machines[i as usize].status()),
            Node::Pole(i) => {
                let net = network(self.pole_network.get(i as usize))?;
                let wired = self.cables.iter().any(|&(a, b)| a == cell || b == cell);
                Some(if !wired {
                    "not connected".to_string()
                } else if net.machines.is_empty() {
                    "no machines on its line".to_string()
                } else {
                    format!("using {} of {}", kw(net.used), kw(net.capacity))
                })
            }
        }
    }

    /// The network that `cell` (a pole or a machine) is on, for its power screen.
    pub fn power_view(&self, cell: BlockCell) -> Option<PowerView> {
        let net = match self.node(cell)? {
            Node::Machine(i) => self.machine_network.get(i as usize),
            Node::Pole(i) => self.pole_network.get(i as usize),
            Node::Conveyor(_) => None,
        }?;
        let network = self.networks.get(*net as usize)?.clone();
        let history = (network.key)
            .and_then(|k| self.history.get(&k))
            .cloned()
            .unwrap_or_default();
        Some(PowerView { network, history })
    }

    /// What a miner or oil pump brings up (from what the game found under it).
    pub fn set_resource(&mut self, cell: BlockCell, resource: Option<Item>) {
        if let Some(m) = self.machine_mut(cell) {
            m.resource = resource;
        }
    }

    // ---- Other players ---------------------------------------------------------------------

    /// What is on the conveyors within `range` of `at` (planet-relative), for a player there:
    /// every group with a conveyor near enough, empty ones included.
    pub fn belts_near(&self, at: DVec3, range: f64) -> Vec<NetBelts> {
        let mut out = Vec::new();
        for (key, list) in &self.groups {
            let near = list.iter().any(|&i| {
                self.conveyors
                    .get(i as usize)
                    .and_then(|c| c.place)
                    .is_some_and(|p| (p.center - at).length_squared() < range * range)
            });
            if !near {
                continue;
            }
            let belts = list
                .iter()
                .filter_map(|&i| self.conveyors.get(i as usize))
                .filter(|c| !c.is_empty())
                .map(|c| {
                    let items = c
                        .carried()
                        .map(|i| (i.stack.item, (i.at.clamp(0.0, 1.0) * 255.0).round() as u8))
                        .collect();
                    (group_slot(c.cell), items)
                })
                .collect();
            out.push(NetBelts { key: *key, belts });
        }
        out
    }

    /// What the host says is on a group of conveyors.
    pub fn apply_belts(&mut self, group: &NetBelts) {
        if !self.linked {
            return; // (not stepped yet: the next message will do)
        }
        let Some(list) = self.groups.get(&group.key) else {
            return;
        };
        for &i in list {
            let Some(c) = self.conveyors.get_mut(i as usize) else {
                continue;
            };
            let slot = group_slot(c.cell);
            c.items = [None; BELT_ITEMS];
            if let Some((_, items)) = group.belts.iter().find(|(s, _)| *s == slot) {
                for (n, &(item, at)) in items.iter().take(BELT_ITEMS).enumerate() {
                    c.items[n] = Some(BeltItem {
                        stack: ItemStack::one(item),
                        at: at as f32 / 255.0,
                    });
                }
            }
        }
    }

    /// The host's word on a machine.
    pub fn set_machine(&mut self, machine: &Machine) {
        if let Some(m) = self.machine_mut(machine.cell)
            && m.kind == machine.kind
        {
            *m = *machine;
        }
    }
}

/// A cable's two ends in a fixed order (so each pair is stored once).
fn ordered(a: BlockCell, b: BlockCell) -> (BlockCell, BlockCell) {
    let key = |c: BlockCell| (c.face.0, c.u, c.v, c.k);
    if key(a) <= key(b) { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests;
