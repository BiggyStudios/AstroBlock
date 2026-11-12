//! The player: a physics body that feels like a Minecraft player but lives in a Kepler
//! universe.
//!
//! # Frames
//! The player's state is a position and velocity in the planet-centred **inertial** frame
//! (`RotatingFrame` calls it PCI) - the frame orbits live in. Gravity and inertia act there,
//! so a jump, a fall through a tunnel or an orbit all follow real Newtonian mechanics, and
//! anything the player leaves (a rocket, later) hands over its velocity naturally.
//!
//! The terrain, however, spins with the planet. Each physics step therefore:
//! 1. turns the controls (walking, jumping, flying) into a change of the velocity *relative
//!    to the ground* (body frame),
//! 2. integrates gravity and air drag (drag acts on the velocity relative to the co-rotating
//!    air) in the inertial frame,
//! 3. moves inertially and resolves collisions in the body frame, between where the player
//!    was on the ground at the start of the step and where it would be at the end. A
//!    collision removes the velocity relative to the ground along the blocked axis.
//!
//! Standing still on the ground thus means co-rotating with the planet (~145 m/s at the
//! equator), and the centrifugal and Coriolis effects come out on their own.
//!
//! # Collision
//! Minecraft-style axis-separated sweeps of an AABB, done in the curved block grid's own
//! (u, v, k) coordinates, where blocks are unit cubes: first radially (k), then along u,
//! then v. The box's size is converted from meters with the local cell size.
//!
//! # Time warp
//! Up to `MAX_PHYSICS_WARP` the physics simply runs faster. Beyond it the player is "on
//! rails" like a KSP vessel: landed, it stays put on the turning planet; in orbit above the
//! atmosphere, it follows its exact Kepler orbit. Anything else (flying in the atmosphere,
//! inside the planet, escaping) can't warp that fast.

pub mod avatar;
pub mod interact;
pub mod viewmodel;

use std::str::FromStr;

use glam::DVec3;

use crate::{
    space::{RotatingFrame, gravity::PlanetGravity, orbit::Orbit},
    world::{
        block::Block,
        planet::{Face, Planet},
    },
};

pub const HALF_WIDTH: f64 = 0.3;
pub const HEIGHT: f64 = 1.8;
pub const EYE_HEIGHT: f64 = 1.62;
const WALK_SPEED: f64 = 4.3;
const SPRINT_SPEED: f64 = 5.6;
/// Character feel. Under real gravity a 1.25-block jump takes a full second (Minecraft's take
/// 0.6 s), which feels floaty. So while the player walks and hops (ground within
/// `ASSIST_HEIGHT` below, moving slower than `ASSIST_SPEED` vertically), gravity pulls
/// `CHARACTER_GRAVITY` times harder - an assist most games use. Anything beyond a hop (long
/// falls, launches, orbits, being thrown off a vehicle) is pure physics. On low-gravity
/// worlds the assist scales with local gravity, so jumps there are still higher and slower.
const CHARACTER_GRAVITY: f64 = 2.4;
const ASSIST_HEIGHT: f64 = 3.0;
const ASSIST_SPEED: f64 = 10.0;
/// Jump take-off speed: 1.25 blocks high under 9.81 * `CHARACTER_GRAVITY` (0.65 s in the air).
const JUMP_SPEED: f64 = 7.67;
const FLY_SPEED: f64 = 11.0;
const GROUND_ACCEL: f64 = 50.0;
const AIR_ACCEL: f64 = 10.0;
/// Slowing down in the air without input (at walking speeds).
const AIR_BRAKE: f64 = 4.0;
const FLY_ACCEL: f64 = 30.0;
/// Drag: mass (kg) and drag coefficient times area (m^2) of a person.
const MASS: f64 = 80.0;
const DRAG_AREA: f64 = 0.7;
/// Physics step (s of game time).
const STEP: f64 = 1.0 / 120.0;
/// Faster time warp than this puts the player on rails.
pub const MAX_PHYSICS_WARP: f64 = 10.0;
/// Relative speed below which a grounded player counts as landed (for rails warp).
const LANDED_SPEED: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GameMode {
    /// Physics, gravity, collisions; breaking blocks takes time.
    Survival,
    /// Like survival, but instant breaking and flight (double-tap jump).
    Creative,
    /// Free camera: no gravity, no collisions.
    Spectator,
}

impl FromStr for GameMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s.to_ascii_lowercase().as_str() {
            "survival" | "s" | "0" => Ok(GameMode::Survival),
            "creative" | "c" | "1" => Ok(GameMode::Creative),
            "spectator" | "sp" | "3" => Ok(GameMode::Spectator),
            _ => Err(()),
        }
    }
}

impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Spectator => "spectator",
        }
    }
}

/// One frame of player controls, already turned into body-frame directions.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    /// Desired horizontal direction (unit or zero), from WASD and the camera's heading.
    pub walk: DVec3,
    /// Spectator movement: the full 3D direction (camera-relative WASD + up/down).
    pub fly_dir: DVec3,
    /// +1 jump / fly up, -1 sneak / fly down.
    pub vertical: f64,
    pub jump: bool,
    pub sprint: bool,
    /// Spectator speed (m/s).
    pub spectator_speed: f64,
}

/// What the player needs to know about the blocks around it.
pub trait Voxels {
    /// The block in cell (u, v, k): u and v in the grid of layer k's shell, possibly beyond
    /// the face's edge.
    fn block(&self, face: Face, u: i32, v: i32, k: i32) -> Block;

    /// A tree (or cactus) block the world grew and nobody has touched: vehicles go through
    /// these and knock them down. Placed logs and leaves are as solid as anything.
    fn grown(&self, _face: Face, _u: i32, _v: i32, _k: i32) -> bool {
        false
    }
}

/// Everything physics needs besides the world.
pub struct Physics<'a> {
    pub planet: &'a Planet,
    pub gravity: &'a PlanetGravity,
    pub drag: bool,
}

/// The player in a saved game.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SavedPlayer {
    mode: GameMode,
    position: DVec3,
    velocity: DVec3,
    on_ground: bool,
    flying: bool,
    in_water: bool,
    fall_peak: Option<f64>,
}

pub struct Player {
    pub mode: GameMode,
    /// Feet position and velocity in the planet-centred inertial frame.
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    /// Creative flight.
    pub flying: bool,
    pub in_water: bool,
    /// Meters fallen at the biggest landing since the game last read it (fall damage).
    /// Landing in water or while flying doesn't count.
    pub fallen: f64,
    /// Highest point (distance from the planet centre) since last standing on something.
    fall_peak: Option<f64>,
}

/// Which way a collision sweep was stopped.
#[derive(Default)]
pub struct Blocked {
    pub down: bool,
    pub up: bool,
    pub u: bool,
    pub v: bool,
}

impl Player {
    /// A player standing still on the ground at body-frame position `feet`.
    pub fn new(mode: GameMode, feet: DVec3, frame: &RotatingFrame) -> Self {
        Self {
            mode,
            position: frame.to_inertial(feet),
            velocity: frame.inertial_velocity(feet, DVec3::ZERO),
            on_ground: false,
            flying: false,
            in_water: false,
            fallen: 0.0,
            fall_peak: None,
        }
    }

    /// Fall damage (half-hearts) for a fall of `meters`: Minecraft's one per block beyond 3.
    pub fn fall_damage(meters: f64) -> f32 {
        (meters - 3.05).ceil().max(0.0) as f32
    }

    /// Feet position in the body (world) frame, relative to the planet's centre.
    pub fn body_position(&self, frame: &RotatingFrame) -> DVec3 {
        frame.to_body(self.position)
    }

    /// Velocity over the ground (body frame).
    pub fn relative_velocity(&self, frame: &RotatingFrame) -> DVec3 {
        frame.relative_velocity(self.position, self.velocity)
    }

    /// What a saved game keeps of the player.
    pub fn saved(&self) -> SavedPlayer {
        SavedPlayer {
            mode: self.mode,
            position: self.position,
            velocity: self.velocity,
            on_ground: self.on_ground,
            flying: self.flying,
            in_water: self.in_water,
            fall_peak: self.fall_peak,
        }
    }

    pub fn restore(&mut self, saved: &SavedPlayer) {
        self.mode = saved.mode;
        self.position = saved.position;
        self.velocity = saved.velocity;
        self.on_ground = saved.on_ground;
        self.flying = saved.flying;
        self.in_water = saved.in_water;
        self.fall_peak = saved.fall_peak;
        self.fallen = 0.0;
    }

    /// Moves the player to body position `feet`, at rest relative to the ground.
    pub fn teleport(&mut self, feet: DVec3, frame: &RotatingFrame) {
        self.position = frame.to_inertial(feet);
        self.velocity = frame.inertial_velocity(feet, DVec3::ZERO);
        self.on_ground = false;
        self.fall_peak = None;
    }

    /// The clock has jumped: still in the same place over the same ground, moving the same.
    pub fn carried_through_time(&mut self, from: &RotatingFrame, to: &RotatingFrame) {
        let body = from.to_body(self.position);
        let relative = from.relative_velocity(self.position, self.velocity);
        self.position = to.to_inertial(body);
        self.velocity = to.inertial_velocity(body, relative);
    }

    pub fn set_mode(&mut self, mode: GameMode) {
        self.mode = mode;
        self.flying = mode == GameMode::Creative && self.flying;
    }

    /// Creative: double-tapping jump toggles flight.
    pub fn toggle_flight(&mut self) {
        if self.mode == GameMode::Creative {
            self.flying = !self.flying;
        }
    }

    /// The orbit the player is on (inertial frame), if it's elliptic.
    pub fn orbit(&self, physics: &Physics, t: f64) -> Option<Orbit> {
        Orbit::from_state(physics.gravity.mu, self.position, self.velocity, t)
    }

    /// Whether time can warp faster than `MAX_PHYSICS_WARP`: landed, spectating, or in an
    /// orbit that stays above the atmosphere.
    pub fn can_rail_warp(&self, physics: &Physics, frame: &RotatingFrame, t: f64) -> bool {
        if self.mode == GameMode::Spectator || self.landed(frame) {
            return true;
        }
        let top = physics.gravity.atmosphere_top();
        self.position.length() > top && self.orbit(physics, t).is_some_and(|o| o.apsides().0 > top)
    }

    fn landed(&self, frame: &RotatingFrame) -> bool {
        self.on_ground && self.relative_velocity(frame).length() < LANDED_SPEED
    }

    /// Advances the player from game time `t0` to `t1`. `real_dt` is the wall-clock frame
    /// time (spectator movement doesn't speed up with time warp). `frame_at` gives the
    /// planet's rotating frame at any time.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        physics: &Physics,
        world: &dyn Voxels,
        input: &PlayerInput,
        (t0, t1): (f64, f64),
        real_dt: f64,
        rails: bool,
        frame_at: &dyn Fn(f64) -> RotatingFrame,
    ) {
        if self.mode == GameMode::Spectator {
            self.spectate(input, real_dt, &frame_at(t0), &frame_at(t1));
            return;
        }
        if rails {
            self.rails(physics, &frame_at(t0), &frame_at(t1), (t0, t1));
            return;
        }
        let steps = ((t1 - t0) / STEP).ceil().clamp(1.0, 240.0) as usize;
        let dt = (t1 - t0) / steps as f64;
        let mut frame = frame_at(t0);
        for i in 0..steps {
            let next = frame_at(t0 + dt * (i + 1) as f64);
            self.step(physics, world, input, dt, &frame, &next);
            frame = next;
        }
    }

    fn spectate(&mut self, input: &PlayerInput, dt: f64, f0: &RotatingFrame, f1: &RotatingFrame) {
        let relative = input.fly_dir * input.spectator_speed;
        let feet = f0.to_body(self.position) + relative * dt;
        self.position = f1.to_inertial(feet);
        self.velocity = f1.inertial_velocity(feet, relative);
        self.on_ground = false;
    }

    fn rails(
        &mut self,
        physics: &Physics,
        f0: &RotatingFrame,
        f1: &RotatingFrame,
        (t0, t1): (f64, f64),
    ) {
        if self.landed(f0) {
            let feet = f0.to_body(self.position);
            self.position = f1.to_inertial(feet);
            self.velocity = f1.inertial_velocity(feet, DVec3::ZERO);
        } else if let Some(orbit) = self.orbit(physics, t0) {
            let (p, v) = orbit.state_at(physics.gravity.mu, t1);
            self.position = p;
            self.velocity = v;
        }
    }

    fn step(
        &mut self,
        physics: &Physics,
        world: &dyn Voxels,
        input: &PlayerInput,
        dt: f64,
        f0: &RotatingFrame,
        f1: &RotatingFrame,
    ) {
        let planet = physics.planet;
        let feet = f0.to_body(self.position);
        let up = feet.normalize_or(DVec3::Y);
        let mut relative = f0.relative_velocity(self.position, self.velocity);

        // Water: is the middle of the body submerged?
        let middle = feet + up * (HEIGHT * 0.5);
        self.in_water = planet
            .cell_at(middle)
            .is_some_and(|c| world.block(c.face, c.u, c.v, c.k) == Block::Water);

        // Walking and hopping get Minecraft-like assists (see `CHARACTER_GRAVITY`); anything
        // else is pure physics.
        let near_ground = self.on_ground || ground_below(planet, world, feet, up);

        // 1. Controls change the velocity relative to the ground.
        let vertical = relative.dot(up);
        let horizontal = relative - up * vertical;
        if self.flying {
            let target = (input.walk + up * input.vertical) * FLY_SPEED * sprint(input);
            relative = approach(relative, target, FLY_ACCEL * dt);
        } else {
            let speed = if input.sprint {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            let target = input.walk * speed;
            let accel = if self.on_ground {
                GROUND_ACCEL
            } else if self.in_water {
                GROUND_ACCEL * 0.25
            } else {
                AIR_ACCEL
            };
            // In the air you can steer at walking speeds near the ground or in thick air (a
            // skydiver steers with the wind), and ease to a stop when hopping without input.
            // Faster than that (thrown, jumped off a vehicle) or in vacuum: keep momentum.
            let thick_air = physics.gravity.air_density(feet.length()) > 0.05;
            let assisted = horizontal.length() < ASSIST_SPEED && (near_ground || thick_air);
            let horizontal =
                if self.on_ground || self.in_water || (assisted && input.walk != DVec3::ZERO) {
                    approach(horizontal, target, accel * dt)
                } else if assisted && near_ground {
                    approach(horizontal, DVec3::ZERO, AIR_BRAKE * dt)
                } else {
                    horizontal
                };
            let mut vertical = vertical;
            if input.jump && self.on_ground && !self.in_water {
                vertical = vertical.max(JUMP_SPEED);
                self.on_ground = false;
            }
            if self.in_water {
                // Swimming: buoyancy nearly cancels gravity, water is thick.
                let swim = if input.jump { 2.5 } else { -0.6 };
                vertical += (swim - vertical) * (1.0 - (-3.0 * dt).exp());
            }
            relative = horizontal + up * vertical;
        }
        self.velocity = f0.inertial_velocity(feet, relative);

        // 2. Forces, in the inertial frame.
        if !self.flying && !self.in_water {
            let gravity = physics.gravity.acceleration(self.position);
            let hopping = near_ground && relative.dot(up).abs() < ASSIST_SPEED;
            let scale = if hopping { CHARACTER_GRAVITY } else { 1.0 };
            self.velocity += gravity * (scale * dt);
        }
        if physics.drag {
            // Drag on the velocity relative to the air, which turns with the planet.
            let rho = physics.gravity.air_density(feet.length());
            let speed = relative.length();
            let decel = 0.5 * rho * DRAG_AREA / MASS * speed;
            // Exact for this step's speed, so huge speeds can't overshoot into reverse.
            let keep = (-decel * dt).exp();
            self.velocity -= f0.to_inertial(relative) * (1.0 - keep);
        }

        // 3. Move inertially; collide on the turning ground.
        let target = f1.to_body(self.position + self.velocity * dt);
        let (feet, blocked, axes) = collide(planet, world, feet, target, (HALF_WIDTH, HEIGHT));
        self.position = f1.to_inertial(feet);
        let relative = f1.relative_velocity(self.position, self.velocity);
        // Fall height, Minecraft style: from the highest point since last standing.
        let r = feet.length();
        if blocked.down && !self.on_ground && !self.in_water && !self.flying {
            self.fallen = self.fallen.max(self.fall_peak.unwrap_or(r) - r);
        }
        self.fall_peak = if blocked.down || self.in_water || self.flying {
            None
        } else {
            Some(self.fall_peak.map_or(r, |p| p.max(r)))
        };
        let relative = remove_blocked(relative, axes, &blocked);
        self.velocity = f1.inertial_velocity(feet, relative);
        self.on_ground = blocked.down;
        if self.flying && self.on_ground {
            self.flying = false;
        }
    }
}

fn sprint(input: &PlayerInput) -> f64 {
    if input.sprint { 2.0 } else { 1.0 }
}

/// Removes the velocity components along the blocked grid axes. Away from face centres the
/// grid's u and v axes aren't perpendicular, so the velocity is split into grid components
/// (not projected onto each axis), or a blocked axis would keep a bit of velocity into the
/// wall.
pub fn remove_blocked(velocity: DVec3, [a, b, up]: [DVec3; 3], blocked: &Blocked) -> DVec3 {
    let mut radial = velocity.dot(up);
    if (blocked.down && radial < 0.0) || (blocked.up && radial > 0.0) {
        radial = 0.0;
    }
    let tangent = velocity - up * velocity.dot(up);
    let c = a.dot(b);
    let (ta, tb) = (tangent.dot(a), tangent.dot(b));
    let det = (1.0 - c * c).max(1e-6);
    let (mut x, mut y) = ((ta - c * tb) / det, (tb - c * ta) / det);
    // a and b aren't exactly tangent; keep whatever they don't capture.
    let rest = tangent - a * x - b * y;
    if blocked.u {
        x = 0.0;
    }
    if blocked.v {
        y = 0.0;
    }
    a * x + b * y + rest + up * radial
}

/// Whether there's solid ground within `ASSIST_HEIGHT` below the feet (body frame).
fn ground_below(planet: &Planet, world: &dyn Voxels, feet: DVec3, up: DVec3) -> bool {
    (0..ASSIST_HEIGHT as i32).any(|d| {
        planet
            .cell_at(feet - up * (d as f64 + 0.5))
            .is_some_and(|c| world.block(c.face, c.u, c.v, c.k).collides())
    })
}

/// Moves `v` towards `target` by at most `max_change`.
pub fn approach(v: DVec3, target: DVec3, max_change: f64) -> DVec3 {
    let d = target - v;
    let len = d.length();
    if len <= max_change {
        target
    } else {
        v + d * (max_change / len)
    }
}

/// Sweeps a box (`half_width` across, `height` up from its feet) from body position `from`
/// towards `to` through the block grid. Returns the reached position, which axes were
/// blocked, and the (u, v, k) directions.
pub fn collide(
    planet: &Planet,
    world: &dyn Voxels,
    from: DVec3,
    to: DVec3,
    (half_width, height): (f64, f64),
) -> (DVec3, Blocked, [DVec3; 3]) {
    let (face, u0, v0, k0) = planet.locate(from);
    let n = planet.res_at((k0.floor() as i32).clamp(0, planet.layers - 1));
    let (u1, v1, k1) = planet.locate_on_face(face, to, n);
    let r = from.length().max(1.0);
    let du_dir = planet.dir(face, u0 + 0.5, v0, n) - planet.dir(face, u0 - 0.5, v0, n);
    let dv_dir = planet.dir(face, u0, v0 + 0.5, n) - planet.dir(face, u0, v0 - 0.5, n);
    let axes = [
        du_dir.normalize_or(DVec3::X),
        dv_dir.normalize_or(DVec3::Z),
        from.normalize_or(DVec3::Y),
    ];
    // Box half extents in grid units.
    let hu = half_width / (du_dir.length() * r).max(1e-3);
    let hv = half_width / (dv_dir.length() * r).max(1e-3);

    // Is grid cell (u, v, k) (in the start grid's units) solid?
    let solid = |iu: i32, iv: i32, ik: i32| -> bool {
        if ik < 0 {
            return true;
        }
        if ik >= planet.layers {
            return false;
        }
        let nk = planet.res_at(ik);
        let (cu, cv) = if nk == n {
            (iu, iv)
        } else {
            let scale = nk as f64 / n as f64;
            (
                ((iu as f64 + 0.5) * scale).floor() as i32,
                ((iv as f64 + 0.5) * scale).floor() as i32,
            )
        };
        world.block(face, cu, cv, ik).collides()
    };
    // Contact tolerance, in grid units (~1 mm). The box's size in grid units changes a little
    // from step to step (blocks aren't all the same size), so a box resting against a wall
    // can end up overlapping it by a hair. A block overlapped by less than SKIN still counts
    // as touching: it blocks movement and pushes the box back out. Blocks overlapped by more
    // (e.g. one placed inside the player) are ignored, so you can walk out of them.
    const SKIN: f64 = 1e-3;
    // Cells overlapped (by more than SKIN) by [lo, hi] along one axis.
    let span = |lo: f64, hi: f64| (lo + SKIN).floor() as i32..=(hi - SKIN).floor() as i32;
    let any_solid = |us: std::ops::RangeInclusive<i32>,
                     vs: std::ops::RangeInclusive<i32>,
                     ks: std::ops::RangeInclusive<i32>| {
        ks.clone()
            .any(|k| vs.clone().any(|v| us.clone().any(|u| solid(u, v, k))))
    };
    // Moves the box [pos - below, pos + above] by `delta` along one axis; `cells(c)` says
    // whether layer c of that axis is solid where the box overlaps it. Returns the new
    // position and whether the box was blocked.
    let sweep = |pos: f64,
                 delta: f64,
                 (below, above): (f64, f64),
                 cells: &dyn Fn(i32) -> bool|
     -> (f64, bool) {
        if delta > 0.0 {
            let edge = pos + above;
            let (first, last) = (
                (edge - SKIN).ceil() as i32,
                (edge + delta).ceil() as i32 - 1,
            );
            if let Some(c) = (first..=last).find(|&c| cells(c)) {
                let stop = c as f64 - above;
                if pos + delta >= stop {
                    return (stop, true);
                }
            }
        } else if delta < 0.0 {
            let edge = pos - below;
            let (first, last) = (
                (edge + delta).floor() as i32,
                (edge + SKIN).floor() as i32 - 1,
            );
            if let Some(c) = (first..=last).rev().find(|&c| cells(c)) {
                let stop = (c + 1) as f64 + below;
                if pos + delta <= stop {
                    return (stop, true);
                }
            }
        }
        (pos + delta, false)
    };

    let mut blocked = Blocked::default();
    let (mut u, mut v) = (u0, v0);

    // Radial first (landing, ceilings), then sideways one axis at a time, like Minecraft.
    let dk = k1 - k0;
    let (k, hit_k) = sweep(k0, dk, (0.0, height), &|c| {
        any_solid(span(u - hu, u + hu), span(v - hv, v + hv), c..=c)
    });
    blocked.down = hit_k && dk < 0.0;
    blocked.up = hit_k && dk > 0.0;
    let ks = span(k, k + height);
    let (nu, hit_u) = sweep(u, u1 - u0, (hu, hu), &|c| {
        any_solid(c..=c, span(v - hv, v + hv), ks.clone())
    });
    u = nu;
    blocked.u = hit_u;
    let (nv, hit_v) = sweep(v, v1 - v0, (hv, hv), &|c| {
        any_solid(span(u - hu, u + hu), c..=c, ks.clone())
    });
    v = nv;
    blocked.v = hit_v;

    let feet = planet.dir(face, u, v, n) * planet.layer_radius(k);
    (feet, blocked, axes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::SolarSystem;

    /// Flat-ish test world: solid below a given layer.
    struct Ground(i32);

    impl Voxels for Ground {
        fn block(&self, _: Face, _: i32, _: i32, k: i32) -> Block {
            if k < self.0 { Block::Stone } else { Block::Air }
        }
    }

    /// Ground below `ground`, plus 3-block-high walls: everything at u >= `wall_u`, and
    /// optionally single pillars.
    struct Room {
        face: Face,
        ground: i32,
        wall_u: i32,
        pillars: Vec<(i32, i32)>,
    }

    impl Voxels for Room {
        fn block(&self, face: Face, u: i32, v: i32, k: i32) -> Block {
            let wall = face == self.face
                && k < self.ground + 3
                && (u >= self.wall_u || self.pillars.contains(&(u, v)));
            if k < self.ground || wall {
                Block::Stone
            } else {
                Block::Air
            }
        }
    }

    /// The player's box in grid units: (face, u range, v range, k).
    fn grid_box(planet: &Planet, feet: DVec3) -> (Face, (f64, f64), (f64, f64), f64) {
        let (face, u, v, k) = planet.locate(feet);
        let n = planet.face_blocks;
        let r = feet.length();
        let cu = (planet.dir(face, u + 0.5, v, n) - planet.dir(face, u - 0.5, v, n)).length() * r;
        let cv = (planet.dir(face, u, v + 0.5, n) - planet.dir(face, u, v - 0.5, n)).length() * r;
        let (hu, hv) = (HALF_WIDTH / cu, HALF_WIDTH / cv);
        (face, (u - hu, u + hu), (v - hv, v + hv), k)
    }

    /// Walking diagonally into a wall slides along it and never enters it (this used to
    /// leak: the box's size in grid units drifts while sliding).
    #[test]
    fn walls_hold_when_walking_diagonally() {
        let (planet, gravity, system) = setup();
        let physics = Physics {
            planet: &planet,
            gravity: &gravity,
            drag: true,
        };
        let frame = |t: f64| system.planet_frame(t);
        let ground = planet.sea_layer + 10;
        // Away from the face centre, where block sizes change from one block to the next.
        let up = DVec3::new(0.4, 0.3, 0.8).normalize();
        let start = up * planet.layer_radius(ground as f64);
        let (face, u0, v0, _) = planet.locate(start);
        let n = planet.face_blocks;
        let axis = |du: f64, dv: f64| {
            (planet.dir(face, u0 + du, v0 + dv, n) - planet.dir(face, u0, v0, n)).normalize()
        };
        for (dir_u, dir_v, pillars) in [
            (1.0f64, 1.0f64, vec![]),
            (1.0, -1.0, vec![]),
            (1.0, 0.2, vec![]),
            // Two pillars touching diagonally: no squeezing between them.
            (
                1.0,
                1.0,
                vec![
                    (u0 as i32 + 2, v0 as i32 + 1),
                    (u0 as i32 + 1, v0 as i32 + 2),
                ],
            ),
        ] {
            let room = Room {
                face,
                ground,
                wall_u: u0 as i32 + 3,
                pillars,
            };
            let mut player = Player::new(GameMode::Survival, start, &frame(0.0));
            let mut t = 0.0;
            for i in 0..1200 {
                let walk = (axis(dir_u, 0.0) * dir_u.abs()
                    + axis(0.0, dir_v.signum()) * dir_v.abs())
                .normalize();
                let input = PlayerInput {
                    walk,
                    sprint: true,
                    jump: i % 90 == 0,
                    ..Default::default()
                };
                player.update(
                    &physics,
                    &room,
                    &input,
                    (t, t + 1.0 / 60.0),
                    1.0 / 60.0,
                    false,
                    &frame,
                );
                t += 1.0 / 60.0;
                let f = frame(t);
                let (bface, us, vs, k) = grid_box(&planet, player.body_position(&f));
                assert_eq!(bface, face);
                // Allow 1 cm of contact tolerance.
                for cu in (us.0 + 0.01).floor() as i32..=(us.1 - 0.01).floor() as i32 {
                    for cv in (vs.0 + 0.01).floor() as i32..=(vs.1 - 0.01).floor() as i32 {
                        for ck in (k + 0.01).floor() as i32..=(k + HEIGHT - 0.01).floor() as i32 {
                            assert!(
                                !room.block(face, cu, cv, ck).is_opaque(),
                                "inside block ({cu}, {cv}, {ck}) after {i} frames, box u {us:?} v {vs:?} k {k}"
                            );
                        }
                    }
                }
            }
        }
    }

    struct Vacuum;

    impl Voxels for Vacuum {
        fn block(&self, _: Face, _: i32, _: i32, _: i32) -> Block {
            Block::Air
        }
    }

    fn setup() -> (Planet, PlanetGravity, SolarSystem) {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let gravity = PlanetGravity::new(planet.sea_radius, 1_200.0, 9_000.0);
        (planet, gravity, SolarSystem::new())
    }

    #[test]
    fn stands_on_the_ground_and_jumps() {
        let (planet, gravity, system) = setup();
        let physics = Physics {
            planet: &planet,
            gravity: &gravity,
            drag: true,
        };
        let ground = planet.sea_layer + 10;
        let up = DVec3::new(0.4, 0.3, 0.8).normalize();
        let start = up * (planet.layer_radius(ground as f64) + 3.0);
        let frame = |t: f64| system.planet_frame(t);
        let mut player = Player::new(GameMode::Survival, start, &frame(0.0));
        let world = Ground(ground);
        let mut t = 0.0;
        let mut run = |player: &mut Player, input: PlayerInput, secs: f64| {
            for _ in 0..(secs * 60.0) as usize {
                player.update(
                    &physics,
                    &world,
                    &input,
                    (t, t + 1.0 / 60.0),
                    1.0 / 60.0,
                    false,
                    &frame,
                );
                t += 1.0 / 60.0;
            }
        };
        run(&mut player, PlayerInput::default(), 2.0);
        assert!(player.on_ground);
        let f = frame(2.0);
        let height = player.body_position(&f).length() - planet.layer_radius(ground as f64);
        assert!(height.abs() < 1e-6, "standing {height} m off the ground");
        // At rest relative to the turning ground (but moving fast in inertial space).
        // (Friction holds it against the centrifugal pull towards the equator.)
        let drift = player.relative_velocity(&f).length();
        assert!(drift < 0.01, "drifting at {drift} m/s");
        assert!(player.velocity.length() > 10.0);

        // A jump reaches ~1.25 m.
        let jump = PlayerInput {
            jump: true,
            ..Default::default()
        };
        run(&mut player, jump, 1.0 / 60.0);
        let mut peak: f64 = 0.0;
        let mut airtime = None;
        for i in 0..90 {
            run(&mut player, PlayerInput::default(), 1.0 / 60.0);
            peak = peak.max(player.position.length() - planet.layer_radius(ground as f64));
            if player.on_ground && airtime.is_none() {
                airtime = Some((i + 2) as f64 / 60.0);
            }
        }
        assert!((peak - 1.25).abs() < 0.05, "jump peak {peak}");
        // Snappy like Minecraft's (0.6 s), not the full second real gravity would take.
        let airtime = airtime.expect("landed");
        assert!((0.6..0.72).contains(&airtime), "airtime {airtime} s");
        assert!(player.on_ground);
    }

    /// Fall damage follows Minecraft: nothing for jumps and 3-block drops, then one per block.
    #[test]
    fn fall_damage_like_minecraft() {
        let (planet, gravity, system) = setup();
        let physics = Physics {
            planet: &planet,
            gravity: &gravity,
            drag: true,
        };
        let frame = |t: f64| system.planet_frame(t);
        let ground = planet.sea_layer + 10;
        let up = DVec3::new(0.2, 0.9, 0.3).normalize();
        for (drop, expected) in [(0.0, 0.0), (3.0, 0.0), (10.0, 7.0)] {
            let start = up * (planet.layer_radius(ground as f64) + drop);
            let mut player = Player::new(GameMode::Survival, start, &frame(0.0));
            let mut t = 0.0;
            let mut fallen = 0.0f64;
            for i in 0..300 {
                let input = PlayerInput {
                    jump: drop == 0.0 && i == 60,
                    ..Default::default()
                };
                player.update(
                    &physics,
                    &Ground(ground),
                    &input,
                    (t, t + 1.0 / 60.0),
                    1.0 / 60.0,
                    false,
                    &frame,
                );
                t += 1.0 / 60.0;
                fallen = fallen.max(std::mem::take(&mut player.fallen));
            }
            assert!(
                (fallen - drop.max(1.25)).abs() < 0.1 || drop == 0.0 && fallen < 1.3,
                "fell {fallen} m"
            );
            assert_eq!(Player::fall_damage(fallen), expected, "drop of {drop} m");
        }
    }

    #[test]
    fn orbits_and_falls_through_the_planet() {
        let (planet, gravity, system) = setup();
        let physics = Physics {
            planet: &planet,
            gravity: &gravity,
            drag: false,
        };
        let frame = |t: f64| system.planet_frame(t);
        // A circular orbit 20 km up stays circular over one revolution.
        let r = planet.sea_radius + 20_000.0;
        let mut player = Player::new(GameMode::Survival, DVec3::X * r, &frame(0.0));
        player.position = DVec3::X * r;
        player.velocity = DVec3::Z * (gravity.mu / r).sqrt();
        let period = player.orbit(&physics, 0.0).unwrap().period(gravity.mu);
        let mut t = 0.0;
        while t < period {
            player.update(
                &physics,
                &Vacuum,
                &PlayerInput::default(),
                (t, t + 0.05),
                0.05,
                false,
                &frame,
            );
            t += 0.05;
        }
        assert!((player.position.length() / r - 1.0).abs() < 1e-3);
        // Rails warp agrees with the physics.
        let mut railed = Player::new(GameMode::Survival, DVec3::X * r, &frame(0.0));
        railed.position = DVec3::X * r;
        railed.velocity = DVec3::Z * (gravity.mu / r).sqrt();
        railed.update(
            &physics,
            &Vacuum,
            &PlayerInput::default(),
            (0.0, t),
            0.05,
            true,
            &frame,
        );
        assert!((railed.position - player.position).length() < 50.0);

        // Dropped into a tunnel through the planet (no air): comes out the other side,
        // nearly as high as it started.
        let start = DVec3::Y * (planet.sea_radius - 10.0);
        let mut faller = Player::new(GameMode::Survival, start, &frame(0.0));
        faller.position = start;
        faller.velocity = DVec3::ZERO;
        let mut t = 0.0;
        let mut lowest = f64::MAX;
        let mut deepest_y = f64::MAX;
        for _ in 0..40_000 {
            faller.update(
                &physics,
                &Vacuum,
                &PlayerInput::default(),
                (t, t + 0.05),
                0.05,
                false,
                &frame,
            );
            t += 0.05;
            lowest = lowest.min(faller.position.length());
            deepest_y = deepest_y.min(faller.position.y);
            if faller.position.y < 0.0 && faller.velocity.y > 0.0 {
                break; // turned around on the far side
            }
        }
        assert!(lowest < 100.0, "passed the centre ({lowest} m)");
        assert!(
            deepest_y < -(planet.sea_radius - 200.0),
            "reached {deepest_y}"
        );
    }
}
