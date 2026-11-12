//! The factory side of the game (see `factory`): turning conveyors, and the console commands,
//! including a small ready-made factory to try things on (`factory demo`).

use super::Game;
use crate::{
    factory::{self, AHEAD},
    render::models::{self, ModelState},
    survival::{
        self,
        items::{Item, ItemStack},
    },
    ui::screen::Screen,
    world::{block::Block, chunk::BlockCell},
};

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
                    self.console.print("Usage: factory recipe <item> (e.g. circuit_board)");
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
                        self.console.print(format!("Making {}", item.name().to_lowercase()));
                    }
                    None => self.console.print("No machine here makes that"),
                }
            }
            Some("industry") => self.industry_demo(),
            Some("turn") => {
                let text = if self.turn_conveyor() {
                    "Turned the conveyor"
                } else {
                    "Not looking at a conveyor"
                };
                self.console.print(text);
            }
            _ => self.console.print(
                "Usage: factory [status|demo|industry|stress|open [machine|power]|recipe <item>|turn]",
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
            // A miner onto a belt that bends round to the right into a chest.
            (3, -1, Miner, back),
            (4, -1, Conveyor, ahead),
            (5, -1, Conveyor, right),
            (5, 0, Conveyor, right),
            (5, 1, Chest, back),
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
                // Something to stand on (oil shale under the pump), and room.
                if let Some(below) = planet.cell(cell.face, cell.u, cell.v, cell.k - 1) {
                    let floor = if block == OilPump { OilShale } else { Stone };
                    let solid = world.streamer.block(below).is_some_and(|b| b.is_opaque());
                    if !solid || block == OilPump {
                        self.survival
                            .put(&mut world, below, floor, ModelState::default(), None);
                    }
                }
                for up in 1..=2 {
                    if let Some(above) = planet.cell(cell.face, cell.u, cell.v, cell.k + up)
                        && world.streamer.block(above).is_some_and(|b| b.is_solid())
                    {
                        self.survival
                            .put(&mut world, above, Air, ModelState::default(), None);
                    }
                }
                let state = ModelState {
                    facing,
                    ..Default::default()
                };
                self.survival.put(&mut world, cell, block, state, None);
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
}
