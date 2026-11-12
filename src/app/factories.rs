//! The factory side of the game (see `factory`): turning conveyors, and the console commands,
//! including a small ready-made factory to try things on (`factory demo`).

use super::Game;
use crate::{
    factory::{self, AHEAD, fluids::Fluid},
    render::models::{self, ModelState},
    survival::{
        self,
        items::{Item, ItemStack},
    },
    ui::screen::Screen,
    world::{block::Block, chunk::BlockCell},
};

/// The ways a pipe in a ready-made factory joins (`Game::build_ahead`): ahead, right, left.
const PIPE_AHEAD: u8 = 1;
const PIPE_RIGHT: u8 = 4;
const PIPE_LEFT: u8 = 8;

impl Game {
    /// R on a conveyor turns it a quarter. False if the crosshair isn't on one.
    pub(super) fn turn_conveyor(&mut self) -> bool {
        let Some(cell) = self
            .target
            .as_ref()
            .filter(|h| h.block == Block::Conveyor)
            .map(|h| h.cell)
        else {
            return false;
        };
        let mut world = survival::World {
            planet: &self.planet,
            streamer: &mut self.streamer,
        };
        self.survival.turn_conveyor(&mut world, cell)
    }

    pub(super) fn factory_command(&mut self, command: &str, args: &[&str]) -> bool {
        if command != "factory" {
            return false;
        }
        match args.first().copied() {
            None | Some("status") => self.factory_status(),
            Some("demo") => self.factory_demo(),
            Some("stress") => {
                let n = |i: usize, d: i32| {
                    args.get(i)
                        .and_then(|a| a.parse::<i32>().ok())
                        .unwrap_or(d)
                        .clamp(1, 400)
                };
                self.factory_stress(n(1, 40), n(2, 60));
            }
            Some("open") => {
                // The screen of the nearest machine of a kind within reach (`factory open
                // [assembler|...]`; `power`: the power screen of the nearest machine).
                let wanted = args.get(1).copied().unwrap_or("coal generator");
                let power = wanted == "power";
                match self.nearest_machine(if power { None } else { Some(wanted) }) {
                    Some((cell, kind)) => {
                        let screen = if power || kind == factory::MachineKind::Accumulator {
                            Screen::Power(cell)
                        } else {
                            Screen::Machine(cell, kind)
                        };
                        self.survival.open(screen);
                    }
                    None => self.console.print("No such machine within reach"),
                }
            }
            Some("recipe") => {
                // factory recipe <item>: the nearest machine that can make it makes it.
                let Some(item) = args.get(1).and_then(|a| Item::from_id(a)) else {
                    self.console
                        .print("Usage: factory recipe <item> (e.g. circuit_board)");
                    return true;
                };
                let feet = self.pose().feet;
                let planet = self.planet.clone();
                let nearest = (self.survival.factory.machines().iter())
                    .filter(|m| factory::recipes::find(m.kind, item).is_some())
                    .map(|m| {
                        let at = crate::player::interact::cell_center(&planet, m.cell);
                        (m.cell, (at - feet).length())
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match nearest {
                    Some((cell, _)) => {
                        let mut back = Vec::new();
                        if let Some(m) = self.survival.factory.machine_mut(cell) {
                            back = m.set_recipe(Some(item));
                        }
                        let pose = self.pose();
                        for stack in back {
                            self.survival.received(stack, &pose);
                        }
                        if let Some(machine) = self.survival.factory.machine(cell).copied() {
                            self.survival
                                .tell(crate::net::protocol::WorldEvent::Machine { cell, machine });
                        }
                        self.console
                            .print(format!("Making {}", item.name().to_lowercase()));
                    }
                    None => self.console.print("No machine here makes that"),
                }
            }
            Some("industry") => self.industry_demo(),
            Some("power") => self.power_demo(),
            Some("chemistry") => self.chemistry_demo(),
            Some("nuclear") => self.nuclear_demo(),
            Some("fill") => {
                // factory fill <fluid> [litres]: what the crosshair is on (a tank, a machine
                // that takes it, a pipe), else the nearest storage tank in reach.
                let Some(fluid) = args.get(1).and_then(|a| Fluid::from_id(a)) else {
                    let names: Vec<String> = Fluid::ALL.iter().map(|f| f.id()).collect();
                    self.console.print(format!(
                        "Usage: factory fill <{}> [litres]",
                        names.join("|")
                    ));
                    return true;
                };
                let litres = args
                    .get(2)
                    .and_then(|a| a.parse::<f32>().ok())
                    .unwrap_or(factory::fluids::TANK_HOLDS);
                let target = self.target.as_ref().map(|h| h.cell);
                let cell = target
                    .filter(|&c| {
                        self.survival.factory.machine(c).is_some()
                            || self.survival.factory.pipes().iter().any(|p| p.cell == c)
                    })
                    .or_else(|| self.nearest_machine(Some("storage tank")).map(|(c, _)| c));
                let put = cell.map_or(0.0, |c| {
                    let c = self.survival.factory.core(c);
                    self.survival.factory.fill_fluid(c, fluid, litres)
                });
                self.console
                    .print(format!("Put in {put:.0} l of {}", fluid.name()));
            }
            Some("drain") => {
                self.survival.factory.drain_all();
                self.console.print("Every tank and pipe emptied");
            }
            Some("fluids") => {
                let nets = self.survival.factory.fluid_networks();
                if nets.is_empty() {
                    self.console.print("No pipes");
                }
                for (i, (pipes, fluid, litres)) in nets.into_iter().enumerate() {
                    let what = fluid.map_or("empty", Fluid::name);
                    self.console
                        .print(format!("line {i}: {pipes} pipes, {what} {litres:.0} l"));
                }
            }
            Some("turn") => {
                let text = if self.turn_conveyor() {
                    "Turned the conveyor"
                } else {
                    "Not looking at a conveyor"
                };
                self.console.print(text);
            }
            _ => self.console.print(
                "Usage: factory [status|demo|industry|power|chemistry|nuclear|stress|open \
                 [machine|power]|recipe <item>|fill <fluid> [l]|drain|fluids|turn]",
            ),
        }
        true
    }

    /// The nearest machine within reach (of the kind named, e.g. "assembler"; any if None).
    fn nearest_machine(&self, kind: Option<&str>) -> Option<(BlockCell, factory::MachineKind)> {
        let feet = self.pose().feet;
        let named = |k: factory::MachineKind| {
            kind.is_none_or(|n| k.name() == n.replace('_', " ") || k.name().contains(n))
        };
        (self.survival.factory.machines().iter())
            .filter(|m| named(m.kind))
            .map(|m| {
                let at = crate::player::interact::cell_center(&self.planet, m.cell);
                (m.cell, m.kind, (at - feet).length())
            })
            .filter(|&(_, _, d)| d < 8.0)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(cell, kind, _)| (cell, kind))
    }

    fn factory_status(&mut self) {
        let f = &self.survival.factory;
        let (conveyors, machines, poles, cables, networks) = f.counts();
        let mut lines = vec![format!(
            "{conveyors} conveyors, {machines} machines, {poles} poles, {cables} cables, \
             {networks} networks"
        )];
        let kw = |w: f32| w / 1000.0;
        for (i, net) in f.networks().iter().enumerate() {
            if net.capacity > 0.0 || net.demand > 0.0 {
                lines.push(format!(
                    "network {i}: using {:.0} kw of {:.0} kw, asked for {:.0} kw",
                    kw(net.used),
                    kw(net.capacity),
                    kw(net.demand)
                ));
            }
        }
        let carried: usize = f.carried();
        lines.push(format!("{carried} items on conveyors"));
        // What is in the containers conveyors reach.
        let list = |slots: &mut dyn Iterator<Item = ItemStack>| {
            let mut totals: Vec<(Item, u32)> = Vec::new();
            for s in slots {
                match totals.iter_mut().find(|(i, _)| *i == s.item) {
                    Some((_, n)) => *n += s.count as u32,
                    None => totals.push((s.item, s.count as u32)),
                }
            }
            let words: Vec<String> = totals
                .iter()
                .map(|(i, n)| format!("{n} {}", i.id()))
                .collect();
            if words.is_empty() {
                "empty".to_string()
            } else {
                words.join(", ")
            }
        };
        for cell in f.stores() {
            let what = if let Some(chest) = self.survival.chests.get(&cell) {
                format!("chest: {}", list(&mut chest.iter().flatten().copied()))
            } else if let Some(furnace) = self.survival.furnaces.get(&cell) {
                format!("furnace: {}", list(&mut furnace.contents()))
            } else {
                continue;
            };
            lines.push(format!("  {} {} {} {what}", cell.face.0, cell.u, cell.v));
        }
        for m in f.machines() {
            lines.push(format!(
                "  {} {} {} {:?}: {}; {}",
                m.cell.face.0,
                m.cell.u,
                m.cell.v,
                m.kind,
                list(&mut m.contents()),
                m.status()
            ));
        }
        for line in lines {
            self.console.print(line);
        }
    }

    /// A stress test: `rows` lines of `length` conveyors ahead, each from a full chest to an
    /// empty one, a block above the ground (on a stone floor).
    fn factory_stress(&mut self, rows: i32, length: i32) {
        let pose = self.pose();
        let planet = self.planet.clone();
        let Some(here) = planet.cell_at(pose.feet + pose.up() * 0.5) else {
            return;
        };
        let ahead = models::facing_towards(&planet, here, pose.look);
        let (du, dv) = AHEAD[ahead as usize];
        let (ru, rv) = (dv, -du);
        let k = here.k + 1;
        let mut world = survival::World {
            planet: &planet,
            streamer: &mut self.streamer,
        };
        let mut chests = Vec::new();
        for r in 0..rows {
            for a in 0..=length + 1 {
                let (u, v) = (
                    here.u + du * (a + 2) + ru * (r - rows / 2),
                    here.v + dv * (a + 2) + rv * (r - rows / 2),
                );
                let (Some(cell), Some(floor)) = (
                    planet.cell(here.face, u, v, k),
                    planet.cell(here.face, u, v, k - 1),
                ) else {
                    continue;
                };
                self.survival
                    .put(&mut world, floor, Block::Stone, ModelState::default(), None);
                let block = if a == 0 || a == length + 1 {
                    Block::Chest
                } else {
                    Block::Conveyor
                };
                let state = ModelState {
                    facing: ahead,
                    ..Default::default()
                };
                self.survival.put(&mut world, cell, block, state, None);
                if a == 0 {
                    chests.push(cell);
                }
            }
        }
        for cell in chests {
            if let Some(chest) = self.survival.chests.get_mut(&cell) {
                for slot in chest.iter_mut() {
                    *slot = Some(ItemStack::new(Item::Block(Block::Cobblestone), 64));
                }
            }
        }
        self.console
            .print(format!("{} conveyors in {rows} lines", rows * length));
    }

    /// Builds a small factory ahead of the player: ore in a chest, a conveyor into an
    /// electric furnace and one out of it into another chest; a coal generator fed from a
    /// chest of coal by a conveyor, wired to the furnace over a pole.
    fn factory_demo(&mut self) {
        let pose = self.pose();
        let planet = self.planet.clone();
        let Some(here) = planet.cell_at(pose.feet + pose.up() * 0.5) else {
            return;
        };
        let ahead = models::facing_towards(&planet, here, pose.look);
        let facing =
            |(du, dv): (i32, i32)| AHEAD.iter().position(|&d| d == (du, dv)).unwrap_or(0) as u8;
        let (du, dv) = AHEAD[ahead as usize];
        // (Right of ahead, seen from above: the grid is right-handed.)
        let (ru, rv) = (dv, -du);
        let at = |a: i32, r: i32| {
            planet.cell(
                here.face,
                here.u + du * a + ru * r,
                here.v + dv * a + rv * r,
                here.k,
            )
        };
        let back = (ahead + 2) % 4;
        let to_left = facing((-ru, -rv));
        let parts: [(i32, i32, Block, u8); 9] = [
            (3, 0, Block::Chest, back),
            (4, 0, Block::Conveyor, ahead),
            (5, 0, Block::ElectricFurnace, back),
            (6, 0, Block::Conveyor, ahead),
            (7, 0, Block::Chest, back),
            (5, 3, Block::CoalGenerator, back),
            (5, 4, Block::Conveyor, to_left),
            (5, 5, Block::Chest, back),
            (3, 2, Block::PowerPole, back),
        ];
        let mut cells = Vec::new();
        {
            let mut world = survival::World {
                planet: &planet,
                streamer: &mut self.streamer,
            };
            for (a, r, block, facing) in parts {
                let Some(cell) = at(a, r) else { return };
                // Something to stand on, and room.
                if let Some(below) = planet.cell(cell.face, cell.u, cell.v, cell.k - 1)
                    && !world.streamer.block(below).is_some_and(|b| b.is_opaque())
                {
                    self.survival
                        .put(&mut world, below, Block::Stone, ModelState::default(), None);
                }
                if let Some(above) = planet.cell(cell.face, cell.u, cell.v, cell.k + 1)
                    && world.streamer.block(above).is_some_and(|b| b.is_solid())
                {
                    self.survival
                        .put(&mut world, above, Block::Air, ModelState::default(), None);
                }
                let state = ModelState {
                    facing,
                    ..Default::default()
                };
                self.survival.put(&mut world, cell, block, state, None);
                cells.push(cell);
            }
        }
        let fill = |s: &mut survival::Survival, cell: BlockCell, stacks: &[(Item, u8)]| {
            if let Some(chest) = s.chests.get_mut(&cell) {
                for (slot, &(item, n)) in chest.iter_mut().zip(stacks) {
                    *slot = Some(ItemStack::new(item, n));
                }
            }
        };
        fill(
            &mut self.survival,
            cells[0],
            &[
                (Item::RawIron, 64),
                (Item::RawCopper, 64),
                (Item::RawIron, 64),
            ],
        );
        fill(&mut self.survival, cells[7], &[(Item::Coal, 64)]);
        let (generator, furnace, pole) = (cells[5], cells[2], cells[8]);
        self.survival.wire(generator, pole);
        self.survival.wire(pole, furnace);
        self.console.print(format!(
            "A factory: ore -> electric furnace -> chest, powered by a coal generator ({:.0} kw)",
            factory::GENERATOR_POWER / 1000.0
        ));
    }

    /// A row of industry ahead of the player, on a stone floor, all on one pole: an oil pump
    /// (over oil shale) into a refinery making plastic; a miner onto a conveyor that bends
    /// round into a chest; sand and coal into an arc furnace making silicon, on into a chip fab
    /// making wafers; copper into an assembler making wire. Four coal generators and an
    /// accumulator power it.
    fn industry_demo(&mut self) {
        let pose = self.pose();
        let planet = self.planet.clone();
        let Some(here) = planet.cell_at(pose.feet + pose.up() * 0.5) else {
            return;
        };
        let ahead = models::facing_towards(&planet, here, pose.look);
        let facing =
            |(du, dv): (i32, i32)| AHEAD.iter().position(|&d| d == (du, dv)).unwrap_or(0) as u8;
        let (du, dv) = AHEAD[ahead as usize];
        let (ru, rv) = (dv, -du);
        let right = facing((ru, rv));
        let at = |a: i32, r: i32| {
            planet.cell(
                here.face,
                here.u + du * a + ru * r,
                here.v + dv * a + rv * r,
                here.k,
            )
        };
        let back = (ahead + 2) % 4;
        use Block::*;
        // (ahead, right, block, facing)
        let parts: Vec<(i32, i32, Block, u8)> = vec![
            // Oil: pump -> refinery (plastic) -> chest.
            (3, -4, OilPump, back),
            (4, -4, Conveyor, ahead),
            (5, -4, Refinery, back),
            (6, -4, Conveyor, ahead),
            (7, -4, Chest, back),
            // A miner (three blocks square) onto a belt that bends round to the right into a
            // chest.
            (3, -1, Miner, back),
            (5, -1, Conveyor, ahead),
            (6, -1, Conveyor, right),
            (6, 0, Conveyor, right),
            (6, 1, Chest, back),
            // Sand and coal -> arc furnace (silicon) -> chip fab (wafers) -> chest.
            (2, 3, Chest, back),
            (3, 3, Conveyor, ahead),
            (4, 3, ArcFurnace, back),
            (5, 3, Conveyor, ahead),
            (6, 3, ChipFab, back),
            (7, 3, Conveyor, ahead),
            (8, 3, Chest, back),
            // Copper -> assembler (wire) -> chest.
            (2, 6, Chest, back),
            (3, 6, Conveyor, ahead),
            (4, 6, Assembler, back),
            (5, 6, Conveyor, ahead),
            (6, 6, Chest, back),
            // Power.
            (10, -3, CoalGenerator, back),
            (10, -1, CoalGenerator, back),
            (10, 1, CoalGenerator, back),
            (10, 3, CoalGenerator, back),
            (10, 5, Accumulator, back),
            (8, 1, PowerPole, back),
        ];
        let mut cells = Vec::new();
        {
            let mut world = survival::World {
                planet: &planet,
                streamer: &mut self.streamer,
            };
            for &(a, r, block, facing) in &parts {
                let Some(cell) = at(a, r) else { return };
                // (A big machine: every cell it fills.)
                let Some(unit) = factory::footprint(&planet, cell, block) else {
                    return;
                };
                let height = block.structure().1;
                for c in unit.iter().filter(|c| c.k == cell.k) {
                    // Something to stand on (oil shale under the pump), and room.
                    if let Some(below) = planet.cell(c.face, c.u, c.v, c.k - 1) {
                        let floor = if block == OilPump { OilShale } else { Stone };
                        let solid = world.streamer.block(below).is_some_and(|b| b.is_opaque());
                        if !solid || block == OilPump {
                            self.survival.put(
                                &mut world,
                                below,
                                floor,
                                ModelState::default(),
                                None,
                            );
                        }
                    }
                    for up in 0..height + 2 {
                        if let Some(above) = planet.cell(c.face, c.u, c.v, c.k + up)
                            && world.streamer.block(above).is_some_and(|b| b.is_solid())
                        {
                            self.survival
                                .put(&mut world, above, Air, ModelState::default(), None);
                        }
                    }
                }
                if block.is_structure() {
                    self.survival
                        .build_structure(&mut world, &unit, block, facing);
                } else {
                    let state = ModelState {
                        facing,
                        ..Default::default()
                    };
                    self.survival.put(&mut world, cell, block, state, None);
                }
                cells.push((cell, block));
            }
        }
        let cell_of = |i: usize| cells[i].0;
        let fill = |s: &mut survival::Survival, cell: BlockCell, stacks: &[(Item, u8)]| {
            if let Some(chest) = s.chests.get_mut(&cell) {
                for (slot, &(item, n)) in chest.iter_mut().zip(stacks) {
                    *slot = Some(ItemStack::new(item, n));
                }
            }
        };
        fill(
            &mut self.survival,
            cell_of(10),
            &[
                (Item::Block(Sand), 64),
                (Item::Coal, 64),
                (Item::Block(Sand), 64),
                (Item::Coal, 64),
            ],
        );
        fill(&mut self.survival, cell_of(17), &[(Item::CopperIngot, 64)]);
        let set = |s: &mut survival::Survival, cell: BlockCell, recipe: Item| {
            if let Some(m) = s.factory.machine_mut(cell) {
                m.set_recipe(Some(recipe));
            }
        };
        set(&mut self.survival, cell_of(2), Item::Plastic);
        set(&mut self.survival, cell_of(12), Item::Silicon);
        set(&mut self.survival, cell_of(14), Item::SiliconWafer);
        set(&mut self.survival, cell_of(19), Item::CopperWire);
        let pole = cell_of(cells.len() - 1);
        for (cell, block) in cells.clone() {
            if block == CoalGenerator
                && let Some(m) = self.survival.factory.machine_mut(cell)
            {
                m.fuel = Some(ItemStack::new(Item::Coal, 64));
            }
            if block.is_machine() {
                self.survival.wire(pole, cell);
            }
        }
        // (Everything above is news for the others too.)
        for (cell, block) in cells {
            if block.is_machine()
                && let Some(machine) = self.survival.factory.machine(cell).copied()
            {
                self.survival
                    .tell(crate::net::protocol::WorldEvent::Machine { cell, machine });
            }
        }
        self.console.print(
            "Industry: oil -> plastic, a miner on a bent belt, sand -> silicon -> wafers, \
             copper -> wire; 4 generators and an accumulator",
        );
    }

    /// Builds `parts` ahead of the player, each as (cells ahead, cells to the right, block,
    /// facing as a turn from facing the player; for a pipe the ways it joins, `PIPE_*`, or 0 for
    /// all it touches): on a stone floor, with room cleared over them (a big machine whole).
    /// What went where, or None if something didn't fit.
    fn build_ahead(&mut self, parts: &[(i32, i32, Block, u8)]) -> Option<Vec<(BlockCell, Block)>> {
        let pose = self.pose();
        let planet = self.planet.clone();
        let here = planet.cell_at(pose.feet + pose.up() * 0.5)?;
        let ahead = models::facing_towards(&planet, here, pose.look);
        let (du, dv) = AHEAD[ahead as usize];
        let (ru, rv) = (dv, -du);
        let back = (ahead + 2) % 4;
        let at = |a: i32, r: i32| {
            planet.cell(
                here.face,
                here.u + du * a + ru * r,
                here.v + dv * a + rv * r,
                here.k,
            )
        };
        let mut placed = Vec::new();
        let mut world = survival::World {
            planet: &planet,
            streamer: &mut self.streamer,
        };
        for &(a, r, block, turn) in parts {
            let cell = at(a, r)?;
            let cells = factory::footprint(&planet, cell, block)?;
            let height = block.structure().1;
            for &c in cells.iter().filter(|c| c.k == cell.k) {
                if let Some(below) = planet.cell(c.face, c.u, c.v, c.k - 1)
                    && !world.streamer.block(below).is_some_and(|b| b.is_opaque())
                {
                    self.survival
                        .put(&mut world, below, Block::Stone, ModelState::default(), None);
                }
                for up in 0..height + 2 {
                    if let Some(c) = planet.cell(c.face, c.u, c.v, c.k + up)
                        && world.streamer.block(c).is_some_and(|b| b.is_solid())
                    {
                        self.survival
                            .put(&mut world, c, Block::Air, ModelState::default(), None);
                    }
                }
            }
            let facing = (back + turn) % 4;
            if block.is_structure() {
                self.survival
                    .build_structure(&mut world, &cells, block, facing);
            } else if block == Block::Pipe {
                // (Ways ahead, back, right, left, up, down as +u, -u, +v, -v, up, down.)
                let side = |(u, v): (i32, i32)| match (u, v) {
                    (1, 0) => 0,
                    (-1, 0) => 1,
                    (0, 1) => 2,
                    _ => 3,
                };
                let sides = [
                    side((du, dv)),
                    side((-du, -dv)),
                    side((ru, rv)),
                    side((-ru, -rv)),
                    4,
                    5,
                ];
                let ways = (0..6)
                    .filter(|i| turn & (1 << i) != 0)
                    .fold(0, |w, i| w | 1 << sides[i]);
                let state = ModelState {
                    links: if turn == 0 {
                        0
                    } else {
                        ways | survival::BUILT_WAYS
                    },
                    ..Default::default()
                };
                self.survival.put(&mut world, cell, block, state, None);
            } else {
                let state = ModelState {
                    facing,
                    ..Default::default()
                };
                self.survival.put(&mut world, cell, block, state, None);
            }
            placed.push((cell, block));
        }
        Some(placed)
    }

    /// Wires every machine of `placed` that uses or makes power to the first pole in it, and
    /// tells the others what they hold.
    fn wire_up(&mut self, placed: &[(BlockCell, Block)]) {
        let Some(&(pole, _)) = placed.iter().find(|(_, b)| *b == Block::PowerPole) else {
            return;
        };
        for &(cell, block) in placed {
            if block.is_machine() && self.survival.factory.is_electric(cell) {
                self.survival.wire(pole, cell);
            }
        }
        for &(cell, block) in placed {
            if block.is_machine()
                && let Some(machine) = self.survival.factory.machine(cell).copied()
            {
                self.survival
                    .tell(crate::net::protocol::WorldEvent::Machine { cell, machine });
            }
        }
    }

    /// Steam and other power ahead of the player: a water tank feeding a coal boiler, its steam
    /// into a steam turbine; a wind turbine and an RTG; an accumulator, all on one pole.
    fn power_demo(&mut self) {
        use Block::*;
        let parts = [
            (4, -6, WindTurbine, 0),
            (4, -3, Rtg, 0),
            (4, 1, StorageTank, 0),
            (4, 2, Pipe, 0),
            (4, 3, Boiler, 0),
            (5, 3, Pipe, 0),
            (6, 3, Pipe, 0),
            (8, 3, SteamTurbine, 1),
            (2, -1, Accumulator, 0),
            (2, 1, PowerPole, 0),
        ];
        let Some(placed) = self.build_ahead(&parts) else {
            self.console.print("No room here");
            return;
        };
        let factory = &mut self.survival.factory;
        factory.fill_fluid(placed[2].0, Fluid::Water, factory::fluids::TANK_HOLDS);
        if let Some(m) = factory.machine_mut(placed[4].0) {
            m.fuel = Some(ItemStack::new(Item::Coal, 64));
        }
        self.wire_up(&placed);
        self.console.print(
            "Power: water -> coal boiler -> steam turbine; a wind turbine, an RTG and an \
             accumulator on one pole",
        );
    }

    /// Chemistry ahead of the player: water from a tank into an electrolyzer (its hydrogen into
    /// a tank, its oxygen down a line to a blender), the blender also fed water and sulfur
    /// (from a chest) making sulfuric acid into a tank; coal generators on a pole.
    fn chemistry_demo(&mut self) {
        use Block::*;
        let mut parts = vec![
            (4, -3, StorageTank, 0),
            (4, -2, Pipe, 0),
            (4, -1, Pipe, 0),
            (4, 0, Electrolyzer, 0),
            (3, 0, Pipe, 0),
            (2, 0, StorageTank, 0),
            (5, 0, Pipe, 0),
            (6, 0, Pipe, 0),
            (8, 3, Blender, 0),
            (11, 3, Pipe, 0),
            (12, 3, StorageTank, 0),
            (8, 7, Chest, 0),
            (8, 6, Conveyor, 3),
        ];
        // The blender's water, round the oxygen line.
        for (a, r) in [
            (5, -3),
            (6, -3),
            (7, -3),
            (8, -3),
            (9, -3),
            (9, -2),
            (9, -1),
            (9, 0),
        ] {
            parts.push((a, r, Pipe, 0));
        }
        for r in [-6, -7, -8] {
            parts.push((2, r, CoalGenerator, 0));
        }
        parts.push((2, -4, CoalGenerator, 0));
        parts.push((1, -2, PowerPole, 0));
        let Some(placed) = self.build_ahead(&parts) else {
            self.console.print("No room here");
            return;
        };
        let factory = &mut self.survival.factory;
        factory.fill_fluid(placed[0].0, Fluid::Water, factory::fluids::TANK_HOLDS);
        if let Some(m) = factory.machine_mut(placed[3].0) {
            m.set_recipe(Some(Item::Fluid(Fluid::Hydrogen)));
        }
        if let Some(m) = factory.machine_mut(placed[8].0) {
            m.set_recipe(Some(Item::Fluid(Fluid::SulfuricAcid)));
        }
        for &(cell, block) in &placed {
            if block == CoalGenerator
                && let Some(m) = self.survival.factory.machine_mut(cell)
            {
                m.fuel = Some(ItemStack::new(Item::Coal, 64));
            }
        }
        if let Some(chest) = self.survival.chests.get_mut(&placed[11].0) {
            chest[0] = Some(ItemStack::new(Item::Sulfur, 64));
        }
        self.wire_up(&placed);
        self.console.print(
            "Chemistry: water -> electrolyzer (hydrogen to a tank, oxygen to the blender); \
             sulfur, water and oxygen -> sulfuric acid into a tank",
        );
    }

    /// A nuclear plant ahead of the player: a reactor with four fuel rods, fed water from a
    /// tank along the line in front of it (which the first turbine's water goes back into),
    /// its steam to two steam turbines; a coal generator and an accumulator to start it, all
    /// on one pole.
    fn nuclear_demo(&mut self) {
        use Block::*;
        let mut parts = vec![
            (9, 0, NuclearReactor, 0),
            (6, -4, StorageTank, 0),
            (8, 5, SteamTurbine, 1),
            (12, 5, SteamTurbine, 1),
            (8, 3, Pipe, PIPE_RIGHT | PIPE_LEFT),
            (11, 3, Pipe, PIPE_RIGHT | PIPE_LEFT),
            (3, -6, CoalGenerator, 0),
            (3, -7, Accumulator, 0),
            (3, -2, PowerPole, 0),
        ];
        // (Past the reactor's side, into it in the middle and into the turbine at the end.)
        for r in -3..=4 {
            let ways = match r {
                0 => PIPE_RIGHT | PIPE_LEFT | PIPE_AHEAD,
                4 => PIPE_LEFT | PIPE_AHEAD,
                _ => PIPE_RIGHT | PIPE_LEFT,
            };
            parts.push((6, r, Pipe, ways));
        }
        let Some(placed) = self.build_ahead(&parts) else {
            self.console.print("No room here");
            return;
        };
        let factory = &mut self.survival.factory;
        factory.fill_fluid(placed[1].0, Fluid::Water, factory::fluids::TANK_HOLDS);
        if let Some(m) = factory.machine_mut(placed[0].0) {
            m.fuel = Some(ItemStack::new(Item::UraniumFuelRod, 4));
        }
        if let Some(m) = factory.machine_mut(placed[6].0) {
            m.fuel = Some(ItemStack::new(Item::Coal, 64));
        }
        self.wire_up(&placed);
        self.console.print(format!(
            "Nuclear: a reactor ({:.0} mw of heat) on water from a tank, its steam into two \
             turbines; keep it in water or it melts down",
            factory::REACTOR_HEAT / 1.0e6
        ));
    }
}
