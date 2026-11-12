//! What machines make, and from what. A recipe machine makes the one picked on its screen; each
//! of its input slots holds one ingredient, and its two input tanks the fluids it takes (piped
//! in, see `fluids`). A recipe may make a second item (`extra`, its own output slot) and fluids
//! (piped out). A recipe is known by what it makes (`output`; a fluid's is `Item::Fluid`), so a
//! machine has one recipe for each product.
//!
//! The production chains:
//! - **Oil:** an oil pump over oil shale brings up crude oil. A refinery makes fuel (burns five
//!   times as long as coal), plastic, rubber, solvent or sulfur from it.
//! - **Metal and silicon:** an arc furnace makes steel from iron and coal, and silicon from sand
//!   and coal. A chip fab cuts silicon into wafers. Ilmenite sand makes titanium (and zircon,
//!   which makes zirconium); a blender leaches bauxite into alumina, which an electrolyzer turns
//!   into aluminium. Lead is smelted from galena.
//! - **Chips:** wafers and copper wire make LEDs, memory and (with gold) CPUs; AI chips also take
//!   memory and a diamond.
//! - **Parts:** an assembler draws copper into wire, insulates it with rubber, lays boards out of
//!   plastic and wire, builds batteries, screens (glass, LEDs, a board, plastic), motherboards,
//!   power supplies and cases, motors, turbine blades, superconducting magnets and pipes.
//! - **Computers:** case + motherboard + power supply + screen.
//! - **Chemistry** (a blender, on pipes): sulfur, water and oxygen (from an electrolyzer) make
//!   sulfuric acid; that and saltpeter make nitric acid, and fluorite hydrofluoric acid.
//!   Concrete is cobblestone, sand and water.
//! - **Nuclear fuel:** sulfuric acid leaches uranium ore into yellowcake; hydrofluoric acid turns
//!   that into uranium hexafluoride, which centrifuges enrich (leaving depleted uranium). A fuel
//!   fabricator makes rods of it with hydrogen and zirconium. A reactor burns them.
//! - **Recycling spent fuel** (on purpose a long way round): spent rods cool under water in a
//!   cooling pool. A reprocessing plant chops them up into nitric acid (the cladding hulls come
//!   out, and an arc furnace makes zircon of them), pulls uranium and plutonium out of that with
//!   solvent (leaving liquid waste), then strips them out of the solvent apart (the solvent comes
//!   back). Plutonium and the recovered uranium make MOX rods, which last half as long again;
//!   an arc furnace sets the liquid waste in glass. A particle accelerator makes plutonium out of
//!   depleted uranium too (and antimatter out of hydrogen, and gold out of lead), on a great
//!   deal of power.

use super::{MachineKind, fluids::Fluid};
use crate::{survival::items::Item, world::block::Block};

pub struct Recipe {
    pub machine: MachineKind,
    /// Up to four ingredients, each with how many it takes.
    pub inputs: &'static [(Item, u8)],
    /// Up to two fluids it takes (litres), into its input tanks in this order.
    pub fluids: &'static [(Fluid, u16)],
    pub output: Item,
    pub count: u8,
    /// A second product, into its own slot.
    pub extra: Option<(Item, u8)>,
    /// Up to two fluids it makes (litres).
    pub fluid_out: &'static [(Fluid, u16)],
    /// Seconds per batch at full power.
    pub time: f32,
}

impl Recipe {
    /// Also takes these fluids.
    const fn taking(mut self, fluids: &'static [(Fluid, u16)]) -> Self {
        self.fluids = fluids;
        self
    }

    /// Also makes this.
    const fn also(mut self, item: Item, count: u8) -> Self {
        self.extra = Some((item, count));
        self
    }

    /// Also makes these fluids.
    const fn giving(mut self, fluids: &'static [(Fluid, u16)]) -> Self {
        self.fluid_out = fluids;
        self
    }

    /// What it makes, for its card: "sulfuric acid 200 l", "titanium ingot x1".
    pub fn makes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !matches!(self.output, Item::Fluid(_)) {
            out.push(amount(self.output, self.count));
        }
        if let Some((item, n)) = self.extra {
            out.push(amount(item, n));
        }
        for &(fluid, n) in self.fluid_out {
            out.push(format!("{} {n} l", fluid.name()));
        }
        out
    }
}

fn amount(item: Item, n: u8) -> String {
    let name = item.name().to_lowercase();
    if n > 1 { format!("{name} x{n}") } else { name }
}

const fn r(
    machine: MachineKind,
    inputs: &'static [(Item, u8)],
    output: Item,
    count: u8,
    time: f32,
) -> Recipe {
    Recipe {
        machine,
        inputs,
        fluids: &[],
        output,
        count,
        extra: None,
        fluid_out: &[],
        time,
    }
}

/// A recipe whose product is a fluid (the first of `out`).
const fn f(
    machine: MachineKind,
    inputs: &'static [(Item, u8)],
    out: &'static [(Fluid, u16)],
    time: f32,
) -> Recipe {
    Recipe {
        output: Item::Fluid(out[0].0),
        count: 1,
        fluid_out: out,
        ..r(machine, inputs, Item::Stick, 0, time)
    }
}

use Fluid::*;
use Item::*;
use MachineKind::{
    Accelerator as Acc, ArcFurnace as Arc, Assembler as Asm, Blender as Mix, Centrifuge as Cen,
    ChipFab as Fab, CoolingPool as Pool, Electrolyzer as Ely, FuelFabricator as Fuf,
    Refinery as Ref, Reprocessing as Rep,
};

pub static RECIPES: &[Recipe] = &[
    // Refinery.
    r(Ref, &[(CrudeOil, 2)], Fuel, 1, 4.0),
    r(Ref, &[(CrudeOil, 1)], Plastic, 2, 3.0),
    r(Ref, &[(CrudeOil, 1)], Rubber, 1, 3.0),
    f(Ref, &[(CrudeOil, 1)], &[(Solvent, 100)], 3.0),
    r(Ref, &[(CrudeOil, 2)], Sulfur, 1, 4.0),
    // Arc furnace.
    r(Arc, &[(IronIngot, 1), (Coal, 1)], SteelIngot, 1, 4.0),
    r(Arc, &[(Block(Block::Sand), 2), (Coal, 1)], Silicon, 1, 4.0),
    r(
        Arc,
        &[(Block(Block::IlmeniteSand), 2), (Coal, 1)],
        TitaniumIngot,
        1,
        6.0,
    )
    .also(Zircon, 1),
    r(Arc, &[(Zircon, 2), (Coal, 1)], Zirconium, 1, 6.0),
    r(Arc, &[(FuelHulls, 2)], Zircon, 1, 5.0),
    r(
        Arc,
        &[(Block(Block::Glass), 2)],
        Block(Block::VitrifiedWaste),
        1,
        10.0,
    )
    .taking(&[(LiquidWaste, 100)]),
    // Assembler.
    r(Asm, &[(CopperIngot, 1)], CopperWire, 3, 1.0),
    r(Asm, &[(CopperWire, 2), (Rubber, 1)], InsulatedWire, 2, 2.0),
    r(Asm, &[(CopperWire, 2), (Rubber, 1)], Cable, 4, 2.0),
    r(Asm, &[(Plastic, 1), (CopperWire, 3)], CircuitBoard, 1, 3.0),
    r(
        Asm,
        &[(SteelIngot, 1), (CopperIngot, 1), (Plastic, 1)],
        Battery,
        1,
        3.0,
    ),
    r(
        Asm,
        &[
            (Block(Block::Glass), 1),
            (Led, 4),
            (CircuitBoard, 1),
            (Plastic, 1),
        ],
        Screen,
        1,
        6.0,
    ),
    r(
        Asm,
        &[(CircuitBoard, 2), (Cpu, 1), (MemoryChip, 2)],
        Motherboard,
        1,
        8.0,
    ),
    r(
        Asm,
        &[(SteelIngot, 1), (InsulatedWire, 3), (CircuitBoard, 1)],
        PowerSupply,
        1,
        5.0,
    ),
    r(
        Asm,
        &[(SteelIngot, 3), (Plastic, 2), (Rubber, 1)],
        ComputerCase,
        1,
        4.0,
    ),
    r(
        Asm,
        &[
            (ComputerCase, 1),
            (Motherboard, 1),
            (PowerSupply, 1),
            (Screen, 1),
        ],
        Computer,
        1,
        10.0,
    ),
    r(
        Asm,
        &[(SteelIngot, 1), (CopperWire, 4), (IronIngot, 2)],
        ElectricMotor,
        1,
        4.0,
    ),
    r(Asm, &[(TitaniumIngot, 2)], TurbineBlade, 1, 4.0),
    r(
        Asm,
        &[(TitaniumIngot, 1), (InsulatedWire, 6), (SteelIngot, 1)],
        SuperconductingMagnet,
        1,
        6.0,
    ),
    r(Asm, &[(IronIngot, 2)], Block(Block::Pipe), 4, 1.5),
    // Chip fab.
    r(Fab, &[(Silicon, 1)], SiliconWafer, 2, 4.0),
    r(Fab, &[(SiliconWafer, 1), (CopperWire, 1)], Led, 4, 3.0),
    r(
        Fab,
        &[(SiliconWafer, 1), (CopperWire, 2)],
        MemoryChip,
        1,
        6.0,
    ),
    r(
        Fab,
        &[(SiliconWafer, 1), (CopperWire, 2), (GoldIngot, 1)],
        Cpu,
        1,
        10.0,
    ),
    r(
        Fab,
        &[
            (SiliconWafer, 2),
            (MemoryChip, 2),
            (GoldIngot, 1),
            (Diamond, 1),
        ],
        AiChip,
        1,
        20.0,
    ),
    // (Also by hand, but an assembler saves the clicks.)
    r(
        Asm,
        &[(IronIngot, 3), (CopperIngot, 2)],
        Block(Block::Conveyor),
        4,
        2.0,
    ),
    // Blender: acids, leaching, concrete.
    f(Mix, &[(Sulfur, 1)], &[(SulfuricAcid, 200)], 4.0).taking(&[(Water, 100), (Oxygen, 50)]),
    f(Mix, &[(Saltpeter, 1)], &[(NitricAcid, 100)], 4.0).taking(&[(SulfuricAcid, 50)]),
    f(Mix, &[(Fluorite, 1)], &[(HydrofluoricAcid, 100)], 4.0).taking(&[(SulfuricAcid, 50)]),
    r(Mix, &[(Block(Block::UraniumOre), 2)], Yellowcake, 1, 8.0).taking(&[(SulfuricAcid, 100)]),
    f(Mix, &[(Yellowcake, 1)], &[(Hexafluoride, 100)], 6.0).taking(&[(HydrofluoricAcid, 100)]),
    r(Mix, &[(Block(Block::BauxiteOre), 2)], Alumina, 1, 5.0).taking(&[(SulfuricAcid, 50)]),
    r(
        Mix,
        &[(Block(Block::Cobblestone), 2), (Block(Block::Sand), 2)],
        Block(Block::Concrete),
        4,
        3.0,
    )
    .taking(&[(Water, 100)]),
    // Electrolyzer.
    f(Ely, &[], &[(Hydrogen, 100), (Oxygen, 50)], 2.0).taking(&[(Water, 100)]),
    r(Ely, &[(Alumina, 2)], AluminiumIngot, 1, 6.0),
    // Centrifuge.
    f(Cen, &[], &[(EnrichedHexafluoride, 20)], 10.0)
        .taking(&[(Hexafluoride, 100)])
        .also(DepletedUranium, 1),
    // Fuel fabricator.
    r(Fuf, &[(Zirconium, 2)], UraniumFuelRod, 1, 15.0)
        .taking(&[(EnrichedHexafluoride, 100), (Hydrogen, 100)]),
    r(
        Fuf,
        &[(Plutonium, 1), (RecoveredUranium, 4), (Zirconium, 2)],
        MoxFuelRod,
        1,
        20.0,
    ),
    // Cooling pool.
    r(Pool, &[(SpentFuelRod, 1)], CooledSpentFuel, 1, 90.0).taking(&[(Water, 50)]),
    // Reprocessing: dissolve, extract, strip.
    f(Rep, &[(CooledSpentFuel, 1)], &[(DissolvedFuel, 100)], 10.0)
        .taking(&[(NitricAcid, 100)])
        .also(FuelHulls, 1),
    f(Rep, &[], &[(LoadedSolvent, 100), (LiquidWaste, 50)], 8.0)
        .taking(&[(DissolvedFuel, 100), (Solvent, 50)]),
    r(Rep, &[], RecoveredUranium, 3, 12.0)
        .taking(&[(LoadedSolvent, 100), (NitricAcid, 50)])
        .also(Plutonium, 1)
        .giving(&[(Solvent, 40)]),
    // Particle accelerator.
    r(Acc, &[(DepletedUranium, 4)], Plutonium, 1, 60.0),
    r(Acc, &[], Antimatter, 1, 120.0).taking(&[(Hydrogen, 200)]),
    r(Acc, &[(LeadIngot, 4)], GoldIngot, 1, 30.0),
];

/// What `kind` can make, in the order its screen shows them.
pub fn for_machine(kind: MachineKind) -> impl Iterator<Item = &'static Recipe> {
    RECIPES.iter().filter(move |r| r.machine == kind)
}

/// The recipe of `kind` that makes `output`.
pub fn find(kind: MachineKind, output: Item) -> Option<&'static Recipe> {
    for_machine(kind).find(|r| r.output == output)
}
