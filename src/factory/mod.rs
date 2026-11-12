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
pub mod fluids;
pub mod recipes;

use std::time::{Duration, Instant};

use glam::DVec3;
use serde::{Deserialize, Serialize};

pub use draw::Art;
use fluids::{EMPTY, Fluid, FluidNet, FluidStack, PIPE_HOLDS, Pipe, TANK_HOLDS, Tank};

use crate::{
    render::models,
    survival::{
        furnace::Furnace,
        inventory::{self, Slot},
        items::{Item, ItemStack},
    },
    ui::screen::CHEST_SLOTS,
    util::{FastMap, FastSet},
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
/// What a solar panel makes with the sun straight above it (W): as much less as the sun is
/// lower, nothing at night.
pub const SOLAR_POWER: f32 = 20_000.0;
/// What a big door's motor takes while it moves (W).
pub const DOOR_POWER: f32 = 20_000.0;
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

// Steam, heat and the other power sources (gameplay numbers, kept in step with each other).
/// Energy a litre of steam carries from what boiled it to a turbine (J).
pub const STEAM_ENERGY: f32 = 100_000.0;
/// What a steam turbine makes at most (W): 50 litres of steam a second. It gives back nine
/// tenths of it as water.
pub const TURBINE_POWER: f32 = 5_000_000.0;
pub const CONDENSED: f32 = 0.9;
/// Heat a boiler turns into steam (W), and how much more a fuel gives it than a generator (a
/// boiler and turbine get more out of coal).
pub const BOILER_HEAT: f32 = 1_200_000.0;
pub const BOILER_GAIN: f32 = 1.5;
/// Heat a geothermal well gives (W), and how deep under the sea it finds that anywhere (layers;
/// else it needs lava or molten rock near it).
pub const GEOTHERMAL_HEAT: f32 = 2_000_000.0;
pub const GEOTHERMAL_DEPTH: i32 = 150;
/// What a wind turbine makes at full wind (W), the wind that takes (m/s at the hub), the least
/// that turns it, and the most it runs in (it stops in a storm).
pub const WIND_POWER: f32 = 150_000.0;
pub const WIND_RATED: f32 = 11.0;
pub const WIND_CUT_IN: f32 = 2.5;
pub const WIND_CUT_OUT: f32 = 28.0;
/// The wind at a turbine's hub against the weather's near the ground, and the height (m over
/// the sea) that doubles it again.
pub const HUB_WIND: f32 = 1.6;
pub const WIND_HEIGHT: f32 = 250.0;
/// An RTG's power (W), day and night, for ever.
pub const RTG_POWER: f32 = 10_000.0;
/// Litres of water a pump brings up a second.
pub const PUMP_RATE: f32 = 300.0;
/// A reactor's heat at full power (W); its control rods move this share of full power a
/// second; the share of its heat that goes on after it stops (decay heat) and how long that
/// takes to die away (s); what it holds per degree (J/°C), where it runs (°C at full power,
/// cooled), where it warns, and where it melts down.
pub const REACTOR_HEAT: f32 = 25_000_000.0;
pub const ROD_SPEED: f32 = 0.25;
pub const DECAY_HEAT: f32 = 0.065;
pub const DECAY_TIME: f32 = 40.0;
pub const REACTOR_CAPACITY: f32 = 1_700_000.0;
pub const AMBIENT: f32 = 25.0;
pub const RUNNING: f32 = 320.0;
pub const OVERHEATING: f32 = 800.0;
pub const MELTDOWN: f32 = 1200.0;
/// Heat in a fuel rod (J): 200 s at full power; a MOX rod lasts half as long again.
pub const ROD_ENERGY: f32 = REACTOR_HEAT * 200.0;

/// What a machine's input tanks hold at least (litres), and its output tanks.
pub const INPUT_HOLDS: f32 = 500.0;
pub const OUTPUT_HOLDS: f32 = 2_000.0;

/// The cells a model block fills when its first cell is `core` (`Block::structure`): a square
/// round it, that many up, the core first. None where it would go off the face or across a
/// change in the grid (deep down), which it can't.
pub fn footprint(planet: &Planet, core: BlockCell, block: Block) -> Option<Vec<BlockCell>> {
    let (r, h) = block.structure();
    let n = planet.res_at(core.k);
    let mut cells = vec![core];
    for dk in 0..h {
        for dv in -r..=r {
            for du in -r..=r {
                if (du, dv, dk) == (0, 0, 0) {
                    continue;
                }
                let (u, v, k) = (core.u + du, core.v + dv, core.k + dk);
                if u < 0 || v < 0 || u >= n || v >= n {
                    return None;
                }
                let cell = planet.cell(core.face, u, v, k)?;
                if planet.res_at(k) != n {
                    return None;
                }
                cells.push(cell);
            }
        }
    }
    Some(cells)
}

/// Heat in a fuel rod (J), or None if it isn't one.
pub fn rod_energy(item: Item) -> Option<f32> {
    match item {
        Item::UraniumFuelRod => Some(ROD_ENERGY),
        Item::MoxFuelRod => Some(ROD_ENERGY * 1.5),
        _ => None,
    }
}

/// What a wind turbine makes in a wind of `speed` (m/s at the hub).
pub fn wind_power(speed: f32) -> f32 {
    if !(WIND_CUT_IN..WIND_CUT_OUT).contains(&speed) {
        return 0.0;
    }
    let t = ((speed - WIND_CUT_IN) / (WIND_RATED - WIND_CUT_IN)).min(1.0);
    WIND_POWER * t * t * t.sqrt()
}

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

/// The cells next to `cell`: +u, -u, +v, -v, up, down (itself where there is none, or the grid
/// changes there).
fn around(planet: &Planet, cell: BlockCell) -> [BlockCell; 6] {
    let n = planet.res_at(cell.k);
    let at = |du: i32, dv: i32, dk: i32| {
        let k = cell.k + dk;
        match planet.cell(cell.face, cell.u + du, cell.v + dv, k) {
            Some(c) if dk == 0 || planet.res_at(k) == n => c,
            _ => cell,
        }
    };
    [
        at(1, 0, 0),
        at(-1, 0, 0),
        at(0, 1, 0),
        at(0, -1, 0),
        at(0, 0, 1),
        at(0, 0, -1),
    ]
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
    Solar,
    /// A big door's motor (in the bottom middle cell of the door).
    BigDoor,
    Tank,
    WaterPump,
    Electrolyzer,
    Centrifuge,
    FuelFabricator,
    Boiler,
    Geothermal,
    Wind,
    Rtg,
    Blender,
    SteamTurbine,
    Reactor,
    CoolingPool,
    Reprocessing,
    Accelerator,
}

impl MachineKind {
    pub fn of(block: Block) -> Option<MachineKind> {
        Some(match block {
            Block::StorageTank => MachineKind::Tank,
            Block::WaterPump => MachineKind::WaterPump,
            Block::Electrolyzer => MachineKind::Electrolyzer,
            Block::Centrifuge => MachineKind::Centrifuge,
            Block::FuelFabricator => MachineKind::FuelFabricator,
            Block::Boiler => MachineKind::Boiler,
            Block::GeothermalWell => MachineKind::Geothermal,
            Block::WindTurbine => MachineKind::Wind,
            Block::Rtg => MachineKind::Rtg,
            Block::Blender => MachineKind::Blender,
            Block::SteamTurbine => MachineKind::SteamTurbine,
            Block::NuclearReactor => MachineKind::Reactor,
            Block::CoolingPool => MachineKind::CoolingPool,
            Block::ReprocessingPlant => MachineKind::Reprocessing,
            Block::ParticleAccelerator => MachineKind::Accelerator,
            Block::SolarPanel => MachineKind::Solar,
            Block::BigDoor | Block::BigDoorOpen => MachineKind::BigDoor,
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
            MachineKind::Solar => "solar panel",
            MachineKind::BigDoor => "big door",
            MachineKind::Tank => "storage tank",
            MachineKind::WaterPump => "water pump",
            MachineKind::Electrolyzer => "electrolyzer",
            MachineKind::Centrifuge => "centrifuge",
            MachineKind::FuelFabricator => "fuel fabricator",
            MachineKind::Boiler => "boiler",
            MachineKind::Geothermal => "geothermal well",
            MachineKind::Wind => "wind turbine",
            MachineKind::Rtg => "rtg",
            MachineKind::Blender => "blender",
            MachineKind::SteamTurbine => "steam turbine",
            MachineKind::Reactor => "nuclear reactor",
            MachineKind::CoolingPool => "cooling pool",
            MachineKind::Reprocessing => "reprocessing plant",
            MachineKind::Accelerator => "particle accelerator",
        }
    }

    /// Makes power (it doesn't use it): generators, panels, wind and steam turbines, RTGs.
    pub fn makes_power(self) -> bool {
        matches!(
            self,
            MachineKind::Generator
                | MachineKind::Solar
                | MachineKind::Wind
                | MachineKind::Rtg
                | MachineKind::SteamTurbine
        )
    }

    /// Neither makes nor uses power (a tank, what boils water by its own heat).
    pub fn unpowered(self) -> bool {
        matches!(
            self,
            MachineKind::Tank | MachineKind::Boiler | MachineKind::Geothermal
        )
    }

    /// Has tanks that pipes join (`Machine::tanks`).
    pub fn has_tanks(self) -> bool {
        matches!(
            self,
            MachineKind::Tank
                | MachineKind::WaterPump
                | MachineKind::Electrolyzer
                | MachineKind::Centrifuge
                | MachineKind::FuelFabricator
                | MachineKind::Boiler
                | MachineKind::Geothermal
                | MachineKind::Blender
                | MachineKind::SteamTurbine
                | MachineKind::Reactor
                | MachineKind::CoolingPool
                | MachineKind::Reprocessing
                | MachineKind::Accelerator
                | MachineKind::Refinery
                | MachineKind::ArcFurnace
        )
    }

    /// What it uses while it works (W); 0 for what makes or stores power.
    pub fn power(self) -> f32 {
        match self {
            MachineKind::Generator
            | MachineKind::Accumulator
            | MachineKind::Solar
            | MachineKind::Wind
            | MachineKind::Rtg
            | MachineKind::SteamTurbine
            | MachineKind::Tank
            | MachineKind::Boiler
            | MachineKind::Geothermal => 0.0,
            MachineKind::BigDoor => DOOR_POWER,
            MachineKind::WaterPump => 30_000.0,
            MachineKind::Electrolyzer => 300_000.0,
            MachineKind::Centrifuge => 150_000.0,
            MachineKind::FuelFabricator => 120_000.0,
            MachineKind::Blender => 250_000.0,
            // (Its pumps and controls: without them its rods drop in.)
            MachineKind::Reactor => 150_000.0,
            MachineKind::CoolingPool => 20_000.0,
            MachineKind::Reprocessing => 400_000.0,
            MachineKind::Accelerator => 2_500_000.0,
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
                | MachineKind::Electrolyzer
                | MachineKind::Centrifuge
                | MachineKind::FuelFabricator
                | MachineKind::Blender
                | MachineKind::CoolingPool
                | MachineKind::Reprocessing
                | MachineKind::Accelerator
        )
    }

    /// Has an output slot (conveyors behind it take from there).
    pub fn has_output(self) -> bool {
        self.has_recipes()
            || matches!(
                self,
                MachineKind::ElectricFurnace
                    | MachineKind::Miner
                    | MachineKind::OilPump
                    | MachineKind::Reactor
            )
    }

    /// Takes fuel in its fuel slot: what burns (generator, boiler), fuel rods (reactor).
    pub fn takes_fuel(self, item: Item) -> bool {
        match self {
            MachineKind::Generator | MachineKind::Boiler => item.fuel().is_some(),
            MachineKind::Reactor => rod_energy(item).is_some(),
            _ => false,
        }
    }

    /// Where cables meet it (model units above the cell's floor).
    pub fn cable_height(self) -> f32 {
        match self {
            MachineKind::Miner => models::MINER_TOP,
            MachineKind::Refinery => models::REFINERY_TOP,
            MachineKind::Solar => 8.0,
            // (On the housing over the door.)
            MachineKind::BigDoor => 3.0 * 16.0 + 5.0,
            MachineKind::Rtg => 13.0,
            MachineKind::WaterPump | MachineKind::FuelFabricator => models::MACHINE_TOP,
            MachineKind::Wind => 6.0,
            _ => models::structure_top(self),
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
    /// A solar panel: which way is up where it stands (planet-relative; worked out when it is
    /// placed in space, see `locate`).
    #[serde(skip)]
    pub up: Option<[f32; 3]>,
    /// A big door: its motor wants to run (the door is moving).
    #[serde(skip)]
    pub wanted: bool,
    /// What it holds of fluids: two tanks for what it takes, two for what it makes (see
    /// `fluids`; a storage tank has only the first).
    #[serde(default)]
    pub tanks: [Tank; 4],
    /// A second product (a recipe's `extra`; a reactor's spent rods go to `output`).
    #[serde(default)]
    pub extra: Slot,
    /// A reactor: how hot it is (°C), how far its rods are out (0..1, its power), and the heat
    /// still coming from what it has burned (W).
    #[serde(default)]
    pub heat: f32,
    #[serde(default)]
    pub level: f32,
    #[serde(default)]
    pub decay: f32,
    /// A wind turbine: the wind at its hub (m/s; the game sets it from the weather), and how
    /// high it stands over the sea (m).
    #[serde(skip)]
    pub wind: f32,
    #[serde(skip)]
    pub altitude: f32,
}

impl Machine {
    fn new(cell: BlockCell, kind: MachineKind) -> Self {
        // (A machine that makes only one thing is set to make it.)
        let mut list = recipes::for_machine(kind);
        let recipe = match (list.next(), list.next()) {
            (Some(only), None) => Some(only.output),
            _ => None,
        };
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
            recipe,
            resource: None,
            up: None,
            wanted: false,
            tanks: [None; 4],
            extra: None,
            heat: AMBIENT,
            level: 0.0,
            decay: 0.0,
            wind: 0.0,
            altitude: 0.0,
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
            self.extra,
        ]
        .into_iter()
        .flatten()
    }

    /// What input tank `i` (0 or 1) takes, and how much it holds; None if nothing.
    pub fn fluid_input(&self, i: usize) -> Option<(Fluid, f32)> {
        match self.kind {
            MachineKind::Reactor if i == 0 => Some((Fluid::Water, 4.0 * INPUT_HOLDS)),
            MachineKind::Boiler | MachineKind::Geothermal if i == 0 => {
                Some((Fluid::Water, INPUT_HOLDS))
            }
            MachineKind::SteamTurbine if i == 0 => Some((Fluid::Steam, INPUT_HOLDS)),
            k if k.has_recipes() => {
                let &(fluid, n) = self.current_recipe()?.fluids.get(i)?;
                Some((fluid, (4.0 * n as f32).max(INPUT_HOLDS)))
            }
            _ => None,
        }
    }

    /// Makes `fluid` (into an output tank), as it is set now.
    pub fn makes(&self, fluid: Fluid) -> bool {
        match self.kind {
            MachineKind::WaterPump | MachineKind::SteamTurbine => fluid == Fluid::Water,
            MachineKind::Boiler | MachineKind::Geothermal | MachineKind::Reactor => {
                fluid == Fluid::Steam
            }
            k if k.has_recipes() => self
                .current_recipe()
                .is_some_and(|r| r.fluid_out.iter().any(|&(f, _)| f == fluid)),
            _ => false,
        }
    }

    /// Takes `fluid`, makes it, or holds some of it: a pipe line carrying it joins this machine.
    /// (A storage tank: if it holds nothing else.)
    pub fn uses(&self, fluid: Fluid) -> bool {
        if self.kind == MachineKind::Tank {
            return self.tanks[0].is_none_or(|s| s.fluid == fluid || s.amount <= EMPTY);
        }
        (0..2).any(|i| self.fluid_input(i).is_some_and(|(f, _)| f == fluid))
            || self.makes(fluid)
            || self.holds(fluid)
    }

    /// Has some of `fluid` in a tank.
    fn holds(&self, fluid: Fluid) -> bool {
        self.tanks
            .iter()
            .any(|t| t.is_some_and(|s| s.fluid == fluid && s.amount > EMPTY))
    }

    /// How much an output tank holds.
    pub fn output_holds(&self) -> f32 {
        match self.kind {
            MachineKind::Tank => TANK_HOLDS,
            MachineKind::Reactor => 2.0 * OUTPUT_HOLDS,
            _ => OUTPUT_HOLDS,
        }
    }

    /// Puts what a recipe or machine makes of `fluid` into an output tank (the one with it in,
    /// or an empty one); returns how much went in.
    fn put_fluid(&mut self, fluid: Fluid, amount: f32) -> f32 {
        let room = self.output_holds();
        let [_, _, a, b] = &mut self.tanks;
        let i = if a.is_some_and(|s| s.fluid == fluid)
            || (a.is_none() && !b.is_some_and(|s| s.fluid == fluid))
        {
            0
        } else {
            1
        };
        let tank = if i == 0 { a } else { b };
        fluids::fill(tank, fluid, amount, room)
    }

    /// Room for `amount` more of `fluid` in its output tanks.
    fn fluid_room(&self, fluid: Fluid, amount: f32) -> bool {
        let room = self.output_holds();
        self.tanks[2..].iter().any(|t| match t {
            Some(s) if s.fluid == fluid => s.amount + amount <= room,
            Some(s) => s.amount <= EMPTY && amount <= room,
            None => amount <= room,
        })
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
    /// accumulator: more than half full. A reactor or boiler: making heat.
    pub fn lit(&self) -> bool {
        match self.kind {
            MachineKind::Accumulator => self.energy > self.energy_total * 0.5,
            MachineKind::Reactor => self.level > 0.02,
            MachineKind::Boiler | MachineKind::Geothermal => self.progress > 0.0,
            MachineKind::Tank => false,
            _ => self.power > 0.0,
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

    /// Room in the second output for `count` more of `item`.
    fn extra_room(&self, item: Item, count: u8) -> bool {
        match self.extra {
            None => true,
            Some(s) => s.item == item && s.count as u32 + count as u32 <= item.max_stack() as u32,
        }
    }

    /// Has all a recipe takes (items and fluids), whether or not there is room for what it makes.
    fn has_ingredients(&self, recipe: &recipes::Recipe) -> bool {
        let items = (recipe.inputs.iter().zip(self.inputs()))
            .all(|(&(item, n), slot)| slot.is_some_and(|s| s.item == item && s.count >= n));
        let fluids = (recipe.fluids.iter().zip(&self.tanks))
            .all(|(&(fluid, n), tank)| fluids::level(tank, fluid) >= n as f32);
        items && fluids
    }

    /// Room for all a recipe makes.
    fn has_room(&self, recipe: &recipes::Recipe) -> bool {
        let item = match recipe.output {
            Item::Fluid(_) => true,
            out => self.room_for(out, recipe.count),
        };
        let extra = recipe.extra.is_none_or(|(i, n)| self.extra_room(i, n));
        // (Two fluids: each into its own tank.)
        let fluids = match recipe.fluid_out {
            [(a, n)] => self.fluid_room(*a, *n as f32),
            [(a, n), (b, m)] => {
                let room = self.output_holds();
                let fits = |t: &Tank, f: Fluid, x: u16| match t {
                    Some(s) if s.amount > EMPTY => s.fluid == f && s.amount + x as f32 <= room,
                    _ => x as f32 <= room,
                };
                let [_, _, t, u] = &self.tanks;
                (fits(t, *a, *n) && fits(u, *b, *m)) || (fits(t, *b, *m) && fits(u, *a, *n))
            }
            _ => true,
        };
        item && extra && fluids
    }

    /// Whether it has everything for what it makes next, and room for it.
    fn can_make(&self) -> bool {
        match self.kind {
            MachineKind::ElectricFurnace => self
                .input
                .and_then(|s| s.item.smelted())
                .is_some_and(|out| self.room_for(out, 1)),
            MachineKind::Miner | MachineKind::OilPump => {
                self.resource.is_some_and(|out| self.room_for(out, 1))
            }
            MachineKind::WaterPump => {
                self.resource.is_some() && self.fluid_room(Fluid::Water, PUMP_RATE * 0.1)
            }
            MachineKind::Reactor => {
                (self.energy > 0.0 || self.fuel.is_some_and(|f| rod_energy(f.item).is_some()))
                    && self.room_for(Item::SpentFuelRod, 1)
            }
            k if k.has_recipes() => self
                .current_recipe()
                .is_some_and(|r| self.has_ingredients(r) && self.has_room(r)),
            _ => false,
        }
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

    /// Wants power now: a machine with something to do, a big door that is moving.
    fn working(&self) -> bool {
        if self.kind == MachineKind::BigDoor {
            return self.wanted;
        }
        self.kind.power() > 0.0 && self.can_make()
    }

    /// What a solar panel makes with the sun the way `sun` points (planet-relative, unit).
    fn sunlight(&self, sun: DVec3) -> f32 {
        let up = self
            .up
            .map_or(DVec3::ZERO, |u| DVec3::from(u.map(f64::from)));
        SOLAR_POWER * (up.dot(sun) as f32).max(0.0)
    }

    /// What its screen shows of its tanks (and a reactor's heat): a line each, in the order
    /// and number `gauge_count` says.
    pub fn gauges(&self) -> Vec<Gauge> {
        let tank = |i: usize, want: Option<Fluid>, holds: f32, into: bool| {
            let held = self.tanks[i];
            let fluid = held.map(|s| s.fluid).or(want);
            let amount = held.map_or(0.0, |s| s.amount);
            let name = fluid.map_or("empty", Fluid::name);
            let way = if into { "in" } else { "out" };
            Gauge {
                text: format!("{way}: {name} {amount:.0} / {holds:.0} l"),
                fill: amount / holds.max(1.0),
                color: fluid.map_or([200, 200, 200], Fluid::color),
            }
        };
        let input = |i: usize| {
            let (f, holds) = self.fluid_input(i).unwrap_or((Fluid::Water, INPUT_HOLDS));
            tank(i, Some(f), holds, true)
        };
        let output = |i: usize, f: Option<Fluid>| tank(i, f, self.output_holds(), false);
        let mut out = Vec::new();
        match self.kind {
            MachineKind::Reactor => {
                out.push(input(0));
                out.push(output(2, Some(Fluid::Steam)));
                out.push(Gauge {
                    text: format!("core {:.0} °c (melts at {:.0})", self.heat, MELTDOWN),
                    fill: self.heat / MELTDOWN,
                    color: if self.heat >= OVERHEATING {
                        [255, 80, 50]
                    } else {
                        [255, 190, 90]
                    },
                });
            }
            MachineKind::Boiler | MachineKind::Geothermal => {
                out.push(input(0));
                out.push(output(2, Some(Fluid::Steam)));
            }
            MachineKind::SteamTurbine => {
                out.push(input(0));
                out.push(output(2, Some(Fluid::Water)));
            }
            MachineKind::WaterPump => out.push(output(2, Some(Fluid::Water))),
            MachineKind::Tank => {
                let held = self.tanks[0];
                let amount = held.map_or(0.0, |s| s.amount);
                out.push(Gauge {
                    text: match held {
                        Some(s) => format!("{} {amount:.0} / {TANK_HOLDS:.0} l", s.fluid.name()),
                        None => format!("empty, holds {TANK_HOLDS:.0} l"),
                    },
                    fill: amount / TANK_HOLDS,
                    color: held.map_or([200, 200, 200], |s| s.fluid.color()),
                });
            }
            k if k.has_recipes() => {
                if let Some(r) = self.current_recipe() {
                    for i in 0..r.fluids.len() {
                        out.push(input(i));
                    }
                    for (j, &(f, _)) in r.fluid_out.iter().enumerate() {
                        // (What it makes goes into whichever output tank has it.)
                        let i = if self.tanks[3].is_some_and(|s| s.fluid == f) {
                            3
                        } else if self.tanks[2].is_some_and(|s| s.fluid == f) {
                            2
                        } else {
                            2 + j
                        };
                        out.push(output(i, Some(f)));
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// What a wind turbine, RTG or steam turbine makes this tick (W).
    fn made_power(&self, dt: f32) -> f32 {
        match self.kind {
            MachineKind::Wind => wind_power(self.wind),
            MachineKind::Rtg => RTG_POWER,
            MachineKind::SteamTurbine => {
                let steam = fluids::level(&self.tanks[0], Fluid::Steam);
                TURBINE_POWER.min(steam * STEAM_ENERGY / dt)
            }
            _ => 0.0,
        }
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

    /// Puts `count` of `item` into the output.
    fn give(&mut self, item: Item, count: u8) {
        match &mut self.output {
            Some(out) => out.count += count,
            None => self.output = Some(ItemStack::new(item, count)),
        }
    }

    /// Works for `dt` seconds' worth of full power: smelts, mines, pumps or makes. (A reactor
    /// works in `Factory::step_heat`.)
    fn work(&mut self, dt: f32) {
        if !self.can_make() || self.kind == MachineKind::Reactor {
            self.progress = 0.0;
            return;
        }
        if self.kind == MachineKind::WaterPump {
            self.put_fluid(Fluid::Water, PUMP_RATE * dt);
            self.progress = 1.0;
            return;
        }
        self.progress += dt;
        if self.progress < self.work_time() {
            return;
        }
        self.progress = 0.0;
        match self.kind {
            MachineKind::ElectricFurnace => {
                if let Some(out) = self.input.and_then(|s| s.item.smelted()) {
                    inventory::take(&mut self.input, 1);
                    self.give(out, 1);
                }
            }
            MachineKind::Miner | MachineKind::OilPump => {
                if let Some(out) = self.resource {
                    self.give(out, 1);
                }
            }
            k if k.has_recipes() => {
                let Some(recipe) = self.current_recipe() else {
                    return;
                };
                for (slot, &(_, n)) in self.inputs_mut().into_iter().zip(recipe.inputs) {
                    inventory::take(slot, n);
                }
                for (i, &(fluid, n)) in recipe.fluids.iter().enumerate() {
                    fluids::drain(&mut self.tanks[i], fluid, n as f32);
                }
                if !matches!(recipe.output, Item::Fluid(_)) {
                    self.give(recipe.output, recipe.count);
                }
                if let Some((item, n)) = recipe.extra {
                    match &mut self.extra {
                        Some(s) => s.count += n,
                        None => self.extra = Some(ItemStack::new(item, n)),
                    }
                }
                for &(fluid, n) in recipe.fluid_out {
                    self.put_fluid(fluid, n as f32);
                }
            }
            _ => {}
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
        if self.kind.takes_fuel(item) {
            // (A reactor a few rods at a time.)
            let most = if self.kind == MachineKind::Reactor {
                4
            } else {
                item.max_stack()
            };
            return fits(self.fuel, most);
        }
        self.slot_for(item)
            .is_some_and(|(i, most)| fits(self.inputs()[i], most))
    }

    /// Takes one item from a conveyor, into the slot it belongs in.
    fn accept(&mut self, stack: ItemStack) -> bool {
        if !self.wants(stack.item) {
            return false;
        }
        if self.kind.takes_fuel(stack.item) {
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
        // (Fluids the new recipe doesn't take are let out.)
        for i in 0..2 {
            let want = self.fluid_input(i).map(|(f, _)| f);
            if self.tanks[i].is_some_and(|s| Some(s.fluid) != want) {
                self.tanks[i] = None;
            }
        }
        out
    }

    /// A few words on what it is doing (the HUD, its screen).
    pub fn status(&self) -> String {
        let kw = |w: f32| {
            if w >= 1.0e6 {
                format!("{:.1} mw", w / 1.0e6)
            } else {
                format!("{:.0} kw", w / 1000.0)
            }
        };
        let percent = |x: f32| format!("{:.0}%", x * 100.0);
        match self.kind {
            MachineKind::Generator if self.power > 0.0 => format!("making {}", kw(self.power)),
            MachineKind::Generator => "no fuel".to_string(),
            MachineKind::Solar if self.power > 0.0 => format!("making {}", kw(self.power)),
            MachineKind::Solar => "no sun".to_string(),
            MachineKind::BigDoor if self.wanted && self.supply <= 0.0 => "no power".to_string(),
            MachineKind::BigDoor if self.wanted => format!("moving, {}", kw(self.power)),
            MachineKind::BigDoor => "ready".to_string(),
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
            MachineKind::WaterPump if self.resource.is_none() => "no water next to it".to_string(),
            MachineKind::Geothermal if self.resource.is_none() => {
                "no heat down here: put it near lava, or deep".to_string()
            }
            MachineKind::Wind if self.power > 0.0 => {
                format!("making {}, wind {:.0} m/s", kw(self.power), self.wind)
            }
            MachineKind::Wind if self.wind >= WIND_CUT_OUT => {
                format!("stopped: wind {:.0} m/s is too strong", self.wind)
            }
            MachineKind::Wind => format!("no wind ({:.0} m/s)", self.wind),
            MachineKind::Rtg => format!("making {}", kw(self.power)),
            MachineKind::SteamTurbine if self.power > 0.0 => format!("making {}", kw(self.power)),
            MachineKind::SteamTurbine => "no steam".to_string(),
            MachineKind::Tank => match self.tanks[0] {
                Some(s) => format!("{:.0} l of {}", s.amount, s.fluid.name()),
                None => "empty".to_string(),
            },
            MachineKind::Boiler | MachineKind::Geothermal => {
                let water = fluids::level(&self.tanks[0], Fluid::Water);
                if self.progress > 0.0 {
                    format!("boiling, {}", kw(self.progress))
                } else if water <= EMPTY {
                    "no water".to_string()
                } else if self.kind == MachineKind::Boiler {
                    "no fuel".to_string()
                } else {
                    "steam has nowhere to go".to_string()
                }
            }
            MachineKind::Reactor => {
                let water = fluids::level(&self.tanks[0], Fluid::Water);
                let heat = format!("{:.0} °c", self.heat);
                if self.heat >= OVERHEATING {
                    if water <= EMPTY {
                        format!("overheating! {heat}: no water")
                    } else {
                        format!("overheating! {heat}")
                    }
                } else if self.level > 0.02 {
                    format!(
                        "{} heat, {heat}",
                        kw(self.level * REACTOR_HEAT + self.decay)
                    )
                } else if !self.can_make() {
                    format!("no fuel rods, {heat}")
                } else if self.supply <= 0.0 {
                    format!("no power for its pumps, {heat}")
                } else {
                    format!("idle: steam has nowhere to go, {heat}")
                }
            }
            k if k.has_recipes() && self.recipe.is_none() => "pick what to make".to_string(),
            _ if !self.working() => {
                let full = self.output.is_some_and(|s| s.count >= s.item.max_stack())
                    || (self.has_inputs() && !self.can_make());
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
            Some(r) => self.has_ingredients(r),
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

/// A line on a machine's screen about one of its tanks: what it says, how full, its colour.
pub struct Gauge {
    pub text: String,
    pub fill: f32,
    pub color: [u8; 3],
}

/// How many gauges a machine's screen shows (`Machine::gauges`), set to make `recipe`.
pub fn gauge_count(kind: MachineKind, recipe: Option<Item>) -> usize {
    match kind {
        MachineKind::Reactor => 3,
        MachineKind::Boiler | MachineKind::Geothermal | MachineKind::SteamTurbine => 2,
        MachineKind::WaterPump | MachineKind::Tank => 1,
        k if k.has_recipes() => recipe
            .and_then(|r| recipes::find(kind, r))
            .map_or(0, |r| r.fluids.len() + r.fluid_out.len()),
        _ => 0,
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
        Block::DiamondOre => 11,
        Block::GoldOre => 10,
        Block::UraniumOre => 9,
        Block::FluoriteOre => 8,
        Block::LeadOre => 7,
        Block::CopperOre => 6,
        Block::SulfurOre => 5,
        Block::IronOre => 4,
        Block::SaltpeterOre => 3,
        Block::BauxiteOre => 2,
        Block::CoalOre => 1,
        _ => 0,
    }
}

/// A storage tank that takes part in a network carrying `fluid` (empty, or holding it).
fn machines_pooled(m: &Machine, fluid: Option<Fluid>) -> bool {
    m.tanks[0].is_none_or(|s| Some(s.fluid) == fluid || s.amount <= EMPTY)
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
    /// (Every cell of a big machine.)
    Machine(u32),
    Pole(u32),
    Pipe(u32),
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
    pipes: Vec<Pipe>,
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
    pipes: Vec<Pipe>,
    /// The cells of big machines, each to the machine's own (its first) cell.
    parts: FastMap<BlockCell, BlockCell>,
    /// Every machine's cells are in `parts` (worked out once placed in space: `locate`).
    parts_known: bool,
    /// Pipes joined together, and the network of each pipe.
    fluid_nets: Vec<FluidNet>,
    pipe_net: Vec<u32>,
    /// Pipes' joins, which of them are machines, and their line's fluid (`ModelState::links`,
    /// `ports`, `fluid`), for their models, after every relink.
    pub pipes_changed: Vec<(BlockCell, u8, u8, u8)>,
    /// Each machine's recipe when the pipes were last joined up: a change rejoins them (a
    /// pipe line joins only machines that use its fluid).
    piped_recipes: Vec<Option<Item>>,
    /// Reactors that melted down (their first cells), for the game to wreck.
    pub meltdowns: Vec<BlockCell>,
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
    /// The way to the sun in this world's frame (unit; the game sets it every frame), for
    /// solar panels.
    pub sun: DVec3,
    /// Every network has all the power it asks for (`power infinite`).
    pub infinite: bool,
}

impl Factory {
    pub fn saved(&self) -> SavedFactory {
        SavedFactory {
            conveyors: self.conveyors.clone(),
            machines: self.machines.clone(),
            poles: self.poles.clone(),
            cables: self.cables.clone(),
            pipes: self.pipes.clone(),
        }
    }

    pub fn restored(saved: SavedFactory) -> Self {
        Self {
            conveyors: saved.conveyors,
            machines: saved.machines,
            poles: saved.poles,
            cables: saved.cables,
            pipes: saved.pipes,
            unlocated: true,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.conveyors.is_empty()
            && self.machines.is_empty()
            && self.poles.is_empty()
            && self.pipes.is_empty()
    }

    pub fn pipes(&self) -> &[Pipe] {
        &self.pipes
    }

    /// The first cell of the machine `cell` is a cell of (a big machine's), else `cell`.
    pub fn core(&self, cell: BlockCell) -> BlockCell {
        self.parts.get(&cell).copied().unwrap_or(cell)
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
        for (i, p) in self.pipes.iter().enumerate() {
            self.index.insert(p.cell, Node::Pipe(i as u32));
        }
        for (&part, core) in &self.parts {
            if let Some(&node @ Node::Machine(_)) = self.index.get(core) {
                self.index.insert(part, node);
            }
        }
        self.indexed = true;
    }

    /// Notes the cells of a big machine (`footprint`).
    fn add_parts(&mut self, planet: &Planet, cell: BlockCell, kind: MachineKind) {
        let Some(block) = Block::ALL
            .iter()
            .copied()
            .find(|b| MachineKind::of(*b) == Some(kind))
        else {
            return;
        };
        if !block.is_structure() {
            return;
        }
        for part in footprint(planet, cell, block)
            .unwrap_or_default()
            .into_iter()
            .skip(1)
        {
            self.parts.insert(part, cell);
        }
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
            Block::Pipe => {
                // (Joining all it touches until told otherwise: `set_pipe_ways`.)
                self.pipes.push(Pipe {
                    cell,
                    held: None,
                    ways: None,
                    axis: 0,
                    around: Some(around(planet, cell)),
                    links: 0,
                });
                Node::Pipe(self.pipes.len() as u32 - 1)
            }
            _ => {
                let Some(kind) = MachineKind::of(block) else {
                    return;
                };
                self.machines.push(Machine::new(cell, kind));
                self.add_parts(planet, cell, kind);
                self.indexed = false;
                if kind.has_tanks() {
                    // Pipe ends pointing into any of its cells join it.
                    let cells: FastSet<BlockCell> = std::iter::once(cell)
                        .chain(self.parts.iter().filter(|p| *p.1 == cell).map(|p| *p.0))
                        .collect();
                    for p in &mut self.pipes {
                        let (Some(around), Some(mut ways)) = (p.around, p.ways) else {
                            continue;
                        };
                        let open = p.open_ends();
                        for (side, c) in around.iter().enumerate() {
                            if open & (1 << side) != 0 && cells.contains(c) {
                                ways |= 1 << side;
                            }
                        }
                        p.ways = Some(ways);
                    }
                }
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
                // (A big machine: all its cells go, and the index is worked out again.)
                let core = m.cell;
                self.parts.retain(|_, c| *c != core);
                self.indexed = false;
                let before = self.cables.len();
                self.cables.retain(|&(a, b)| a != core && b != core);
                self.linked = false;
                let cables = (before - self.cables.len()) as u32;
                return Some(Removed { contents, cables });
            }
            Node::Pole(i) => {
                self.poles.swap_remove(i as usize);
                if let Some(&moved) = self.poles.get(i as usize) {
                    self.index.insert(moved, Node::Pole(i));
                }
            }
            Node::Pipe(i) => {
                self.pipes.swap_remove(i as usize);
                if let Some(moved) = self.pipes.get(i as usize) {
                    self.index.insert(moved.cell, Node::Pipe(i));
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
        // (Before the index is built: a saved game just loaded, a machine just placed.)
        let cell = self.core(cell);
        if let Some(i) = self.pipes.iter().position(|p| p.cell == cell) {
            return Some(Node::Pipe(i as u32));
        }
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
        match self.node(cell) {
            Some(Node::Pole(_)) => true,
            Some(Node::Machine(i)) => self
                .machines
                .get(i as usize)
                .is_some_and(|m| !m.kind.unpowered()),
            _ => false,
        }
    }

    pub fn connected(&self, a: BlockCell, b: BlockCell) -> bool {
        let pair = ordered(self.core(a), self.core(b));
        self.cables.contains(&pair)
    }

    /// Runs a cable between two poles or machines; false if there already is one (or they
    /// can't take one). (A big machine's goes to its first cell.)
    pub fn connect(&mut self, a: BlockCell, b: BlockCell) -> bool {
        let (a, b) = (self.core(a), self.core(b));
        if a == b || self.connected(a, b) || !self.is_electric(a) || !self.is_electric(b) {
            return false;
        }
        self.cables.push(ordered(a, b));
        self.linked = false;
        true
    }

    pub fn disconnect(&mut self, a: BlockCell, b: BlockCell) -> bool {
        let pair = ordered(self.core(a), self.core(b));
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
        self.relink_pipes();
        self.linked = true;
    }

    /// Pipe networks: pipes joined (`Pipe::ways`, either one's) that carry the same fluid or
    /// both nothing yet; a line that carries nothing joins one carrying something (and then
    /// carries it too) if all it is joined to carry the same. Then the machines with tanks they
    /// join that use their fluid (any, for a line that carries nothing yet). Each pipe's joins
    /// (for its model). Where networks with different fluids were joined (only an old save
    /// could), the one most of it holds wins and the rest is lost.
    fn relink_pipes(&mut self) {
        let n = self.pipes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        self.piped_recipes = self.machines.iter().map(|m| m.recipe).collect();
        // (Pipes from an old save, or put down whole: they join all they touch.)
        for i in 0..n {
            let Some(around) = self.pipes[i]
                .around
                .filter(|_| self.pipes[i].ways.is_none())
            else {
                continue;
            };
            let own = self.pipes[i].cell;
            let mut ways = 0;
            for (side, c) in around.iter().enumerate() {
                let joins = match self.index.get(c) {
                    Some(Node::Pipe(_)) => true,
                    Some(Node::Machine(m)) => self.machines[*m as usize].kind.has_tanks(),
                    _ => false,
                };
                if *c != own && joins {
                    ways |= 1 << side;
                }
            }
            self.pipes[i].ways = Some(ways);
        }
        // Which pipes are joined: (i, its side, j).
        let mut joins: Vec<(usize, usize, usize)> = Vec::new();
        for i in 0..n {
            let Some(around) = self.pipes[i].around else {
                continue;
            };
            let own = self.pipes[i].cell;
            let ways = self.pipes[i].ways.unwrap_or(0);
            for (side, c) in around.iter().enumerate() {
                let Some(&Node::Pipe(j)) = self.index.get(c).filter(|_| *c != own) else {
                    continue;
                };
                let j = j as usize;
                let back = self.pipes[j]
                    .around
                    .and_then(|a| a.iter().position(|x| *x == own));
                let theirs = back.is_some_and(|b| self.pipes[j].ways.unwrap_or(0) & (1 << b) != 0);
                if ways & (1 << side) != 0 || theirs {
                    joins.push((i, side, j));
                }
            }
        }
        for &(i, _, j) in &joins {
            if self.pipes[i].carries() == self.pipes[j].carries() {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
        // Lines that carry nothing yet, joined to lines that do: with the one fluid there.
        let mut found: FastMap<usize, Option<Fluid>> = FastMap::default();
        for &(i, _, j) in &joins {
            if self.pipes[i].carries().is_none()
                && let Some(f) = self.pipes[j].carries()
            {
                let r = root(&mut parent, i);
                let seen = found.entry(r).or_insert(Some(f));
                if *seen != Some(f) {
                    *seen = None;
                }
            }
        }
        for i in 0..n {
            let r = root(&mut parent, i);
            if let Some(Some(f)) = found.get(&r) {
                self.pipes[i].held = Some(FluidStack {
                    fluid: *f,
                    amount: 0.0,
                });
            }
        }
        for &(i, _, j) in &joins {
            if self.pipes[i].carries() == self.pipes[j].carries() {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
        let mut links = vec![0u8; n];
        for &(i, side, j) in &joins {
            if root(&mut parent, i) == root(&mut parent, j) {
                links[i] |= 1 << side;
            }
        }
        // The machines they join.
        let mut touching: Vec<(usize, u32)> = Vec::new();
        for (i, mut links) in links.into_iter().enumerate() {
            let pipe = &self.pipes[i];
            let Some(around) = pipe.around else {
                continue;
            };
            let mut ports = 0u8;
            let (own, carries) = (pipe.cell, pipe.carries());
            let ways = pipe.ways.unwrap_or(0);
            for (side, c) in around.iter().enumerate() {
                if *c == own || ways & (1 << side) == 0 {
                    continue;
                }
                if let Some(&Node::Machine(m)) = self.index.get(c) {
                    let machine = &self.machines[m as usize];
                    if machine.kind.has_tanks() && carries.is_none_or(|f| machine.uses(f)) {
                        links |= 1 << side;
                        ports |= 1 << side;
                        touching.push((i, m));
                    }
                }
            }
            self.pipes[i].links = links;
            let fluid = carries.map_or(0, |f| f as u8 + 1);
            self.pipes_changed.push((own, links, ports, fluid));
        }
        self.fluid_nets.clear();
        let mut net_of: FastMap<usize, u32> = FastMap::default();
        self.pipe_net = (0..n)
            .map(|i| {
                let r = root(&mut parent, i);
                *net_of.entry(r).or_insert_with(|| {
                    self.fluid_nets.push(FluidNet::default());
                    self.fluid_nets.len() as u32 - 1
                })
            })
            .collect();
        for (i, &net) in self.pipe_net.iter().enumerate() {
            self.fluid_nets[net as usize].pipes.push(i as u32);
        }
        for (pipe, m) in touching {
            let net = &mut self.fluid_nets[self.pipe_net[pipe] as usize];
            if !net.machines.contains(&m) {
                net.machines.push(m);
            }
        }
        // One fluid to a network.
        for net in &self.fluid_nets {
            let mut most: Option<(Fluid, f32)> = None;
            let mut totals: Vec<(Fluid, f32)> = Vec::new();
            for &p in &net.pipes {
                if let Some(s) = self.pipes[p as usize].held {
                    match totals.iter_mut().find(|(f, _)| *f == s.fluid) {
                        Some((_, a)) => *a += s.amount,
                        None => totals.push((s.fluid, s.amount)),
                    }
                }
            }
            for &(f, a) in &totals {
                if most.is_none_or(|(_, m)| a > m) {
                    most = Some((f, a));
                }
            }
            for &p in &net.pipes {
                let pipe = &mut self.pipes[p as usize];
                if pipe
                    .held
                    .is_some_and(|s| Some(s.fluid) != most.map(|m| m.0))
                {
                    pipe.held = None;
                }
            }
        }
    }

    /// Places conveyors in space (for drawing and the network), those that aren't yet, and
    /// finds which way is up at solar panels.
    pub fn locate(&mut self, planet: &Planet) {
        for m in &mut self.machines {
            if matches!(m.kind, MachineKind::Solar | MachineKind::Wind) && m.up.is_none() {
                let up = models::cell_point(planet, m.cell, [8.0, 8.0, 16.0])
                    - models::cell_point(planet, m.cell, [8.0, 8.0, 0.0]);
                m.up = Some(up.normalize_or(DVec3::Y).as_vec3().to_array());
                m.altitude = (m.cell.k - planet.sea_layer) as f32;
            }
        }
        if !self.parts_known {
            // (A saved game: where its big machines' cells are.)
            let machines: Vec<(BlockCell, MachineKind)> =
                self.machines.iter().map(|m| (m.cell, m.kind)).collect();
            for (cell, kind) in machines {
                self.add_parts(planet, cell, kind);
            }
            for p in &mut self.pipes {
                if p.around.is_none() {
                    p.around = Some(around(planet, p.cell));
                }
            }
            self.parts_known = true;
            self.indexed = false;
            self.linked = false;
        }
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
        self.step_fluids();
        self.step_power(dt);
        self.step_heat(dt);
        self.step_conveyors(dt, stores);
    }

    /// Each pipe network: what is in its pipes and storage tanks is one pool. The machines on it
    /// that want its fluid take theirs (shared alike when there isn't enough), then what
    /// machines have made of it (or anything, into an empty network) goes in, as far as there is
    /// room, and the pool is spread back over the pipes and tanks.
    fn step_fluids(&mut self) {
        // (Pipes join only what uses their fluid: a recipe changed, they join up again.)
        if !self.pipes.is_empty()
            && self
                .machines
                .iter()
                .map(|m| m.recipe)
                .ne(self.piped_recipes.iter().copied())
        {
            self.linked = false;
        }
        let mut rejoin = false;
        for net in &self.fluid_nets {
            let pipes = &mut self.pipes;
            let machines = &mut self.machines;
            // The pool: its fluid, how much, and how much room.
            let mut fluid: Option<Fluid> = None;
            let mut amount = 0.0;
            for &p in &net.pipes {
                if let Some(s) = pipes[p as usize].held {
                    fluid = Some(s.fluid);
                    amount += s.amount;
                }
            }
            let carried = fluid;
            let tanks: Vec<u32> = net
                .machines
                .iter()
                .copied()
                .filter(|&m| machines[m as usize].kind == MachineKind::Tank)
                .collect();
            for &t in &tanks {
                if let Some(s) = machines[t as usize].tanks[0]
                    && fluid.is_none()
                    && s.amount > EMPTY
                {
                    fluid = Some(s.fluid);
                }
            }
            let mut capacity = net.pipes.len() as f32 * PIPE_HOLDS;
            for &t in &tanks {
                let m = &machines[t as usize];
                if machines_pooled(m, fluid) {
                    capacity += TANK_HOLDS;
                    amount += m.tanks[0].map_or(0.0, |s| s.amount);
                }
            }

            // Out to what wants it.
            if let Some(f) = fluid {
                let wants = |m: &Machine| -> Option<(usize, f32)> {
                    (0..2).find_map(|i| {
                        let (want, holds) = m.fluid_input(i)?;
                        (want == f).then(|| (i, (holds - fluids::level(&m.tanks[i], f)).max(0.0)))
                    })
                };
                let demand: f32 = net
                    .machines
                    .iter()
                    .filter_map(|&m| wants(&machines[m as usize]))
                    .map(|(_, room)| room)
                    .sum();
                if demand > 0.0 && amount > 0.0 {
                    let share = (amount / demand).min(1.0);
                    for &m in &net.machines {
                        let machine = &mut machines[m as usize];
                        if let Some((i, room)) = wants(machine) {
                            let put =
                                fluids::fill(&mut machine.tanks[i], f, room * share, f32::MAX);
                            amount -= put;
                        }
                    }
                }
            }

            // In from what made it.
            for &m in &net.machines {
                if machines[m as usize].kind == MachineKind::Tank {
                    continue;
                }
                for i in 2..4 {
                    let Some(s) = machines[m as usize].tanks[i] else {
                        continue;
                    };
                    if fluid.is_some_and(|f| f != s.fluid) {
                        continue;
                    }
                    // (Into an empty line only what something on it takes, or a tank keeps:
                    // an electrolyzer's oxygen goes down the line to what wants oxygen.)
                    let wanted = !tanks.is_empty()
                        || net.machines.iter().any(|&o| {
                            (0..2).any(|t| {
                                machines[o as usize].fluid_input(t).map(|x| x.0) == Some(s.fluid)
                            })
                        });
                    if fluid.is_none() && !wanted {
                        continue;
                    }
                    if fluid.is_none() {
                        fluid = Some(s.fluid);
                        // (Tanks holding something else stay out of it.)
                        capacity = net.pipes.len() as f32 * PIPE_HOLDS;
                        for &t in &tanks {
                            if machines_pooled(&machines[t as usize], fluid) {
                                capacity += TANK_HOLDS;
                            }
                        }
                    }
                    let machine = &mut machines[m as usize];
                    let put = s.amount.min(capacity - amount).max(0.0);
                    fluids::drain(&mut machine.tanks[i], s.fluid, put);
                    amount += put;
                }
            }

            // Spread back over the pool. (An empty line keeps to what it carried while
            // something on it takes or makes that; else it is free for anything again.)
            let Some(f) = fluid.filter(|_| amount > EMPTY) else {
                let keep = fluid.filter(|&f| {
                    net.machines.iter().any(|&m| {
                        let m = &machines[m as usize];
                        if m.kind == MachineKind::Tank {
                            m.holds(f)
                        } else {
                            m.uses(f)
                        }
                    })
                });
                rejoin |= keep != carried;
                for &p in &net.pipes {
                    pipes[p as usize].held = keep.map(|fluid| FluidStack { fluid, amount: 0.0 });
                }
                for &t in &tanks {
                    let m = &mut machines[t as usize];
                    if m.tanks[0].is_some_and(|s| s.amount <= EMPTY) {
                        m.tanks[0] = None;
                    }
                }
                continue;
            };
            // (A line that carried nothing now carries this: it joins only what uses it.)
            rejoin |= carried.is_none();
            let full = (amount / capacity.max(1.0)).clamp(0.0, 1.0);
            for &p in &net.pipes {
                pipes[p as usize].held = Some(FluidStack {
                    fluid: f,
                    amount: PIPE_HOLDS * full,
                });
            }
            for &t in &tanks {
                let m = &mut machines[t as usize];
                if machines_pooled(m, Some(f)) {
                    m.tanks[0] = Some(FluidStack {
                        fluid: f,
                        amount: TANK_HOLDS * full,
                    });
                }
            }
        }
        if rejoin {
            self.linked = false;
        }
    }

    /// Heat into steam: boilers burn their fuel, geothermal wells take the heat under them, and
    /// reactors burn their rods (moving their rods towards full power while they have power,
    /// fuel and room for steam). What heat the water can't carry off heats a reactor up: dry, it
    /// melts down.
    fn step_heat(&mut self, dt: f32) {
        let authority = self.authority;
        for m in &mut self.machines {
            match m.kind {
                MachineKind::Boiler | MachineKind::Geothermal => {
                    let water = fluids::level(&m.tanks[0], Fluid::Water);
                    let steam_room = m.fluid_room(Fluid::Steam, 1.0);
                    let heat = if m.kind == MachineKind::Boiler {
                        BOILER_HEAT
                    } else if m.resource.is_some() {
                        GEOTHERMAL_HEAT
                    } else {
                        0.0
                    };
                    // As much as the water allows.
                    let wanted = (heat * dt / STEAM_ENERGY).min(water);
                    let mut litres = if steam_room { wanted } else { 0.0 };
                    if m.kind == MachineKind::Boiler && litres > 0.0 {
                        // Fuel for it, a generator's worth and half again.
                        let need = litres * STEAM_ENERGY / BOILER_GAIN;
                        let got = m.burn(need);
                        litres *= got / need;
                    }
                    let was = m.lit();
                    fluids::drain(&mut m.tanks[0], Fluid::Water, litres);
                    m.put_fluid(Fluid::Steam, litres);
                    m.progress = litres * STEAM_ENERGY / dt;
                    if m.lit() != was {
                        self.lit_changed.push((m.cell, m.lit()));
                    }
                }
                MachineKind::Reactor => {
                    let was = m.lit();
                    let fuelled = m.can_make();
                    let powered = m.supply > 0.5 || self.infinite;
                    let steam = fluids::level(&m.tanks[2], Fluid::Steam)
                        .max(fluids::level(&m.tanks[3], Fluid::Steam));
                    let room = steam < 0.9 * m.output_holds();
                    let target = if fuelled && powered && room { 1.0 } else { 0.0 };
                    m.level += (target - m.level).clamp(-ROD_SPEED * dt, ROD_SPEED * dt);
                    // Fission, from the rod in it (the next one goes in when it is spent).
                    let mut fission = m.level * REACTOR_HEAT;
                    let mut need = fission * dt;
                    while need > 0.0 {
                        if m.energy <= 0.0 {
                            if m.energy_total > 0.0 {
                                m.give(Item::SpentFuelRod, 1);
                                m.energy_total = 0.0;
                            }
                            let Some(e) = m.fuel.and_then(|f| rod_energy(f.item)) else {
                                break;
                            };
                            if !m.room_for(Item::SpentFuelRod, 1) {
                                break;
                            }
                            inventory::take(&mut m.fuel, 1);
                            m.energy = e;
                            m.energy_total = e;
                        }
                        let e = need.min(m.energy);
                        m.energy -= e;
                        need -= e;
                    }
                    if need > 0.0 {
                        fission -= need / dt;
                        m.level = (fission / REACTOR_HEAT).max(0.0);
                    }
                    m.decay += (DECAY_HEAT * fission - m.decay) * (dt / DECAY_TIME).min(1.0);
                    let heat = fission + m.decay;
                    // Water carries it off as steam (what has no room is let off).
                    let wanted = heat * dt / STEAM_ENERGY;
                    let took = fluids::drain(&mut m.tanks[0], Fluid::Water, wanted);
                    m.put_fluid(Fluid::Steam, took);
                    let cooled = if wanted > 0.0 { took / wanted } else { 1.0 };
                    let excess = heat * (1.0 - cooled);
                    if excess > 1.0 {
                        m.heat += excess * dt / REACTOR_CAPACITY;
                    } else {
                        let settle = AMBIENT + (RUNNING - AMBIENT) * (heat / REACTOR_HEAT);
                        m.heat += (settle - m.heat) * (dt / 8.0).min(1.0);
                    }
                    if m.heat >= MELTDOWN {
                        m.heat = MELTDOWN;
                        if authority && !self.meltdowns.contains(&m.cell) {
                            self.meltdowns.push(m.cell);
                        }
                    }
                    if m.lit() != was {
                        self.lit_changed.push((m.cell, m.lit()));
                    }
                }
                _ => {}
            }
        }
    }

    /// Each network: its generators make power whenever they have fuel, used or not. Machines
    /// that have something to do get it first (all slowed alike when there isn't enough), what
    /// is left charges the accumulators, and when the generators fall short the accumulators
    /// make up what they can.
    /// Whether the network the machine in `cell` is on has power to give, made or stored.
    pub fn has_power(&self, cell: BlockCell) -> bool {
        if self.infinite {
            return true;
        }
        let Some(Node::Machine(i)) = self.node(cell) else {
            return false;
        };
        self.machine_network
            .get(i as usize)
            .and_then(|&n| self.networks.get(n as usize))
            .is_some_and(|n| n.capacity > 0.0 || n.stored > 1.0)
    }

    fn step_power(&mut self, dt: f32) {
        let (sun, infinite) = (self.sun, self.infinite);
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
                    MachineKind::Solar => {
                        gens += 1;
                        let made = m.sunlight(sun);
                        if made > 0.0 {
                            fuelled += 1;
                            capacity += made;
                        }
                    }
                    k if k.makes_power() => {
                        gens += 1;
                        let made = m.made_power(dt);
                        if made > 0.0 {
                            fuelled += 1;
                            capacity += made;
                        }
                    }
                    MachineKind::Accumulator => {
                        accumulators += 1;
                        stored += m.energy;
                        storage += m.energy_total;
                        can_take += ACCUMULATOR_POWER.min((m.energy_total - m.energy) / dt);
                        can_give += ACCUMULATOR_POWER.min(m.energy / dt);
                    }
                    k if k.unpowered() => {}
                    kind => {
                        users += 1;
                        if m.working() {
                            busy += 1;
                            demand += kind.power();
                        }
                    }
                }
            }
            if infinite {
                // (Made out of nothing: as much as is asked for, and the accumulators filled.)
                capacity = capacity.max(demand + can_take);
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
                    MachineKind::Solar => {
                        m.power = m.sunlight(sun);
                        m.supply = if capacity > 0.0 {
                            (used + charge) / capacity
                        } else {
                            0.0
                        };
                    }
                    k if k.makes_power() => {
                        m.power = m.made_power(dt);
                        if k == MachineKind::SteamTurbine {
                            // The steam it used, and its water back (let off if there is
                            // nowhere for it).
                            let used = m.power * dt / STEAM_ENERGY;
                            fluids::drain(&mut m.tanks[0], Fluid::Steam, used);
                            m.put_fluid(Fluid::Water, used * CONDENSED);
                        }
                        m.supply = if capacity > 0.0 {
                            (used + charge) / capacity
                        } else {
                            0.0
                        };
                    }
                    k if k.unpowered() => {}
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
                        if kind == MachineKind::BigDoor {
                            // (A door's motor makes nothing: the door moves by `supply`.)
                        } else if working && share > 0.0 {
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
            Node::Pipe(i) => {
                let net = self
                    .pipe_net
                    .get(i as usize)
                    .and_then(|&n| self.fluid_nets.get(n as usize));
                let held = self.pipes[i as usize].held;
                let (pipes, machines) = net.map_or((1, 0), |n| (n.pipes.len(), n.machines.len()));
                let on = match machines {
                    0 => "nothing on it".to_string(),
                    1 => "1 machine".to_string(),
                    n => format!("{n} machines"),
                };
                // (What its storage tanks hold is the line's too: one pool.)
                let mut litres = held.map_or(0.0, |s| s.amount) * pipes as f32;
                let mut capacity = PIPE_HOLDS * pipes as f32;
                for &m in net.map_or(&[][..], |n| &n.machines[..]) {
                    let m = &self.machines[m as usize];
                    if m.kind == MachineKind::Tank
                        && let Some(s) =
                            m.tanks[0].filter(|s| Some(s.fluid) == held.map(|h| h.fluid))
                    {
                        litres += s.amount;
                        capacity += TANK_HOLDS;
                    }
                }
                Some(match held {
                    Some(s) if litres > 0.5 => format!(
                        "{}: {:.0} of {:.0} l, {on}",
                        s.fluid.name(),
                        litres,
                        capacity
                    ),
                    Some(s) => format!("{} line, empty, {on}", s.fluid.name()),
                    None => format!("empty: takes what is sent in, {on}"),
                })
            }
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
            Node::Conveyor(_) | Node::Pipe(_) => None,
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

    /// Sets the wind at every wind turbine: `wind(up, altitude)` is the wind (m/s) at the hub
    /// of one standing at `up` (unit, planet-relative) that high over the sea.
    pub fn set_wind(&mut self, wind: impl Fn(DVec3, f32) -> f32) {
        for m in &mut self.machines {
            if m.kind == MachineKind::Wind
                && let Some(up) = m.up
            {
                m.wind = wind(DVec3::from(up.map(f64::from)), m.altitude);
            }
        }
    }

    /// Fills what `cell` is (a storage tank, a machine's first input tank that takes it, or a
    /// pipe's network) with `amount` of `fluid` (the console, tests). How much went in.
    pub fn fill_fluid(&mut self, cell: BlockCell, fluid: Fluid, amount: f32) -> f32 {
        self.ensure_index();
        match self.index.get(&cell).copied() {
            Some(Node::Machine(i)) => {
                let m = &mut self.machines[i as usize];
                if m.kind == MachineKind::Tank {
                    return fluids::fill(&mut m.tanks[0], fluid, amount, TANK_HOLDS);
                }
                for t in 0..2 {
                    if let Some((want, holds)) = m.fluid_input(t)
                        && want == fluid
                    {
                        return fluids::fill(&mut m.tanks[t], fluid, amount, holds);
                    }
                }
                0.0
            }
            Some(Node::Pipe(i)) => {
                let p = &mut self.pipes[i as usize];
                let carried = p.carries();
                let put = fluids::fill(&mut p.held, fluid, amount, PIPE_HOLDS);
                if p.carries() != carried {
                    self.linked = false;
                }
                put
            }
            _ => 0.0,
        }
    }

    /// The ways a pipe put in `cell` joins (bits as `Pipe::ways`): what it was put against (a
    /// pipe, or a machine with tanks), what is in front of its open ends (straight on from that,
    /// or along `axis` both ways), and pipes whose open ends or sockets point at it. So a line
    /// goes where it is built: one beside another doesn't join it.
    pub fn pipe_ways(
        &mut self,
        planet: &Planet,
        cell: BlockCell,
        support: BlockCell,
        axis: u8,
    ) -> u8 {
        self.ensure_index();
        let next = around(planet, cell);
        let joinable = |c: BlockCell| match self.index.get(&c) {
            Some(Node::Pipe(_)) => true,
            Some(Node::Machine(m)) => self.machines[*m as usize].kind.has_tanks(),
            _ => false,
        };
        let mut ways = 0u8;
        for (side, &c) in next.iter().enumerate() {
            if c != cell && c == support && joinable(c) {
                ways |= 1 << side;
            }
        }
        let open = Pipe {
            cell,
            held: None,
            ways: Some(ways),
            axis,
            around: None,
            links: 0,
        }
        .open_ends();
        for (side, &c) in next.iter().enumerate() {
            if c == cell {
                continue;
            }
            if open & (1 << side) != 0 && joinable(c) {
                ways |= 1 << side;
            }
            if let Some(Node::Pipe(j)) = self.index.get(&c) {
                let p = &self.pipes[*j as usize];
                let back = p.around.and_then(|a| a.iter().position(|&x| x == cell));
                if let Some(back) = back
                    && (p.ways.is_some_and(|w| w & (1 << back) != 0)
                        || p.open_ends() & (1 << back) != 0)
                {
                    ways |= 1 << side;
                }
            }
        }
        ways
    }

    /// The pipe in `cell` joins `ways` (None: all it touches, worked out when the pipes are
    /// next joined up) and runs along `axis` while it joins nothing. The pipes it joins join it
    /// back.
    pub fn set_pipe_ways(&mut self, cell: BlockCell, ways: Option<u8>, axis: u8) {
        self.ensure_index();
        let Some(&Node::Pipe(i)) = self.index.get(&cell) else {
            return;
        };
        let pipe = &mut self.pipes[i as usize];
        pipe.ways = ways;
        pipe.axis = axis.min(2);
        self.linked = false;
        let (Some(ways), Some(next)) = (ways, pipe.around) else {
            return;
        };
        for (side, &c) in next.iter().enumerate() {
            if ways & (1 << side) == 0 || c == cell {
                continue;
            }
            if let Some(&Node::Pipe(j)) = self.index.get(&c) {
                let p = &mut self.pipes[j as usize];
                let back = p.around.and_then(|a| a.iter().position(|&x| x == cell));
                if let (Some(back), Some(w)) = (back, p.ways) {
                    p.ways = Some(w | 1 << back);
                }
            }
        }
    }

    /// Empties every tank and pipe (the console's `factory drain`, to try what running dry does).
    pub fn drain_all(&mut self) {
        for m in &mut self.machines {
            m.tanks = [None; 4];
        }
        for p in &mut self.pipes {
            p.held = None;
        }
        self.linked = false;
    }

    /// Pipe networks: (pipes, fluid, litres) each.
    pub fn fluid_networks(&self) -> Vec<(usize, Option<Fluid>, f32)> {
        self.fluid_nets
            .iter()
            .map(|net| {
                let mut fluid = None;
                let mut amount = 0.0;
                for &p in &net.pipes {
                    if let Some(s) = self.pipes[p as usize].held {
                        fluid = Some(s.fluid);
                        amount += s.amount;
                    }
                }
                (net.pipes.len(), fluid, amount)
            })
            .collect()
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
