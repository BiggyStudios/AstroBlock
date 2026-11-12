//! Vehicles built from blocks: planes, rockets, rovers.
//!
//! A vehicle is a `Vessel` with a `Craft`: a grid of parts of its own, which moves and turns as
//! one rigid body. It starts from a vehicle core and grows from there. Everything about how
//! it flies comes from what it is made of (`Stats`):
//! - Mass, centre of mass and moments of inertia add up from the parts. Fuel weighs, so the
//!   centre of mass moves as the tanks empty.
//! - Each engine pushes where it sits, in the direction it points. An engine that isn't in
//!   line with the centre of mass turns the vehicle.
//! - Each wing lifts where it sits, so a wing behind the centre of mass stabilises and one
//!   in front doesn't. The body drags with the area it shows to the airflow.
//!
//! # Axes
//! Grid coordinates are in meters from the core; a part fills the cube of side 1 around its
//! integer position. +Y was "up" where the core was placed, and walking aboard is always
//! towards -Y. `Stats::forward` is where the engines push (without engines: the way the core
//! faces).
//!
//! The physics is in `craft_physics`, the mesh in `craft_model`.

use glam::{DMat3, DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::{
    util::FastMap,
    world::block::{Block, DoorKind},
};

/// A position in a vehicle's grid.
pub type At = [i16; 3];

/// Most parts in one vehicle, and how far from the core one may be: its mesh stores
/// positions as 16 bits (+-64 m).
pub const MAX_PARTS: usize = 6_000;
pub const MAX_REACH: i16 = 60;

/// Fuel in a tank (kg).
pub const TANK_FUEL: f64 = 100.0;
/// The speed a propeller has no thrust left at (m/s).
pub const PROPELLER_TOP: f64 = 400.0;
/// Wheels stick this far out of their cell, and the spring gives this much at rest (m).
pub const WHEEL_RADIUS: f64 = 0.7;
pub const SPRING_REST: f64 = 0.12;

// Electricity (see `Stats::battery` and `craft_physics`).
/// What the core's own battery holds, and an accumulator on board (J).
pub const CORE_BATTERY: f64 = 1_000_000.0;
pub const ACCUMULATOR_BATTERY: f64 = 5_000_000.0;
/// A solar panel square on to the sun at the home planet's distance (W).
pub const SOLAR_POWER: f64 = 20_000.0;
/// A coal generator on board, burning what is in the tanks: what it makes (W), and the energy
/// in a kilogram of that fuel (J).
pub const GENERATOR_POWER: f64 = 180_000.0;
pub const FUEL_ENERGY: f64 = 2_000_000.0;
/// A drill while it bores (W), and what opening a big door costs (J).
pub const DRILL_POWER: f64 = 12_000.0;
pub const BIG_DOOR_ENERGY: f64 = 40_000.0;
/// An RTG on board: what it makes, day and night, for ever (W).
pub const RTG_POWER: f64 = 10_000.0;

/// `Part::facing` of a door: bits 0-2 the way its front looks (as for any part), bits 3-6 which
/// cell of the door it is (across + 3 x up), bit 7 open.
pub const DOOR_PIECE_SHIFT: u8 = 3;
pub const DOOR_OPEN: u8 = 0x80;

/// The radius of a wheel part (m), if it is one.
pub fn wheel_radius(block: Block) -> Option<f64> {
    match block {
        Block::Wheel | Block::LandingGear => Some(WHEEL_RADIUS),
        Block::LargeWheel => Some(1.1),
        Block::GiantWheel => Some(1.6),
        _ => None,
    }
}

/// Blocks that are models in the world and can go on a vehicle too: power, and doors.
pub fn is_vehicle_model(block: Block) -> bool {
    matches!(
        block,
        Block::Accumulator
            | Block::CoalGenerator
            | Block::SolarPanel
            | Block::Rtg
            | Block::Door
            | Block::SlidingDoor
            | Block::BigDoor
    )
}

/// Which cell of its door a door part is: (across, up).
pub fn door_piece(facing: u8) -> (i16, i16) {
    let i = ((facing >> DOOR_PIECE_SHIFT) & 0xf) as i16;
    (i % 3, i / 3)
}

/// The way across a door whose front looks along `front` (grid axes, up is +Y): its cells go
/// from the anchor that way, then up.
pub fn door_across(front: u8) -> At {
    let f = direction(front);
    let a = DVec3::Y.cross(f);
    [a.x.round() as i16, 0, a.z.round() as i16]
}

/// The cells of a door of `kind` with its anchor (bottom left, seen from the front) at
/// `anchor`, front looking along `front`: each with the facing its part has.
pub fn door_cells(kind: DoorKind, anchor: At, front: u8) -> Vec<(At, u8)> {
    let (w, h) = kind.size();
    let across = door_across(front);
    let mut cells = Vec::new();
    for row in 0..h as i16 {
        for col in 0..w as i16 {
            let at = [
                anchor[0] + across[0] * col,
                anchor[1] + row,
                anchor[2] + across[2] * col,
            ];
            let piece = (col + 3 * row) as u8;
            cells.push((at, (front & 7) | piece << DOOR_PIECE_SHIFT));
        }
    }
    cells
}

/// A door on a vehicle: where its anchor cell is, what kind, which way it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CraftDoor {
    pub anchor: At,
    pub kind: DoorKind,
    pub front: u8,
}

/// A drill on a vehicle: where, and the way it bores (a direction index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drill {
    pub at: At,
    pub facing: u8,
}

/// The six directions a part can face.
pub const DIRECTIONS: [DVec3; 6] = [
    DVec3::X,
    DVec3::NEG_X,
    DVec3::Y,
    DVec3::NEG_Y,
    DVec3::Z,
    DVec3::NEG_Z,
];

/// The direction index closest to `v`.
pub fn facing_of(v: DVec3) -> u8 {
    (0..6)
        .max_by(|&a, &b| DIRECTIONS[a].dot(v).total_cmp(&DIRECTIONS[b].dot(v)))
        .unwrap_or(0) as u8
}

/// (Only the low bits say the way: a door keeps more in the rest, see `DOOR_PIECE_SHIFT`.)
pub fn direction(facing: u8) -> DVec3 {
    DIRECTIONS[((facing & 7) as usize).min(5)]
}

pub fn center(at: At) -> DVec3 {
    DVec3::new(at[0] as f64, at[1] as f64, at[2] as f64)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub at: At,
    pub block: Block,
    /// Engines: the way the exhaust leaves. Wings: the way they lift. Seats: the way you
    /// look. The core: forward.
    pub facing: u8,
}

/// What an engine does.
#[derive(Clone, Copy, Debug)]
pub struct EngineKind {
    /// Thrust in the thick of the air (or in vacuum, for rockets) (N).
    pub thrust: f64,
    /// Fuel: thrust / this (kg/s). Infinite for an engine that runs on electricity.
    pub exhaust: f64,
    /// What it does in air of a given relative density, 0..1 at a given speed.
    pub air: AirUse,
    /// Electricity at full throttle (W): it burns no fuel.
    pub power: f64,
    /// How far its nozzle swings to steer (rad); 0 for a fixed one.
    pub gimbal: f64,
}

impl EngineKind {
    pub fn electric(&self) -> bool {
        self.power > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AirUse {
    /// Bites less the faster it goes; stops at this speed (m/s).
    Propeller(f64),
    /// Needs air, gets somewhat stronger with speed.
    Breathes,
    /// Doesn't care.
    Rocket,
    /// A vacuum nozzle: only this share of the thrust at sea level.
    Vacuum(f64),
}

pub fn engine_kind(block: Block) -> Option<EngineKind> {
    let kind = |thrust, exhaust, air| {
        Some(EngineKind {
            thrust,
            exhaust,
            air,
            power: 0.0,
            gimbal: 0.0,
        })
    };
    match block {
        Block::Propeller => kind(26_000.0, 65_000.0, AirUse::Propeller(PROPELLER_TOP)),
        Block::JetEngine => kind(26_000.0, 18_000.0, AirUse::Breathes),
        Block::RocketEngine => kind(30_000.0, 4_000.0, AirUse::Rocket),
        Block::OrbitalEngine => kind(7_000.0, 7_000.0, AirUse::Vacuum(0.3)),
        Block::VectorEngine => Some(EngineKind {
            gimbal: 0.12,
            ..kind(26_000.0, 4_000.0, AirUse::Rocket)?
        }),
        // (As good at sea level as in vacuum, and further on its fuel than a rocket engine.)
        Block::Aerospike => kind(22_000.0, 5_400.0, AirUse::Rocket),
        // 600 N for 300 kg: it can't even lift itself, but in space it pushes for as long as
        // there is power.
        Block::IonEngine => Some(EngineKind {
            power: 40_000.0,
            ..kind(600.0, f64::INFINITY, AirUse::Vacuum(0.0))?
        }),
        // Antimatter heats what it throws out: a rocket thirty times as frugal, at a price.
        Block::AntimatterDrive => kind(40_000.0, 120_000.0, AirUse::Rocket),
        _ => None,
    }
}

impl EngineKind {
    /// Thrust at full throttle (N). `air` is the density relative to sea level, `speed`
    /// the speed through the air.
    pub fn thrust_at(&self, air: f64, speed: f64) -> f64 {
        match self.air {
            AirUse::Propeller(top) => self.thrust * air * (1.0 - speed / top).clamp(0.0, 1.0),
            AirUse::Breathes => self.thrust * air.powf(0.7) * (1.0 + (speed / 450.0).min(1.0)),
            AirUse::Rocket => self.thrust,
            AirUse::Vacuum(share) => self.thrust * (1.0 - (1.0 - share) * air.min(1.0)),
        }
    }
}

/// Mass of a part without fuel (kg).
pub fn part_mass(block: Block) -> f64 {
    use Block::*;
    match block {
        VehicleCore => 150.0,
        Hull => 40.0,
        Seat => 30.0,
        FuelTank => 50.0,
        Propeller => 200.0,
        JetEngine => 450.0,
        RocketEngine => 500.0,
        OrbitalEngine => 180.0,
        Wing => 30.0,
        Flap => 40.0,
        Gyro => 120.0,
        Wheel => 80.0,
        LandingGear => 90.0,
        LargeWheel => 160.0,
        GiantWheel => 340.0,
        VectorEngine => 560.0,
        Aerospike => 420.0,
        IonEngine => 300.0,
        AntimatterDrive => 1_500.0,
        Rtg => 250.0,
        VehicleDrill => 400.0,
        SolarPanel => 50.0,
        Accumulator => 300.0,
        CoalGenerator => 400.0,
        Door | DoorOpen => 50.0,
        SlidingDoor | SlidingDoorOpen => 60.0,
        BigDoor | BigDoorOpen => 90.0,
        Glass => 40.0,
        WhiteWool => 20.0,
        OakLeaves | SpruceLeaves | JungleLeaves => 15.0,
        OakLog | SpruceLog | OakPlanks | SprucePlanks | CraftingTable | Chest => 60.0,
        Snow | Ice => 100.0,
        Dirt | Grass | SnowyGrass | Sand | RedSand | IlmeniteSand | Regolith | Ash => 400.0,
        Gravel => 500.0,
        _ => 800.0,
    }
}

/// Whether someone walking aboard bumps into it.
pub fn is_walkable_solid(block: Block) -> bool {
    !matches!(block, Block::Air | Block::Water | Block::Seat)
}

/// Whether it counts as a cube hiding the faces next to it.
pub fn is_cube(block: Block) -> bool {
    use Block::*;
    !matches!(
        block,
        Air | Water
            | Glass
            | Seat
            | Propeller
            | JetEngine
            | RocketEngine
            | OrbitalEngine
            | Wing
            | Flap
            | Wheel
            | LandingGear
            | LargeWheel
            | GiantWheel
            | VectorEngine
            | Aerospike
            | IonEngine
            | AntimatterDrive
            | VehicleDrill
    ) && !is_vehicle_model(block)
}

/// Whether a part points somewhere (and `R` can turn it).
pub fn can_turn(block: Block) -> bool {
    use Block::*;
    matches!(
        block,
        Propeller
            | JetEngine
            | RocketEngine
            | OrbitalEngine
            | VectorEngine
            | Aerospike
            | IonEngine
            | AntimatterDrive
            | VehicleDrill
            | SolarPanel
            | Wing
            | Flap
            | Seat
    )
}

/// The way a new part faces when it is put on the side of something that looks `outward`.
pub fn auto_facing(block: Block, outward: DVec3, stats: &Stats) -> u8 {
    let (forward, up) = (stats.forward, stats.up);
    match block {
        // Facing is where the exhaust goes: a propeller at either end pulls forwards.
        Block::Propeller if outward.dot(forward).abs() > 0.5 => facing_of(-forward),
        Block::Propeller
        | Block::JetEngine
        | Block::RocketEngine
        | Block::OrbitalEngine
        | Block::VectorEngine
        | Block::Aerospike
        | Block::IonEngine
        | Block::AntimatterDrive => facing_of(outward),
        // A drill bores away from what it is put on.
        Block::VehicleDrill => facing_of(outward),
        // Panels face the sky; machines and doors face out (or forward, put on a floor).
        Block::SolarPanel => facing_of(up),
        Block::Accumulator
        | Block::CoalGenerator
        | Block::Rtg
        | Block::Door
        | Block::SlidingDoor
        | Block::BigDoor => {
            let flat = DVec3::new(outward.x, 0.0, outward.z);
            if flat.length() > 0.5 {
                facing_of(flat)
            } else if forward.y.abs() < 0.5 {
                facing_of(-forward)
            } else {
                facing_of(DVec3::NEG_Z)
            }
        }
        Block::Wing | Block::Flap => {
            // Flat along the way it flies, so it lifts at right angles to its span.
            let normal = outward.cross(forward);
            if normal.length() < 0.5 {
                facing_of(up)
            } else if normal.dot(up) < 0.0 {
                facing_of(-normal)
            } else {
                facing_of(normal)
            }
        }
        Block::Seat => facing_of(if forward.y.abs() > 0.5 { up } else { forward }),
        _ => 0,
    }
}

/// An engine on a vehicle.
#[derive(Clone, Copy, Debug)]
pub struct Engine {
    /// Grid coordinates.
    pub position: DVec3,
    /// The way it pushes the vehicle (unit).
    pub push: DVec3,
    pub kind: EngineKind,
}

/// A wing or a flap.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub position: DVec3,
    /// Unit, perpendicular to the surface.
    pub normal: DVec3,
    /// A flap: moves with the controls.
    pub moves: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct WheelMount {
    pub position: DVec3,
    pub driven: bool,
    pub radius: f64,
}

/// The area a vehicle shows along one axis (m^2) and that area's centre.
#[derive(Clone, Copy, Debug, Default)]
pub struct Silhouette {
    pub area: f64,
    pub center: DVec3,
}

/// Everything that follows from a vehicle's parts.
#[derive(Clone, Debug)]
pub struct Stats {
    /// Without fuel (kg).
    pub dry_mass: f64,
    dry_moment: DVec3,
    /// About the grid's origin.
    dry_inertia: DMat3,
    /// Fuel when full (kg).
    pub capacity: f64,
    tank_moment: DVec3,
    tank_inertia: DMat3,
    pub engines: Vec<Engine>,
    pub surfaces: Vec<Surface>,
    pub wheels: Vec<WheelMount>,
    /// Outer corners of the hull, which meet the ground.
    pub corners: Vec<DVec3>,
    /// Along x, y and z.
    pub silhouette: [Silhouette; 3],
    /// Reaction wheels (N m).
    pub gyro_torque: f64,
    /// The vehicle's axes in grid coordinates: where it goes, and its pilot's up and right.
    pub forward: DVec3,
    pub up: DVec3,
    pub right: DVec3,
    /// Where you can sit.
    pub seats: Vec<At>,
    pub has_core: bool,
    /// Furthest part from the origin (m).
    pub radius: f64,
    /// What its batteries hold when full (J): the core's and the accumulators'.
    pub battery: f64,
    /// Each solar panel's face (grid axes, unit).
    pub solar: Vec<DVec3>,
    /// Coal generators aboard (they burn the tanks' fuel).
    pub generators: usize,
    /// RTGs aboard.
    pub rtgs: usize,
    pub drills: Vec<Drill>,
    pub doors: Vec<CraftDoor>,
    /// Something on it runs on electricity: driven wheels, ion engines, drills, big doors.
    pub electric: bool,
}

/// How heavy a vehicle is right now and how that is spread out.
#[derive(Clone, Copy, Debug)]
pub struct Massing {
    pub mass: f64,
    /// Centre of mass (grid coordinates).
    pub com: DVec3,
    /// About the centre of mass, and its inverse (grid axes).
    pub inertia: DMat3,
    pub inverse: DMat3,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            dry_mass: 0.0,
            dry_moment: DVec3::ZERO,
            dry_inertia: DMat3::ZERO,
            capacity: 0.0,
            tank_moment: DVec3::ZERO,
            tank_inertia: DMat3::ZERO,
            engines: Vec::new(),
            surfaces: Vec::new(),
            wheels: Vec::new(),
            corners: Vec::new(),
            silhouette: [Silhouette::default(); 3],
            gyro_torque: 0.0,
            forward: DVec3::Z,
            up: DVec3::Y,
            right: DVec3::NEG_X,
            seats: Vec::new(),
            has_core: false,
            radius: 1.0,
            battery: 0.0,
            solar: Vec::new(),
            generators: 0,
            rtgs: 0,
            drills: Vec::new(),
            doors: Vec::new(),
            electric: false,
        }
    }
}

/// How a point mass at `r` adds to an inertia tensor, with the spread of a unit cube.
fn inertia_of(mass: f64, r: DVec3) -> DMat3 {
    let spread = mass / 6.0;
    let s = r.length_squared();
    DMat3::from_cols(
        DVec3::new(s - r.x * r.x, -r.x * r.y, -r.x * r.z) * mass + DVec3::X * spread,
        DVec3::new(-r.y * r.x, s - r.y * r.y, -r.y * r.z) * mass + DVec3::Y * spread,
        DVec3::new(-r.z * r.x, -r.z * r.y, s - r.z * r.z) * mass + DVec3::Z * spread,
    )
}

/// One part's blocks, as saved and sent. (The rest is worked out again from these.)
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "Vec<Part>", into = "Vec<Part>")]
pub struct Craft {
    parts: Vec<Part>,
    index: FastMap<At, usize>,
    pub stats: Stats,
    /// Changes whenever a part does (the mesh is rebuilt).
    pub version: u32,
}

impl From<Vec<Part>> for Craft {
    fn from(parts: Vec<Part>) -> Self {
        let mut craft = Self {
            parts,
            index: FastMap::default(),
            stats: Stats::default(),
            version: 1,
        };
        craft.rebuild();
        craft
    }
}

impl From<Craft> for Vec<Part> {
    fn from(craft: Craft) -> Self {
        craft.parts
    }
}

/// What breaking something off left behind.
#[derive(Debug, Default, PartialEq)]
pub struct Removed {
    /// What came off: the part itself, and anything no longer joined to the core.
    pub parts: Vec<Part>,
    /// That was the core: nothing is left.
    pub core_gone: bool,
}

impl Craft {
    /// A new vehicle: just the core, facing `facing`.
    pub fn new(facing: u8) -> Self {
        vec![Part {
            at: [0, 0, 0],
            block: Block::VehicleCore,
            facing,
        }]
        .into()
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn get(&self, at: At) -> Option<Part> {
        self.index.get(&at).map(|&i| self.parts[i])
    }

    /// The part a hull corner (`Stats::corners`) belongs to.
    pub fn part_at_corner(&self, corner: DVec3) -> Option<At> {
        for sx in [-0.5, 0.5] {
            for sy in [-0.5, 0.5] {
                for sz in [-0.5, 0.5] {
                    let c = corner + DVec3::new(sx, sy, sz);
                    let at = [c.x.round() as i16, c.y.round() as i16, c.z.round() as i16];
                    if self.index.contains_key(&at) {
                        return Some(at);
                    }
                }
            }
        }
        None
    }

    /// Whether someone walking aboard is stopped by what is in cell `at` (not an open door).
    pub fn solid(&self, at: At) -> bool {
        self.get(at).is_some_and(|p| {
            is_walkable_solid(p.block) && !(p.block.door().is_some() && p.facing & DOOR_OPEN != 0)
        })
    }

    /// The door the part at `at` is a cell of: its index in `Stats::doors`, and the door.
    pub fn door_at(&self, at: At) -> Option<(usize, CraftDoor)> {
        let part = self.get(at)?;
        part.block.door()?;
        let (col, row) = door_piece(part.facing);
        let across = door_across(part.facing);
        let anchor = [
            at[0] - across[0] * col,
            at[1] - row,
            at[2] - across[2] * col,
        ];
        let n = self.stats.doors.iter().position(|d| d.anchor == anchor)?;
        Some((n, self.stats.doors[n]))
    }

    /// Whether door `n` (of `Stats::doors`) stands open.
    pub fn door_open(&self, n: usize) -> bool {
        self.stats
            .doors
            .get(n)
            .and_then(|d| self.get(d.anchor))
            .is_some_and(|p| p.facing & DOOR_OPEN != 0)
    }

    /// Opens or closes the door that has a cell at `at` (all of its cells). Nothing else about
    /// the vehicle changes, so its mesh stays (the door's leaves move on their own).
    pub fn set_door(&mut self, at: At, open: bool) -> bool {
        let Some((_, door)) = self.door_at(at) else {
            return false;
        };
        for (cell, _) in door_cells(door.kind, door.anchor, door.front) {
            if let Some(&i) = self.index.get(&cell)
                && self.parts[i].block.door().is_some()
            {
                let f = &mut self.parts[i].facing;
                *f = if open {
                    *f | DOOR_OPEN
                } else {
                    *f & !DOOR_OPEN
                };
            }
        }
        true
    }

    /// Adds several parts at once (a door's cells): all of them, or none if any can't go
    /// where it would (taken, too far, too many) or none of them touches the vehicle.
    pub fn place_all(&mut self, parts: &[Part]) -> bool {
        let free = parts.iter().all(|p| {
            p.at.iter().all(|c| c.abs() <= MAX_REACH)
                && !self.index.contains_key(&p.at)
                && p.block != Block::VehicleCore
        });
        if !free
            || self.parts.len() + parts.len() > MAX_PARTS
            || !parts.iter().any(|p| self.touches(p.at))
        {
            return false;
        }
        for p in parts {
            self.index.insert(p.at, self.parts.len());
            self.parts.push(*p);
        }
        self.rebuild();
        true
    }

    /// Whether a part can go at `at`.
    pub fn can_place(&self, at: At) -> bool {
        at.iter().all(|c| c.abs() <= MAX_REACH)
            && self.parts.len() < MAX_PARTS
            && !self.index.contains_key(&at)
            && self.touches(at)
    }

    fn touches(&self, at: At) -> bool {
        NEIGHBOURS
            .iter()
            .any(|d| self.index.contains_key(&offset(at, *d)))
    }

    /// Adds a part (false if it can't go there, or is a second core).
    pub fn place(&mut self, part: Part) -> bool {
        if !self.can_place(part.at) || part.block == Block::VehicleCore {
            return false;
        }
        self.index.insert(part.at, self.parts.len());
        self.parts.push(part);
        self.rebuild();
        true
    }

    /// Takes the part at `at` off, and whatever that leaves unconnected to the core.
    pub fn remove(&mut self, at: At) -> Removed {
        if !self.index.contains_key(&at) {
            return Removed::default();
        }
        if self.get(at).is_some_and(|p| p.block == Block::VehicleCore) {
            let parts = std::mem::take(&mut self.parts);
            self.rebuild();
            return Removed {
                parts,
                core_gone: true,
            };
        }
        let mut gone: Vec<Part> = Vec::new();
        self.parts.retain(|p| {
            let keep = p.at != at;
            if !keep {
                gone.push(*p);
            }
            keep
        });
        self.reindex();
        // Whatever can't be reached from the core any more has come off too.
        let mut seen: FastMap<At, ()> = FastMap::default();
        let mut stack = vec![[0i16; 3]];
        seen.insert([0; 3], ());
        while let Some(c) = stack.pop() {
            for d in NEIGHBOURS {
                let n = offset(c, d);
                if self.index.contains_key(&n) && seen.insert(n, ()).is_none() {
                    stack.push(n);
                }
            }
        }
        self.parts.retain(|p| {
            let keep = seen.contains_key(&p.at);
            if !keep {
                gone.push(*p);
            }
            keep
        });
        self.rebuild();
        Removed {
            parts: gone,
            core_gone: false,
        }
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, p) in self.parts.iter().enumerate() {
            self.index.insert(p.at, i);
        }
    }

    /// Works out everything that follows from the parts.
    fn rebuild(&mut self) {
        self.reindex();
        self.version = self.version.wrapping_add(1).max(1);
        let mut s = Stats::default();
        let mut core_facing = 4u8;
        for part in &self.parts {
            let r = center(part.at);
            let mass = part_mass(part.block);
            s.dry_mass += mass;
            s.dry_moment += r * mass;
            s.dry_inertia += inertia_of(mass, r);
            s.radius = s.radius.max(r.length() + 1.0);
            if let Some(radius) = wheel_radius(part.block) {
                let driven = part.block != Block::LandingGear;
                s.electric |= driven;
                s.wheels.push(WheelMount {
                    position: r,
                    driven,
                    radius,
                });
            }
            if let Some((kind, _)) = part.block.door() {
                let (col, row) = door_piece(part.facing);
                let across = door_across(part.facing);
                let anchor = [
                    part.at[0] - across[0] * col,
                    part.at[1] - row,
                    part.at[2] - across[2] * col,
                ];
                if !s.doors.iter().any(|d| d.anchor == anchor) {
                    s.doors.push(CraftDoor {
                        anchor,
                        kind,
                        front: part.facing & 7,
                    });
                    s.electric |= kind == DoorKind::Big;
                }
            }
            match part.block {
                Block::VehicleCore => {
                    s.has_core = true;
                    core_facing = part.facing;
                    s.gyro_torque += 4_000.0;
                    s.battery += CORE_BATTERY;
                }
                Block::Accumulator => s.battery += ACCUMULATOR_BATTERY,
                Block::SolarPanel => s.solar.push(direction(part.facing)),
                Block::CoalGenerator => s.generators += 1,
                Block::Rtg => s.rtgs += 1,
                Block::VehicleDrill => {
                    s.electric = true;
                    s.drills.push(Drill {
                        at: part.at,
                        facing: part.facing,
                    });
                }
                Block::Gyro => s.gyro_torque += 20_000.0,
                Block::FuelTank => {
                    s.capacity += TANK_FUEL;
                    s.tank_moment += r * TANK_FUEL;
                    s.tank_inertia += inertia_of(TANK_FUEL, r);
                }
                Block::Seat => s.seats.push(part.at),
                Block::Wing | Block::Flap => s.surfaces.push(Surface {
                    position: r,
                    normal: direction(part.facing),
                    moves: part.block == Block::Flap,
                }),
                _ => {}
            }
            if let Some(kind) = engine_kind(part.block) {
                s.electric |= kind.electric();
                s.engines.push(Engine {
                    position: r,
                    push: -direction(part.facing),
                    kind,
                });
            }
        }

        // Which way is forward: where the engines push, else where the core points.
        let net: DVec3 = s.engines.iter().map(|e| e.push * e.kind.thrust).sum();
        s.forward = if net.length() > 1.0 {
            direction(facing_of(net))
        } else {
            direction(core_facing)
        };
        s.up = if s.forward.y.abs() > 0.5 {
            let dorsal = direction(core_facing);
            if dorsal.y.abs() > 0.5 {
                DVec3::Z
            } else {
                dorsal
            }
        } else {
            DVec3::Y
        };
        s.right = s.forward.cross(s.up);

        self.silhouettes(&mut s);
        self.find_corners(&mut s);
        self.stats = s;
    }

    /// The area shown along each axis. Wings don't count: they make their own drag.
    fn silhouettes(&self, s: &mut Stats) {
        for axis in 0..3 {
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut columns: FastMap<(i16, i16), (i16, i16)> = FastMap::default();
            for part in &self.parts {
                if matches!(part.block, Block::Wing | Block::Flap) {
                    continue;
                }
                let along = part.at[axis];
                columns
                    .entry((part.at[a], part.at[b]))
                    .and_modify(|(lo, hi)| {
                        *lo = (*lo).min(along);
                        *hi = (*hi).max(along);
                    })
                    .or_insert((along, along));
            }
            let mut sum = DVec3::ZERO;
            for ((x, y), (lo, hi)) in &columns {
                let mut c = DVec3::ZERO;
                c[a] = *x as f64;
                c[b] = *y as f64;
                c[axis] = (*lo as f64 + *hi as f64) * 0.5;
                sum += c;
            }
            let area = columns.len() as f64;
            s.silhouette[axis] = Silhouette {
                area,
                center: if area > 0.0 { sum / area } else { DVec3::ZERO },
            };
        }
    }

    /// Corners of the hull with little around them: what actually touches the ground.
    fn find_corners(&self, s: &mut Stats) {
        const MOST: usize = 320;
        let hull = |at: At| {
            self.get(at)
                .is_some_and(|p| wheel_radius(p.block).is_none())
        };
        // Corners are in half-meter units, so they are integers.
        let mut seen: FastMap<[i32; 3], ()> = FastMap::default();
        for part in &self.parts {
            if !hull(part.at) {
                continue;
            }
            for sx in [-1i32, 1] {
                for sy in [-1i32, 1] {
                    for sz in [-1i32, 1] {
                        let corner = [
                            part.at[0] as i32 * 2 + sx,
                            part.at[1] as i32 * 2 + sy,
                            part.at[2] as i32 * 2 + sz,
                        ];
                        if seen.contains_key(&corner) {
                            continue;
                        }
                        let mut around = 0;
                        for dx in [-1i32, 1] {
                            for dy in [-1i32, 1] {
                                for dz in [-1i32, 1] {
                                    let cell = [
                                        ((corner[0] + dx) / 2) as i16,
                                        ((corner[1] + dy) / 2) as i16,
                                        ((corner[2] + dz) / 2) as i16,
                                    ];
                                    if hull(cell) {
                                        around += 1;
                                    }
                                }
                            }
                        }
                        if around <= 3 {
                            seen.insert(corner, ());
                        }
                    }
                }
            }
        }
        let mut corners: Vec<[i32; 3]> = seen.into_keys().collect();
        corners.sort_unstable();
        let step = (corners.len() / MOST).max(1);
        s.corners = corners
            .iter()
            .step_by(step)
            .map(|c| DVec3::new(c[0] as f64, c[1] as f64, c[2] as f64) * 0.5)
            .collect();
    }

    /// How heavy it is with `fuel` kg aboard, and how that is spread.
    pub fn massing(&self, fuel: f64) -> Massing {
        let s = &self.stats;
        let share = if s.capacity > 0.0 {
            (fuel / s.capacity).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mass = (s.dry_mass + share * s.capacity).max(1.0);
        let com = (s.dry_moment + s.tank_moment * share) / mass;
        let about_origin = s.dry_inertia + s.tank_inertia * share;
        // The parallel axis theorem, back from the origin to the centre of mass.
        let c2 = com.length_squared();
        let shift = DMat3::from_cols(
            DVec3::new(c2 - com.x * com.x, -com.x * com.y, -com.x * com.z),
            DVec3::new(-com.y * com.x, c2 - com.y * com.y, -com.y * com.z),
            DVec3::new(-com.z * com.x, -com.z * com.y, c2 - com.z * com.z),
        ) * mass;
        let inertia = about_origin - shift;
        Massing {
            mass,
            com,
            inertia,
            inverse: inertia.inverse(),
        }
    }

    /// The nearest part along a ray in grid coordinates: where, the face it came in by
    /// (a direction index), and how far along.
    pub fn ray(&self, origin: DVec3, dir: DVec3, reach: f64) -> Option<(At, u8, f64)> {
        // Stepping from cell to cell, where cell i covers i - 0.5 .. i + 0.5.
        let mut cell = (origin + 0.5).floor();
        let step = DVec3::new(dir.x.signum(), dir.y.signum(), dir.z.signum());
        let border = |axis: usize, c: f64| {
            let edge = c + if dir[axis] > 0.0 { 0.5 } else { -0.5 };
            if dir[axis].abs() < 1e-12 {
                f64::INFINITY
            } else {
                (edge - origin[axis]) / dir[axis]
            }
        };
        let mut next = DVec3::new(border(0, cell.x), border(1, cell.y), border(2, cell.z));
        let delta = (DVec3::ONE / dir).abs();
        let mut t = 0.0;
        let mut face = 0u8;
        for _ in 0..512 {
            let at = [cell.x as i16, cell.y as i16, cell.z as i16];
            if cell.abs().max_element() <= (MAX_REACH + 2) as f64 && self.index.contains_key(&at) {
                return Some((at, face, t));
            }
            let axis = if next.x < next.y && next.x < next.z {
                0
            } else if next.y < next.z {
                1
            } else {
                2
            };
            t = next[axis];
            if t > reach {
                return None;
            }
            cell[axis] += step[axis];
            next[axis] += delta[axis];
            // The face entered by: back the way the ray came.
            face = (axis as u8) * 2 + u8::from(step[axis] > 0.0);
        }
        None
    }

    /// Moves a person's box from `from` towards `to` through the grid (per axis, up first:
    /// the player's sweep, in straight coordinates). `from` and `to` are the feet.
    pub fn sweep(&self, from: DVec3, to: DVec3, half: f64, height: f64) -> (DVec3, Bumped) {
        let mut pos = from;
        let mut bumped = Bumped::default();
        let span = |lo: f64, hi: f64| {
            ((lo + 0.5 + SKIN).floor() as i32)..=((hi + 0.5 - SKIN).floor() as i32)
        };
        let lo = |p: DVec3, axis: usize| if axis == 1 { p.y } else { p[axis] - half };
        let hi = |p: DVec3, axis: usize| {
            if axis == 1 {
                p.y + height
            } else {
                p[axis] + half
            }
        };
        for axis in [1usize, 0, 2] {
            let delta = to[axis] - pos[axis];
            if delta == 0.0 {
                continue;
            }
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let blocked_at = |c: i32, p: DVec3| {
                span(lo(p, a), hi(p, a)).any(|i| {
                    span(lo(p, b), hi(p, b)).any(|j| {
                        let mut cell = [0i32; 3];
                        cell[axis] = c;
                        cell[a] = i;
                        cell[b] = j;
                        self.solid([cell[0] as i16, cell[1] as i16, cell[2] as i16])
                    })
                })
            };
            let mut reached = pos[axis] + delta;
            let mut hit = false;
            if delta > 0.0 {
                let edge = hi(pos, axis);
                let first = (edge + 0.5 - SKIN).ceil() as i32;
                let last = (edge + delta + 0.5).ceil() as i32 - 1;
                if let Some(c) = (first..=last).find(|&c| blocked_at(c, pos)) {
                    let stop = c as f64 - 0.5 - (hi(pos, axis) - pos[axis]);
                    if reached >= stop {
                        reached = stop;
                        hit = true;
                    }
                }
            } else {
                let edge = lo(pos, axis);
                let first = (edge + delta + 0.5).floor() as i32;
                let last = (edge + 0.5 + SKIN).floor() as i32 - 1;
                if let Some(c) = (first..=last).rev().find(|&c| blocked_at(c, pos)) {
                    let stop = c as f64 + 0.5 + (pos[axis] - lo(pos, axis));
                    if reached <= stop {
                        reached = stop;
                        hit = true;
                    }
                }
            }
            pos[axis] = reached;
            if hit {
                bumped.axes[axis] = true;
                if axis == 1 && delta < 0.0 {
                    bumped.floor = true;
                }
            }
        }
        (pos, bumped)
    }

    /// Whether anything solid is under a person's feet at `feet`, however far down.
    pub fn over_something(&self, feet: DVec3, half: f64) -> bool {
        // (The same columns `sweep` stands on.)
        let span = |c: f64| {
            ((c - half + 0.5 + SKIN).floor() as i32)..=((c + half + 0.5 - SKIN).floor() as i32)
        };
        let top = (feet.y + 0.5).floor() as i32;
        span(feet.x).any(|x| {
            span(feet.z).any(|z| {
                (-(MAX_REACH as i32)..=top).any(|y| self.solid([x as i16, y as i16, z as i16]))
            })
        })
    }

    /// The vehicle's own "forward" as a rotation from grid axes (for placing it level).
    pub fn level_rotation(east: DVec3, up: DVec3, north: DVec3) -> DQuat {
        DQuat::from_mat3(&DMat3::from_cols(east, up, north))
    }
}

/// Overlaps thinner than this (m) don't count as being in a cell (rounding, as the player's own
/// collision).
const SKIN: f64 = 1e-3;

/// What a sweep ran into.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bumped {
    pub axes: [bool; 3],
    pub floor: bool,
}

const NEIGHBOURS: [[i16; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

fn offset(a: At, d: [i16; 3]) -> At {
    [a[0] + d[0], a[1] + d[1], a[2] + d[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Craft {
        let mut c = Craft::new(4);
        for z in 1..=3 {
            assert!(c.place(Part {
                at: [0, 0, z],
                block: Block::Hull,
                facing: 0,
            }));
        }
        c
    }

    #[test]
    fn parts_must_touch_and_cores_are_unique() {
        let mut c = Craft::new(4);
        let far = Part {
            at: [5, 0, 0],
            block: Block::Hull,
            facing: 0,
        };
        assert!(!c.place(far));
        let second = Part {
            at: [1, 0, 0],
            block: Block::VehicleCore,
            facing: 0,
        };
        assert!(!c.place(second));
        assert_eq!(c.parts().len(), 1);
    }

    #[test]
    fn breaking_the_middle_drops_what_it_held() {
        let mut c = line();
        let gone = c.remove([0, 0, 2]);
        assert_eq!(gone.parts.len(), 2, "that part and the one past it");
        assert!(!gone.core_gone);
        assert_eq!(c.parts().len(), 2);
    }

    #[test]
    fn breaking_the_core_ends_it() {
        let mut c = line();
        let gone = c.remove([0, 0, 0]);
        assert!(gone.core_gone);
        assert_eq!(gone.parts.len(), 4);
        // The same crash breaking more parts after the core (it used to panic).
        let more = c.remove([0, 0, 2]);
        assert!(more.parts.is_empty() && !more.core_gone);
        assert!(c.parts().is_empty() && c.get([0, 0, 1]).is_none());
    }

    #[test]
    fn the_centre_of_mass_is_where_the_weight_is() {
        let c = line();
        let m = c.massing(0.0);
        assert!((m.mass - (150.0 + 3.0 * 40.0)).abs() < 1e-9);
        let expected = (40.0 * (1.0 + 2.0 + 3.0)) / m.mass;
        assert!((m.com.z - expected).abs() < 1e-9, "{}", m.com);
        assert!(m.com.x.abs() < 1e-9 && m.com.y.abs() < 1e-9);
        // Long in z: easy to spin about z, harder about the others.
        assert!(m.inertia.z_axis.z < m.inertia.x_axis.x);
    }

    #[test]
    fn fuel_moves_the_centre_of_mass() {
        let mut c = Craft::new(2);
        for y in 1..=4 {
            assert!(c.place(Part {
                at: [0, y, 0],
                block: Block::FuelTank,
                facing: 0,
            }));
        }
        assert_eq!(c.stats.capacity, 400.0);
        let (full, empty) = (c.massing(400.0), c.massing(0.0));
        assert!(full.mass > empty.mass);
        assert!(full.com.y > empty.com.y, "fuel sits above the core");
        assert_eq!(c.massing(9_999.0).mass, full.mass);
    }

    #[test]
    fn forward_is_where_the_engines_push() {
        let mut c = Craft::new(4);
        assert_eq!(c.stats.forward, DVec3::Z);
        // An engine under it, exhaust down: it's a rocket now, whatever the core says.
        assert!(c.place(Part {
            at: [0, -1, 0],
            block: Block::RocketEngine,
            facing: 3,
        }));
        assert_eq!(c.stats.forward, DVec3::Y);
        assert_eq!(c.stats.up, DVec3::Z);
        assert_eq!(c.stats.engines[0].push, DVec3::Y);
    }

    #[test]
    fn parts_face_sensibly_by_where_they_go() {
        // A plane: forward +Z, up +Y.
        let plane = Craft::new(4).stats;
        let at_back = auto_facing(Block::JetEngine, DVec3::NEG_Z, &plane);
        assert_eq!(direction(at_back), DVec3::NEG_Z, "exhaust out of the back");
        let nose = auto_facing(Block::Propeller, DVec3::Z, &plane);
        assert_eq!(
            direction(nose),
            DVec3::NEG_Z,
            "a nose propeller pulls forwards"
        );
        let wing = auto_facing(Block::Wing, DVec3::X, &plane);
        assert_eq!(direction(wing), DVec3::Y, "a side wing lifts up");
        let fin = auto_facing(Block::Flap, DVec3::Y, &plane);
        assert_eq!(direction(fin).y, 0.0, "a fin on top stands upright");
        assert_eq!(direction(fin).z, 0.0, "and is edge-on to the flow");

        // A rocket: forward +Y. A fin on its side is still flat along the flow.
        let mut rocket = Craft::new(2);
        assert!(rocket.place(Part {
            at: [0, -1, 0],
            block: Block::RocketEngine,
            facing: 3,
        }));
        let fin = auto_facing(Block::Wing, DVec3::X, &rocket.stats);
        assert_eq!(direction(fin).y, 0.0);
        assert_eq!(direction(fin).x, 0.0);
    }

    #[test]
    fn the_ray_finds_the_part_and_the_face() {
        let c = line();
        let (at, face, t) = c
            .ray(DVec3::new(0.0, 5.0, 2.0), DVec3::NEG_Y, 10.0)
            .expect("hits");
        assert_eq!(at, [0, 0, 2]);
        assert_eq!(direction(face), DVec3::Y, "came in from above");
        assert!((t - 4.5).abs() < 1e-9, "{t}");
        assert!(
            c.ray(DVec3::new(3.0, 5.0, 2.0), DVec3::NEG_Y, 10.0)
                .is_none()
        );
        // Starting inside a part finds that part at once.
        assert_eq!(
            c.ray(DVec3::ZERO, DVec3::X, 3.0).map(|h| h.0),
            Some([0, 0, 0])
        );
    }

    #[test]
    fn people_stand_on_it_and_walls_stop_them() {
        let mut c = Craft::new(4);
        assert!(c.place(Part {
            at: [1, 0, 0],
            block: Block::Hull,
            facing: 0,
        }));
        // Dropping onto the top (cells end at y = 0.5).
        let (p, bump) = c.sweep(
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 0.2, 0.0),
            0.3,
            1.8,
        );
        assert!(bump.floor);
        assert!((p.y - 0.5).abs() < 1e-9, "{p}");
        // Walking into the side from beside it.
        let (p, bump) = c.sweep(
            DVec3::new(0.0, -1.0, 2.0),
            DVec3::new(0.0, -1.0, 2.0),
            0.3,
            1.8,
        );
        assert!(!bump.axes[2], "not moving");
        assert_eq!(p.z, 2.0);
        let start = DVec3::new(-2.0, 0.0, 0.0);
        let (p, bump) = c.sweep(start, DVec3::new(0.0, 0.0, 0.0), 0.3, 1.8);
        assert!(bump.axes[0]);
        assert!((p.x + 0.8).abs() < 1e-9, "{p}");
    }

    #[test]
    fn it_survives_saving() {
        let c = line();
        let text = serde_json::to_string(&c).unwrap();
        let back: Craft = serde_json::from_str(&text).unwrap();
        assert_eq!(back.parts().len(), 4);
        assert_eq!(back.stats.dry_mass, c.stats.dry_mass);
        assert!(back.get([0, 0, 3]).is_some());
    }
}
