use super::*;
use crate::world::planet::Face;

fn planet() -> Planet {
    Planet::new(DVec3::ZERO, 64, 128, 1)
}

fn at(planet: &Planet, u: i32, v: i32) -> BlockCell {
    BlockCell {
        face: Face(2),
        u: 600 + u,
        v: 700 + v,
        k: planet.sea_layer + 3,
    }
}

fn stack(item: Item, n: u8) -> Slot {
    Some(ItemStack::new(item, n))
}

/// Chests, furnaces and a factory, offline.
#[derive(Default)]
struct World {
    chests: FastMap<BlockCell, [Slot; CHEST_SLOTS]>,
    furnaces: FastMap<BlockCell, Furnace>,
    factory: Factory,
}

impl World {
    fn new() -> Self {
        let mut w = World::default();
        w.factory.authority = true;
        w
    }

    fn run(&mut self, seconds: f64) {
        // In frame-sized pieces, as the game does.
        let mut left = seconds;
        while left > 0.0 {
            let dt = left.min(1.0 / 60.0);
            let mut stores = Stores {
                chests: &mut self.chests,
                furnaces: &mut self.furnaces,
            };
            self.factory.run(dt, &mut stores);
            left -= dt;
        }
    }

    fn count(&self, chest: BlockCell, item: Item) -> u32 {
        self.chests[&chest]
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }
}

/// Facing that runs along +u.
const EAST: u8 = 1;

#[test]
fn a_conveyor_moves_items_from_a_chest_into_a_furnace() {
    let p = planet();
    let mut w = World::new();
    let (chest, belt, furnace) = (at(&p, 0, 0), at(&p, 1, 0), at(&p, 2, 0));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::RawIron, 5);
    slots[1] = stack(Item::Coal, 2);
    w.chests.insert(chest, slots);
    w.furnaces.insert(furnace, Furnace::default());
    w.factory.add(&p, belt, Block::Conveyor, EAST);
    assert_eq!(w.factory.conveyor(belt).unwrap().front, furnace);
    w.run(10.0);
    let f = w.furnaces[&furnace];
    assert_eq!(f.input, stack(Item::RawIron, 5));
    assert_eq!(f.fuel, stack(Item::Coal, 2));
    assert!(w.chests[&chest].iter().all(Option::is_none));
}

#[test]
fn a_line_of_conveyors_carries_items_at_belt_speed() {
    let p = planet();
    let mut w = World::new();
    let (from, to) = (at(&p, 0, 0), at(&p, 11, 0));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Block(Block::Cobblestone), 64);
    w.chests.insert(from, slots);
    w.chests.insert(to, [None; CHEST_SLOTS]);
    for u in 1..=10 {
        w.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    // Ten conveyors take 10 / 1.6 = 6.25 s (plus a tick or two at each end).
    w.run(5.5);
    assert_eq!(w.count(to, Item::Block(Block::Cobblestone)), 0);
    w.run(1.5);
    assert!(w.count(to, Item::Block(Block::Cobblestone)) >= 1);
    // Then a steady stream of about BELT_SPEED * BELT_ITEMS a second.
    let before = w.count(to, Item::Block(Block::Cobblestone));
    w.run(5.0);
    let rate = (w.count(to, Item::Block(Block::Cobblestone)) - before) as f32 / 5.0;
    assert!((5.0..=6.6).contains(&rate), "{rate} items/s");
}

#[test]
fn a_line_only_takes_what_its_end_can_use() {
    let p = planet();
    let mut w = World::new();
    let chest = at(&p, 0, 0);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 3);
    slots[1] = stack(Item::RawIron, 3);
    slots[2] = stack(Item::Block(Block::Dirt), 3);
    slots[3] = stack(Item::Coal, 3);
    w.chests.insert(chest, slots);
    for u in 1..=3 {
        w.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    let generator = at(&p, 4, 0);
    w.factory.add(&p, generator, Block::CoalGenerator, 0);
    w.run(10.0);
    // Sticks and coal are fuel (it burns them, used or not); the iron and the dirt stay behind.
    let m = w.factory.machine(generator).unwrap();
    assert!(
        m.lit() && m.fuel.is_some_and(|f| f.item == Item::Coal),
        "{:?}",
        m.fuel
    );
    assert_eq!(w.count(chest, Item::Stick) + w.count(chest, Item::Coal), 0);
    assert_eq!(w.count(chest, Item::RawIron), 3);
    assert_eq!(w.count(chest, Item::Block(Block::Dirt)), 3);
}

#[test]
fn a_generator_powers_an_electric_furnace_over_a_cable() {
    let p = planet();
    let mut w = World::new();
    let (generator, pole, furnace) = (at(&p, 0, 0), at(&p, 5, 0), at(&p, 9, 0));
    w.factory.add(&p, generator, Block::CoalGenerator, 0);
    w.factory.add(&p, pole, Block::PowerPole, 0);
    w.factory.add(&p, furnace, Block::ElectricFurnace, 0);
    // Three coal: 21.6 MJ, two minutes at full power.
    w.factory.machine_mut(generator).unwrap().fuel = stack(Item::Coal, 3);
    w.factory.machine_mut(furnace).unwrap().input = stack(Item::RawIron, 30);
    // Not connected: the furnace does nothing, but the generator makes power all the same
    // (and it goes to waste).
    w.run(20.0);
    assert_eq!(w.factory.machine(furnace).unwrap().output, None);
    let g = w.factory.machine(generator).unwrap();
    assert!(
        g.lit() && (g.power - GENERATOR_POWER).abs() < 1.0,
        "{}",
        g.power
    );
    assert_eq!(g.fuel, stack(Item::Coal, 2));

    assert!(w.factory.connect(generator, pole));
    assert!(w.factory.connect(furnace, pole));
    assert!(!w.factory.connect(pole, generator), "already connected");
    // 5 s an item at full power.
    w.run(60.0);
    let out = w.factory.machine(furnace).unwrap().output.unwrap();
    assert_eq!(out.item, Item::IronIngot);
    assert!((11..=12).contains(&out.count), "{}", out.count);
    // The fuel runs out two minutes in: 40 s more of smelting, then it stops.
    w.run(60.0);
    let out = w.factory.machine(furnace).unwrap().output.unwrap();
    assert!((19..=20).contains(&out.count), "{}", out.count);
    let g = w.factory.machine(generator).unwrap();
    assert!(!g.lit() && g.fuel.is_none());
    assert!(
        w.factory
            .machine(furnace)
            .unwrap()
            .status()
            .contains("no power")
    );
}

#[test]
fn accumulators_keep_what_would_go_to_waste_and_give_it_back() {
    let p = planet();
    let mut w = World::new();
    let (generator, store, furnace) = (at(&p, 0, 0), at(&p, 2, 0), at(&p, 4, 0));
    w.factory.add(&p, generator, Block::CoalGenerator, 0);
    w.factory.add(&p, store, Block::Accumulator, 0);
    w.factory.add(&p, furnace, Block::ElectricFurnace, 0);
    assert!(w.factory.connect(generator, store));
    w.factory.machine_mut(generator).unwrap().fuel = stack(Item::Coal, 1);
    // Nothing uses the power: all 180 kW go into the accumulator.
    w.run(10.0);
    let a = w.factory.machine(store).unwrap();
    assert!((a.energy - 1.8e6).abs() < 2.0e4, "{} J", a.energy);
    let view = w.factory.power_view(store).unwrap();
    assert!((view.network.charging - GENERATOR_POWER).abs() < 1.0);
    assert_eq!(view.network.accumulators, 1);
    assert!(view.history.samples.len() >= 9);
    // The coal runs out (40 s); by then it is full of what the generator made.
    w.run(40.0);
    let stored = w.factory.machine(store).unwrap().energy;
    assert!(stored > 4.9e6, "{stored} J");
    // A furnace on it now runs on what was stored: 90 kW for about 55 s.
    assert!(w.factory.connect(store, furnace));
    w.factory.machine_mut(furnace).unwrap().input = stack(Item::RawCopper, 64);
    w.run(30.0);
    let f = w.factory.machine(furnace).unwrap();
    assert!((f.supply - 1.0).abs() < 1e-3, "{}", f.supply);
    assert_eq!(f.output.unwrap().count, 6);
    w.run(40.0);
    let f = w.factory.machine(furnace).unwrap();
    assert_eq!(f.supply, 0.0);
    assert!((10..=11).contains(&f.output.unwrap().count));
    assert!(w.factory.machine(store).unwrap().energy < 1.0);
}

/// Solar panels make power while the sun is up over them, by how high it is, and none at
/// night.
#[test]
fn solar_panels_make_power_by_day() {
    let p = planet();
    let mut w = World::new();
    let (panel, store) = (at(&p, 0, 0), at(&p, 2, 0));
    w.factory.add(&p, panel, Block::SolarPanel, 0);
    w.factory.add(&p, store, Block::Accumulator, 0);
    assert!(w.factory.connect(panel, store));
    w.factory.locate(&p);
    let up = models::cell_point(&p, panel, [8.0, 8.0, 16.0])
        - models::cell_point(&p, panel, [8.0, 8.0, 0.0]);
    let up = up.normalize();
    w.factory.sun = up;
    w.run(10.0);
    let noon = w.factory.machine(store).unwrap().energy;
    assert!(
        (noon - SOLAR_POWER * 10.0).abs() < SOLAR_POWER * 0.2,
        "{noon} J"
    );
    assert!(w.factory.machine(panel).unwrap().lit());
    // Low in the sky: less.
    let low = (up + up.any_orthonormal_vector() * 3.0).normalize();
    w.factory.sun = low;
    w.run(10.0);
    let evening = w.factory.machine(store).unwrap().energy - noon;
    assert!(evening > 0.0 && evening < 0.5 * noon, "{evening} J");
    // Night.
    w.factory.sun = -up;
    let before = w.factory.machine(store).unwrap().energy;
    w.run(10.0);
    assert_eq!(w.factory.machine(store).unwrap().energy, before);
    assert_eq!(w.factory.machine(panel).unwrap().status(), "no sun");
}

/// A big door's motor only gets power while the door moves, and only from a network that has
/// some; `power infinite` gives it (and everything) all it asks.
#[test]
fn a_big_door_runs_on_its_network() {
    let p = planet();
    let mut w = World::new();
    let door = at(&p, 0, 0);
    w.factory.add(&p, door, Block::BigDoor, 0);
    w.run(1.0);
    assert!(!w.factory.has_power(door), "on its own, no power");
    w.factory.machine_mut(door).unwrap().wanted = true;
    w.run(1.0);
    assert_eq!(w.factory.machine(door).unwrap().supply, 0.0);
    powered(&mut w, &p, door);
    w.run(1.0);
    assert!(w.factory.has_power(door));
    let m = w.factory.machine(door).unwrap();
    assert!((m.supply - 1.0).abs() < 1e-3 && (m.power - DOOR_POWER).abs() < 1.0);
    // Standing still, it takes nothing.
    w.factory.machine_mut(door).unwrap().wanted = false;
    w.run(1.0);
    assert_eq!(w.factory.machine(door).unwrap().power, 0.0);

    // Infinite: a furnace with no generator at all works at full speed.
    let mut w = World::new();
    let furnace = at(&p, 4, 0);
    w.factory.add(&p, furnace, Block::ElectricFurnace, 0);
    w.factory.machine_mut(furnace).unwrap().input = stack(Item::RawIron, 4);
    w.run(1.0);
    assert_eq!(w.factory.machine(furnace).unwrap().supply, 0.0);
    w.factory.infinite = true;
    w.run(1.0);
    assert!((w.factory.machine(furnace).unwrap().supply - 1.0).abs() < 1e-3);
}

/// A generator full of coal wired to `machine`.
fn powered(w: &mut World, p: &Planet, machine: BlockCell) {
    let generator = at(p, 0, 10);
    w.factory.add(p, generator, Block::CoalGenerator, 0);
    w.factory.machine_mut(generator).unwrap().fuel = stack(Item::Coal, 64);
    assert!(w.factory.connect(generator, machine));
}

#[test]
fn an_assembler_makes_its_recipe_from_what_a_line_brings() {
    let p = planet();
    let mut w = World::new();
    let (from, assembler, to) = (at(&p, 0, 0), at(&p, 2, 0), at(&p, 4, 0));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 10);
    slots[1] = stack(Item::CopperIngot, 10);
    w.chests.insert(from, slots);
    w.chests.insert(to, [None; CHEST_SLOTS]);
    w.factory.add(&p, at(&p, 1, 0), Block::Conveyor, EAST);
    w.factory.add(&p, assembler, Block::Assembler, 0);
    w.factory.add(&p, at(&p, 3, 0), Block::Conveyor, EAST);
    powered(&mut w, &p, assembler);
    // Nothing picked: it takes nothing.
    w.run(5.0);
    assert_eq!(w.count(from, Item::CopperIngot), 10);
    assert!(
        w.factory
            .machine(assembler)
            .unwrap()
            .status()
            .contains("pick")
    );
    // Copper wire: 1 s each, three wires an ingot; the sticks stay in the chest.
    let back = w
        .factory
        .machine_mut(assembler)
        .unwrap()
        .set_recipe(Some(Item::CopperWire));
    assert!(back.is_empty());
    w.run(20.0);
    assert_eq!(w.count(to, Item::CopperWire), 30);
    assert_eq!(w.count(from, Item::Stick), 10);
    // Changing the recipe hands back what doesn't fit it.
    let m = w.factory.machine_mut(assembler).unwrap();
    m.input = stack(Item::CopperIngot, 3);
    let back = m.set_recipe(Some(Item::CircuitBoard));
    assert_eq!(back, vec![ItemStack::new(Item::CopperIngot, 3)]);
    assert!(m.input_takes(0, Item::Plastic) && m.input_takes(1, Item::CopperWire));
    assert!(!m.input_takes(0, Item::CopperWire));
}

#[test]
fn a_miner_brings_up_what_is_under_it_on_power() {
    let p = planet();
    let mut w = World::new();
    // (Three blocks square: the belt starts by its back, from the cell at u = 1.)
    let (miner, to) = (at(&p, 0, 0), at(&p, 3, 0));
    w.factory.add(&p, miner, Block::Miner, 3); // its back towards +u
    w.factory.add(&p, at(&p, 2, 0), Block::Conveyor, EAST);
    w.chests.insert(to, [None; CHEST_SLOTS]);
    w.run(5.0);
    assert!(
        w.factory
            .machine(miner)
            .unwrap()
            .status()
            .contains("nothing")
    );
    w.factory.set_resource(miner, mined_from(Block::IronOre));
    w.run(5.0);
    assert!(
        w.factory
            .machine(miner)
            .unwrap()
            .status()
            .contains("no power")
    );
    powered(&mut w, &p, miner);
    w.run(26.0);
    // 2.5 s each: ten in 25 s, carried off into the chest.
    let n = w.count(to, Item::RawIron);
    assert!((9..=10).contains(&n), "{n}");
    assert_eq!(
        mined_from(Block::Stone),
        Some(Item::Block(Block::Cobblestone))
    );
    assert_eq!(mined_from(Block::Air), None);
}

#[test]
fn a_conveyor_fed_from_one_side_bends_round() {
    let p = planet();
    let mut w = World::new();
    // A line east from a chest, turning north at (2, 0) into a chest at (2, 1).
    let (from, corner, to) = (at(&p, 0, 0), at(&p, 2, 0), at(&p, 2, 1));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 8);
    w.chests.insert(from, slots);
    w.chests.insert(to, [None; CHEST_SLOTS]);
    w.factory.add(&p, at(&p, 1, 0), Block::Conveyor, EAST);
    w.factory.add(&p, corner, Block::Conveyor, 2); // +v
    w.run(0.2);
    let bend = w.factory.conveyor(corner).unwrap().bend;
    assert_ne!(bend, 0, "it should come round from the side");
    assert!(w.factory.bends_changed.contains(&(corner, bend)));
    assert_eq!(w.factory.conveyor(at(&p, 1, 0)).unwrap().bend, 0);
    // Items go round it, onto its start (not its middle), and on.
    w.run(12.0);
    assert_eq!(w.count(to, Item::Stick), 8);
    // Straight again once something feeds it from behind too.
    w.factory.add(&p, at(&p, 2, -1), Block::Conveyor, 2);
    w.run(0.2);
    assert_eq!(w.factory.conveyor(corner).unwrap().bend, 0);
}

#[test]
fn every_ingredient_can_be_made() {
    use crate::world::block::Block::*;
    // What comes out of the ground, furnaces and pumps.
    // What comes out of the ground (by hand or a miner), furnaces and pumps.
    let mut made: Vec<Item> = [
        Sand,
        Stone,
        IronOre,
        CopperOre,
        GoldOre,
        DiamondOre,
        CoalOre,
        LeadOre,
        FluoriteOre,
        SulfurOre,
        SaltpeterOre,
    ]
    .iter()
    .filter_map(|&b| mined_from(b))
    .chain([
        Item::CrudeOil,
        Item::Block(Sand),
        Item::Block(Glass),
        Item::Block(Cobblestone),
        Item::Block(UraniumOre),
        Item::Block(BauxiteOre),
        Item::Block(IlmeniteSand),
        // (A water pump's.)
        Item::Fluid(Fluid::Water),
    ])
    .collect();
    let smelted: Vec<Item> = made.iter().filter_map(|i| i.smelted()).collect();
    made.extend(smelted);
    // Then whatever recipes make from those (and a reactor from its rods), until nothing new
    // comes.
    loop {
        let before = made.len();
        for r in recipes::RECIPES {
            let fluids = r
                .fluids
                .iter()
                .all(|(f, _)| made.contains(&Item::Fluid(*f)));
            if !(r.inputs.iter().all(|(i, _)| made.contains(i)) && fluids) {
                continue;
            }
            let products = std::iter::once(r.output)
                .chain(r.extra.map(|(i, _)| i))
                .chain(r.fluid_out.iter().map(|(f, _)| Item::Fluid(*f)));
            for p in products {
                if !made.contains(&p) {
                    made.push(p);
                }
            }
        }
        if made.contains(&Item::UraniumFuelRod) && !made.contains(&Item::SpentFuelRod) {
            made.push(Item::SpentFuelRod);
        }
        if made.len() == before {
            break;
        }
    }
    for r in recipes::RECIPES {
        assert!(made.contains(&r.output), "{:?} can't be made", r.output);
        assert!(r.inputs.len() <= 4 && r.count >= 1 && r.time > 0.0);
        assert!(r.fluids.len() <= 2 && r.fluid_out.len() <= 2);
        assert!(r.machine.has_recipes() && r.machine.has_tanks() || r.fluids.is_empty());
    }
    for item in [
        Item::Computer,
        Item::AiChip,
        Item::Screen,
        Item::Fuel,
        Item::Rubber,
        Item::Plutonium,
        Item::MoxFuelRod,
        Item::Antimatter,
        Item::Block(VitrifiedWaste),
        Item::AluminiumIngot,
        Item::SuperconductingMagnet,
    ] {
        assert!(made.contains(&item), "{item:?}");
    }
    // A machine makes each thing one way (a recipe is known by what it makes).
    for (i, a) in recipes::RECIPES.iter().enumerate() {
        for b in &recipes::RECIPES[i + 1..] {
            assert!(
                a.machine != b.machine || a.output != b.output,
                "two ways to make {:?}",
                a.output
            );
        }
    }
}

#[test]
fn machines_share_what_a_network_makes() {
    let p = planet();
    let mut w = World::new();
    let generator = at(&p, 0, 0);
    w.factory.add(&p, generator, Block::CoalGenerator, 0);
    w.factory.machine_mut(generator).unwrap().fuel = stack(Item::Coal, 10);
    let furnaces: Vec<_> = (1..=3).map(|u| at(&p, u * 2, 0)).collect();
    for &f in &furnaces {
        w.factory.add(&p, f, Block::ElectricFurnace, 0);
        w.factory.machine_mut(f).unwrap().input = stack(Item::RawCopper, 64);
        assert!(w.factory.connect(generator, f));
    }
    // 180 kW for three 90 kW furnaces: each runs at two thirds.
    w.run(31.0);
    for &f in &furnaces {
        let m = w.factory.machine(f).unwrap();
        assert!((m.supply - 2.0 / 3.0).abs() < 1e-3, "{}", m.supply);
        assert_eq!(m.output.unwrap().count, 4); // 31 s * 2/3 / 5 s
    }
    assert_eq!(w.factory.networks().len(), 1);
    // Cutting one off leaves the other two at full power.
    assert!(w.factory.disconnect(generator, furnaces[2]));
    w.run(1.0);
    assert!((w.factory.machine(furnaces[0]).unwrap().supply - 1.0).abs() < 1e-3);
    assert_eq!(w.factory.machine(furnaces[2]).unwrap().supply, 0.0);
}

#[test]
fn conveyors_join_from_the_side_and_never_head_on() {
    let p = planet();
    let mut w = World::new();
    // A line running east into a chest, fed from the side by one running north into it.
    let (feed, out) = (at(&p, 0, 0), at(&p, 4, 0));
    let side = at(&p, 2, -2);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 4);
    w.chests.insert(feed, slots);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Coal, 4);
    w.chests.insert(side, slots);
    w.chests.insert(out, [None; CHEST_SLOTS]);
    for u in 1..=3 {
        w.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    w.factory.add(&p, at(&p, 2, -1), Block::Conveyor, 2); // +v
    w.run(20.0);
    assert_eq!(w.count(out, Item::Stick), 4);
    assert_eq!(w.count(out, Item::Coal), 4);

    // Two conveyors pointing at each other pass nothing.
    let mut w = World::new();
    let chest = at(&p, 0, 5);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 4);
    w.chests.insert(chest, slots);
    w.factory.add(&p, at(&p, 1, 5), Block::Conveyor, EAST);
    w.factory.add(&p, at(&p, 2, 5), Block::Conveyor, 3); // -u, back at it
    w.run(10.0);
    let c = w.factory.conveyor(at(&p, 2, 5)).unwrap();
    assert!(c.is_empty());
    assert_eq!(w.factory.conveyor(at(&p, 1, 5)).unwrap().len(), 4);
}

#[test]
fn without_authority_conveyors_only_carry() {
    let p = planet();
    let mut w = World::new();
    w.factory.authority = false;
    let chest = at(&p, 0, 0);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Stick, 4);
    w.chests.insert(chest, slots);
    w.factory.add(&p, at(&p, 1, 0), Block::Conveyor, EAST);
    w.run(5.0);
    assert_eq!(w.count(chest, Item::Stick), 4);
    assert!(w.factory.changed.is_empty());
}

#[test]
fn a_saved_factory_and_what_others_are_sent_come_back_the_same() {
    let p = planet();
    let mut w = World::new();
    let chest = at(&p, 0, 0);
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Coal, 64);
    w.chests.insert(chest, slots);
    for u in 1..=6 {
        w.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    let (g, pole) = (at(&p, 7, 0), at(&p, 7, 3));
    w.factory.add(&p, g, Block::CoalGenerator, 0);
    w.factory.add(&p, pole, Block::PowerPole, 0);
    w.factory.connect(g, pole);
    w.run(2.0);

    let json = serde_json::to_string(&w.factory.saved()).unwrap();
    let back: SavedFactory = serde_json::from_str(&json).unwrap();
    let copy = Factory::restored(back);
    assert_eq!(copy.counts().0, 6);
    assert!(copy.connected(pole, g));
    let items = |f: &Factory| -> Vec<(Item, u8)> {
        (1..=6)
            .flat_map(|u| {
                f.conveyor(at(&p, u, 0))
                    .unwrap()
                    .carried()
                    .copied()
                    .collect::<Vec<_>>()
            })
            .map(|i| (i.stack.item, (i.at * 255.0).round() as u8))
            .collect()
    };
    assert_eq!(items(&copy), items(&w.factory));

    // What another player is sent: the same items on the same conveyors.
    w.factory.locate(&p);
    let near = w
        .factory
        .belts_near(models::cell_point(&p, chest, [8.0; 3]), 32.0);
    assert!(!near.is_empty());
    let mut other = World::new();
    other.factory = Factory::restored(SavedFactory::default());
    for u in 1..=6 {
        other.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    other.factory.authority = false;
    other.run(0.1);
    for group in &near {
        other.factory.apply_belts(group);
    }
    assert_eq!(items(&other.factory), items(&w.factory));
    // Taking the generator away takes its cable with it.
    let removed = w.factory.remove(g).unwrap();
    assert_eq!(removed.cables, 1);
    assert!(w.factory.cables().is_empty());
}

/// A big factory: 20,000 loaded conveyors in loops stay cheap to run.
#[test]
fn a_big_factory_is_cheap() {
    let p = planet();
    let mut w = World::new();
    let (rows, length) = (100, 200);
    for r in 0..rows {
        for u in 0..length {
            let cell = at(&p, u, r * 2);
            w.factory.add(&p, cell, Block::Conveyor, EAST);
        }
        // Each row starts at a full chest and ends in an empty one.
        let mut slots = [None; CHEST_SLOTS];
        for s in &mut slots {
            *s = stack(Item::Block(Block::Cobblestone), 64);
        }
        w.chests.insert(at(&p, -1, r * 2), slots);
        w.chests.insert(at(&p, length, r * 2), [None; CHEST_SLOTS]);
    }
    w.run(45.0); // filling up
    let start = Instant::now();
    let ticks = 100;
    for _ in 0..ticks {
        let mut stores = Stores {
            chests: &mut w.chests,
            furnaces: &mut w.furnaces,
        };
        w.factory.tick(TICK as f32, &mut stores);
    }
    let per_tick = start.elapsed().as_secs_f64() * 1000.0 / ticks as f64;
    println!("{} conveyors: {per_tick:.3} ms a tick", rows * length);
    // (Debug builds are far slower; this is for `cargo test --release`.)
    if !cfg!(debug_assertions) {
        assert!(per_tick < 2.0, "{per_tick} ms a tick");
    }
    let carried: usize = (0..length)
        .map(|u| w.factory.conveyor(at(&p, u, 0)).unwrap().len())
        .sum();
    assert!(
        carried > length as usize,
        "{carried} items on the first row"
    );
}

#[test]
fn one_line_brings_a_machine_all_its_ingredients() {
    // Sand and coal down one line into an arc furnace making silicon (two sand, one coal). The
    // line must not fill up with whichever the chest gives first.
    let p = planet();
    let mut w = World::new();
    let (from, furnace, to) = (at(&p, 0, 0), at(&p, 4, 0), at(&p, 6, 0));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::Block(Block::Sand), 64);
    slots[1] = stack(Item::Coal, 64);
    w.chests.insert(from, slots);
    w.chests.insert(to, [None; CHEST_SLOTS]);
    for u in 1..=3 {
        w.factory.add(&p, at(&p, u, 0), Block::Conveyor, EAST);
    }
    w.factory.add(&p, furnace, Block::ArcFurnace, 0);
    w.factory.add(&p, at(&p, 5, 0), Block::Conveyor, EAST);
    w.factory
        .machine_mut(furnace)
        .unwrap()
        .set_recipe(Some(Item::Silicon));
    powered(&mut w, &p, furnace);
    w.run(60.0);
    // 4 s a batch: about 14 in a minute, less the time the first ingredients take to arrive.
    let n = w.count(to, Item::Silicon);
    assert!(n >= 10, "{n} silicon");
}

// ---- Fluids, power and the big machines --------------------------------------------------

/// Generators enough for `watts`, from (0, 12) on, each cabled to `machines`.
fn generators(w: &mut World, p: &Planet, watts: f32, machines: &[BlockCell]) {
    let n = (watts / GENERATOR_POWER).ceil() as i32;
    for i in 0..n {
        let g = at(p, i * 2, 12);
        w.factory.add(p, g, Block::CoalGenerator, 0);
        w.factory.machine_mut(g).unwrap().fuel = stack(Item::Coal, 64);
        for &m in machines {
            assert!(w.factory.connect(g, m));
        }
    }
}

fn level(w: &World, cell: BlockCell, i: usize, fluid: Fluid) -> f32 {
    fluids::level(&w.factory.machine(cell).unwrap().tanks[i], fluid)
}

#[test]
fn a_boiler_and_a_turbine_make_power_from_water_and_coal() {
    // A tank's water down a pipe into a boiler; its steam down another into a steam turbine
    // (three by three: the pipe meets one of its outer cells), which runs an electric furnace.
    let p = planet();
    let mut w = World::new();
    let (tank, boiler, turbine, furnace) = (at(&p, 0, 0), at(&p, 2, 0), at(&p, 5, 0), at(&p, 5, 5));
    w.factory.add(&p, tank, Block::StorageTank, 0);
    w.factory.add(&p, at(&p, 1, 0), Block::Pipe, 0);
    w.factory.add(&p, boiler, Block::Boiler, 0);
    w.factory.add(&p, at(&p, 3, 0), Block::Pipe, 0);
    w.factory.add(&p, turbine, Block::SteamTurbine, 0);
    w.factory.add(&p, furnace, Block::ElectricFurnace, 0);
    assert!(w.factory.connect(turbine, furnace));
    assert_eq!(w.factory.fill_fluid(tank, Fluid::Water, 5000.0), 5000.0);
    w.factory.machine_mut(boiler).unwrap().fuel = stack(Item::Coal, 10);
    w.factory.machine_mut(furnace).unwrap().input = stack(Item::RawIron, 10);
    w.run(25.0);
    let t = w.factory.machine(turbine).unwrap();
    assert!(
        (t.power - BOILER_HEAT).abs() < BOILER_HEAT * 0.1,
        "the turbine makes {} w",
        t.power
    );
    assert!(level(&w, tank, 0, Fluid::Water) < 5000.0 - 100.0);
    let made = w
        .factory
        .machine(furnace)
        .unwrap()
        .output
        .map_or(0, |s| s.count);
    assert!(made >= 2, "{made} ingots");
    // The two lines kept apart: water in one, steam in the other.
    let lines = w.factory.fluid_networks();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().any(|l| l.1 == Some(Fluid::Water)));
    assert!(lines.iter().any(|l| l.1 == Some(Fluid::Steam)));
}

/// A reactor (and a turbine for its steam) with its own generator for its pumps; its water
/// from a tank, down a pipe.
fn reactor_plant(w: &mut World, p: &Planet) -> (BlockCell, BlockCell, BlockCell) {
    let (reactor, tank, turbine) = (at(p, 0, 0), at(p, -4, 0), at(p, 5, 0));
    w.factory.add(p, reactor, Block::NuclearReactor, 0);
    w.factory.add(p, tank, Block::StorageTank, 0);
    w.factory.add(p, at(p, -3, 0), Block::Pipe, 0);
    w.factory.add(p, at(p, 3, 0), Block::Pipe, 0);
    w.factory.add(p, turbine, Block::SteamTurbine, 0);
    generators(w, p, 150_000.0, &[reactor]);
    w.factory.fill_fluid(tank, Fluid::Water, fluids::TANK_HOLDS);
    w.factory.machine_mut(reactor).unwrap().fuel = stack(Item::UraniumFuelRod, 2);
    (reactor, tank, turbine)
}

#[test]
fn a_reactor_boils_water_for_its_turbines() {
    let p = planet();
    let mut w = World::new();
    let (reactor, _, turbine) = reactor_plant(&mut w, &p);
    // (Its steam fills its tank first, at full power.)
    w.run(45.0);
    let r = *w.factory.machine(reactor).unwrap();
    assert!(r.level > 0.1 && r.heat < OVERHEATING, "{r:?}");
    assert!(r.heat > AMBIENT + 20.0, "it warms up: {}", r.heat);
    assert_eq!(r.fuel.map(|s| s.count), Some(1), "a rod went in");
    // The turbine runs flat out on its steam, and the reactor keeps to what is taken.
    let made = w.factory.machine(turbine).unwrap().power;
    assert!(made > TURBINE_POWER * 0.95, "{made} w");
    assert!(
        r.level < 0.8,
        "it eases off with nowhere for its steam: {}",
        r.level
    );
    // Without power for its pumps its rods go in.
    let cables: Vec<_> = w.factory.cables().to_vec();
    for (a, b) in cables {
        w.factory.disconnect(a, b);
    }
    w.run(6.0);
    assert!(w.factory.machine(reactor).unwrap().level < 0.01);
}

#[test]
fn a_reactor_without_water_melts_down() {
    let p = planet();
    let mut w = World::new();
    let (reactor, tank, _) = reactor_plant(&mut w, &p);
    w.run(10.0);
    assert!(w.factory.meltdowns.is_empty());
    // The water runs out.
    w.factory.machine_mut(tank).unwrap().tanks[0] = None;
    w.factory.machine_mut(reactor).unwrap().tanks[0] = None;
    for pipe in &mut w.factory.pipes {
        pipe.held = None;
    }
    let mut warned = false;
    for _ in 0..60 {
        w.run(5.0);
        let m = w.factory.machine(reactor).unwrap();
        warned |= m.status().contains("overheating");
        if !w.factory.meltdowns.is_empty() {
            break;
        }
    }
    assert!(warned, "it said it was overheating first");
    assert_eq!(w.factory.meltdowns, vec![reactor]);
}

#[test]
fn an_electrolyzer_sends_each_gas_down_the_line_that_wants_it() {
    // Water into an electrolyzer; one line on to a blender making sulfuric acid (it wants
    // oxygen), one to an empty tank. The oxygen goes to the blender, the hydrogen to the tank.
    let p = planet();
    let mut w = World::new();
    let (water, cell, blender, gas, acid) = (
        at(&p, 0, 0),
        at(&p, 2, 0),
        at(&p, 6, 0),
        at(&p, 2, 2),
        at(&p, 10, 0),
    );
    w.factory.add(&p, water, Block::StorageTank, 0);
    w.factory.add(&p, at(&p, 1, 0), Block::Pipe, 0);
    w.factory.add(&p, cell, Block::Electrolyzer, 0);
    w.factory.add(&p, at(&p, 3, 0), Block::Pipe, 0);
    w.factory.add(&p, blender, Block::Blender, 0);
    w.factory.add(&p, at(&p, 2, 1), Block::Pipe, 0);
    w.factory.add(&p, gas, Block::StorageTank, 0);
    w.factory.add(&p, at(&p, 9, 0), Block::Pipe, 0);
    w.factory.add(&p, acid, Block::StorageTank, 0);
    w.factory.fill_fluid(water, Fluid::Water, 10_000.0);
    w.factory
        .machine_mut(cell)
        .unwrap()
        .set_recipe(Some(Item::Fluid(Fluid::Hydrogen)));
    let b = w.factory.machine_mut(blender).unwrap();
    b.set_recipe(Some(Item::Fluid(Fluid::SulfuricAcid)));
    b.input = stack(Item::Sulfur, 20);
    w.factory.fill_fluid(blender, Fluid::Water, 2000.0);
    generators(&mut w, &p, 600_000.0, &[cell, blender]);
    w.run(40.0);
    assert!(level(&w, gas, 0, Fluid::Hydrogen) > 100.0);
    assert!(level(&w, acid, 0, Fluid::SulfuricAcid) >= 200.0);
    let b = w.factory.machine(blender).unwrap();
    assert!(b.input.map_or(0, |s| s.count) < 20, "it used sulfur");
    assert_eq!(level(&w, blender, 1, Fluid::Hydrogen), 0.0);
}

#[test]
fn wind_turbines_and_rtgs_make_power() {
    assert_eq!(wind_power(1.0), 0.0);
    assert_eq!(wind_power(WIND_RATED + 2.0), WIND_POWER);
    assert_eq!(wind_power(WIND_CUT_OUT + 1.0), 0.0);
    assert!(wind_power(6.0) > 0.0 && wind_power(6.0) < wind_power(9.0));
    let p = planet();
    let mut w = World::new();
    let (wind, rtg, furnace) = (at(&p, 0, 0), at(&p, 2, 0), at(&p, 4, 0));
    w.factory.add(&p, wind, Block::WindTurbine, 0);
    w.factory.add(&p, rtg, Block::Rtg, 0);
    w.factory.add(&p, furnace, Block::ElectricFurnace, 0);
    w.factory.locate(&p);
    w.factory.set_wind(|_, _| WIND_RATED);
    assert!(w.factory.connect(wind, furnace) && w.factory.connect(rtg, furnace));
    w.run(1.0);
    let net = &w.factory.networks()[0];
    assert!(
        (net.capacity - WIND_POWER - RTG_POWER).abs() < 1.0,
        "{}",
        net.capacity
    );
    w.factory.set_wind(|_, _| 0.0);
    w.run(1.0);
    assert!((w.factory.networks()[0].capacity - RTG_POWER).abs() < 1.0);
}

#[test]
fn a_big_machine_is_one_from_any_of_its_cells() {
    // Spent rods along a conveyor into the side of a cooling pool (three by three), which cools
    // them under water; its cells are all the one machine, and taking any away takes it.
    let p = planet();
    let mut w = World::new();
    let (chest, pool) = (at(&p, -1, 0), at(&p, 2, 0));
    let mut slots = [None; CHEST_SLOTS];
    slots[0] = stack(Item::SpentFuelRod, 2);
    w.chests.insert(chest, slots);
    w.factory.add(&p, at(&p, 0, 0), Block::Conveyor, EAST);
    w.factory.add(&p, pool, Block::CoolingPool, 0);
    let side = at(&p, 1, 1);
    assert_eq!(w.factory.core(side), pool);
    assert_eq!(w.factory.machine(side).unwrap().cell, pool);
    // (It makes only one thing, so it is set to make it.)
    assert_eq!(
        w.factory.machine(pool).unwrap().recipe,
        Some(Item::CooledSpentFuel)
    );
    w.factory.fill_fluid(pool, Fluid::Water, 500.0);
    generators(&mut w, &p, 20_000.0, &[side]);
    w.run(100.0);
    let m = w.factory.machine(pool).unwrap();
    assert_eq!(
        m.output.map(|s| (s.item, s.count)),
        Some((Item::CooledSpentFuel, 1))
    );
    assert!(w.factory.remove(at(&p, 3, -1)).is_some());
    assert!(w.factory.machine(pool).is_none() && w.factory.machine(side).is_none());
}

#[test]
fn pipes_keep_what_they_hold_when_saved() {
    let p = planet();
    let mut w = World::new();
    let tank = at(&p, 0, 0);
    w.factory.add(&p, tank, Block::StorageTank, 0);
    for u in 1..=3 {
        w.factory.add(&p, at(&p, u, 0), Block::Pipe, 0);
    }
    w.factory.fill_fluid(tank, Fluid::NitricAcid, 1000.0);
    w.run(0.5);
    let before = w.factory.fluid_networks();
    let json = serde_json::to_string(&w.factory.saved()).unwrap();
    let mut restored = Factory::restored(serde_json::from_str(&json).unwrap());
    restored.locate(&p);
    restored.authority = true;
    let mut w2 = World {
        factory: restored,
        ..World::new()
    };
    w2.run(0.5);
    let after = w2.factory.fluid_networks();
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].1, Some(Fluid::NitricAcid));
    assert!(
        (before[0].2 - after[0].2).abs() < 1.0,
        "{before:?} {after:?}"
    );
}

/// Puts a pipe in `cell` as a player would: clicked against `support`, looking along `axis`
/// (0 u, 1 v, 2 up).
fn build_pipe(w: &mut World, p: &Planet, cell: BlockCell, support: BlockCell, axis: u8) {
    let ways = w.factory.pipe_ways(p, cell, support, axis);
    w.factory.add(p, cell, Block::Pipe, 0);
    w.factory.set_pipe_ways(cell, Some(ways), axis);
}

/// The cell under `cell` (the ground a pipe is put on).
fn under(p: &Planet, cell: BlockCell) -> BlockCell {
    p.cell(cell.face, cell.u, cell.v, cell.k - 1).unwrap()
}

#[test]
fn pipe_lines_side_by_side_keep_apart() {
    // Two lines, touching all along: one built pipe on pipe from a tank of water, the other
    // put on the ground looking along it from a tank of oxygen. Each reaches its own tank.
    let p = planet();
    let mut w = World::new();
    let (water, oxygen) = (at(&p, 0, 0), at(&p, 0, 1));
    let (water_out, oxygen_out) = (at(&p, 5, 0), at(&p, 5, 1));
    for tank in [water, oxygen, water_out, oxygen_out] {
        w.factory.add(&p, tank, Block::StorageTank, 0);
    }
    let mut support = water;
    for u in 1..=4 {
        build_pipe(&mut w, &p, at(&p, u, 0), support, 1);
        support = at(&p, u, 0);
    }
    for u in 1..=4 {
        build_pipe(&mut w, &p, at(&p, u, 1), under(&p, at(&p, u, 1)), 0);
    }
    w.factory.fill_fluid(water, Fluid::Water, 5000.0);
    w.factory.fill_fluid(oxygen, Fluid::Oxygen, 5000.0);
    w.run(2.0);
    let lines = w.factory.fluid_networks();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(level(&w, water_out, 0, Fluid::Water) > 1000.0);
    assert!(level(&w, oxygen_out, 0, Fluid::Oxygen) > 1000.0);
    assert_eq!(level(&w, water_out, 0, Fluid::Oxygen), 0.0);
}

#[test]
fn pipes_turn_corners_and_join_what_their_ends_point_at() {
    // From a tank: on along u, round a corner by clicking the last pipe's side, on along v on
    // the ground; a tank put afterwards in front of the open end joins it.
    let p = planet();
    let mut w = World::new();
    let (from, to) = (at(&p, 0, 0), at(&p, 2, 3));
    w.factory.add(&p, from, Block::StorageTank, 0);
    build_pipe(&mut w, &p, at(&p, 1, 0), from, 0);
    build_pipe(&mut w, &p, at(&p, 2, 0), at(&p, 1, 0), 0);
    build_pipe(&mut w, &p, at(&p, 2, 1), at(&p, 2, 0), 1);
    build_pipe(&mut w, &p, at(&p, 2, 2), under(&p, at(&p, 2, 2)), 1);
    // (A pipe put down beside the line, looking along it, joins nothing.)
    build_pipe(&mut w, &p, at(&p, 3, 1), under(&p, at(&p, 3, 1)), 1);
    w.factory.add(&p, to, Block::StorageTank, 0);
    w.factory.fill_fluid(from, Fluid::Water, 5000.0);
    w.run(2.0);
    assert!(level(&w, to, 0, Fluid::Water) > 1000.0);
    let lines = w.factory.fluid_networks();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().any(|l| l.0 == 1 && l.1.is_none()));
}

#[test]
fn an_empty_line_nothing_uses_carries_something_else() {
    let p = planet();
    let mut w = World::new();
    let (from, to) = (at(&p, 0, 0), at(&p, 3, 0));
    w.factory.add(&p, from, Block::StorageTank, 0);
    w.factory.add(&p, to, Block::StorageTank, 0);
    build_pipe(&mut w, &p, at(&p, 1, 0), from, 0);
    build_pipe(&mut w, &p, at(&p, 2, 0), at(&p, 1, 0), 0);
    w.factory.fill_fluid(from, Fluid::Water, 1000.0);
    w.run(1.0);
    assert_eq!(w.factory.fluid_networks()[0].1, Some(Fluid::Water));
    // Emptied (the line keeps to water while a tank holds some) ...
    for tank in [from, to] {
        w.factory.machine_mut(tank).unwrap().tanks[0] = None;
    }
    for pipe in &mut w.factory.pipes {
        pipe.held = Some(FluidStack {
            fluid: Fluid::Water,
            amount: 0.0,
        });
    }
    w.run(0.5);
    assert_eq!(w.factory.fluid_networks()[0].1, None);
    // ... it takes what is sent down it next.
    w.factory.fill_fluid(from, Fluid::Hydrogen, 1000.0);
    w.run(1.0);
    assert_eq!(w.factory.fluid_networks()[0].1, Some(Fluid::Hydrogen));
    assert!(level(&w, to, 0, Fluid::Hydrogen) > 100.0);
}
