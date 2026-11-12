//! What machines make, and from what. A recipe machine (refinery, arc furnace, assembler, chip
//! fab) makes the one picked on its screen; each of its input slots holds one ingredient.
//!
//! The production chains:
//! - **Oil:** an oil pump over oil shale brings up crude oil. A refinery makes fuel (burns five
//!   times as long as coal), plastic or rubber from it.
//! - **Metal and silicon:** an arc furnace makes steel from iron and coal, and silicon from sand
//!   and coal. A chip fab cuts silicon into wafers.
//! - **Chips:** wafers and copper wire make LEDs, memory and (with gold) CPUs; AI chips also take
//!   memory and a diamond.
//! - **Parts:** an assembler draws copper into wire, insulates it with rubber, lays boards out of
//!   plastic and wire, builds batteries, screens (glass, LEDs, a board, plastic), motherboards,
//!   power supplies and cases.
//! - **Computers:** case + motherboard + power supply + screen.

use super::MachineKind;
use crate::{survival::items::Item, world::block::Block};

pub struct Recipe {
    pub machine: MachineKind,
    /// Up to four ingredients, each with how many it takes.
    pub inputs: &'static [(Item, u8)],
    pub output: Item,
    pub count: u8,
    /// Seconds per batch at full power.
    pub time: f32,
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
        output,
        count,
        time,
    }
}

use Item::*;
use MachineKind::{ArcFurnace as Arc, Assembler as Asm, ChipFab as Fab, Refinery as Ref};

pub static RECIPES: [Recipe; 21] = [
    // Refinery.
    r(Ref, &[(CrudeOil, 2)], Fuel, 1, 4.0),
    r(Ref, &[(CrudeOil, 1)], Plastic, 2, 3.0),
    r(Ref, &[(CrudeOil, 1)], Rubber, 1, 3.0),
    // Arc furnace.
    r(Arc, &[(IronIngot, 1), (Coal, 1)], SteelIngot, 1, 4.0),
    r(Arc, &[(Block(Block::Sand), 2), (Coal, 1)], Silicon, 1, 4.0),
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
];

/// What `kind` can make, in the order its screen shows them.
pub fn for_machine(kind: MachineKind) -> impl Iterator<Item = &'static Recipe> {
    RECIPES.iter().filter(move |r| r.machine == kind)
}

/// The recipe of `kind` that makes `output`.
pub fn find(kind: MachineKind, output: Item) -> Option<&'static Recipe> {
    for_machine(kind).find(|r| r.output == output)
}
