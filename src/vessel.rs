//! Craft: for now one test rocket with infinite fuel, flying under patched conics.
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
//! - Heating: a Sutton-Graves-like flux, sqrt(air density) x speed^3 (above ~300 m/s), heats
//!   the hull; it cools by radiation and convection. Too hot and it burns up.
//! - Ground: on the planet the block world stops it (the player's collision sweep, an upright
//!   box at the engine), on the moon its surface mesh. Hitting it faster than
//!   `CRASH_SPEED`, or settling more than `MAX_LANDING_TILT` off vertical, destroys it.
//!   Otherwise it lands: it then rides along with the turning ground and stands upright.
//!
//! # Time warp
//! On rails (`rails`) it follows its conic exactly (`trajectory::advance`), until it would
//! enter an atmosphere or come within `RAILS_CLEARANCE` of a surface.

use glam::{DMat3, DQuat, DVec3};

use crate::{
    player::{self, Voxels},
    render::MeshHandle,
    space::{
        RotatingFrame,
        bodies::{Bodies, Body},
        conic::Conic,
        trajectory,
    },
    world::{noise::smoothstep, planet::Planet},
};

/// Mass (kg) and engine thrust (N): a thrust-to-weight ratio of 2 at the planet's surface.
pub const MASS: f64 = 3_000.0;
pub const THRUST: f64 = 60_000.0;
/// From the engine bell to the nose tip (m), and the centre of mass above the bell.
pub const HEIGHT: f64 = 8.0;
pub const COM_HEIGHT: f64 = 3.0;
/// Collision box: half width, and height from the bell up (the fins and tank bottom).
const BOX: (f64, f64) = (1.0, 4.0);
const STEP: f64 = 1.0 / 120.0;
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
const HEAT_FLUX: f64 = 0.05;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl Controls {
    fn any(&self) -> bool {
        self.pitch != 0.0 || self.yaw != 0.0 || self.roll != 0.0
    }
}

/// Resting on a body: where (centre of mass) and how, in the body's rotating frame.
#[derive(Clone, Copy, Debug)]
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

pub struct Vessel {
    pub body: Body,
    /// Centre of mass, relative to `body`, inertial axes.
    pub position: DVec3,
    pub velocity: DVec3,
    /// Vessel axes -> inertial axes.
    pub attitude: DQuat,
    /// Angular velocity (rad/s, inertial axes).
    pub spin: DVec3,
    pub throttle: f64,
    pub sas: bool,
    pub sas_mode: SasMode,
    /// The attitude stability mode holds (captured when steering stops).
    hold: Option<DQuat>,
    pub landed: Option<Landed>,
    /// Hull temperature (K), and the heat flux on it now (W/m^2).
    pub temperature: f64,
    pub heating: f64,
    /// Dynamic pressure (Pa).
    pub pressure: f64,
    /// Why it was destroyed, once it is.
    pub destroyed: Option<&'static str>,
    /// Velocity change the engine delivered since the game last took it (inertial, m/s):
    /// maneuver nodes count down with it.
    pub burned: DVec3,
    pub mesh: Option<MeshHandle>,
}

impl Vessel {
    /// A vessel standing upright on `body` with its engine at `base` (that body's frame).
    pub fn on_pad(bodies: &Bodies, body: Body, base: DVec3, t: f64) -> Self {
        let up = base.normalize_or(DVec3::Y);
        let east = DVec3::Y
            .cross(up)
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let attitude = DQuat::from_mat3(&DMat3::from_cols(east, up, east.cross(up)));
        let mut vessel = Self {
            body,
            position: DVec3::ZERO,
            velocity: DVec3::ZERO,
            attitude: DQuat::IDENTITY,
            spin: DVec3::ZERO,
            throttle: 0.0,
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
        };
        vessel.sync_landed(bodies, t);
        vessel
    }

    pub fn nose(&self) -> DVec3 {
        self.attitude * DVec3::Y
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
        bodies.view(active, self.body, self.position, t)
    }

    /// The air around it: density (kg/m^3) and the velocity relative to that air (inertial).
    fn air(&self, bodies: &Bodies, t: f64) -> (f64, DVec3) {
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
        let r = self.position.length();
        let clear = match bodies.atmosphere_top(self.body) {
            Some(top) => r > top,
            None => r > bodies.radius(self.body) + RAILS_CLEARANCE,
        };
        self.throttle == 0.0 && clear
    }

    /// Follows the conic from `t0` to `t1`; returns the time reached (earlier if it had to
    /// stop for an atmosphere or a surface) and whether it stopped.
    pub fn rails(&mut self, bodies: &Bodies, (t0, t1): (f64, f64)) -> (f64, bool) {
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
        (t, stopped)
    }

    /// Physics from `t0` to `t1`. `target` is SAS's direction (from `sas_direction`).
    pub fn update(
        &mut self,
        env: &Env,
        controls: &Controls,
        (t0, t1): (f64, f64),
        target: Option<DVec3>,
    ) {
        let steps = ((t1 - t0) / STEP).ceil().clamp(1.0, 1200.0) as usize;
        let dt = (t1 - t0) / steps as f64;
        for i in 0..steps {
            if self.destroyed.is_some() {
                return;
            }
            self.step(env, controls, t0 + dt * i as f64, dt, target);
        }
    }

    fn step(&mut self, env: &Env, controls: &Controls, t: f64, dt: f64, target: Option<DVec3>) {
        let bodies = env.bodies;
        let gravity = bodies.gravity(self.body, self.position);
        let thrust = self.throttle.clamp(0.0, 1.0) * THRUST / MASS;
        if self.landed.is_some() {
            if thrust > gravity.length() * 1.02 {
                self.landed = None; // lift-off
            } else {
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
        if rho > 0.0 && speed > 0.0 {
            let side = 1.0 - (nose.dot(air / speed)).powi(2);
            let cda = CDA_NOSE + (CDA_SIDE - CDA_NOSE) * side.max(0.0);
            let keep = (-0.5 * rho * cda / MASS * speed * dt).exp();
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
    fn cool(&mut self, rho: f64, flux: f64, dt: f64) {
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
        let thrust = self.throttle * THRUST / MASS;
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
    fn change_soi(&mut self, bodies: &Bodies, t: f64) {
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
        assert!(v.can_rail_warp(&bodies));
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
                (MASS * MOON_GRAVITY / THRUST * (1.0 + (sink - 3.0) * 0.5)).clamp(0.0, 1.0)
            };
            Some(v.position.normalize())
        });
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(v.landed.is_some(), "not landed");
        let height = v.world_position(&bodies, Body::Moon, t).length() - surface;
        assert!((height - COM_HEIGHT).abs() < 1e-3, "{height}");
    }
}
