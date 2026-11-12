//! How a vehicle built from blocks moves: one rigid body pushed by what it is made of.
//!
//! Forces are worked out where they act and in grid axes, so a part's position decides what
//! it does to the rotation. Each step:
//! - Engines push where they sit, at a strength that depends on the air.
//! - Each wing or flap has its own angle to the airflow (including what the vehicle's turning
//!   adds), and lifts or stalls on its own. The body drags with the area it shows.
//! - Reaction wheels (the core and any gyros) and flaps answer the pilot and SAS.
//! - Wheels are springs: each finds the ground below it, and grips sideways and along.
//! - The rest of the hull meets the ground at its outer corners, with impulses.
//!
//! Sitting still on the ground it becomes `landed`, as a rocket does: it stops being
//! simulated, turns with the planet, and can be time-warped.

use glam::{DMat3, DQuat, DVec3};

use super::{
    craft::{
        At, Craft, DRILL_POWER, FUEL_ENERGY, GENERATOR_POWER, Massing, RTG_POWER, SOLAR_POWER,
        SPRING_REST, Stats, WHEEL_RADIUS, center,
    },
    vessel::{
        CRASH_SPEED, Controls, Env, Ground, HEAT_REFERENCE_MASS, Landed, REFUEL_TIME, Vessel,
    },
};
use crate::world::{block::Block, noise::smoothstep};

/// Biggest angular speed (rad/s).
const MAX_SPIN: f64 = 12.0;
/// Air density at sea level (kg/m^3).
const SEA_AIR: f64 = 1.225;
/// How a surface lifts: the slope up to where it stalls, at this share of the flow against it.
const LIFT_SLOPE: f64 = 5.5;
const STALL: f64 = 0.28;
const AFTER_STALL: f64 = 0.75;
const BODY_DRAG: f64 = 0.3;
/// Stall guard: the pilot and SAS can't take fixed wings past this share of the flow against
/// them (a little under `STALL`), easing off over the last `GUARD_MARGIN`. It only matters with
/// this much dynamic pressure (Pa): slower or higher up, the wings aren't carrying the vehicle.
const GUARD_ANGLE: f64 = 0.22;
const GUARD_MARGIN: f64 = 0.08;
const GUARD_PRESSURE: f64 = 60.0;
/// How far flaps deflect (as the sine of the angle), and how much of that per unit of wanted
/// change in spin (it is at full travel with a quarter of a unit to go).
const FLAP_TRAVEL: f64 = 0.35;
const FLAP_GAIN: f64 = 4.0;
/// SAS: turn rate per radian of error (1/s), how fast it reaches that rate (s), its fastest
/// turn (rad/s).
const SAS_GAIN: f64 = 2.0;
const SAS_RESPONSE: f64 = 0.3;
const SAS_RATE: f64 = 0.8;
/// How fast the pilot asks to turn with a key held (rad/s).
const PILOT_RATE: f64 = 0.9;
/// Tyres: grip, how hard driven wheels pull (m/s^2 of the weight on them) and their power
/// (W per kg on them).
const GRIP: f64 = 1.1;
const WHEEL_PULL: f64 = 4.5;
const WHEEL_POWER: f64 = 180.0;
const STEER_ANGLE: f64 = 0.5;
/// Steering eases off with speed: this many m/s halves it.
const STEER_SPEED: f64 = 18.0;
const ROLLING: f64 = 0.015;
/// Wheels see this far above their axle (m), so they climb a one-block step: the terrain is
/// full of them. The spring never pushes harder than this many times its load when they do.
const STEP_UP: f64 = 1.05;
const MOST_SQUEEZE: f64 = 3.0;
/// How wheels see the ground (see `meet_the_ground`): samples each way along the vehicle, how
/// far apart (m), and how steep the ramps they make of steps are.
const ROUND_SAMPLES: i32 = 4;
const ROUND_SPACING: f64 = 0.3;
const ROUND_SLOPE: f64 = 0.8;
/// A wheel hanging off an edge still reaches the ground this much further down (m). Its grip
/// there, and on a wheel hardly carrying anything, as a share of its share of the weight.
const DROOP: f64 = 1.0;
const LEAST_GRIP: f64 = 0.5;
/// The hull on the ground of a vehicle with wheels slides: it is sitting on a step, not parked.
const SKID_FRICTION: f64 = 0.1;
/// Rolling back onto driven wheels: past this angle from upright (rad), towards this turn rate
/// (rad/s), reaching it this fast (s), lifting it this many times its weight.
const RIGHT_FROM: f64 = 0.65;
const RIGHT_SPEED: f64 = 2.0;
const RIGHT_TIME: f64 = 0.2;
const RIGHT_LIFT: f64 = 1.3;
/// Sliding sideways this fast (m/s), a tyre gives all its grip.
const SLIDE_SPEED: f64 = 0.7;
/// The hull on the ground.
const HULL_FRICTION: f64 = 0.7;
const BOUNCE: f64 = 0.05;
/// Beyond this it isn't a part breaking off: the whole vehicle goes.
const WRECK_SPEED: f64 = 45.0;
/// Hitting the ground on its wheels can be this much harder than `CRASH_SPEED`.
const WHEEL_TOUGHNESS: f64 = 1.6;
/// Knocking down a tree the world grew costs the speed hitting this much would (kg per block).
const TRUNK_MASS: f64 = 120.0;
const LEAVES_MASS: f64 = 8.0;
/// Two vehicles: closing faster than this (m/s) wakes a parked one, and overlapping more than
/// this (m) pushes them apart.
const KNOCK_SPEED: f64 = 0.05;
const OVERLAP: f64 = 0.03;
/// Stillness that counts as parked (m/s, rad/s), and for how long (s).
const STILL_SPEED: f64 = 0.25;
const STILL_SPIN: f64 = 0.08;
const STILL_TIME: f64 = 0.6;
/// Driven wheels on electricity: what they take per joule of work they do (gameplay-tuned, as
/// a real motor would empty a battery in seconds here: wheels are very strong), and what each
/// takes just for being driven (W). A rover at full speed takes about 20 kW: a core's battery
/// lasts it under a minute, an accumulator a few, and two solar panels in good sun keep up.
const DRIVE_COST: f64 = 0.15;
const MOTOR_IDLE: f64 = 300.0;

impl Vessel {
    /// Electricity for one step of `dt` while the vehicle asks for `draw` (W, before any
    /// shortage): the panels make what the sun gives them, the generators burn fuel for what
    /// the batteries and `draw` still want, and the batteries take or give the difference.
    /// Returns the fuel the generators burned (kg).
    pub(super) fn electricity(&mut self, craft: &Craft, draw: f64, dt: f64) -> f64 {
        let s = &craft.stats;
        if self.charge < 0.0 {
            self.charge = s.battery;
        }
        let (sun, light) = self.sun;
        let solar: f64 = if light > 0.0 {
            s.solar
                .iter()
                .map(|n| (self.attitude * *n).dot(sun).max(0.0))
                .sum::<f64>()
                * SOLAR_POWER
                * light
        } else {
            0.0
        };
        let used = draw * self.power_share;
        let mut made = solar + s.rtgs as f64 * RTG_POWER;
        let mut burned = 0.0;
        if s.generators > 0 && self.fuel > 0.0 {
            let wanted = ((s.battery - self.charge) / dt + used - solar).max(0.0);
            let output = wanted.min(GENERATOR_POWER * s.generators as f64);
            burned = (output * dt / FUEL_ENERGY).min(self.fuel);
            made += burned * FUEL_ENERGY / dt;
        }
        let before = self.charge;
        self.charge = (self.charge + (made - used) * dt).clamp(0.0, s.battery);
        self.power_draw = draw;
        // (What is made counts as far as it goes somewhere: full batteries take nothing more.)
        self.power_made = if self.charge >= s.battery {
            used + (self.charge - before) / dt
        } else {
            made
        };
        burned
    }

    /// How much of what is asked for the next step gets: all of it while the batteries have
    /// some, else what is being made, shared out.
    fn power_share(&self) -> f64 {
        if self.charge > 1.0 || self.power_draw <= 0.0 {
            1.0
        } else {
            (self.power_made / self.power_draw).clamp(0.0, 1.0)
        }
    }
}

/// Lift for a surface with this much of the flow against it (the sine of its angle).
fn lift(against: f64) -> f64 {
    let a = against.abs();
    let value = if a < STALL {
        LIFT_SLOPE * a
    } else {
        let peak = LIFT_SLOPE * STALL;
        AFTER_STALL + (peak - AFTER_STALL) * (1.0 - (a - STALL) / 0.2).clamp(0.0, 1.0)
    };
    value * against.signum()
}

/// `wanted` (a turn rate, grid axes) with its pitch limited so that fixed wings keep flying:
/// holding the stick back rides along the edge of the stall instead of going over it. Also
/// says whether it had to step in.
fn guard_the_stall(wanted: DVec3, stats: &Stats, air: DVec3, pressure: f64) -> (DVec3, bool) {
    let wings = stats
        .surfaces
        .iter()
        .any(|s| !s.moves && s.normal.dot(stats.up).abs() > 0.9);
    if !wings || pressure < GUARD_PRESSURE {
        return (wanted, false);
    }
    // Positive with the nose above the way it is going.
    let alpha = -air.dot(stats.up) / air.length();
    let room = |to_limit: f64| PILOT_RATE * (to_limit / GUARD_MARGIN).clamp(-1.0, 1.0);
    let nose_up = wanted.dot(stats.right);
    let limited = nose_up
        .max(-room(GUARD_ANGLE + alpha))
        .min(room(GUARD_ANGLE - alpha));
    (
        wanted + stats.right * (limited - nose_up),
        limited != nose_up,
    )
}

/// How the flaps share out a turn. Each one's push also turns the vehicle about the other axes
/// (a fin high on the tail, asked to help with a roll, mostly swings the nose), so they are
/// solved together: the smallest deflections that add up to the turn asked for and no other.
struct FlapPlan {
    solved: DVec3,
    scale: f64,
}

impl FlapPlan {
    /// `asked`: the wanted change in spin (grid axes, at most 1 long).
    fn new(stats: &Stats, com: DVec3, asked: DVec3) -> Self {
        let none = Self {
            solved: DVec3::ZERO,
            scale: 0.0,
        };
        let arms = || {
            stats
                .surfaces
                .iter()
                .filter(|s| s.moves)
                .map(|s| (s.position - com).cross(s.normal))
        };
        let mut sum = DMat3::ZERO;
        for t in arms() {
            sum += DMat3::from_cols(t * t.x, t * t.y, t * t.z);
        }
        let trace = sum.x_axis.x + sum.y_axis.y + sum.z_axis.z;
        if trace <= 0.0 || asked == DVec3::ZERO {
            return none;
        }
        // (A little extra on the diagonal: flaps often can't turn it about some axis at all.)
        let solved = (sum + DMat3::from_diagonal(DVec3::splat(trace * 1e-3))).inverse() * asked;
        let most = arms().map(|t| t.dot(solved).abs()).fold(0.0, f64::max);
        if most < 1e-4 {
            return none;
        }
        Self {
            solved,
            scale: (asked.length() * FLAP_GAIN).min(1.0) / most,
        }
    }

    /// From -1 to 1, for a flap whose push along its normal gives this torque per newton.
    fn deflection(&self, arm: DVec3) -> f64 {
        arm.dot(self.solved) * self.scale
    }
}

/// Forces gathered in one step.
struct Load<'a> {
    m: &'a Massing,
    force: DVec3,
    torque: DVec3,
}

impl Load<'_> {
    /// `force` acting at `at` (grid coordinates and axes).
    fn add(&mut self, at: DVec3, force: DVec3) {
        self.force += force;
        self.torque += (at - self.m.com).cross(force);
    }
}

impl Vessel {
    /// `Vessel::step` for a vehicle built from blocks.
    pub(super) fn step_craft(
        &mut self,
        env: &Env,
        controls: &Controls,
        t: f64,
        dt: f64,
        target: Option<DVec3>,
    ) {
        let Some(craft) = self.craft.take() else {
            return;
        };
        self.simulate(&craft, env, controls, t, dt, target);
        self.craft = Some(craft);
        let broken = std::mem::take(&mut self.breaking);
        for at in broken {
            self.break_part(at);
        }
    }

    fn simulate(
        &mut self,
        craft: &Craft,
        env: &Env,
        controls: &Controls,
        t: f64,
        dt: f64,
        target: Option<DVec3>,
    ) {
        let bodies = env.bodies;
        let stats = &craft.stats;
        let electric_engines = stats.engines.iter().any(|e| e.kind.electric());
        if self.fuel <= 0.0 && !electric_engines {
            self.throttle = 0.0;
        }
        let throttle = self.throttle.clamp(0.0, 1.0);
        self.power_share = self.power_share();
        let share = self.power_share;
        // What it asks of the batteries this step (W).
        let mut draw = 0.0;
        if self.drilling {
            draw += DRILL_POWER * stats.drills.len() as f64;
        }
        // (For the eye: the front wheels turn with the stick even standing still, or in the
        // air; on the ground `meet_the_ground` says by how much at this speed.)
        self.steer = (controls.yaw * STEER_ANGLE) as f32;

        if self.landed.is_some() {
            self.flaps.clear();
            let wakes = (throttle > 0.0 && !stats.engines.is_empty()) || controls.drive != 0.0;
            if !wakes {
                if self.body == crate::space::bodies::Body::Planet {
                    let rate = stats.capacity / REFUEL_TIME;
                    self.fuel = (self.fuel + rate * dt).min(stats.capacity);
                }
                // (Parked, the panels still charge it, and a drill still runs down the batteries.)
                let burned = self.electricity(craft, draw, dt);
                self.fuel -= burned;
                self.sync_landed(bodies, t + dt);
                self.heating = 0.0;
                self.pressure = 0.0;
                self.cool(0.0, 0.0, dt);
                return;
            }
            self.landed = None;
            self.still = 0.0;
        }

        let gravity = bodies.gravity(self.body, self.position);
        let m = craft.massing(self.fuel);
        let attitude = self.attitude;
        let from_inertial = attitude.inverse();
        let spin = from_inertial * self.spin;
        let (rho, air) = if env.drag {
            self.air(bodies, t)
        } else {
            (0.0, self.velocity)
        };
        let air = from_inertial * air;
        let speed = air.length();
        let relative_air = (rho / SEA_AIR).min(1.5);
        self.pressure = 0.5 * rho * speed * speed;

        let mut load = Load {
            m: &m,
            force: DVec3::ZERO,
            torque: DVec3::ZERO,
        };

        // Engines. Those on electricity get what there is of it; gimballed ones can steer.
        let mut burn = 0.0;
        let mut gimbal = 0.0;
        for e in &stats.engines {
            if throttle <= 0.0 {
                break;
            }
            let electric = e.kind.electric();
            if !electric && self.fuel <= 0.0 {
                continue;
            }
            let mut thrust = e.kind.thrust_at(relative_air, speed) * throttle;
            if electric {
                draw += e.kind.power * throttle;
                thrust *= share;
            } else {
                burn += thrust / e.kind.exhaust;
            }
            gimbal += thrust * e.kind.gimbal.sin() * (e.position - m.com).length();
            load.add(e.position, e.push * thrust);
        }
        let push = attitude * load.force / m.mass;

        // Who is asking for what turn: the pilot, else SAS.
        let (asked, flaps) = self.steering(craft, &m, spin, air, controls, target, gimbal);
        let plan = FlapPlan::new(stats, m.com, flaps);
        self.flaps.clear();
        self.flaps.extend(stats.surfaces.iter().map(|s| {
            let d = plan.deflection((s.position - m.com).cross(s.normal));
            if s.moves {
                d.clamp(-1.0, 1.0) as f32
            } else {
                0.0
            }
        }));
        // Air on the surfaces.
        if rho > 0.0 {
            for s in &stats.surfaces {
                let at = s.position - m.com;
                let flow = air + spin.cross(at);
                let v = flow.length();
                if v < 0.5 {
                    continue;
                }
                let dir = flow / v;
                let mut against = dir.dot(s.normal);
                if s.moves {
                    // Pushing along the normal is what turns the vehicle the way asked.
                    let deflected = against - plan.deflection(at.cross(s.normal)) * FLAP_TRAVEL;
                    // It goes no further than the stall: past that it would lose the very
                    // force it moved for.
                    against = if deflected.abs() > STALL && deflected.abs() > against.abs() {
                        deflected.signum() * (STALL * 0.98).max(against.abs())
                    } else {
                        deflected
                    };
                }
                let q = 0.5 * rho * v * v;
                let cl = lift(against);
                let lift_dir = (s.normal - dir * dir.dot(s.normal))
                    .try_normalize()
                    .unwrap_or(s.normal);
                let cd = 0.03 + 0.05 * cl * cl + 1.1 * against * against;
                load.add(s.position, (lift_dir * -cl - dir * cd) * q);
            }
            if speed > 0.1 {
                let dir = air / speed;
                let q = 0.5 * rho * speed * speed;
                let mut area = 0.0;
                let mut center = DVec3::ZERO;
                for axis in 0..3 {
                    let share = stats.silhouette[axis].area * dir[axis].abs();
                    area += share;
                    center += stats.silhouette[axis].center * share;
                }
                if area > 0.0 {
                    load.add(center / area, dir * (-q * area * BODY_DRAG));
                }
            }
        }

        // The wheels don't fight SAS on the ground, and gyros can't lift a car up.
        let has_drive = stats.wheels.iter().any(|w| w.driven);
        if !self.touching || (controls.any() && !has_drive) {
            load.torque += asked;
        } else if controls.drive == 0.0 {
            load.torque += asked * 0.2;
        }

        // Turn.
        let gyroscopic = spin.cross(m.inertia * spin);
        let mut new_spin = spin + m.inverse * (load.torque - gyroscopic) * dt;
        if new_spin.length() > MAX_SPIN {
            new_spin *= MAX_SPIN / new_spin.length();
        }
        self.spin = attitude * new_spin;
        self.attitude = (DQuat::from_scaled_axis(self.spin * dt) * attitude).normalize();

        // Move.
        self.velocity += (gravity + attitude * load.force / m.mass) * dt;
        self.burned += push * dt;

        // Fuel: the centre of mass shifts as it goes, the vehicle itself doesn't.
        if burn > 0.0 {
            self.fuel -= burn * dt;
            if self.fuel <= 0.0 {
                self.fuel = 0.0;
                self.throttle = 0.0;
                self.flameout = true;
            }
            let after = craft.massing(self.fuel);
            self.position += self.attitude * (after.com - m.com);
        }

        // Heat.
        self.heating =
            super::vessel::HEAT_FLUX * rho.sqrt() * speed.powi(3) * smoothstep(250.0, 450.0, speed);
        let scale = (HEAT_REFERENCE_MASS / m.mass).clamp(0.2, 2.0);
        self.cool(rho, self.heating * scale, dt);
        if self.temperature > super::vessel::MAX_TEMPERATURE {
            self.destroyed = Some("burned up on reentry");
            return;
        }

        self.position += self.velocity * dt;
        self.touching = false;
        let m = craft.massing(self.fuel);
        if let Some(ground) = env.ground
            && ground.body == self.body
        {
            draw += self.meet_the_ground(craft, &m, ground, env, controls, t + dt, dt);
        }
        // Electricity, and the fuel the generators burned for it.
        let burned = self.electricity(craft, draw, dt);
        if burned > 0.0 {
            let before = craft.massing(self.fuel).com;
            self.fuel = (self.fuel - burned).max(0.0);
            self.position += self.attitude * (craft.massing(self.fuel).com - before);
        }
        let running = self.throttle > 0.0
            && (self.fuel > 0.0 || electric_engines)
            && !stats.engines.is_empty();
        self.settle_craft(bodies, t + dt, controls, running, dt);
        self.change_soi(bodies, t + dt);
    }

    /// What the pilot and SAS want: the torque from reaction wheels and gimballed engines
    /// (`gimbal`: the most those can give, N m) in grid axes, and what the flaps should do (a
    /// wanted change in angular velocity).
    #[allow(clippy::too_many_arguments)]
    fn steering(
        &mut self,
        craft: &Craft,
        m: &Massing,
        spin: DVec3,
        air: DVec3,
        controls: &Controls,
        target: Option<DVec3>,
        gimbal: f64,
    ) -> (DVec3, DVec3) {
        let s = &craft.stats;
        self.gimbal = DVec3::ZERO;
        let pilot = s.right * controls.pitch + s.forward * controls.roll - s.up * controls.yaw;
        let wanted: Option<DVec3> = if controls.any() {
            self.hold = None;
            Some(pilot * PILOT_RATE)
        } else if self.sas {
            let local_nose = s.forward;
            match target {
                Some(dir) => {
                    let dir = self.attitude.inverse() * dir;
                    let axis = local_nose.cross(dir);
                    let angle = axis.length().atan2(local_nose.dot(dir));
                    self.hold = None;
                    Some(axis.normalize_or_zero() * (angle * SAS_GAIN).min(SAS_RATE))
                }
                None => {
                    let hold = *self.hold.get_or_insert(self.attitude);
                    let (axis, angle) = (hold * self.attitude.inverse()).to_axis_angle();
                    let angle = if angle > std::f64::consts::PI {
                        angle - std::f64::consts::TAU
                    } else {
                        angle
                    };
                    let local = self.attitude.inverse() * axis;
                    Some(local * (angle * SAS_GAIN).clamp(-SAS_RATE, SAS_RATE))
                }
            }
        } else {
            None
        };
        let Some(wanted) = wanted else {
            return (DVec3::ZERO, DVec3::ZERO);
        };
        let (wanted, guarded) = guard_the_stall(wanted, s, air, self.pressure);
        if guarded {
            // SAS holds what the wings can carry, not an attitude they can't get to.
            self.hold = None;
        }
        let change = (wanted - spin) / SAS_RESPONSE;
        // Reaction wheels and gimbals: as much of that as they have.
        let torque = m.inertia * change;
        let most = s.gyro_torque + gimbal;
        let torque = if torque.length() > most {
            torque * (most / torque.length())
        } else {
            torque
        };
        if gimbal > 0.0 {
            // (For the eye: the nozzles swing by how much of their reach this takes.)
            self.gimbal = (torque / most).clamp_length_max(1.0);
        }
        (torque, change.clamp_length_max(1.0))
    }

    /// Wheels on their springs, then the hull's corners. Returns the electricity the driven
    /// wheels asked for (W).
    #[allow(clippy::too_many_arguments)]
    fn meet_the_ground(
        &mut self,
        craft: &Craft,
        m: &Massing,
        ground: Ground,
        env: &Env,
        controls: &Controls,
        t: f64,
        dt: f64,
    ) -> f64 {
        let planet = ground.planet;
        let stats = &craft.stats;
        let top = planet.layer_radius(planet.layers as f64) + 2.0;
        let mut draw = 0.0;
        if self.position.length() - stats.radius - m.com.length() > top {
            return draw;
        }
        let power = self.power_share;
        let frame = env.bodies.frame(ground.body, t);
        let omega = frame.rotation * frame.omega;
        let attitude = self.attitude;
        let inverse = m.inverse;
        // In inertial axes: what a push at arm `r` does to the spin.
        let turn = |torque: DVec3| attitude * (inverse * (attitude.inverse() * torque));
        // (Trees the world grew don't count: it goes through them, knocking them down.)
        let solid = |p: DVec3| {
            planet.cell_at(p).is_some_and(|c| {
                let block = ground.world.block(c.face, c.u, c.v, c.k);
                block.collides() && !(block.is_plant() && ground.world.grown(c.face, c.u, c.v, c.k))
            })
        };

        // ---- Wheels.
        let gravity = env
            .bodies
            .gravity(self.body, self.position)
            .length()
            .max(0.5);
        let count = stats.wheels.len().max(1) as f64;
        let share = m.mass / count;
        let stiffness = share * gravity / SPRING_REST;
        let damping = 2.0 * 0.7 * (stiffness * share).sqrt();
        let ahead = stats.forward;
        for wheel in &stats.wheels {
            // A bigger wheel reaches further, climbs higher and grips a little more.
            let bigger = wheel.radius - WHEEL_RADIUS;
            let reach = wheel.radius + SPRING_REST;
            let step_up = STEP_UP + bigger;
            let grip = GRIP * (1.0 + 0.3 * bigger);
            let local = wheel.position - m.com;
            let origin = self.position + attitude * local;
            let down = attitude * DVec3::NEG_Y;
            let body_origin = frame.to_body(origin);
            let body_down = frame.to_body(down);
            let body_ahead = frame.to_body(attitude * ahead);
            // How far below the axle the ground is at a point `offset` along the vehicle. None:
            // out of reach, or a wall (it is solid even as high as a wheel can climb).
            let span = reach + DROOP + step_up;
            let steps = (span / 0.4).ceil() as usize;
            let surface = |offset: f64| {
                let start = body_origin + body_ahead * offset - body_down * step_up;
                if solid(start) {
                    return None;
                }
                let mut lo = 0.0;
                for i in 1..=steps {
                    let d = span * i as f64 / steps as f64;
                    if solid(start + body_down * d) {
                        // Then pin it down. (Nothing is thin enough to step over.)
                        let mut hi = d;
                        for _ in 0..7 {
                            let mid = (lo + hi) * 0.5;
                            if solid(start + body_down * mid) {
                                hi = mid;
                            } else {
                                lo = mid;
                            }
                        }
                        return Some(hi - step_up);
                    }
                    lo = d;
                }
                None
            };
            // A wheel rolls up a step, it doesn't meet it head on: the ground is rounded off
            // into ramps, so the wheel starts to rise before the step and the hull clears it.
            let hit = (-ROUND_SAMPLES..=ROUND_SAMPLES)
                .filter_map(|i| {
                    let offset = i as f64 * ROUND_SPACING;
                    surface(offset).map(|d| d + offset.abs() * ROUND_SLOPE)
                })
                .reduce(f64::min);
            let Some(distance) = hit else { continue };
            let contact = origin + down * distance.min(reach);
            let arm = contact - self.position;
            let normal = frame.to_inertial(body_origin.normalize_or(DVec3::Y));
            let v = self.velocity + self.spin.cross(arm) - omega.cross(contact);
            let closing = v.dot(normal);
            let squeeze = (reach - distance).min(SPRING_REST * MOST_SQUEEZE);
            let mut press = 0.0;
            if squeeze > 0.0 {
                self.touching = true;
                if closing < -CRASH_SPEED * WHEEL_TOUGHNESS {
                    self.breaking.push(cell_of(wheel.position));
                }
                press = (stiffness * squeeze - damping * closing).max(0.0);
            }

            // Which way this wheel rolls.
            let mut roll = attitude * ahead;
            let is_front = wheel.position.dot(ahead) > m.com.dot(ahead);
            if is_front {
                let ground_speed = (v - normal * closing).length();
                let angle = controls.yaw * STEER_ANGLE / (1.0 + ground_speed / STEER_SPEED);
                roll = DQuat::from_axis_angle(normal, -angle) * roll;
                self.steer = angle as f32;
            }
            let roll = (roll - normal * roll.dot(normal)).normalize_or(attitude * ahead);
            let side = normal.cross(roll);
            let (along, across) = (v.dot(roll), v.dot(side));
            // (A wheel that is hanging, or barely loaded, still has some: enough to drive a
            // vehicle off the step its hull has come to rest on.)
            let most = grip * press.max(share * gravity * LEAST_GRIP);
            // Grip builds up with how fast the tyre slides sideways, as a real one does.
            // (All or nothing, the rear wheels would cancel any turn the front ones start.)
            let lateral = -most * (across / SLIDE_SPEED).clamp(-1.0, 1.0);
            let mut drive = 0.0;
            if wheel.driven && controls.drive != 0.0 && !controls.brake {
                // Electric motors: as hard as the electricity there is lets them.
                // (What is asked for counts, not what it got: `electricity` shares it out.)
                let limit = share * WHEEL_PULL.min(WHEEL_POWER / along.abs().max(1.0));
                let wanted = controls.drive * limit;
                drive = wanted * power;
                draw += (wanted.clamp(-most, most) * along).abs() * DRIVE_COST + MOTOR_IDLE;
            }
            let brake = if controls.brake {
                (-along * share / dt.max(0.02)).clamp(-most, most)
            } else {
                -along.signum() * (ROLLING * press).min(along.abs() * share / dt.max(0.02))
            };
            let longitudinal = (drive + brake).clamp(-most, most);
            let force = normal * press + roll * longitudinal + side * lateral;
            self.velocity += force * (dt / m.mass);
            self.spin += turn(arm.cross(force)) * dt;
        }

        // ---- Trees in the way of any part (the corners below only find the outside ones).
        if (self.velocity - omega.cross(self.position)).length() > 0.3 {
            for part in craft.parts() {
                let origin = self.position + attitude * (center(part.at) - m.com);
                let Some(cell) = planet.cell_at(frame.to_body(origin)) else {
                    continue;
                };
                let block = ground.world.block(cell.face, cell.u, cell.v, cell.k);
                self.knocks_down(cell, block, ground, omega, m.mass);
            }
        }

        // ---- Hull.
        let up_here = self.position.normalize_or(DVec3::Y);
        let (mut deepest, mut out) = (0.0f64, DVec3::ZERO);
        for corner in &stats.corners {
            let origin = self.position + attitude * (*corner - m.com);
            let body = frame.to_body(origin);
            let Some(cell) = planet.cell_at(body) else {
                continue;
            };
            let block = ground.world.block(cell.face, cell.u, cell.v, cell.k);
            if !block.collides() || self.knocks_down(cell, block, ground, omega, m.mass) {
                continue;
            }
            let on_wheels = !stats.wheels.is_empty();
            let (normal, depth) = exit_from_block(planet, ground, body, cell, on_wheels);
            let normal = frame.to_inertial(normal);
            let arm = origin - self.position;
            let v = self.velocity + self.spin.cross(arm) - omega.cross(origin);
            let closing = v.dot(normal);
            self.touching = true;
            if depth > deepest {
                deepest = depth;
                out = normal;
            }
            if closing >= 0.0 {
                continue;
            }
            if closing < -CRASH_SPEED {
                self.breaking.extend(craft.part_at_corner(*corner));
            }
            if closing < -WRECK_SPEED {
                self.destroyed = Some("crashed");
                return draw;
            }
            let leverage = |n: DVec3| 1.0 / m.mass + n.dot(turn(arm.cross(n)).cross(arm));
            let j = -(1.0 + BOUNCE) * closing / leverage(normal);
            let tangent = v - normal * closing;
            let mut impulse = normal * j;
            if let Some(dir) = tangent.try_normalize() {
                let friction = if on_wheels {
                    SKID_FRICTION
                } else {
                    HULL_FRICTION
                };
                let slip = (tangent.length() / leverage(dir)).min(friction * j);
                impulse -= dir * slip;
            }
            self.velocity += impulse / m.mass;
            self.spin += turn(arm.cross(impulse));
        }
        if deepest > 0.0 {
            // Out of the ground gently, not by the whole overlap at once.
            self.position += out * (deepest * 0.2).min(0.05);
        }

        // ---- A vehicle on driven wheels that has tipped too far rolls back onto them.
        if self.touching && stats.wheels.iter().any(|w| w.driven) {
            let (standing, up) = (attitude * stats.up, up_here);
            let angle = standing.cross(up).length().atan2(standing.dot(up));
            if angle > RIGHT_FROM {
                let axis = standing
                    .cross(up)
                    .try_normalize()
                    .unwrap_or(attitude * stats.forward);
                let wanted = (0.6 + (angle - RIGHT_FROM) * 3.0).min(RIGHT_SPEED);
                let turning = (self.spin - omega).dot(axis);
                self.spin += axis * ((wanted - turning) / RIGHT_TIME).clamp(0.0, 30.0) * dt;
                // (Pressed against the ground it couldn't turn.)
                self.velocity += up * gravity * RIGHT_LIFT * dt;
            }
        }
        draw
    }

    /// A block of a tree the world grew, in the way: noted for the game to knock down (the
    /// whole tree), and it takes a little of the speed, as a light thing hit would. Returns
    /// whether it was one (and so isn't ground).
    fn knocks_down(
        &mut self,
        cell: crate::world::chunk::BlockCell,
        block: Block,
        ground: Ground,
        omega: DVec3,
        mass: f64,
    ) -> bool {
        if !block.is_plant() || !ground.world.grown(cell.face, cell.u, cell.v, cell.k) {
            return false;
        }
        if !self.felled.contains(&cell) {
            self.felled.push(cell);
            let weight = match block {
                Block::OakLeaves | Block::SpruceLeaves | Block::JungleLeaves => LEAVES_MASS,
                _ => TRUNK_MASS,
            };
            let over_ground = self.velocity - omega.cross(self.position);
            self.velocity -= over_ground * (weight / (mass + weight));
        }
        true
    }

    /// Sat still on the ground long enough: parked.
    fn settle_craft(
        &mut self,
        bodies: &crate::space::bodies::Bodies,
        t: f64,
        controls: &Controls,
        running: bool,
        dt: f64,
    ) {
        if !self.touching || running || controls.drive != 0.0 {
            self.still = 0.0;
            return;
        }
        let frame = bodies.frame(self.body, t);
        let omega = frame.rotation * frame.omega;
        let moving = (self.velocity - omega.cross(self.position)).length();
        let turning = (self.spin - omega).length();
        if moving > STILL_SPEED || turning > STILL_SPIN {
            self.still = 0.0;
            return;
        }
        self.still += dt;
        if self.still > STILL_TIME {
            self.landed = Some(Landed {
                position: frame.to_body(self.position),
                attitude: (frame.rotation.inverse() * self.attitude).normalize(),
            });
            self.hold = None;
            self.sync_landed(bodies, t);
        }
    }
}

/// One vehicle's corner inside another's part.
struct Touch {
    /// Where (relative to the body, inertial axes).
    at: DVec3,
    /// Pushes `a` away from `b` (inertial).
    normal: DVec3,
    depth: f64,
    /// The part of each that touches.
    parts: [Option<At>; 2],
}

/// Two vehicles on the same body, at the same time, against each other: hull corners of each
/// inside a part of the other are pushed apart with impulses, as the hull is from the ground.
/// `moves`: which of the two this machine simulates. The other's own machine works out the
/// same touch and moves it, so each only changes its own, by its share. Hitting harder than
/// `CRASH_SPEED` breaks the parts that touch. Returns whether they touched.
pub fn meet_each_other(a: &mut Vessel, b: &mut Vessel, moves: [bool; 2]) -> bool {
    if a.body != b.body || !(moves[0] || moves[1]) {
        return false;
    }
    let (Some(ca), Some(cb)) = (a.craft.as_deref(), b.craft.as_deref()) else {
        return false;
    };
    let reach = ca.stats.radius + cb.stats.radius + 2.0;
    if (a.position - b.position).length() > reach {
        return false;
    }
    let (ma, mb) = (ca.massing(a.fuel), cb.massing(b.fuel));
    let mut touches = Vec::new();
    corners_inside(a, ca, &ma, b, cb, &mb, false, &mut touches);
    corners_inside(b, cb, &mb, a, ca, &ma, true, &mut touches);
    if touches.is_empty() {
        return false;
    }

    let turn = |attitude: DQuat, m: &Massing, torque: DVec3| {
        attitude * (m.inverse * (attitude.inverse() * torque))
    };
    let share = mb.mass / (ma.mass + mb.mass);
    // Worked out on copies: one that isn't simulated here only stands in for its own
    // machine's share. (What each touch does changes what the next one sees.)
    let mut motion = [(a.velocity, a.spin), (b.velocity, b.spin)];
    let (mut deepest, mut out) = (0.0f64, DVec3::ZERO);
    let mut broken: [Vec<At>; 2] = [Vec::new(), Vec::new()];
    let (mut knocked, mut wrecked) = (false, false);
    for touch in &touches {
        let n = touch.normal;
        let (ra, rb) = (touch.at - a.position, touch.at - b.position);
        let [(va, wa), (vb, wb)] = motion;
        let relative = (va + wa.cross(ra)) - (vb + wb.cross(rb));
        let closing = relative.dot(n);
        if touch.depth > deepest {
            deepest = touch.depth;
            out = n;
        }
        if closing >= 0.0 {
            continue;
        }
        knocked |= closing < -KNOCK_SPEED;
        if closing < -CRASH_SPEED {
            for (list, part) in broken.iter_mut().zip(touch.parts) {
                list.extend(part);
            }
        }
        wrecked |= closing < -WRECK_SPEED;
        let leverage = |d: DVec3| {
            1.0 / ma.mass
                + 1.0 / mb.mass
                + d.dot(turn(a.attitude, &ma, ra.cross(d)).cross(ra))
                + d.dot(turn(b.attitude, &mb, rb.cross(d)).cross(rb))
        };
        let j = -(1.0 + BOUNCE) * closing / leverage(n);
        let mut impulse = n * j;
        let tangent = relative - n * closing;
        if let Some(dir) = tangent.try_normalize() {
            impulse -= dir * (tangent.length() / leverage(dir)).min(HULL_FRICTION * j);
        }
        motion[0].0 += impulse / ma.mass;
        motion[0].1 += turn(a.attitude, &ma, ra.cross(impulse));
        motion[1].0 -= impulse / mb.mass;
        motion[1].1 -= turn(b.attitude, &mb, rb.cross(impulse));
    }
    // Out of each other gently, each by its share. (Barely touching, nothing: two parked
    // side by side stay parked.)
    let overlapping = deepest > OVERLAP;
    let push = (deepest * 0.3).min(0.08);
    let pushes = [out * push * share, -out * push * (1.0 - share)];

    for (i, v) in [&mut *a, &mut *b].into_iter().enumerate() {
        if !moves[i] {
            continue;
        }
        if knocked || overlapping {
            // Knocked: it is moving again (and simulated from here on).
            v.landed = None;
            v.still = 0.0;
        }
        if v.landed.is_some() {
            continue;
        }
        (v.velocity, v.spin) = motion[i];
        if overlapping {
            v.position += pushes[i];
        }
        if wrecked {
            v.destroyed = Some("crashed");
        }
        for &at in &broken[i] {
            v.break_part(at);
        }
    }
    true
}

/// Each hull corner of `of` inside a part of `into`, with the shortest way out of that part
/// to where `into` has no part. `swap`: `of` is the second of the pair (the normal is turned
/// round so it always pushes the first away from the second).
#[allow(clippy::too_many_arguments)]
fn corners_inside(
    of: &Vessel,
    of_craft: &Craft,
    of_mass: &Massing,
    into: &Vessel,
    into_craft: &Craft,
    into_mass: &Massing,
    swap: bool,
    touches: &mut Vec<Touch>,
) {
    let back = into.attitude.inverse();
    for &corner in &of_craft.stats.corners {
        let at = of.position + of.attitude * (corner - of_mass.com);
        let local = back * (at - into.position) + into_mass.com;
        let cell = cell_of(local);
        if into_craft.get(cell).is_none() {
            continue;
        }
        let inside = local - center(cell);
        let mut best: Option<(DVec3, f64)> = None;
        for axis in 0..3 {
            for sign in [-1i16, 1] {
                let mut next = cell;
                next[axis] += sign;
                if into_craft.get(next).is_some() {
                    continue;
                }
                let mut dir = DVec3::ZERO;
                dir[axis] = sign as f64;
                let depth = 0.5 - inside.dot(dir);
                if best.is_none_or(|(_, d)| depth < d) {
                    best = Some((dir, depth));
                }
            }
        }
        let Some((dir, depth)) = best else { continue };
        let normal = into.attitude * dir;
        let (normal, parts) = if swap {
            (-normal, [Some(cell), of_craft.part_at_corner(corner)])
        } else {
            (normal, [of_craft.part_at_corner(corner), Some(cell)])
        };
        touches.push(Touch {
            at,
            normal,
            depth: depth.max(0.0),
            parts,
        });
    }
}

/// The grid cell around a point (a wheel's centre).
fn cell_of(p: DVec3) -> At {
    [p.x.round() as i16, p.y.round() as i16, p.z.round() as i16]
}

/// The quickest way out of the solid block at `body` (body frame): the direction, and how
/// far. Up unless a side (or below) is much nearer and open.
fn exit_from_block(
    planet: &crate::world::planet::Planet,
    ground: Ground,
    body: DVec3,
    cell: crate::world::chunk::BlockCell,
    climbs: bool,
) -> (DVec3, f64) {
    let (face, u, v, k) = planet.locate(body);
    let n = planet.res_at(cell.k);
    let up = body.normalize_or(DVec3::Y);
    let free = |du: i32, dv: i32, dk: i32| {
        let (u, v, k) = (cell.u + du, cell.v + dv, cell.k + dk);
        let block = ground.world.block(cell.face, u, v, k);
        !block.collides() || (block.is_plant() && ground.world.grown(cell.face, u, v, k))
    };
    let (fu, fv, fk) = (u - u.floor(), v - v.floor(), k - k.floor());
    // On wheels a single block isn't a wall: over it. (A taller one is.)
    if climbs && free(0, 0, 1) {
        return (up, 1.0 - fk);
    }
    let mut best = (
        up,
        if free(0, 0, 1) {
            1.0 - fk
        } else {
            1.0 + (1.0 - fk)
        },
    );
    let along = |du: f64, dv: f64| {
        (planet.dir(face, u + du * 0.5, v + dv * 0.5, n)
            - planet.dir(face, u - du * 0.5, v - dv * 0.5, n))
        .normalize_or(DVec3::X)
    };
    // (Down too: a corner pushed up into a ceiling goes back down, not up through the rock.)
    for (open, distance, dir) in [
        (free(0, 0, -1), fk, -up),
        (free(1, 0, 0), 1.0 - fu, along(1.0, 0.0)),
        (free(-1, 0, 0), fu, -along(1.0, 0.0)),
        (free(0, 1, 0), 1.0 - fv, along(0.0, 1.0)),
        (free(0, -1, 0), fv, -along(0.0, 1.0)),
    ] {
        if open && distance < best.1 * 0.8 {
            best = (dir, distance);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        flight::craft::{Part, facing_of},
        player::Voxels,
        space::{
            SolarSystem,
            bodies::{Bodies, Body},
            gravity::PlanetGravity,
        },
        world::{
            block::Block,
            planet::{Face, Planet},
        },
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

    fn part(at: At, block: Block, normal: DVec3) -> Part {
        Part {
            at,
            block,
            facing: facing_of(normal),
        }
    }

    /// A vehicle resting (grid +Y up, forward = +Z) with the core `height` above the floor.
    fn parked(bodies: &Bodies, planet: &Planet, craft: Craft, ground: i32, height: f64) -> Vessel {
        let up = DVec3::new(1.0, 0.05, 0.02).normalize();
        let east = DVec3::Y.cross(up).normalize();
        let north = up.cross(east);
        // Grid X east, Y up, Z north would be left-handed the wrong way round: X = Y x Z.
        let z = north;
        let x = up.cross(z);
        let rotation = DQuat::from_mat3(&DMat3::from_cols(x, up, z));
        let core = up * (planet.layer_radius(ground as f64) + height);
        Vessel::built(bodies, Body::Planet, core, rotation, 0.0, craft)
    }

    /// (With full batteries throughout: the tests of how it drives and flies aren't about
    /// electricity. `run` is, see the tests of power.)
    fn step(v: &mut Vessel, env: &Env, t: &mut f64, seconds: f64, controls: &Controls) {
        let dt = 1.0 / 60.0;
        for _ in 0..(seconds / dt) as usize {
            v.fill_charge();
            v.update(env, controls, (*t, *t + dt), None);
            *t += dt;
            if v.destroyed.is_some() {
                return;
            }
        }
    }

    fn altitude(v: &Vessel, bodies: &Bodies) -> f64 {
        v.position.length() - bodies.radius(Body::Planet)
    }

    /// Over the ground (m/s).
    fn ground_speed(v: &Vessel, bodies: &Bodies, t: f64) -> f64 {
        v.surface_velocity(bodies, t).length()
    }

    fn rover() -> Craft {
        rover_on(Block::Wheel)
    }

    fn rover_on(wheel: Block) -> Craft {
        let mut c = Craft::new(4);
        for (x, z) in [
            (1, 0),
            (-1, 0),
            (1, 1),
            (-1, 1),
            (0, 1),
            (1, 2),
            (-1, 2),
            (0, 2),
        ] {
            assert!(c.place(part([x, 0, z], Block::Hull, DVec3::X)), "{x} {z}");
        }
        assert!(c.place(part([0, 1, 0], Block::Seat, DVec3::Z)));
        for (x, z) in [(2, 0), (-2, 0), (2, 2), (-2, 2)] {
            assert!(c.place(part([x, 0, z], wheel, DVec3::X)));
        }
        c
    }

    fn glider_body(wings: i16) -> Craft {
        let mut c = Craft::new(4);
        for z in 1..=6 {
            assert!(c.place(part([0, 0, z], Block::Hull, DVec3::X)));
        }
        for z in (-3..=-1).rev() {
            assert!(c.place(part([0, 0, z], Block::Hull, DVec3::X)));
        }
        for x in 1..=wings {
            // The outer ends of a full-size wing are its ailerons.
            let block = if wings >= 7 && x >= 5 {
                Block::Flap
            } else {
                Block::Wing
            };
            for z in 2..=3 {
                assert!(c.place(part([x, 0, z], block, DVec3::Y)));
                assert!(c.place(part([-x, 0, z], block, DVec3::Y)));
            }
        }
        for x in 1..=3 {
            assert!(c.place(part([x, 0, -3], Block::Flap, DVec3::Y)));
            assert!(c.place(part([-x, 0, -3], Block::Flap, DVec3::Y)));
        }
        // The fin, standing up at the tail.
        assert!(c.place(part([0, 1, -3], Block::Flap, DVec3::X)));
        assert!(c.place(part([0, 2, -3], Block::Flap, DVec3::X)));
        c
    }

    fn plane() -> Craft {
        let mut c = glider_body(7);
        assert!(c.place(part([0, 1, 1], Block::Seat, DVec3::Z)));
        assert!(c.place(part([0, 0, 7], Block::Propeller, DVec3::NEG_Z)));
        assert!(c.place(part([0, 1, 3], Block::FuelTank, DVec3::X)));
        assert!(c.place(part([0, -1, 5], Block::LandingGear, DVec3::X)));
        c
    }

    /// Rock everywhere but a tunnel two blocks high from layer `.0` up.
    struct Tunnel(i32);

    impl Voxels for Tunnel {
        fn block(&self, _: Face, _: i32, _: i32, k: i32) -> Block {
            if (self.0..self.0 + 2).contains(&k) {
                Block::Air
            } else {
                Block::Stone
            }
        }
    }

    #[test]
    fn a_vehicle_in_a_tunnel_stays_in_it() {
        // A core and a seat, built underground. Holding W (pitch) turns it on the core's
        // reaction wheel against the roof: that used to push it up into the rock, metre after
        // metre, until it came out in a cave.
        let (planet, bodies) = setup();
        let floor = planet.sea_layer - 40;
        let world = Tunnel(floor);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        let mut c = Craft::new(4);
        assert!(c.place(part([0, 1, 0], Block::Seat, DVec3::Z)));
        let mut v = parked(&bodies, &planet, c, floor, 0.5);
        let mut t = 0.0;
        let w = Controls {
            pitch: 1.0,
            drive: 1.0,
            ..Default::default()
        };
        for controls in [w, Controls { pitch: -1.0, ..w }, w] {
            step(&mut v, &env, &mut t, 20.0 / 3.0, &controls);
            assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
            let core = v.position.length() - planet.layer_radius(floor as f64);
            assert!(
                (-0.2..2.2).contains(&core),
                "{core} m above the tunnel floor"
            );
        }
    }

    #[test]
    fn a_rover_settles_on_its_wheels_and_drives() {
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
        let mut v = parked(&bodies, &planet, rover(), ground, 1.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 4.0, &Controls::default());
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(v.landed.is_some(), "it parks");
        let core = v.position.length() - planet.layer_radius(ground as f64);
        assert!((0.3..1.2).contains(&core), "resting {core} m up");

        let forward = Controls {
            drive: 1.0,
            ..Default::default()
        };
        step(&mut v, &env, &mut t, 6.0, &forward);
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(v.landed.is_none());
        let speed = ground_speed(&v, &bodies, t);
        assert!(speed > 8.0, "{speed} m/s after 6 s");

        // Braking stops it, and it parks again.
        let brake = Controls {
            brake: true,
            ..Default::default()
        };
        step(&mut v, &env, &mut t, 8.0, &brake);
        assert!(ground_speed(&v, &bodies, t) < 1.0, "stopped");
    }

    /// Flat ground with a step `height` blocks up, from a certain distance on.
    struct Step {
        ground: i32,
        height: i32,
        along: (Face, i32, i32, i32, i32),
    }

    impl Voxels for Step {
        fn block(&self, face: Face, u: i32, v: i32, k: i32) -> Block {
            let (f, u0, v0, du, dv) = self.along;
            let past = face == f && (u - u0) * du + (v - v0) * dv >= 0;
            let top = self.ground + if past { self.height } else { 0 };
            if k < top { Block::Stone } else { Block::Air }
        }
    }

    /// Drives at a step and reports how far up it got (m).
    fn climbs(height: i32) -> f64 {
        climbs_with(height, Block::Wheel)
    }

    /// `climbs` on wheels of another kind.
    fn climbs_with(height: i32, wheel: Block) -> f64 {
        let (planet, bodies) = setup();
        let ground = planet.sea_layer + 5;
        // Where it will be, and which way (in grid cells) is ahead of it.
        let lift = crate::flight::craft::wheel_radius(wheel).unwrap_or(0.7) + 0.3;
        let mut v = parked(&bodies, &planet, rover_on(wheel), ground, lift);
        let frame = bodies.frame(Body::Planet, 0.0);
        let at = frame.to_body(v.position);
        let ahead = frame.to_body(v.nose());
        let start = planet.cell_at(at).unwrap();
        let wall = planet.cell_at(at + ahead * 14.0).unwrap();
        let (du, dv) = (wall.u - start.u, wall.v - start.v);
        let (du, dv) = if du.abs() > dv.abs() {
            (du.signum(), 0)
        } else {
            (0, dv.signum())
        };
        let world = Step {
            ground,
            height,
            along: (start.face, wall.u, wall.v, du, dv),
        };
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        let before = v.position.length();
        let go = Controls {
            drive: 1.0,
            ..Default::default()
        };
        step(&mut v, &env, &mut t, 9.0, &go);
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        v.position.length() - before
    }

    #[test]
    fn wheels_climb_one_block_but_not_a_wall() {
        let one = climbs(1);
        assert!(one > 0.8, "got {one} m up a one-block step");
        let three = climbs(3);
        assert!(three < 0.8, "got {three} m up a wall");
    }

    /// Flat ground with a row of tree trunks (five logs high) across the way, grown by the
    /// world or placed by someone.
    struct Trunks {
        ground: i32,
        along: (Face, i32, i32, i32, i32),
        grown: bool,
    }

    impl Trunks {
        fn log(&self, face: Face, u: i32, v: i32, k: i32) -> bool {
            let (f, u0, v0, du, dv) = self.along;
            face == f
                && (u - u0) * du + (v - v0) * dv == 0
                && (self.ground..self.ground + 5).contains(&k)
        }
    }

    impl Voxels for Trunks {
        fn block(&self, face: Face, u: i32, v: i32, k: i32) -> Block {
            if self.log(face, u, v, k) {
                Block::OakLog
            } else if k < self.ground {
                Block::Stone
            } else {
                Block::Air
            }
        }

        fn grown(&self, face: Face, u: i32, v: i32, k: i32) -> bool {
            self.grown && self.log(face, u, v, k)
        }
    }

    /// Drives a rover at a row of trunks 14 m ahead: how far it got (m), whether it lost any
    /// parts, and how many log blocks it noted for knocking down.
    fn drive_at_trunks(grown: bool) -> (f64, bool, usize) {
        let (planet, bodies) = setup();
        let ground = planet.sea_layer + 5;
        let mut v = parked(&bodies, &planet, rover(), ground, 1.0);
        let frame = bodies.frame(Body::Planet, 0.0);
        let at = frame.to_body(v.position);
        let ahead = frame.to_body(v.nose());
        let start = planet.cell_at(at).unwrap();
        let row = planet.cell_at(at + ahead * 14.0).unwrap();
        let (du, dv) = (row.u - start.u, row.v - start.v);
        let (du, dv) = if du.abs() > dv.abs() {
            (du.signum(), 0)
        } else {
            (0, dv.signum())
        };
        let world = Trunks {
            ground,
            along: (start.face, row.u, row.v, du, dv),
            grown,
        };
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        v.landed = None;
        let parts = v.craft.as_ref().unwrap().parts().len();
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        let go = Controls {
            drive: 1.0,
            ..Default::default()
        };
        let mut felled = 0;
        for _ in 0..12 {
            step(&mut v, &env, &mut t, 1.0, &go);
            felled += std::mem::take(&mut v.felled).len();
        }
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let gone = v.craft.as_ref().unwrap().parts().len() < parts;
        // (The planet turns: measured in its own frame at the time.)
        let now = bodies.frame(Body::Planet, t);
        let travelled = (now.to_body(v.position) - at).dot(ahead);
        (travelled, gone, felled)
    }

    #[test]
    fn grown_trees_go_down_and_placed_logs_stop_it() {
        let (travelled, gone, felled) = drive_at_trunks(true);
        assert!(travelled > 30.0, "only {travelled} m through grown trees");
        assert!(!gone, "lost parts to grown trees");
        assert!(felled > 0, "noted no trees to knock down");

        let (travelled, _, felled) = drive_at_trunks(false);
        assert!(travelled < 14.5, "went {travelled} m through placed logs");
        assert_eq!(felled, 0);
    }

    /// Two vehicles: `a` driving into `b`, which is parked `gap` m ahead (core to core).
    fn rear_end(drive_for: f64) -> (Vessel, Vessel, f64) {
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
        let mut a = parked(&bodies, &planet, rover(), ground, 1.0);
        let mut b = parked(&bodies, &planet, rover(), ground, 1.0);
        let ahead = b.nose();
        let start = b.position;
        // (Rovers reach 2 m ahead of their core: 4 m of road between them.)
        b.position += ahead * 6.0;
        b.landed = None;
        a.landed = None;
        let mut t = 0.0;
        let dt = 1.0 / 60.0;
        let go = Controls {
            drive: 1.0,
            ..Default::default()
        };
        for n in 0..((2.0 + drive_for) / dt) as usize {
            let controls = if n as f64 * dt > 2.0 {
                go
            } else {
                Controls::default()
            };
            a.update(&env, &controls, (t, t + dt), None);
            b.update(&env, &Controls::default(), (t, t + dt), None);
            t += dt;
            meet_each_other(&mut a, &mut b, [true, true]);
        }
        let (now, then) = (
            bodies.frame(Body::Planet, t),
            bodies.frame(Body::Planet, 0.0),
        );
        let pushed =
            (now.to_body(b.position) - then.to_body(start + ahead * 6.0)).dot(then.to_body(ahead));
        (a, b, pushed)
    }

    #[test]
    fn a_vehicle_pushes_another_and_doesnt_go_through_it() {
        let (a, b, pushed) = rear_end(5.0);
        assert!(a.destroyed.is_none() && b.destroyed.is_none());
        assert!(pushed > 1.0, "the one in front moved {pushed} m");
        // Never one inside the other: cores stay about a rover's length apart.
        let apart = (a.position - b.position).length();
        assert!(apart > 2.6, "cores {apart} m apart");
    }

    /// Ground of one-block steps both ways, as most of the terrain is.
    struct Terraces(i32);

    impl Voxels for Terraces {
        fn block(&self, _: Face, u: i32, v: i32, k: i32) -> Block {
            let top = self.0 + (u.div_euclid(3) & 1) + (v.div_euclid(5) & 1);
            if k < top { Block::Stone } else { Block::Air }
        }
    }

    #[test]
    fn a_rover_doesnt_get_stuck_on_steps() {
        let (planet, bodies) = setup();
        let ground = planet.sea_layer + 5;
        let world = Terraces(ground);
        let env = Env {
            bodies: &bodies,
            ground: Some(Ground {
                body: Body::Planet,
                planet: &planet,
                world: &world,
            }),
            drag: true,
        };
        let mut v = parked(&bodies, &planet, rover(), ground, 3.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        let go = Controls {
            drive: 1.0,
            ..Default::default()
        };
        let on_the_ground = |v: &Vessel, t: f64| bodies.frame(Body::Planet, t).to_body(v.position);
        let mut last = on_the_ground(&v, t);
        let (mut path, mut stopped) = (0.0, 0.0);
        for _ in 0..40 {
            step(&mut v, &env, &mut t, 0.5, &go);
            let here = on_the_ground(&v, t);
            let moved = (here - last).length();
            last = here;
            path += moved;
            if moved < 0.25 {
                stopped += 0.5;
            }
        }
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        assert!(path > 120.0, "drove only {path} m in 20 s");
        assert!(stopped <= 1.0, "stopped for {stopped} s");
    }

    #[test]
    fn a_rover_on_its_roof_rolls_back_onto_its_wheels() {
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
        let mut v = parked(&bodies, &planet, rover(), ground, 2.5);
        v.landed = None;
        v.attitude = DQuat::from_axis_angle(v.nose(), 3.0) * v.attitude;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 8.0, &Controls::default());
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let standing = (v.attitude * DVec3::Y).dot(v.position.normalize());
        assert!(standing > 0.9, "still on its side: {standing}");
    }

    #[test]
    fn the_throttle_left_up_on_a_rover_doesnt_keep_it_from_parking() {
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
        let mut craft = rover();
        assert!(craft.place(part([1, 1, 2], Block::FuelTank, DVec3::X)));
        let mut v = parked(&bodies, &planet, craft, ground, 1.0);
        v.landed = None;
        v.throttle = 1.0;
        assert!(!v.engines_on(), "nothing to run");
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 4.0, &Controls::default());
        assert!(v.landed.is_some(), "it parks");
        assert_eq!(v.throttle, 1.0);
    }

    #[test]
    fn a_small_plane_is_quick() {
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
        let mut v = parked(&bodies, &planet, full_plane(), ground, 2.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        v.throttle = 1.0;
        let start = t;
        while ground_speed(&v, &bodies, t) < 50.0 && t < 30.0 {
            step(&mut v, &env, &mut t, 0.05, &Controls::default());
        }
        assert!(t - start < 8.0, "{} s to 50 m/s", t - start);

        // Flat out and level, it gets well past its old 58 m/s.
        let air = Env {
            bodies: &bodies,
            ground: None,
            drag: true,
        };
        let mut v = flying_plane(&bodies, 45.0, 1.0);
        let mut t = 0.0;
        for _ in 0..1600 {
            let (nose, _) = angles(&v, &bodies, t);
            let level = Controls {
                pitch: (nose * -0.5).clamp(-1.0, 1.0),
                ..Default::default()
            };
            step(&mut v, &air, &mut t, 0.05, &level);
        }
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let speed = ground_speed(&v, &bodies, t);
        assert!(speed > 90.0, "only {speed} m/s");
    }

    #[test]
    fn a_rover_steers() {
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
        let mut v = parked(&bodies, &planet, rover(), ground, 1.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        let controls = Controls {
            drive: 1.0,
            ..Default::default()
        };
        step(&mut v, &env, &mut t, 3.0, &controls);
        let before = v.nose();
        let right = v.right();
        let turn = Controls {
            drive: 1.0,
            yaw: 1.0,
            ..Default::default()
        };
        for _ in 0..6 {
            step(&mut v, &env, &mut t, 0.5, &turn);
        }
        let after = v.nose();
        let angle = before.dot(after).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(angle > 20.0, "turned {angle} degrees");
        assert!(after.dot(right) > 0.1, "yaw +1 turns the nose to the right");
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
    }

    #[test]
    fn wings_hold_a_plane_up_at_speed_and_not_when_slow() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: true,
        };
        let r = bodies.radius(Body::Planet) + 200.0;
        let up = DVec3::X;
        let run = |speed: f64| {
            let craft = plane();
            let mut v = Vessel::built(
                &bodies,
                Body::Planet,
                up * r,
                DQuat::from_mat3(&DMat3::from_cols(up.cross(DVec3::Y), up, DVec3::Y)),
                0.0,
                craft,
            );
            v.landed = None;
            // Level, heading north, with a little nose up.
            v.attitude = DQuat::from_axis_angle(v.attitude * DVec3::X, -0.06) * v.attitude;
            let frame = bodies.frame(Body::Planet, 0.0);
            let omega = frame.rotation * frame.omega;
            v.velocity = omega.cross(v.position) + DVec3::Y * speed;
            v.sas = true;
            v.spin = omega;
            let mut t = 0.0;
            let height = altitude(&v, &bodies);
            step(&mut v, &env, &mut t, 1.0, &Controls::default());
            altitude(&v, &bodies) - height
        };
        let (fast, slow) = (run(70.0), run(12.0));
        assert!(fast > -1.0, "at 70 m/s it lost {fast} m in a second");
        assert!(slow < -2.0, "at 12 m/s it lost only {slow} m in a second");
    }

    #[test]
    fn a_plane_takes_off_from_a_runway() {
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
        let mut craft = plane();
        for (x, z) in [(1, 1), (-1, 1)] {
            assert!(craft.place(part([x, 0, z], Block::Hull, DVec3::X)));
            assert!(craft.place(part([x, -1, z], Block::LandingGear, DVec3::X)));
        }
        let mut v = parked(&bodies, &planet, craft, ground, 2.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let start = altitude(&v, &bodies);
        v.throttle = 1.0;
        step(&mut v, &env, &mut t, 14.0, &Controls::default());
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let rolled = ground_speed(&v, &bodies, t);
        assert!(rolled > 25.0, "{rolled} m/s down the runway");
        let up = Controls {
            pitch: 1.0,
            ..Default::default()
        };
        // Rotate gently, then let SAS hold the climb.
        let none = Controls::default();
        for _ in 0..600 {
            if v.nose().dot(v.position.normalize()) > 0.14 {
                break;
            }
            step(&mut v, &env, &mut t, 0.02, &up);
        }
        step(&mut v, &env, &mut t, 10.0, &none);
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        let climbed = altitude(&v, &bodies) - start;
        assert!(climbed > 8.0, "only {climbed} m up");
    }

    fn full_plane() -> Craft {
        let mut craft = plane();
        for (x, z) in [(1, 1), (-1, 1)] {
            assert!(craft.place(part([x, 0, z], Block::Hull, DVec3::X)));
            assert!(craft.place(part([x, -1, z], Block::LandingGear, DVec3::X)));
        }
        craft
    }

    /// (nose above the horizon, angle of attack) in degrees.
    fn angles(v: &Vessel, bodies: &Bodies, t: f64) -> (f64, f64) {
        let up = v.position.normalize();
        let nose = v.nose();
        let air = v.surface_velocity(bodies, t).normalize_or_zero();
        let craft_up = v.attitude * v.craft.as_ref().unwrap().stats.up;
        let alpha = (-air.dot(craft_up)).atan2(air.dot(nose)).to_degrees();
        (nose.dot(up).asin().to_degrees(), alpha)
    }

    #[test]
    fn holding_the_stick_back_climbs_without_stalling() {
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
        let mut v = parked(&bodies, &planet, full_plane(), ground, 2.0);
        v.landed = None;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        let start = altitude(&v, &bodies);
        v.throttle = 1.0;
        for _ in 0..400 {
            if ground_speed(&v, &bodies, t) >= 40.0 {
                break;
            }
            step(&mut v, &env, &mut t, 0.05, &Controls::default());
        }
        assert!(ground_speed(&v, &bodies, t) >= 40.0, "it gets up to speed");
        let up = Controls {
            pitch: 1.0,
            ..Default::default()
        };
        let mut worst: f64 = 0.0;
        for _ in 0..32 {
            step(&mut v, &env, &mut t, 0.25, &up);
            worst = worst.max(angles(&v, &bodies, t).1);
        }
        assert!(v.destroyed.is_none(), "{:?}", v.destroyed);
        // (`STALL` is 16.3 degrees.)
        assert!(worst < 16.3, "the wings reached {worst} degrees");
        let climbed = altitude(&v, &bodies) - start;
        assert!(climbed > 40.0, "only {climbed} m up after 8 s");

        // Let go: SAS settles it where the wings can carry it.
        let released = altitude(&v, &bodies);
        step(&mut v, &env, &mut t, 10.0, &Controls::default());
        let (_, alpha) = angles(&v, &bodies, t);
        assert!(alpha.abs() < 16.3, "alpha {alpha} after letting go");
        assert!(
            altitude(&v, &bodies) > released - 15.0,
            "fell from {released} to {}",
            altitude(&v, &bodies)
        );
    }

    #[test]
    fn at_full_power_a_plane_can_hold_a_climb() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: true,
        };
        let mut v = flying_plane(&bodies, 45.0, 15.0);
        let mut t = 0.0;
        let start = altitude(&v, &bodies);
        step(&mut v, &env, &mut t, 25.0, &Controls::default());
        let climbed = altitude(&v, &bodies) - start;
        assert!(climbed > 60.0, "climbed {climbed} m in 25 s");
        let speed = ground_speed(&v, &bodies, t);
        assert!(speed > 32.0, "slowed to {speed} m/s");
    }

    /// A plane in level flight, heading north, 300 m up.
    fn flying_plane(bodies: &Bodies, speed: f64, degrees: f64) -> Vessel {
        let r = bodies.radius(Body::Planet) + 300.0;
        let up = DVec3::X;
        let mut v = Vessel::built(
            bodies,
            Body::Planet,
            up * r,
            DQuat::from_mat3(&DMat3::from_cols(up.cross(DVec3::Y), up, DVec3::Y)),
            0.0,
            full_plane(),
        );
        v.landed = None;
        let frame = bodies.frame(Body::Planet, 0.0);
        let omega = frame.rotation * frame.omega;
        // (Along its own nose: `built` turns the attitude into the planet's tilted frame.)
        v.velocity = omega.cross(v.position) + v.nose() * speed;
        v.attitude =
            DQuat::from_axis_angle(v.attitude * DVec3::X, -degrees.to_radians()) * v.attitude;
        v.spin = omega;
        v.sas = true;
        v.throttle = 1.0;
        v
    }

    /// (bank with the right wing down, nose above the horizon, heading change from `start`,
    /// sideslip), in degrees.
    fn flight_angles(v: &Vessel, bodies: &Bodies, t: f64, start: DVec3) -> [f64; 4] {
        let up = v.position.normalize();
        let stats = &v.craft.as_ref().unwrap().stats;
        let right = v.attitude * stats.right;
        let nose = v.nose();
        let flat = (nose - up * nose.dot(up)).normalize_or_zero();
        let start = (start - up * start.dot(up)).normalize_or_zero();
        let heading = flat.cross(start).dot(up).atan2(flat.dot(start));
        let air = v.surface_velocity(bodies, t).normalize_or_zero();
        [
            (-right.dot(up)).asin().to_degrees(),
            nose.dot(up).asin().to_degrees(),
            heading.to_degrees(),
            air.dot(right).asin().to_degrees(),
        ]
    }

    #[test]
    fn rolling_stays_on_the_axis_it_was_asked_for() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: true,
        };
        for (roll, side) in [(1.0, 1.0), (-1.0, -1.0)] {
            let mut v = flying_plane(&bodies, 52.0, 5.0);
            let start = v.nose();
            let mut t = 0.0;
            let controls = Controls {
                roll,
                ..Default::default()
            };
            step(&mut v, &env, &mut t, 1.25, &controls);
            let [bank, nose, heading, _] = flight_angles(&v, &bodies, t, start);
            assert!(bank * side > 45.0, "banked only {bank} degrees");
            assert!(heading.abs() < 2.0, "the nose swung {heading} degrees");
            assert!((nose - 5.0).abs() < 2.0, "the nose went to {nose} degrees");
        }
    }

    #[test]
    fn flaps_that_cant_turn_it_that_way_stay_still() {
        // Only flat flaps: they can pitch and roll it, but not yaw it.
        let mut c = Craft::new(4);
        for x in 1..=3 {
            assert!(c.place(part([x, 0, 0], Block::Flap, DVec3::Y)));
            assert!(c.place(part([-x, 0, 0], Block::Flap, DVec3::Y)));
        }
        let at = |x: f64| DVec3::new(x, 0.0, 0.0).cross(DVec3::Y);
        let yaw = FlapPlan::new(&c.stats, DVec3::ZERO, DVec3::Y);
        assert!(yaw.deflection(at(3.0)).abs() < 1e-6);
        let roll = FlapPlan::new(&c.stats, DVec3::ZERO, DVec3::Z);
        assert!((roll.deflection(at(3.0)) - 1.0).abs() < 1e-6, "full travel");
        assert!(
            roll.deflection(at(-3.0)) < -0.99,
            "and the other way across"
        );
    }

    fn rocket(off_centre: i16) -> Craft {
        let mut c = Craft::new(2);
        for y in 1..=8 {
            assert!(c.place(part([0, y, 0], Block::FuelTank, DVec3::X)));
        }
        assert!(c.place(part([0, 9, 0], Block::Seat, DVec3::Z)));
        for x in 1..=off_centre.abs() {
            assert!(c.place(part([x, 0, 0], Block::Hull, DVec3::X)));
        }
        assert!(c.place(part([off_centre, -1, 0], Block::RocketEngine, DVec3::NEG_Y)));
        c
    }

    #[test]
    fn engines_under_the_centre_of_mass_fly_straight_and_off_centre_they_tumble() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let r = bodies.radius(Body::Planet) + 60_000.0;
        let run = |off: i16| {
            let craft = rocket(off);
            let mut v = Vessel::built(
                &bodies,
                Body::Planet,
                DVec3::X * r,
                DQuat::IDENTITY,
                0.0,
                craft,
            );
            v.landed = None;
            v.sas = false;
            v.spin = DVec3::ZERO;
            v.velocity = DVec3::Y * 1_400.0;
            v.fuel = v.fuel_capacity();
            v.throttle = 1.0;
            let nose = v.nose();
            let mut t = 0.0;
            step(&mut v, &env, &mut t, 2.0, &Controls::default());
            nose.dot(v.nose()).clamp(-1.0, 1.0).acos()
        };
        let (straight, skewed) = (run(0), run(2));
        assert!(straight < 0.01, "straight engine turned it {straight} rad");
        assert!(
            skewed > 0.5,
            "off-centre engine only turned it {skewed} rad"
        );
    }

    #[test]
    fn fuel_gives_what_the_rocket_equation_says_and_the_gauge_empties() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let r = bodies.radius(Body::Planet) + 60_000.0;
        let mut v = Vessel::built(
            &bodies,
            Body::Planet,
            DVec3::X * r,
            DQuat::IDENTITY,
            0.0,
            rocket(0),
        );
        v.landed = None;
        v.velocity = DVec3::Y * 1_400.0;
        v.fuel = v.fuel_capacity();
        v.sas = false;
        v.spin = DVec3::ZERO;
        let promised = v.delta_v();
        assert!(promised > 500.0, "{promised} m/s");
        v.throttle = 1.0;
        let mut t = 0.0;
        step(&mut v, &env, &mut t, 120.0, &Controls::default());
        assert_eq!(v.fuel, 0.0);
        assert_eq!(v.throttle, 0.0);
        let delivered = v.burned.length();
        assert!(
            (delivered - promised).abs() < promised * 0.03,
            "delivered {delivered} of {promised}"
        );
    }

    #[test]
    fn a_hard_landing_breaks_parts_and_a_very_hard_one_everything() {
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
            drag: false,
        };
        let drop = |speed: f64| {
            let craft = glider_body(2);
            let mut v = parked(&bodies, &planet, craft, ground, 3.0);
            v.landed = None;
            v.sas = false;
            let frame = bodies.frame(Body::Planet, 0.0);
            let omega = frame.rotation * frame.omega;
            let up = v.position.normalize();
            v.velocity = omega.cross(v.position) - up * speed;
            v.spin = omega;
            let before = v.craft.as_ref().unwrap().parts().len();
            let mut t = 0.0;
            step(&mut v, &env, &mut t, 1.0, &Controls::default());
            let after = v.craft.as_ref().map_or(0, |c| c.parts().len());
            (before, after, v.destroyed.is_some())
        };
        let (before, gentle, wrecked) = drop(4.0);
        assert_eq!(gentle, before, "a gentle touch breaks nothing");
        assert!(!wrecked);
        let (before, hard, wrecked) = drop(25.0);
        assert!(hard < before, "a hard one breaks something");
        assert!(!wrecked);
        let (_, _, wrecked) = drop(80.0);
        assert!(wrecked);
    }

    #[test]
    fn in_space_reaction_wheels_turn_it_and_sas_holds_it() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let r = bodies.radius(Body::Planet) + 60_000.0;
        let mut craft = rocket(0);
        assert!(craft.place(part([1, 3, 0], Block::Gyro, DVec3::X)));
        let mut v = Vessel::built(
            &bodies,
            Body::Planet,
            DVec3::X * r,
            DQuat::IDENTITY,
            0.0,
            craft,
        );
        v.landed = None;
        v.velocity = DVec3::Y * 1_400.0;
        v.spin = DVec3::ZERO;
        v.sas = true;
        let start = v.nose();
        let mut t = 0.0;
        let steer = Controls {
            pitch: 1.0,
            ..Default::default()
        };
        step(&mut v, &env, &mut t, 2.0, &steer);
        let turned = start.dot(v.nose()).clamp(-1.0, 1.0).acos();
        assert!(turned > 0.3, "turned {turned} rad");
        // Let go: SAS stops the turn and holds.
        step(&mut v, &env, &mut t, 6.0, &Controls::default());
        let held = v.nose();
        assert!(v.spin.length() < 0.02, "still turning {}", v.spin.length());
        step(&mut v, &env, &mut t, 2.0, &Controls::default());
        assert!(held.dot(v.nose()) > 0.9999);
    }

    /// Like `step`, but the batteries are left as they are: these tests are about them.
    fn run(v: &mut Vessel, env: &Env, t: &mut f64, seconds: f64, controls: &Controls) {
        let dt = 1.0 / 60.0;
        for _ in 0..(seconds / dt) as usize {
            v.update(env, controls, (*t, *t + dt), None);
            *t += dt;
        }
    }

    /// A rover's wheels run on its batteries: driving empties them, and with them empty it
    /// doesn't go (the brakes still work: no power needed).
    #[test]
    fn a_rover_runs_on_electricity_and_stops_without_it() {
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
        let mut v = parked(&bodies, &planet, rover(), ground, 1.0);
        v.landed = None;
        let mut t = 0.0;
        run(&mut v, &env, &mut t, 2.0, &Controls::default());
        let full = v.charge;
        assert_eq!(full, crate::flight::craft::CORE_BATTERY);
        let go = Controls {
            drive: 1.0,
            ..Default::default()
        };
        run(&mut v, &env, &mut t, 6.0, &go);
        let used = full - v.charge;
        assert!(ground_speed(&v, &bodies, t) > 8.0);
        // (About 20 kW at full speed, less on the way up to it.)
        assert!((10_000.0..200_000.0).contains(&used), "{used} J in 6 s");

        // Stopped (brakes need no power) with flat batteries, it won't go again.
        let brake = Controls {
            brake: true,
            ..Default::default()
        };
        run(&mut v, &env, &mut t, 6.0, &brake);
        assert!(ground_speed(&v, &bodies, t) < 0.5);
        v.charge = 0.0;
        run(&mut v, &env, &mut t, 6.0, &go);
        let speed = ground_speed(&v, &bodies, t);
        assert!(speed < 0.5, "going {speed} m/s on no power");
        assert_eq!(v.charge, 0.0);
        assert_eq!(v.power_share, 0.0);
    }

    /// Solar panels charge the batteries by how square on to the sun they are, and not at all
    /// in a shadow; a generator burns the tanks' fuel to top them up.
    #[test]
    fn panels_and_generators_charge_the_batteries() {
        let mut c = Craft::new(4);
        assert!(c.place(part([0, 1, 0], Block::SolarPanel, DVec3::Y)));
        assert!(c.place(part([1, 0, 0], Block::Accumulator, DVec3::X)));
        let (_, bodies) = setup();
        let mut v = Vessel::built(
            &bodies,
            Body::Planet,
            DVec3::X * 50_000.0,
            DQuat::IDENTITY,
            0.0,
            c,
        );
        v.charge = 0.0;
        let up = v.attitude * DVec3::Y;
        v.sun = (up, 1.0);
        let craft = v.craft.take().unwrap();
        v.electricity(&craft, 0.0, 10.0);
        let square = v.charge;
        assert!((square - SOLAR_POWER * 10.0).abs() < 1.0, "{square}");
        v.sun = ((up + v.attitude * DVec3::X).normalize(), 1.0);
        v.electricity(&craft, 0.0, 10.0);
        assert!(
            v.charge - square < SOLAR_POWER * 10.0 * 0.75,
            "at an angle, less"
        );
        let before = v.charge;
        v.sun = (up, 0.0);
        v.electricity(&craft, 0.0, 10.0);
        assert_eq!(v.charge, before, "nothing in the dark");

        // A generator: burns fuel only for what the batteries take.
        let mut c = Craft::new(4);
        assert!(c.place(part([1, 0, 0], Block::CoalGenerator, DVec3::X)));
        assert!(c.place(part([0, 1, 0], Block::FuelTank, DVec3::X)));
        let mut v = Vessel::built(
            &bodies,
            Body::Planet,
            DVec3::X * 50_000.0,
            DQuat::IDENTITY,
            0.0,
            c,
        );
        let fuel = v.fuel;
        let craft = v.craft.take().unwrap();
        v.electricity(&craft, 0.0, 1.0);
        assert_eq!(v.fuel, fuel, "full batteries: nothing burned");
        v.charge = 0.0;
        let burned = v.electricity(&craft, 0.0, 1.0);
        assert!(burned > 0.0 && v.charge > 0.0);
        assert!((v.charge - burned * crate::flight::craft::FUEL_ENERGY).abs() < 1.0);
    }

    /// Something in space, its engines down (+Y up the grid), no air and no ground.
    fn in_space(craft: Craft, bodies: &Bodies) -> Vessel {
        let r = bodies.radius(Body::Planet) + 60_000.0;
        let mut v = Vessel::built(
            bodies,
            Body::Planet,
            DVec3::X * r,
            DQuat::IDENTITY,
            0.0,
            craft,
        );
        v.landed = None;
        v.sas = false;
        v.spin = DVec3::ZERO;
        v.velocity = DVec3::Y * 1_400.0;
        v
    }

    /// Ion engines push very gently, for as long as there is power, and burn no fuel; with flat
    /// batteries they don't push at all.
    #[test]
    fn ion_engines_run_on_power_alone() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let mut c = Craft::new(2);
        assert!(c.place(part([0, -1, 0], Block::IonEngine, DVec3::NEG_Y)));
        let ion = crate::flight::craft::engine_kind(Block::IonEngine).unwrap();
        // It can't even hold up its own weight at home.
        assert!(ion.thrust < crate::flight::craft::part_mass(Block::IonEngine) * 9.81 * 0.5);
        let mut v = in_space(c, &bodies);
        v.throttle = 1.0;
        let mut t = 0.0;
        let (before, nose) = (v.velocity, v.nose());
        let fuel = v.fuel;
        run(&mut v, &env, &mut t, 5.0, &Controls::default());
        // (What gravity did along the nose comes off.)
        let g = bodies.gravity(Body::Planet, v.position).dot(nose) * 5.0;
        let pushed = (v.velocity - before).dot(nose) - g;
        let expected = ion.thrust / v.mass() * 5.0;
        assert!(
            pushed > 0.5 * expected && pushed < 1.5 * expected,
            "{pushed} vs {expected}"
        );
        assert_eq!(v.fuel, fuel);
        assert!(
            v.charge < crate::flight::craft::CORE_BATTERY,
            "it used power"
        );
        assert!(v.engines_on());

        // Flat: nothing.
        v.charge = 0.0;
        run(&mut v, &env, &mut t, 0.2, &Controls::default());
        let before = v.velocity;
        let nose = v.nose();
        run(&mut v, &env, &mut t, 5.0, &Controls::default());
        let pushed = (v.velocity - before).dot(nose);
        // (Gravity still pulls; it should be the same as with no engine at all.)
        let g = bodies.gravity(Body::Planet, v.position).dot(nose) * 5.0;
        assert!(
            (pushed - g).abs() < 0.05 * expected.max(0.01),
            "{pushed} vs gravity {g}"
        );
    }

    /// A gimballed engine steers with its thrust: under power it turns a long rocket much
    /// faster than the core's reaction wheels alone can start it turning.
    #[test]
    fn vector_engines_steer_with_their_thrust() {
        let (_, bodies) = setup();
        let env = Env {
            bodies: &bodies,
            ground: None,
            drag: false,
        };
        let turned = |engine: Block| {
            let mut c = Craft::new(2);
            for y in 1..=8 {
                assert!(c.place(part([0, y, 0], Block::FuelTank, DVec3::X)));
            }
            assert!(c.place(part([0, -1, 0], engine, DVec3::NEG_Y)));
            let mut v = in_space(c, &bodies);
            v.throttle = 1.0;
            let start = v.nose();
            let mut t = 0.0;
            let pitch = Controls {
                pitch: 1.0,
                ..Default::default()
            };
            run(&mut v, &env, &mut t, 0.6, &pitch);
            start.dot(v.nose()).clamp(-1.0, 1.0).acos()
        };
        let (fixed, vectored) = (turned(Block::RocketEngine), turned(Block::VectorEngine));
        assert!(
            vectored > 2.0 * fixed,
            "vectored {vectored} rad, fixed {fixed} rad"
        );
    }

    /// Giant wheels climb a step two blocks high that ordinary ones can't.
    #[test]
    fn giant_wheels_climb_higher() {
        assert!(climbs(2) < 0.8, "ordinary wheels don't climb two blocks");
        let giant = climbs_with(2, Block::GiantWheel);
        assert!(giant > 1.6, "got {giant} m up two blocks on giant wheels");
    }
}
