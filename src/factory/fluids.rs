//! Fluids and pipes.
//!
//! **Pipes** (`Block::Pipe`) join the ways they were built to (`Pipe::ways`, worked out where
//! one is put down: `Factory::pipe_ways`): the pipe or machine it was put against, what is in
//! front of its open ends, pipes whose open ends point at it. So a line goes where it was built,
//! and lines side by side stay apart. Joined pipes make one network, with the machines they join
//! (any cell of a big one) on it. A network carries one fluid at a time: what it holds is shared
//! over its pipes and the storage tanks on it, as one pool. Each tick the machines on it that
//! want that fluid take what they need (all alike when there isn't enough), then the machines
//! with that fluid in an output tank (or anything, into an empty network) put theirs in, as far
//! as there is room.
//!
//! What a machine holds is in its `Machine::tanks`: two for what it takes, two for what it
//! makes (a storage tank uses only the first).
//!
//! An empty network takes only a fluid that a machine on it wants (or anything, if a storage
//! tank is on it), so an electrolyzer's oxygen goes down the line to what takes oxygen and its
//! hydrogen down another. A line keeps to the fluid it carries (its pipes show it) while it
//! holds some or something on it uses it; then it is free for any again. Pipes of different
//! fluids never join, even built to, and a line joins only machines that use its fluid.

use serde::{Deserialize, Serialize};

use crate::{world::chunk::BlockCell, world::textures::Tex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Fluid {
    Water,
    Steam,
    SulfuricAcid,
    NitricAcid,
    HydrofluoricAcid,
    Hydrogen,
    Oxygen,
    /// Uranium hexafluoride, the gas uranium is enriched as.
    Hexafluoride,
    EnrichedHexafluoride,
    /// Spent fuel dissolved in nitric acid.
    DissolvedFuel,
    /// Solvent carrying the uranium and plutonium out of dissolved fuel.
    LoadedSolvent,
    /// What is left of spent fuel: the fission products, for vitrifying.
    LiquidWaste,
    /// A kerosene-like solvent from the refinery.
    Solvent,
}

impl Fluid {
    pub const ALL: [Fluid; 13] = [
        Fluid::Water,
        Fluid::Steam,
        Fluid::SulfuricAcid,
        Fluid::NitricAcid,
        Fluid::HydrofluoricAcid,
        Fluid::Hydrogen,
        Fluid::Oxygen,
        Fluid::Hexafluoride,
        Fluid::EnrichedHexafluoride,
        Fluid::DissolvedFuel,
        Fluid::LoadedSolvent,
        Fluid::LiquidWaste,
        Fluid::Solvent,
    ];

    /// Lowercase, for screens.
    pub fn name(self) -> &'static str {
        match self {
            Fluid::Water => "water",
            Fluid::Steam => "steam",
            Fluid::SulfuricAcid => "sulfuric acid",
            Fluid::NitricAcid => "nitric acid",
            Fluid::HydrofluoricAcid => "hydrofluoric acid",
            Fluid::Hydrogen => "hydrogen",
            Fluid::Oxygen => "oxygen",
            Fluid::Hexafluoride => "uranium hexafluoride",
            Fluid::EnrichedHexafluoride => "enriched hexafluoride",
            Fluid::DissolvedFuel => "dissolved fuel",
            Fluid::LoadedSolvent => "loaded solvent",
            Fluid::LiquidWaste => "liquid waste",
            Fluid::Solvent => "solvent",
        }
    }

    /// `snake_case` (the console).
    pub fn id(self) -> String {
        self.name().replace(' ', "_")
    }

    pub fn from_id(id: &str) -> Option<Fluid> {
        let id = id.to_lowercase();
        Fluid::ALL.into_iter().find(|f| f.id() == id)
    }

    /// Its colour (sRGB): its icon, the gauges on its tanks.
    pub fn color(self) -> [u8; 3] {
        match self {
            Fluid::Water => [60, 130, 230],
            Fluid::Steam => [226, 232, 240],
            Fluid::SulfuricAcid => [236, 218, 90],
            Fluid::NitricAcid => [240, 150, 60],
            Fluid::HydrofluoricAcid => [170, 240, 210],
            Fluid::Hydrogen => [236, 90, 90],
            Fluid::Oxygen => [110, 170, 255],
            Fluid::Hexafluoride => [214, 222, 186],
            Fluid::EnrichedHexafluoride => [150, 240, 100],
            Fluid::DissolvedFuel => [130, 200, 70],
            Fluid::LoadedSolvent => [210, 160, 70],
            Fluid::LiquidWaste => [70, 220, 110],
            Fluid::Solvent => [232, 226, 196],
        }
    }

    /// Kept in cylinders (its icon), not flasks.
    pub fn is_gas(self) -> bool {
        matches!(
            self,
            Fluid::Steam
                | Fluid::Hydrogen
                | Fluid::Oxygen
                | Fluid::Hexafluoride
                | Fluid::EnrichedHexafluoride
        )
    }

    pub fn tex(self) -> Tex {
        Tex::ALL[Tex::FluidWater as usize + self as usize]
    }
}

/// Some of a fluid (litres).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidStack {
    pub fluid: Fluid,
    pub amount: f32,
}

/// What a machine's tank holds.
pub type Tank = Option<FluidStack>;

/// Litres a pipe holds, and a storage tank.
pub const PIPE_HOLDS: f32 = 100.0;
pub const TANK_HOLDS: f32 = 20_000.0;

/// Below this a tank or network counts as empty (so a network can carry something else).
pub const EMPTY: f32 = 0.01;

/// Puts up to `amount` of `fluid` into `tank` (holding at most `room`); returns how much went in.
pub fn fill(tank: &mut Tank, fluid: Fluid, amount: f32, room: f32) -> f32 {
    match tank {
        Some(s) if s.fluid == fluid => {
            let put = amount.min(room - s.amount).max(0.0);
            s.amount += put;
            put
        }
        Some(s) if s.amount > EMPTY => 0.0,
        _ => {
            let put = amount.min(room).max(0.0);
            *tank = (put > 0.0).then_some(FluidStack { fluid, amount: put });
            put
        }
    }
}

/// Takes up to `amount` of `fluid` out of `tank`; returns how much came out.
pub fn drain(tank: &mut Tank, fluid: Fluid, amount: f32) -> f32 {
    let Some(s) = tank else { return 0.0 };
    if s.fluid != fluid {
        return 0.0;
    }
    let took = amount.min(s.amount).max(0.0);
    s.amount -= took;
    if s.amount <= EMPTY {
        *tank = None;
    }
    took
}

/// How much of `fluid` is in `tank`.
pub fn level(tank: &Tank, fluid: Fluid) -> f32 {
    tank.filter(|s| s.fluid == fluid).map_or(0.0, |s| s.amount)
}

/// A pipe, and what it holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pipe {
    pub cell: BlockCell,
    pub held: Tank,
    /// The ways it was built to join (bits as `links`): a way kept after what was there went
    /// is an open socket, and what is put there joins it. None in a save from before (it joins
    /// all it touches, worked out once).
    #[serde(default)]
    pub ways: Option<u8>,
    /// Which way it runs while it joins nothing: along u (0), v (1) or up (2).
    #[serde(default)]
    pub axis: u8,
    /// The cells next to it: +u, -u, +v, -v, up, down (worked out once it is placed in
    /// space: `Factory::locate`).
    #[serde(skip)]
    pub around: Option<[BlockCell; 6]>,
    /// Which of those it joins (bits in that order: `ModelState::links`, its model).
    #[serde(skip)]
    pub links: u8,
}

impl Pipe {
    /// The fluid its line carries (kept while empty), if it has carried any yet.
    pub fn carries(&self) -> Option<Fluid> {
        self.held.map(|s| s.fluid)
    }

    /// Its open ends (bits as `ways`): along `axis` while it joins nothing, straight on from
    /// its one way, none past that. What is put in front of an open end joins it.
    pub fn open_ends(&self) -> u8 {
        let ways = self.ways.unwrap_or(0b11_1111);
        match ways.count_ones() {
            0 => 0b11 << (2 * self.axis.min(2)),
            1 => 1 << (ways.trailing_zeros() ^ 1),
            _ => 0,
        }
    }
}

/// Pipes joined together, and the machines on them.
#[derive(Clone, Debug, Default)]
pub struct FluidNet {
    pub pipes: Vec<u32>,
    pub machines: Vec<u32>,
}
