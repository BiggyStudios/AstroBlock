//! Craft: for now one test rocket, flying under patched conics.
//!
//! # State
//! A vessel's centre of mass moves relative to its *reference body* (`body`: the one whose
//! sphere of influence it is in) in that body's inertial frame. Only that body's gravity
//! acts (KSP's patched conics); crossing an SOI boundary re-expresses the state relative to
//! the new body. Its attitude is a rotation from vessel axes to inertial axes:
//! - +Y is the nose (the engine pushes along it),
//! - +X is right, +Z the dorsal side (the pilot's up), so X x Y = Z.
//!
//! On the pad the nose points up, right points east (so yawing right tips it east, the way
//! to launch) and the dorsal side faces south.
//!
//! # Physics (each 1/120 s step)
//! - Rotation: reaction wheels turn it (`WHEEL_ACCEL`); with SAS on and no input they hold
//!   an attitude or turn towards a target direction (prograde, normal, a maneuver...). In
//!   air, fins turn the nose into the airflow (weathervaning) and damp rotation.
//! - Forces: gravity, thrust along the nose, drag on the velocity relative to the
//!   co-rotating air (much more side-on than nose-first).
//! - Fuel: burning it makes the rocket lighter (the rocket equation), and when it is gone
//!   the engine stops. It refills only standing on the home planet.
//! - Heating: a Sutton-Graves-like flux, sqrt(air density) x speed^3 (above ~300 m/s), heats
//!   the hull; it cools by radiation and convection. Too hot and it burns up.
//! - Ground: the loaded block world stops it (the player's collision sweep, an upright box
//!   at the engine; see `Ground`). Hitting it faster than `CRASH_SPEED`, or settling more than `MAX_LANDING_TILT` off vertical, destroys it.
//!   Otherwise it lands: it then rides along with the turning ground and stands upright.
//!
//! # Time warp
//! On rails (`rails`) it follows its conic exactly (`trajectory::advance`), until it would
//! enter an atmosphere or come within `RAILS_CLEARANCE` of a surface.

use glam::{DMat3, DQuat, DVec3};

use super::{
    craft::{At, Craft, Part},
    craft_model::{Looks, Pivot},
};
use crate::{
    player::{self, Voxels},
    render::MeshHandle,
    space::{
        RotatingFrame,
        bodies::{Bodies, Body},
        conic::Conic,
        trajectory,
    },
    world::{chunk::BlockCell, noise::smoothstep, planet::Planet},
};

/// Mass without fuel, and of a full tank (kg).
pub const DRY_MASS: f64 = 2_200.0;
pub const FUEL_CAPACITY: f64 = 800.0;
/// What `Vessel::fill_up` gives a vehicle without tanks (kg): more than any frame burns.
const TANKLESS_FUEL: f64 = 50.0;
/// Engine thrust (N): barely more than the weight of a full rocket at the surface (1.26
/// times), so lift-off is slow and how you spend the fuel matters.
pub const THRUST: f64 = 37_000.0;
/// How fast the exhaust leaves (m/s), which sets what a tank is worth: exhaust x
/// ln(full / empty) = 1,890 m/s. A decent ascent to orbit takes about 1,200 of that, a trip
/// to the moon and back about 600, so it leaves little to spare.
pub const EXHAUST_SPEED: f64 = 6_100.0;
/// Seconds to fill an empty tank, landed on the home planet.
pub(super) const REFUEL_TIME: f64 = 20.0;
/// From the engine bell to the nose tip (m), and the centre of mass above the bell.
pub const HEIGHT: f64 = 8.0;
pub const COM_HEIGHT: f64 = 3.0;
/// Collision box: half width, and height from the bell up (the fins and tank bottom).
const BOX: (f64, f64) = (1.0, 4.0);
const STEP: f64 = 1.0 / 120.0;
/// Vehicles built from blocks have stiffer contacts (wheels on springs).
const CRAFT_STEP: f64 = 1.0 / 240.0;
/// Reaction wheels: angular acceleration (rad/s^2) and the fastest SAS turn (rad/s).
const WHEEL_ACCEL: f64 = 1.5;
const MAX_TURN_RATE: f64 = 1.0;
/// SAS: turn rate per radian of error (1/s), and how fast it reaches that rate (s).
const SAS_GAIN: f64 = 2.5;
const SAS_RESPONSE: f64 = 0.2;
/// Touching the ground faster than this (m/s) is a crash.
pub const CRASH_SPEED: f64 = 12.0;
const MAX_LANDING_TILT: f64 = 0.9;
/// Drag area times coefficient (m^2): nose first, and side on.
const CDA_NOSE: f64 = 1.2;
const CDA_SIDE: f64 = 14.0;
/// Fins: turning towards the airflow (rad/s^2 per Pa of dynamic pressure) and damping
/// (1/s per Pa).
const AERO_TORQUE: f64 = 2.5e-5;
const AERO_DAMPING: f64 = 1.5e-5;
/// Hull heating. The flux constant is gameplay-tuned: speeds here are a tenth of Earth's,
/// so the real constant would give no heating at all.
pub(super) const HEAT_FLUX: f64 = 0.05;
/// The mass (kg) the heating numbers were tuned for: a vehicle of another mass heats up that
/// much slower or faster.
pub(super) const HEAT_REFERENCE_MASS: f64 = 3_000.0;
const HEATED_AREA: f64 = 3.0;
const RADIATING_AREA: f64 = 40.0;
const EMISSIVITY: f64 = 0.8;
const HEAT_CAPACITY: f64 = 1.8e5;
const AIR_TEMPERATURE: f64 = 260.0;
pub const START_TEMPERATURE: f64 = 290.0;
pub const MAX_TEMPERATURE: f64 = 1_700.0;
const STEFAN_BOLTZMANN: f64 = 5.670_374e-8;
/// Rails warp stops this far (m) above an airless surface.
pub const RAILS_CLEARANCE: f64 = 3_000.0;
/// How fast a vessel drawn from someone else's news catches up with the latest (s), and how
/// far off it may be before it simply jumps there (m).
const NEWS_EASE: f64 = 0.15;
const NEWS_SNAP: f64 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SasMode {
    /// Hold the current attitude.
    Stability,
    Prograde,
    Retrograde,
    Normal,
    Antinormal,
    RadialOut,
    RadialIn,
    /// The next maneuver node's burn direction.
    Maneuver,
}

impl SasMode {
    pub const ALL: [SasMode; 8] = [
        SasMode::Stability,
        SasMode::Prograde,
        SasMode::Retrograde,
        SasMode::Normal,
        SasMode::Antinormal,
        SasMode::RadialOut,
        SasMode::RadialIn,
        SasMode::Maneuver,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SasMode::Stability => "hold",
            SasMode::Prograde => "pro",
            SasMode::Retrograde => "retro",
            SasMode::Normal => "norm",
            SasMode::Antinormal => "anti",
            SasMode::RadialOut => "rad+",
            SasMode::RadialIn => "rad-",
            SasMode::Maneuver => "node",
        }
    }
}

/// Steering input, -1..1 each: pitch up (towards the dorsal side), yaw right, roll right.
#[derive(Clone, Copy, Debug, Default)]
pub struct Controls {
    pub pitch: f64,
    pub yaw: f64,
    pub roll: f64,
    /// Driven wheels: forwards (1) or backwards (-1).
    pub drive: f64,
    pub brake: bool,
}

impl Controls {
    pub(super) fn any(&self) -> bool {
        self.pitch != 0.0 || self.yaw != 0.0 || self.roll != 0.0
    }
}

/// Resting on a body: where (centre of mass) and how, in the body's rotating frame.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Landed {
    pub position: DVec3,
    pub attitude: DQuat,
}

/// A body's block world: its grid and blocks.
#[derive(Clone, Copy)]
pub struct Ground<'a> {
    pub body: Body,
    pub planet: &'a Planet,
    pub world: &'a dyn Voxels,
}

/// What a vessel's physics needs from the world.
pub struct Env<'a> {
    pub bodies: &'a Bodies,
    /// The block world loaded now (home's or the moon's); vessels at other bodies don't
    /// meet any ground.
    pub ground: Option<Ground<'a>>,
    pub drag: bool,
}

/// Engine off and clear of air and ground: this thing simply follows its conic.
pub fn coasting(bodies: &Bodies, body: Body, position: DVec3, throttle: f64) -> bool {
    throttle == 0.0 && position.length() > bodies.rails_floor(body, RAILS_CLEARANCE)
}

fn full_tank() -> f64 {
    FUEL_CAPACITY
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Vessel {
    /// Tells it apart across the network.
    #[serde(default)]
    pub id: u32,
    /// Who is flying it, in a multiplayer game.
    #[serde(skip)]
    pub pilot: Option<u32>,
    pub body: Body,
    /// Centre of mass, relative to `body`, inertial axes.
    pub position: DVec3,
    pub velocity: DVec3,
    /// Vessel axes -> inertial axes.
    pub attitude: DQuat,
    /// Angular velocity (rad/s, inertial axes).
    pub spin: DVec3,
    pub throttle: f64,
    /// Fuel left (kg). (Saves from before there was fuel have a full tank.)
    #[serde(default = "full_tank")]
    pub fuel: f64,
    /// The tank just ran dry (the game tells the pilot).
    #[serde(skip)]
    pub flameout: bool,
    pub sas: bool,
    pub sas_mode: SasMode,
    /// The attitude stability mode holds (captured when steering stops).
    pub(super) hold: Option<DQuat>,
    pub landed: Option<Landed>,
    /// Hull temperature (K), and the heat flux on it now (W/m^2).
    pub temperature: f64,
    pub heating: f64,
    /// Dynamic pressure (Pa).
    pub pressure: f64,
    /// Why it was destroyed, once it is.
    #[serde(skip)]
    pub destroyed: Option<&'static str>,
    /// Velocity change the engine delivered since the game last took it (inertial, m/s):
    /// maneuver nodes count down with it.
    pub burned: DVec3,
    #[serde(skip)]
    pub mesh: Option<MeshHandle>,
    /// What it is built from; none for the plain test rocket.
    #[serde(default)]
    pub craft: Option<Box<Craft>>,
    /// The `Craft::version` the mesh was built from.
    #[serde(skip)]
    pub mesh_version: u32,
    /// The light the mesh was built with (sky << 4 | block).
    #[serde(skip)]
    pub mesh_light: u32,
    /// How long it has sat still on the ground (s).
    #[serde(skip)]
    pub(super) still: f64,
    /// Something touched the ground in the last step.
    #[serde(skip)]
    pub touching: bool,
    /// Parts being broken off in this step.
    #[serde(skip)]
    pub(super) breaking: Vec<At>,
    /// What has come off since the game last looked.
    #[serde(skip)]
    pub broke_off: Vec<Part>,
    /// Grown trees it has run into since the game last looked (the game knocks them down).
    #[serde(skip)]
    pub felled: Vec<BlockCell>,
    /// Someone else's, seen here: how far the drawn vessel still is from where the latest
    /// news puts it (inertial), and how it is still turned from there. Both ease away
    /// (`ease_news`), so news doesn't make it jump.
    #[serde(skip)]
    pub shown_offset: DVec3,
    #[serde(skip)]
    pub shown_turn: DQuat,
    /// What the physics last did with the moving parts (and others are told): each surface's
    /// flap deflection (-1..1, by `Stats::surfaces`; 0 for wings), and the front wheels' angle
    /// (rad).
    #[serde(skip)]
    pub flaps: Vec<f32>,
    #[serde(skip)]
    pub steer: f32,
    /// The moving pieces' meshes (built with `mesh`), and what they are doing as drawn.
    #[serde(skip)]
    pub moving: Vec<(Pivot, MeshHandle)>,
    #[serde(skip)]
    pub looks: Looks,
    /// `looks` has been set to how the vehicle is (doors open or shut) when first drawn.
    #[serde(skip)]
    pub looks_started: bool,
    /// Electricity in its batteries (J; see `Stats::battery`). Negative: full (a vehicle saved
    /// before there was electricity).
    #[serde(default = "full_battery")]
    pub charge: f64,
    /// Its drills are running.
    #[serde(default)]
    pub drilling: bool,
    /// Electricity in the last step: what was asked for and what was made (W), and how much of
    /// what was asked for the next step gets (0..1).
    #[serde(skip)]
    pub power_draw: f64,
    #[serde(skip)]
    pub power_made: f64,
    #[serde(skip, default = "one")]
    pub power_share: f64,
    /// The way to the sun (inertial) and how strong its light is here (1 at home, 0 in a
    /// shadow), worked out once a frame.
    #[serde(skip)]
    pub sun: (DVec3, f64),
    /// How the gimballed engines' nozzles are swung (grid axes: the torque they give, as a
    /// share of what they can).
    #[serde(skip)]
    pub gimbal: DVec3,
}

fn full_battery() -> f64 {
    -1.0
}

fn one() -> f64 {
    1.0
}

impl Vessel {
    /// Takes every mesh it has (to free them).
    pub fn take_meshes(&mut self) -> Vec<MeshHandle> {
        let moving = std::mem::take(&mut self.moving);
        self.mesh
            .take()
            .into_iter()
            .chain(moving.into_iter().map(|(_, m)| m))
            .collect()
    }

    /// A vessel standing upright on `body` with its engine at `base` (that body's frame).
    pub fn on_pad(bodies: &Bodies, body: Body, base: DVec3, t: f64) -> Self {
        let up = base.normalize_or(DVec3::Y);
        let east = DVec3::Y
            .cross(up)
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let attitude = DQuat::from_mat3(&DMat3::from_cols(east, up, east.cross(up)));
        let mut vessel = Self {
            id: 0,
            pilot: None,
            body,
            position: DVec3::ZERO,
            velocity: DVec3::ZERO,
            attitude: DQuat::IDENTITY,
            spin: DVec3::ZERO,
            throttle: 0.0,
            fuel: FUEL_CAPACITY,
            flameout: false,
            sas: true,
            sas_mode: SasMode::Stability,
            hold: None,
            landed: Some(Landed {
                position: base + up * COM_HEIGHT,
                attitude,
            }),
            temperature: START_TEMPERATURE,
            heating: 0.0,
            pressure: 0.0,
            destroyed: None,
            burned: DVec3::ZERO,
            mesh: None,
            craft: None,
            mesh_version: 0,
            mesh_light: 0,
            still: 0.0,
            touching: false,
            breaking: Vec::new(),
            broke_off: Vec::new(),
            felled: Vec::new(),
            shown_offset: DVec3::ZERO,
            shown_turn: DQuat::IDENTITY,
            flaps: Vec::new(),
            steer: 0.0,
            moving: Vec::new(),
            looks: Looks::default(),
            looks_started: false,
            charge: 0.0,
            drilling: false,
            power_draw: 0.0,
            power_made: 0.0,
            power_share: 1.0,
            sun: (DVec3::Y, 0.0),
            gimbal: DVec3::ZERO,
        };
        vessel.sync_landed(bodies, t);
        vessel
    }

    /// A vehicle resting with its core at `core` (the body's frame), grid axes turned by
    /// `rotation` (grid to body frame). It starts with full tanks.
    pub fn built(
        bodies: &Bodies,
        body: Body,
        core: DVec3,
        rotation: DQuat,
        t: f64,
        craft: Craft,
    ) -> Self {
        let mut vessel = Self::on_pad(bodies, body, core, t);
        let com = craft.massing(craft.stats.capacity).com;
        vessel.fuel = craft.stats.capacity;
        vessel.charge = craft.stats.battery;
        vessel.landed = Some(Landed {
            position: core + rotation * com,
            attitude: rotation.normalize(),
        });
        vessel.craft = Some(Box::new(craft));
        vessel.sync_landed(bodies, t);
        vessel
    }

    /// Fuel when full (kg).
    pub fn fuel_capacity(&self) -> f64 {
        self.craft
            .as_ref()
            .map_or(FUEL_CAPACITY, |c| c.stats.capacity)
    }

    /// Grid coordinates of the centre of mass (zero for the plain rocket).
    pub fn com(&self) -> DVec3 {
        self.craft
            .as_ref()
            .map_or(DVec3::ZERO, |c| c.massing(self.fuel).com)
    }

    /// A point of the vehicle's grid, relative to `body`, inertial axes.
    pub fn grid_to_inertial(&self, p: DVec3) -> DVec3 {
        self.position + self.attitude * (p - self.com())
    }

    /// The velocity of a point of its grid (inertial).
    pub fn point_velocity(&self, p: DVec3) -> DVec3 {
        self.velocity + self.spin.cross(self.attitude * (p - self.com()))
    }

    /// Changes its parts, keeping every part where it is in the world (the centre of mass
    /// moves, and the state follows it).
    pub fn rebuild_craft(&mut self, change: impl FnOnce(&mut Craft)) {
        let Some(mut craft) = self.craft.take() else {
            return;
        };
        let before = craft.massing(self.fuel).com;
        if self.charge < 0.0 {
            self.charge = craft.stats.battery;
        }
        change(&mut craft);
        self.fuel = self.fuel.min(craft.stats.capacity);
        self.charge = self.charge.min(craft.stats.battery);
        let shift = craft.massing(self.fuel).com - before;
        self.position += self.attitude * shift;
        if let Some(l) = &mut self.landed {
            l.position += l.attitude * shift;
        }
        self.craft = Some(craft);
    }

    /// Knocks a part off in a collision (with whatever only it held on).
    pub fn break_part(&mut self, at: At) {
        let mut removed = crate::flight::craft::Removed::default();
        self.rebuild_craft(|c| removed = c.remove(at));
        if removed.core_gone {
            self.destroyed = Some("crashed");
        }
        self.broke_off.extend(removed.parts);
    }

    /// A full tank at once (`fuel infinite`). A vehicle without tanks gets enough for its
    /// engines to run (it weighs nothing: there is nowhere to put it). The parts stay where
    /// they are as the centre of mass moves.
    pub fn fill_up(&mut self) {
        let full = match &self.craft {
            Some(c) if c.stats.capacity > 0.0 => c.stats.capacity,
            Some(_) => TANKLESS_FUEL,
            None => FUEL_CAPACITY,
        };
        if self.fuel >= full {
            return;
        }
        if let Some(c) = &self.craft {
            let shift = c.massing(full).com - c.massing(self.fuel).com;
            self.position += self.attitude * shift;
        }
        self.fuel = full;
    }

    /// The throttle is up and there is an engine with something to burn (or electricity, for
    /// one that runs on it). (Not for use inside a physics step: the craft is taken out of the
    /// vessel for that.)
    pub fn engines_on(&self) -> bool {
        if self.throttle <= 0.0 {
            return false;
        }
        match &self.craft {
            None => self.fuel > 0.0,
            Some(c) => c.stats.engines.iter().any(|e| {
                if e.kind.electric() {
                    self.charge != 0.0 || self.power_made > 0.0
                } else {
                    self.fuel > 0.0
                }
            }),
        }
    }

    /// The engines that are pushing now, and how hard (0..1 of full thrust, by what they have
    /// to burn).
    pub fn running_engines(&self) -> Vec<(crate::flight::craft::Engine, f64)> {
        let Some(c) = &self.craft else {
            return Vec::new();
        };
        if self.throttle <= 0.0 {
            return Vec::new();
        }
        c.stats
            .engines
            .iter()
            .filter_map(|e| {
                let power = if e.kind.electric() {
                    self.power_share
                } else if self.fuel > 0.0 {
                    1.0
                } else {
                    0.0
                };
                (power > 0.0).then_some((*e, power * self.throttle))
            })
            .collect()
    }

    /// Full batteries at once (`power infinite`).
    pub fn fill_charge(&mut self) {
        if let Some(c) = &self.craft {
            self.charge = c.stats.battery;
            self.power_share = 1.0;
        }
    }

    /// How full the batteries are (0..1).
    pub fn charge_share(&self) -> f64 {
        match &self.craft {
            Some(c) if c.stats.battery > 0.0 => {
                if self.charge < 0.0 {
                    1.0
                } else {
                    (self.charge / c.stats.battery).clamp(0.0, 1.0)
                }
            }
            _ => 0.0,
        }
    }

    /// While nothing simulates it (parked, or coasting on rails): its panels and generators
    /// still charge the batteries, over `dt` seconds.
    pub fn charge_idle(&mut self, bodies: &Bodies, t: f64, dt: f64) {
        let Some(craft) = self.craft.take() else {
            return;
        };
        if craft.stats.solar.is_empty() && craft.stats.generators == 0 && craft.stats.rtgs == 0 {
            self.craft = Some(craft);
            return;
        }
        self.sun = bodies.sunlight(self.body, self.position, t);
        // (In steps, so a generator doesn't overshoot what the batteries take.)
        let steps = (dt / 1.0).ceil().clamp(1.0, 64.0);
        for _ in 0..steps as usize {
            let burned = self.electricity(&craft, 0.0, dt / steps);
            self.fuel = (self.fuel - burned).max(0.0);
        }
        self.craft = Some(craft);
    }

    /// Mass right now (kg).
    pub fn mass(&self) -> f64 {
        match &self.craft {
            Some(c) => c.massing(self.fuel).mass,
            None => DRY_MASS + self.fuel,
        }
    }

    /// What the engine can do at full throttle right now (m/s^2).
    pub fn acceleration(&self) -> f64 {
        match &self.craft {
            Some(c) => c.stats.engines.iter().map(|e| e.kind.thrust).sum::<f64>() / self.mass(),
            None => THRUST / self.mass(),
        }
    }

    /// How much velocity the fuel left can still add (m/s).
    pub fn delta_v(&self) -> f64 {
        let Some(c) = &self.craft else {
            return EXHAUST_SPEED * (self.mass() / DRY_MASS).ln();
        };
        // All engines running: what they burn together sets the exhaust speed.
        let thrust: f64 = c.stats.engines.iter().map(|e| e.kind.thrust).sum();
        let burn: f64 = c
            .stats
            .engines
            .iter()
            .map(|e| e.kind.thrust / e.kind.exhaust)
            .sum();
        if burn <= 0.0 {
            return 0.0;
        }
        (thrust / burn) * (self.mass() / c.stats.dry_mass).ln()
    }

    pub fn nose(&self) -> DVec3 {
        self.attitude * self.craft.as_ref().map_or(DVec3::Y, |c| c.stats.forward)
    }

    /// To the pilot's right (unit, inertial).
    pub fn right(&self) -> DVec3 {
        self.attitude * self.craft.as_ref().map_or(DVec3::X, |c| c.stats.right)
    }

    /// Over the pilot's head (unit, inertial).
    pub fn dorsal(&self) -> DVec3 {
        self.attitude * self.craft.as_ref().map_or(DVec3::Z, |c| c.stats.up)
    }

    /// Updates the inertial state of a landed vessel from where it rests.
    pub fn sync_landed(&mut self, bodies: &Bodies, t: f64) {
        if let Some(l) = self.landed {
            let frame = bodies.frame(self.body, t);
            self.position = frame.to_inertial(l.position);
            self.velocity = frame.inertial_velocity(l.position, DVec3::ZERO);
            self.attitude = frame.rotation * l.attitude;
            self.spin = frame.rotation * frame.omega;
        }
    }

    /// Position and velocity relative to the planet (PCI).
    pub fn pci(&self, bodies: &Bodies, t: f64) -> (DVec3, DVec3) {
        bodies.to_pci(self.body, self.position, self.velocity, t)
    }

    /// Where the centre of mass is in the frame the game draws in (the `active` body's).
    pub fn world_position(&self, bodies: &Bodies, active: Body, t: f64) -> DVec3 {
        bodies.view(active, self.body, self.position + self.shown_offset, t)
    }

    /// The attitude it is drawn with (see `shown_turn`).
    pub fn shown_attitude(&self) -> DQuat {
        (self.shown_turn * self.attitude).normalize()
    }

    /// News of someone else's vessel has just moved it from where it was drawn (`before`:
    /// position and attitude): it is drawn there still, and eases over (`ease_news`).
    pub fn news_moved(&mut self, body_before: Body, before: (DVec3, DQuat)) {
        let offset = before.0 - self.position;
        let turn = (before.1 * self.attitude.inverse()).normalize();
        if body_before == self.body && offset.length() < NEWS_SNAP && turn.w.abs() > 0.7 {
            self.shown_offset = offset;
            self.shown_turn = turn;
        } else {
            self.shown_offset = DVec3::ZERO;
            self.shown_turn = DQuat::IDENTITY;
        }
    }

    /// What is drawn catches up with the news (`dt` in real seconds).
    pub fn ease_news(&mut self, dt: f64) {
        let keep = (-dt / NEWS_EASE).exp();
        self.shown_offset *= keep;
        self.shown_turn = DQuat::IDENTITY.slerp(self.shown_turn, keep).normalize();
        if self.shown_offset.length_squared() < 1e-8 {
            self.shown_offset = DVec3::ZERO;
        }
    }

    /// The air around it: density (kg/m^3) and the velocity relative to that air (inertial).
    pub(super) fn air(&self, bodies: &Bodies, t: f64) -> (f64, DVec3) {
        if self.body != Body::Planet {
            return (0.0, self.velocity);
        }
        let frame = bodies.frame(Body::Planet, t);
        let omega = frame.rotation * frame.omega;
        let air = self.velocity - omega.cross(self.position);
        (bodies.gravity.air_density(self.position.length()), air)
    }

    /// Velocity relative to the surface below (inertial axes).
    pub fn surface_velocity(&self, bodies: &Bodies, t: f64) -> DVec3 {
        let frame = bodies.frame(self.body, t);
        let omega = frame.rotation * frame.omega;
        self.velocity - omega.cross(self.position)
    }

    /// Whether the navball should show surface-relative directions (low down).
    pub fn surface_mode(&self, bodies: &Bodies) -> bool {
        let altitude = self.position.length() - bodies.radius(self.body);
        match self.body {
            Body::Planet => altitude < 12_000.0,
            Body::Moon => altitude < 4_000.0,
            Body::Star => false,
        }
    }

    /// The direction SAS turns to in `mode` (None: hold the attitude). `maneuver` is the
    /// next node's burn direction.
    pub fn sas_direction(&self, bodies: &Bodies, t: f64, maneuver: Option<DVec3>) -> Option<DVec3> {
        let v = if self.surface_mode(bodies) {
            self.surface_velocity(bodies, t)
        } else {
            self.velocity
        };
        let (prograde, normal, radial) = trajectory::orbit_axes(self.position, v);
        if v.length() < 0.5
            && matches!(
                self.sas_mode,
                SasMode::Prograde | SasMode::Retrograde | SasMode::Normal | SasMode::Antinormal
            )
        {
            return None;
        }
        match self.sas_mode {
            SasMode::Stability => None,
            SasMode::Prograde => Some(prograde),
            SasMode::Retrograde => Some(-prograde),
            SasMode::Normal => Some(normal),
            SasMode::Antinormal => Some(-normal),
            SasMode::RadialOut => Some(radial),
            SasMode::RadialIn => Some(-radial),
            SasMode::Maneuver => maneuver.and_then(DVec3::try_normalize),
        }
    }

    pub fn conic(&self, bodies: &Bodies) -> Conic {
        Conic::from_state(bodies.mu(self.body), self.position, self.velocity)
    }

    /// Whether time can warp beyond physics speed: landed, or coasting clear of air and
    /// ground.
    pub fn can_rail_warp(&self, bodies: &Bodies) -> bool {
        if self.landed.is_some() {
            return true;
        }
        coasting(bodies, self.body, self.position, self.throttle)
    }

    /// Follows the conic from `t0` to `t1`; returns the time reached (earlier if it had to
    /// stop for an atmosphere or a surface) and whether it stopped.
    pub fn rails(&mut self, bodies: &Bodies, (t0, t1): (f64, f64)) -> (f64, bool) {
        self.charge_idle(bodies, t0, t1 - t0);
        if self.landed.is_some() {
            self.sync_landed(bodies, t1);
            return (t1, false);
        }
        let (body, r, v, t, stopped) = trajectory::advance(
            bodies,
            self.body,
            self.position,
            self.velocity,
            (t0, t1),
            RAILS_CLEARANCE,
        );
        self.body = body;
        self.position = r;
        self.velocity = v;
        self.spin = DVec3::ZERO;
        self.feel_the_sun(bodies);
        (t, stopped)
    }

    /// Near the star: its light heats the hull (at least to what sunlight alone makes it), and
    /// inside its hot zone it burns up.
    fn feel_the_sun(&mut self, bodies: &Bodies) {
        let Some(hot) = bodies.hot_zone(self.body) else {
            return;
        };
        let distance = self.position.length();
        let sunlit = bodies.sunlit_temperature(distance, MAX_TEMPERATURE);
        self.temperature = self.temperature.max(sunlit.min(MAX_TEMPERATURE));
        if distance <= hot + 1.0 {
            self.destroyed = Some("burned up near the sun");
        }
    }

    /// Physics from `t0` to `t1`. `target` is SAS's direction (from `sas_direction`).
    pub fn update(
        &mut self,
        env: &Env,
        controls: &Controls,
        (t0, t1): (f64, f64),
        target: Option<DVec3>,
    ) {
        let step = if self.craft.is_some() {
            CRAFT_STEP
        } else {
            STEP
        };
        let steps = ((t1 - t0) / step).ceil().clamp(1.0, 2400.0) as usize;
        let dt = (t1 - t0) / steps as f64;
        self.sun = env.bodies.sunlight(self.body, self.position, t0);
        for i in 0..steps {
            if self.destroyed.is_some() {
                return;
            }
            if self.craft.is_some() {
                self.step_craft(env, controls, t0 + dt * i as f64, dt, target);
            } else {
                self.step(env, controls, t0 + dt * i as f64, dt, target);
            }
            self.feel_the_sun(env.bodies);
        }
    }

    fn step(&mut self, env: &Env, controls: &Controls, t: f64, dt: f64, target: Option<DVec3>) {
        let bodies = env.bodies;
        let gravity = bodies.gravity(self.body, self.position);
        if self.fuel <= 0.0 {
            self.throttle = 0.0;
        }
        let throttle = self.throttle.clamp(0.0, 1.0);
        let thrust = throttle * self.acceleration();
        if self.landed.is_some() {
            if thrust > gravity.length() * 1.02 {
                self.landed = None; // lift-off
            } else {
                if self.body == Body::Planet {
                    self.fuel = (self.fuel + FUEL_CAPACITY / REFUEL_TIME * dt).min(FUEL_CAPACITY);
                }
                self.sync_landed(bodies, t + dt);
                self.heating = 0.0;
                self.pressure = 0.0;
                self.cool(0.0, 0.0, dt);
                return;
            }
        }

        // Rotation: input, else SAS.
        let (rho, air) = if env.drag {
            self.air(bodies, t)
        } else {
            (0.0, self.velocity)
        };
        let nose = self.nose();
        let local = DVec3::new(controls.pitch, controls.roll, -controls.yaw);
        if controls.any() {
            self.spin += self.attitude * local * (WHEEL_ACCEL * dt);
            self.hold = None;
        } else if self.sas {
            let wanted = match target {
                Some(dir) => {
                    // Turn the nose towards `dir`; no roll.
                    let axis = nose.cross(dir);
                    let angle = axis.length().atan2(nose.dot(dir));
                    axis.normalize_or_zero() * (angle * SAS_GAIN).min(MAX_TURN_RATE)
                }
                None => {
                    let hold = *self.hold.get_or_insert(self.attitude);
                    let (axis, angle) = (hold * self.attitude.inverse()).to_axis_angle();
                    let angle = if angle > std::f64::consts::PI {
                        angle - std::f64::consts::TAU
                    } else {
                        angle
                    };
                    axis * (angle * SAS_GAIN).clamp(-MAX_TURN_RATE, MAX_TURN_RATE)
                }
            };
            let change = (wanted - self.spin) / SAS_RESPONSE;
            let max = WHEEL_ACCEL;
            let change = if change.length() > max {
                change * (max / change.length())
            } else {
                change
            };
            self.spin += change * dt;
        }
        if target.is_some() || !self.sas {
            // Holding starts afresh from wherever it points when stability takes over.
            self.hold = None;
        }
        // Fins: weathervane into the airflow, and damping.
        let speed = air.length();
        self.pressure = 0.5 * rho * speed * speed;
        if self.pressure > 1.0 && speed > 1.0 {
            let flow = air / speed;
            self.spin += nose.cross(flow) * (AERO_TORQUE * self.pressure * dt);
            self.spin *= (-AERO_DAMPING * self.pressure * dt).exp();
        }
        self.attitude = (DQuat::from_scaled_axis(self.spin * dt) * self.attitude).normalize();

        // Forces.
        let push = self.nose() * thrust;
        self.velocity += (gravity + push) * dt;
        self.burned += push * dt;
        if throttle > 0.0 {
            self.fuel -= throttle * THRUST / EXHAUST_SPEED * dt;
            if self.fuel <= 0.0 {
                self.fuel = 0.0;
                self.throttle = 0.0;
                self.flameout = true;
            }
        }
        if rho > 0.0 && speed > 0.0 {
            let side = 1.0 - (nose.dot(air / speed)).powi(2);
            let cda = CDA_NOSE + (CDA_SIDE - CDA_NOSE) * side.max(0.0);
            let keep = (-0.5 * rho * cda / self.mass() * speed * dt).exp();
            self.velocity -= air * (1.0 - keep);
        }

        // Heat.
        self.heating = HEAT_FLUX * rho.sqrt() * speed.powi(3) * smoothstep(250.0, 450.0, speed);
        self.cool(rho, self.heating, dt);
        if self.temperature > MAX_TEMPERATURE {
            self.destroyed = Some("burned up on reentry");
            return;
        }

        // Move, and meet the ground.
        let from = self.position;
        self.position += self.velocity * dt;
        if let Some(ground) = env.ground
            && ground.body == self.body
        {
            self.meet_ground(bodies, ground, from, nose, t, dt);
        }
        self.change_soi(bodies, t + dt);
    }

    /// Heating by `flux` (W/m^2), cooling by radiation and the air.
    pub(super) fn cool(&mut self, rho: f64, flux: f64, dt: f64) {
        let t = self.temperature;
        let radiated =
            EMISSIVITY * STEFAN_BOLTZMANN * RADIATING_AREA * (t.powi(4) - AIR_TEMPERATURE.powi(4));
        let convection =
            (15.0 + 150.0 * (rho / 1.225).sqrt()) * RADIATING_AREA * (t - AIR_TEMPERATURE);
        let change = (flux * HEATED_AREA - radiated - convection) / HEAT_CAPACITY;
        self.temperature = (t + change * dt).max(4.0);
    }

    /// Collision with a body's block world (its frame), landing and crashing.
    fn meet_ground(
        &mut self,
        bodies: &Bodies,
        ground: Ground,
        from: DVec3,
        nose0: DVec3,
        t: f64,
        dt: f64,
    ) {
        let planet = ground.planet;
        let top = planet.layer_radius(planet.layers as f64) + 20.0;
        let nose1 = self.nose();
        if (from - nose0 * COM_HEIGHT).length() > top && self.position.length() > top {
            return;
        }
        let f0 = bodies.frame(ground.body, t);
        let f1 = bodies.frame(ground.body, t + dt);
        let base0 = f0.to_body(from - nose0 * COM_HEIGHT);
        let base1 = f1.to_body(self.position - nose1 * COM_HEIGHT);
        let (base, blocked, axes) = player::collide(planet, ground.world, base0, base1, BOX);
        if !(blocked.down || blocked.up || blocked.u || blocked.v) {
            return;
        }
        let up = base.normalize_or(DVec3::Y);
        let rel = f1.relative_velocity(self.position, self.velocity);
        let down_speed = if blocked.down {
            (-rel.dot(up)).max(0.0)
        } else {
            0.0
        };
        let side_speed = if blocked.u || blocked.v {
            (rel - up * rel.dot(up)).length()
        } else {
            0.0
        };
        if down_speed > CRASH_SPEED || side_speed > CRASH_SPEED {
            self.destroyed = Some("crashed");
            return;
        }
        let mut rel = player::remove_blocked(rel, axes, &blocked);
        if blocked.down {
            // Friction with the ground.
            let along = rel - up * rel.dot(up);
            rel -= along * (1.0 - (-6.0 * dt).exp());
        }
        let nose_body = f1.to_body(nose1);
        let com = base + nose_body * COM_HEIGHT;
        self.position = f1.to_inertial(com);
        self.velocity = f1.inertial_velocity(com, rel);
        if blocked.down {
            self.settle(&f1, base, nose_body, rel);
        }
    }

    /// On the ground and nearly still: lands (upright), or topples if leaning too far.
    fn settle(&mut self, frame: &RotatingFrame, base: DVec3, nose_body: DVec3, rel: DVec3) {
        let thrust = self.throttle * self.acceleration();
        let weight = self.gravity_here();
        if rel.length() > 0.8 || thrust > weight {
            return;
        }
        let up = base.normalize_or(DVec3::Y);
        let tilt = nose_body.dot(up).clamp(-1.0, 1.0).acos();
        if tilt > MAX_LANDING_TILT {
            self.destroyed = Some("tipped over");
            return;
        }
        let attitude_body = frame.rotation.inverse() * self.attitude;
        let upright = DQuat::from_rotation_arc(nose_body, up) * attitude_body;
        self.landed = Some(Landed {
            position: base + up * COM_HEIGHT,
            attitude: upright.normalize(),
        });
        self.hold = None;
        self.spin = DVec3::ZERO;
    }

    fn gravity_here(&self) -> f64 {
        // Close enough for the lift-off / landing decision.
        match self.body {
            Body::Planet => 9.81,
            Body::Moon => crate::space::bodies::MOON_GRAVITY,
            Body::Star => 0.0,
        }
    }

    /// Crossing into a child's sphere of influence, or out of the reference body's.
    pub(super) fn change_soi(&mut self, bodies: &Bodies, t: f64) {
        if self.position.length() > bodies.soi(self.body)
            && let Some(parent) = self.body.parent()
        {
            let (o, ov) = bodies.state(self.body, t);
            self.position += o;
            self.velocity += ov;
            self.body = parent;
            return;
        }
        for &child in self.body.children() {
            let (o, ov) = bodies.state(child, t);
            if (self.position - o).length() < bodies.soi(child) {
                self.position -= o;
                self.velocity -= ov;
                self.body = child;
                return;
            }
        }
    }

    /// Ray from `origin` along unit `dir` (planet body frame) against the rocket's box;
    /// the distance, if it hits within `reach`.
    pub fn ray_hit(
        &self,
        bodies: &Bodies,
        active: Body,
        t: f64,
        origin: DVec3,
        dir: DVec3,
        reach: f64,
    ) -> Option<f64> {
        if self.craft.is_some() {
            return None; // targeted part by part: see `Game::craft_target`
        }
        let frame = bodies.frame(active, t);
        let center = self.world_position(bodies, active, t);
        let to_local = (frame.rotation.inverse() * self.attitude).inverse();
        let o = to_local * (origin - center);
        let d = to_local * dir;
        let lo = DVec3::new(-1.2, -COM_HEIGHT, -1.2);
        let hi = DVec3::new(1.2, HEIGHT - COM_HEIGHT, 1.2);
        let (mut t0, mut t1) = (0.0f64, reach);
        for i in 0..3 {
            if d[i].abs() < 1e-12 {
                if o[i] < lo[i] || o[i] > hi[i] {
                    return None;
                }
                continue;
            }
            let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        space::{SolarSystem, bodies::MOON_GRAVITY, gravity::PlanetGravity},
        world::{block::Block, planet::Face},
    };

    struct Floor(i32);

    impl Voxels for Floor {
        fn block(&self, _: Face, _: i32, _: i32, k: i32) -> Block {
            if k < self.0 { Block::Stone } else { Block::Air }
        }
    }

    fn setup() -> (Planet, Bodies) {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let gravity = PlanetGravity::new(planet.sea_radius, 1_200.0, 9_000.0);
        let bodies = Bodies::new(SolarSystem::new(), gravity, 10_982.0);
        (planet, bodies)
    }

    fn fly(
        vessel: &mut Vessel,
        env: &Env,
        t: &mut f64,
        seconds: f64,
        mut steer: impl FnMut(&mut Vessel, f64) -> Option<DVec3>,
    ) {
        let dt = 1.0 / 30.0;
        for _ in 0..(seconds / dt) as usize {
            let target = steer(vessel, *t);
            vessel.update(env, &Controls::default(), (*t, *t + dt), target);
            *t += dt;
            if vessel.destroyed.is_some() {
                return;
            }
        }
    }

    #[test]
    fn stands_on_the_pad_then_flies_to_orbit() {
        let (planet, bodies) = setup();
        let ground = planet.sea_layer + 5;
        let world = Floor(ground);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        let up = DVec3::new(1.0, 0.05, 0.02).normalize();
        let base = up * planet.layer_radius(ground as f64);
        let mut t = 0.0;
        let mut v = Vessel::on_pad(&bodies, Body::Planet, base, t);
        // Idle on the pad: stays put, upright.
        fly(&mut v, &env, &mut t, 5.0, |_, _| None);
        assert!(v.landed.is_some());
        let height = v.world_position(&bodies, Body::Planet, t).length()
            - planet.layer_radius(ground as f64);
        assert!((height - COM_HEIGHT).abs() < 1e-6, "{height}");

        // A simple gravity turn: up, then tip east with altitude; cut at a 25 km apoapsis.
        v.throttle = 1.0;
        let east_of = |v: &Vessel, t: f64| {
            let frame = bodies.frame(Body::Planet, t);
            let up = v.position.normalize();
            let east = frame.to_inertial(DVec3::Y).cross(up).normalize();
            (up, east)
        };
        let mut max_temperature: f64 = 0.0;
        let mut ascent = |v: &mut Vessel, t: f64| {
            max_temperature = max_temperature.max(v.temperature);
            let altitude = v.position.length() - bodies.radius(Body::Planet);
            let ap = v.conic(&bodies).apoapsis().unwrap_or(f64::MAX) - bodies.radius(Body::Planet);
            if ap > 25_000.0 {
                v.throttle = 0.0;
            }
            let (up, east) = east_of(v, t);
            let angle = (altitude / 14_000.0).clamp(0.0, 1.0) * 1.35;
            Some(up * angle.cos() + east * angle.sin())
        };
        while v.throttle > 0.0 && t < 200.0 && v.destroyed.is_none() {
            fly(&mut v, &env, &mut t, 0.5, &mut ascent);
        }
        let burn_time = t - 5.0;
        assert!(burn_time > 60.0, "at apoapsis after {burn_time} s");
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(v.landed.is_none());
        let conic = v.conic(&bodies);
        let ap = conic.apoapsis().unwrap() - bodies.radius(Body::Planet);
        assert!(ap > 24_000.0, "apoapsis {ap}");
        assert!(
            max_temperature < 800.0,
            "launch heating {max_temperature} K"
        );
        // Coast to apoapsis, then circularize prograde.
        let wait = conic
            .time_to_anomaly(v.position, std::f64::consts::PI)
            .unwrap();
        fly(&mut v, &env, &mut t, wait - 10.0, |_, _| None);
        v.sas_mode = SasMode::Prograde;
        fly(&mut v, &env, &mut t, 120.0, |v, t| {
            let pe = v.conic(&bodies).periapsis() - bodies.radius(Body::Planet);
            v.throttle = if pe > 15_000.0 { 0.0 } else { 1.0 };
            v.sas_direction(&bodies, t, None)
        });
        let pe = v.conic(&bodies).periapsis() - bodies.radius(Body::Planet);
        assert!(pe > 12_000.0, "periapsis {pe}");
        let left = v.delta_v();
        assert!((400.0..1_000.0).contains(&left), "{left} m/s left in orbit");
        assert!(v.can_rail_warp(&bodies));
    }

    fn in_orbit(bodies: &Bodies, altitude: f64) -> Vessel {
        let r = bodies.radius(Body::Planet) + altitude;
        let mut v = Vessel::on_pad(bodies, Body::Planet, DVec3::X * r, 0.0);
        v.landed = None;
        v.position = DVec3::X * r;
        v.velocity = DVec3::Y * (bodies.mu(Body::Planet) / r).sqrt();
        v.attitude = DQuat::from_rotation_arc(DVec3::Y, DVec3::Y);
        v.spin = DVec3::ZERO;
        v.sas_mode = SasMode::Prograde;
        v
    }

    /// A tank is worth what the rocket equation says, and then the engine stops.
    #[test]
    fn the_tank_runs_dry() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let mut v = in_orbit(&bodies, 30_000.0);
        let promised = v.delta_v();
        assert!((1_800.0..2_000.0).contains(&promised), "{promised} m/s");
        v.throttle = 1.0;
        let mut t = 0.0;
        fly(&mut v, &env, &mut t, 400.0, |v, t| {
            v.sas_direction(&bodies, t, None)
        });
        assert_eq!(v.fuel, 0.0);
        assert_eq!(v.throttle, 0.0, "the engine stops");
        assert!(v.flameout);
        let delivered = v.burned.length();
        assert!(
            (delivered - promised).abs() < promised * 0.05,
            "delivered {delivered} of {promised}"
        );
        // Pushing the throttle does nothing any more, and it can warp again.
        v.throttle = 1.0;
        let before = v.burned;
        fly(&mut v, &env, &mut t, 1.0, |_, _| None);
        assert_eq!(v.burned, before);
        assert_eq!(v.throttle, 0.0);
    }

    /// From a low orbit with what is left after getting there, the edge of the home
    /// planet's influence is hours away, not minutes.
    #[test]
    fn leaving_home_takes_hours() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let mut v = in_orbit(&bodies, 30_000.0);
        v.fuel = DRY_MASS * ((700.0 / EXHAUST_SPEED).exp() - 1.0);
        v.throttle = 1.0;
        let mut t = 0.0;
        fly(&mut v, &env, &mut t, 120.0, |v, t| {
            v.sas_direction(&bodies, t, None)
        });
        assert_eq!(v.fuel, 0.0);
        let start = t;
        let (mut body, mut r, mut vel) = (v.body, v.position, v.velocity);
        while body == Body::Planet && t - start < 200_000.0 {
            (body, r, vel, _, _) =
                trajectory::advance(&bodies, body, r, vel, (t, t + 120.0), RAILS_CLEARANCE);
            t += 120.0;
        }
        let hours = (t - start) / 3_600.0;
        println!("left the home planet's influence after {hours:.1} h");
        assert_eq!(body, Body::Star, "it does escape");
        assert!(hours > 2.0, "out after {hours} h");
    }

    /// Falling towards the sun the hull heats up on the way, and it burns up before it gets
    /// there: on rails (time warp) too, which used to carry it to 3 km over the surface.
    #[test]
    fn a_craft_falling_into_the_sun_burns_up_on_the_way() {
        let (_, bodies) = setup();
        let star = bodies.radius(Body::Star);
        let mut v = in_orbit(&bodies, 30_000.0);
        v.body = Body::Star;
        v.position = DVec3::X * star * 20.0;
        // Mostly falling, a little sideways: periapsis inside the star.
        v.velocity = DVec3::new(-200_000.0, 0.0, 20_000.0);
        let mut t = 0.0;
        let mut hottest = 0.0f64;
        while v.destroyed.is_none() && t < 1.0e6 {
            let (reached, _) = v.rails(&bodies, (t, t + 600.0));
            assert!(reached > t || v.destroyed.is_some(), "stuck at {t}");
            t = reached;
            hottest = hottest.max(v.temperature);
        }
        assert_eq!(v.destroyed, Some("burned up near the sun"));
        let r = v.position.length() / star;
        assert!((3.4..3.6).contains(&r), "burned up {r} star radii out");
        assert!(hottest > 1_600.0, "{hottest} K");
        // Far out, sunlight hardly warms it.
        let far = bodies.sunlit_temperature(bodies.system.planet.orbit.semi_major_axis, 1_700.0);
        assert!((200.0..240.0).contains(&far), "{far} K at home's distance");
    }

    #[test]
    fn it_only_refuels_on_the_home_planet() {
        let (planet, bodies) = setup();
        let mut t = 0.0;
        let up = DVec3::X * planet.layer_radius(planet.sea_layer as f64);
        let world = Floor(planet.sea_layer);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        let mut home = Vessel::on_pad(&bodies, Body::Planet, up, t);
        home.fuel = 0.0;
        fly(&mut home, &env, &mut t, 30.0, |_, _| None);
        assert_eq!(home.fuel, FUEL_CAPACITY);

        let moon_up = DVec3::X * bodies.radius(Body::Moon);
        let moon_env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let mut there = Vessel::on_pad(&bodies, Body::Moon, moon_up, t);
        there.fuel = 100.0;
        fly(&mut there, &moon_env, &mut t, 30.0, |_, _| None);
        assert_eq!(there.fuel, 100.0);
    }

    /// Coming back from the moon fast (950 m/s at the top of the air): a shallow entry
    /// survives the heat, a steep one burns up.
    #[test]
    fn reentry_heats_the_hull() {
        let (planet, bodies) = setup();
        let world = Floor(planet.sea_layer);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        let entry = |angle_deg: f64| {
            let mut t = 0.0;
            let r = bodies.gravity.atmosphere_top() + 10.0;
            let mut v = Vessel::on_pad(&bodies, Body::Planet, DVec3::X * r, t);
            v.landed = None;
            v.sas = false;
            v.spin = DVec3::ZERO;
            v.position = DVec3::X * r;
            let a = angle_deg.to_radians();
            v.velocity = (DVec3::NEG_Z * a.cos() - DVec3::X * a.sin()) * 950.0;
            // Nose first (fins keep it that way).
            v.attitude = DQuat::from_rotation_arc(DVec3::Y, v.velocity.normalize());
            let mut peak: f64 = 0.0;
            fly(&mut v, &env, &mut t, 400.0, |v, _| {
                peak = peak.max(v.temperature);
                None
            });
            (peak, v.destroyed, v.velocity.length())
        };
        let (shallow, destroyed, _) = entry(12.0);
        assert!(
            destroyed.is_none_or(|d| d == "crashed"),
            "{destroyed:?} at {shallow} K"
        );
        assert!(
            (600.0..MAX_TEMPERATURE).contains(&shallow),
            "shallow entry peaked at {shallow} K"
        );
        let (steep, destroyed, _) = entry(40.0);
        assert_eq!(
            destroyed,
            Some("burned up on reentry"),
            "steep entry peaked at {steep} K"
        );
    }

    /// Hovering down onto the moon's blocks (its own grid, 11 km) and landing upright.
    #[test]
    fn lands_on_the_moon() {
        let (_, bodies) = setup();
        let moon = Planet::new(DVec3::ZERO, 512, 512, 2);
        let floor = moon.sea_layer + 3;
        let world = Floor(floor);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Moon,
                planet: &moon,
                world: &world,
            }),
            drag: true,
        };
        let mut t = 0.0;
        let up_body = DVec3::new(0.3, 0.8, 0.5).normalize();
        let surface = moon.layer_radius(floor as f64);
        let mut v = Vessel::on_pad(&bodies, Body::Moon, up_body * (surface + 40.0), t);
        v.landed = None;
        // Hover down gently: throttle against gravity, settling at ~3 m/s.
        fly(&mut v, &env, &mut t, 90.0, |v, t| {
            let frame = bodies.frame(Body::Moon, t);
            let rel = frame.relative_velocity(v.position, v.velocity);
            let up = frame.to_body(v.position).normalize();
            let sink = -rel.dot(up);
            v.throttle = if v.landed.is_some() {
                0.0
            } else {
                (v.mass() * MOON_GRAVITY / THRUST * (1.0 + (sink - 3.0) * 0.5)).clamp(0.0, 1.0)
            };
            Some(v.position.normalize())
        });
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(v.landed.is_some(), "not landed");
        let height = v.world_position(&bodies, Body::Moon, t).length() - surface;
        assert!((height - COM_HEIGHT).abs() < 1e-3, "{height}");
    }
}
