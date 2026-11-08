//! The first-person view: the player's bare arm and hand, holding the selected item (tools
//! as extruded pixel sprites, blocks as small cubes, tables and furnaces as small models).
//!
//! It is posed in camera space every frame and animated:
//! - a swing arc for hits, digging (repeating while mining) and swinging at nothing, with
//!   or without an item
//! - a short push for placing and using
//! - lowering and raising when the held item changes
//! - bringing food to the visor while eating
//! - a walk bob, lag behind the view when turning, a dip on landing, slow breathing
//!
//! The mesh (`models::view_model`) is rebuilt each frame in camera-relative world space and
//! drawn last with its own depth range (`Camera::view_model_projection`), so it never pokes
//! into walls.

use anyhow::Result;
use glam::{Affine3A, DVec3, Mat3, Quat, Vec2, Vec3};

use crate::{
    render::{
        DrawItem, Fade, MeshHandle, Renderer,
        models::{self, HeldShape, Light},
    },
    survival::items::Item,
    ui::anim::{approach, ease_out},
};

/// What the view model reacts to this frame.
pub struct ViewInput {
    pub item: Option<Item>,
    /// Counters of swings and uses so far (each increase starts that gesture).
    pub swings: u32,
    pub uses: u32,
    /// Mining a block (swings repeat).
    pub mining: bool,
    pub eating: bool,
    /// Walking speed over the ground (m/s) and whether standing on it.
    pub ground_speed: f32,
    pub on_ground: bool,
    /// Camera yaw and pitch (radians), for the sway.
    pub look: (f32, f32),
}

const SWING_TIME: f32 = 0.32;
/// Where the swing's wind-up and strike peak (0..1 of it).
const SWING_WIND: f32 = 0.2;
const SWING_STRIKE: f32 = 0.45;
const USE_TIME: f32 = 0.22;
const LOWER_TIME: f32 = 0.12;
const RAISE_TIME: f32 = 0.2;
/// How fast what is left of an interrupted gesture fades (1/s).
const CARRY_RATE: f32 = 20.0;
/// A landing only counts after this long in the air (s), so stepping up a block doesn't dip.
const MIN_AIR_TIME: f32 = 0.15;
/// Springs (rad/s): the landing dip and how closely the hand follows its lag.
const LAND_SPRING: f32 = 13.0;
const SWAY_SPRING: f32 = 24.0;
/// The hand's lag behind turning fades at this rate (1/s), and the most it can lag (m).
const LAG_RATE: f32 = 11.0;
const MAX_SWAY: f32 = 0.055;

/// Where the hand rests (camera space, meters: x right, y up, z backwards).
const REST: Vec3 = Vec3::new(0.26, -0.24, -0.48);
/// How the forearm is held: turned this far to the right of the view and raised this far
/// (radians), never rolled, so it comes up from the bottom of the screen and sits level.
const ARM_YAW: f32 = 0.3;
const ARM_RAISE: f32 = 0.55;
/// Arm units (see `models::bare_arm`) to meters.
const ARM_UNIT: f32 = 1.0 / 84.0;
/// Where the arm turns during swings: its far end, near the shoulder (arm units).
const SHOULDER: Vec3 = Vec3::new(0.0, -40.0, 0.0);

/// A pose offset: rotations about the shoulder (pitch up, yaw left, roll), a translation
/// (camera space, meters) and a wrist turn of the held item (radians, tips it back).
#[derive(Clone, Copy, Default)]
struct Pose {
    pitch: f32,
    yaw: f32,
    roll: f32,
    shift: Vec3,
    wrist: f32,
}

impl Pose {
    fn add(self, o: Pose) -> Pose {
        Pose {
            pitch: self.pitch + o.pitch,
            yaw: self.yaw + o.yaw,
            roll: self.roll + o.roll,
            shift: self.shift + o.shift,
            wrist: self.wrist + o.wrist,
        }
    }

    fn scale(self, k: f32) -> Pose {
        Pose {
            pitch: self.pitch * k,
            yaw: self.yaw * k,
            roll: self.roll * k,
            shift: self.shift * k,
            wrist: self.wrist * k,
        }
    }

    /// Through keyframes `(time 0..1, pose)` at `t` on one smooth curve (a Catmull-Rom spline
    /// with the ends at rest): the motion flows through the keys instead of stopping at each.
    fn keyframes(keys: &[(f32, Pose)], t: f32) -> Pose {
        let tangent = |i: usize| {
            if i == 0 || i + 1 == keys.len() {
                return Pose::default();
            }
            let (before, after) = (keys[i - 1], keys[i + 1]);
            after
                .1
                .add(before.1.scale(-1.0))
                .scale(1.0 / (after.0 - before.0))
        };
        for i in 0..keys.len() - 1 {
            let ((t0, a), (t1, b)) = (keys[i], keys[i + 1]);
            if t <= t1 {
                let h = t1 - t0;
                let x = ((t - t0) / h).clamp(0.0, 1.0);
                let (x2, x3) = (x * x, x * x * x);
                return a
                    .scale(2.0 * x3 - 3.0 * x2 + 1.0)
                    .add(tangent(i).scale(h * (x3 - 2.0 * x2 + x)))
                    .add(b.scale(3.0 * x2 - 2.0 * x3))
                    .add(tangent(i + 1).scale(h * (x3 - x2)));
            }
        }
        keys.last().map_or(Pose::default(), |k| k.1)
    }
}

/// A critically damped spring, solved exactly so it behaves the same at any frame rate.
#[derive(Clone, Copy, Default)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, target: f32, omega: f32, dt: f32) {
        let x0 = self.x - target;
        let k = self.v + omega * x0;
        let e = (-omega * dt).exp();
        self.x = target + (x0 + k * dt) * e;
        self.v = (self.v - omega * k * dt) * e;
    }
}

/// Keeps `x` below `max` without a hard corner where it would clip.
fn soft_limit(x: f32, max: f32) -> f32 {
    max * (x / max).tanh()
}

/// Arm units -> camera space at rest: y along the forearm, z its top.
fn arm_base() -> Quat {
    Quat::from_rotation_y(-ARM_YAW)
        * Quat::from_rotation_x(ARM_RAISE)
        * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

/// The swing: a short wind-up, a fast strike down and across, and an easy recovery.
fn swing_pose(t: f32) -> Pose {
    let wind = Pose {
        pitch: 0.1,
        yaw: -0.06,
        shift: Vec3::new(0.01, 0.0, 0.025),
        wrist: 0.22,
        ..Pose::default()
    };
    let strike = Pose {
        pitch: -0.2,
        yaw: 0.2,
        roll: 0.05,
        shift: Vec3::new(-0.03, 0.02, -0.08),
        wrist: -1.02,
    };
    Pose::keyframes(
        &[
            (0.0, Pose::default()),
            (SWING_WIND, wind),
            (SWING_STRIKE, strike),
            (1.0, Pose::default()),
        ],
        t,
    )
}

/// Using / placing: a push forward.
fn use_pose(t: f32) -> Pose {
    let push = Pose {
        pitch: -0.14,
        yaw: 0.07,
        shift: Vec3::new(-0.01, 0.0, -0.08),
        wrist: -0.25,
        ..Pose::default()
    };
    Pose::keyframes(
        &[(0.0, Pose::default()), (0.35, push), (1.0, Pose::default())],
        t,
    )
}

#[derive(Default)]
pub struct ViewModel {
    time: f32,
    /// The item in the hand now (may lag the selection while switching) and its shape.
    shown: Option<Item>,
    shape: Option<HeldShape>,
    /// 0 = lowered out of view, 1 = up.
    raise: f32,
    /// Gesture progress (0..1) and the counters already seen.
    swing: Option<f32>,
    /// A slow-motion length (s) for the next swing (the `swing <seconds>` debug command).
    slow_swing: Option<f32>,
    using: Option<f32>,
    seen: Option<(u32, u32)>,
    /// What was left of a gesture cut short by a new one, fading out (so nothing jumps).
    carry: Pose,
    /// Walk cycle phase (radians) and how much of the bob shows.
    bob_phase: f32,
    bob: f32,
    /// How far the view has turned away from the hand (radians, fading), the hand following
    /// that (meters), and last frame's look.
    lag: Vec2,
    sway: [Spring; 2],
    last_look: Option<(f32, f32)>,
    /// The dip after landing (a spring kicked downwards) and the time in the air so far.
    landing: Spring,
    air_time: f32,
    /// How far the food is up at the visor (0..1).
    eat: f32,
    mesh: Option<MeshHandle>,
}

impl ViewModel {
    /// Makes the next swing last `seconds` (to inspect it).
    pub fn slow_next_swing(&mut self, seconds: f32) {
        self.slow_swing = Some(seconds.max(0.05));
    }

    /// Advances the animations by `dt` (real seconds).
    pub fn update(&mut self, input: &ViewInput, dt: f32) {
        self.time += dt;

        // What is left of interrupted gestures fades.
        self.carry = self.carry.scale((-CARRY_RATE * dt).exp());

        // Gestures start when their counters move. A swing still winding up just goes on; a
        // later one hands over what is left of itself to the carry.
        let (swings, uses) = self.seen.unwrap_or((input.swings, input.uses));
        if input.swings != swings {
            match self.swing {
                Some(t) if t < SWING_STRIKE => {}
                Some(t) => {
                    self.carry = self.carry.add(swing_pose(t));
                    self.swing = Some(0.0);
                }
                None => self.swing = Some(0.0),
            }
        }
        let swing_time = self.slow_swing.unwrap_or(SWING_TIME);
        if input.uses != uses {
            if let Some(t) = self.using {
                self.carry = self.carry.add(use_pose(t));
            }
            self.using = Some(0.0);
        }
        self.seen = Some((input.swings, input.uses));
        if input.mining && self.swing.is_none() {
            self.swing = Some(0.0);
        }
        let advance = |g: &mut Option<f32>, time: f32| {
            if let Some(t) = g {
                *t += dt / time;
                if *t >= 1.0 {
                    *g = None;
                }
            }
        };
        advance(&mut self.swing, swing_time);
        if self.swing.is_none() {
            self.slow_swing = None;
        }
        advance(&mut self.using, USE_TIME);

        // Switching items: lower the old one, swap out of sight, raise the new one.
        if input.item != self.shown || (self.shape.is_none() && self.shown.is_some()) {
            self.raise -= dt / LOWER_TIME;
            if self.raise <= 0.0 {
                self.raise = 0.0;
                self.shown = input.item;
                self.shape = input.item.map(HeldShape::of);
            }
        } else {
            self.raise = (self.raise + dt / RAISE_TIME).min(1.0);
        }

        // Walk bob, landing dip.
        let speed = if input.on_ground {
            input.ground_speed.min(8.0)
        } else {
            0.0
        };
        self.bob_phase += speed * dt * 2.0;
        self.bob = approach(self.bob, (speed / 4.3).min(1.3), 8.0, dt);
        if input.on_ground {
            if self.air_time >= MIN_AIR_TIME {
                self.landing.v -= 1.5;
            }
            self.air_time = 0.0;
        } else {
            self.air_time += dt;
        }
        self.landing.step(0.0, LAND_SPRING, dt);

        // Sway: the view turns away from the hand, the hand catches up. Working from how
        // far the view moved (not its speed) keeps a mouse's uneven steps from showing.
        let (yaw, pitch) = input.look;
        let (last_yaw, last_pitch) = self.last_look.unwrap_or(input.look);
        self.last_look = Some(input.look);
        let wrap = |a: f32| {
            (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
        };
        self.lag -= Vec2::new(wrap(yaw - last_yaw), pitch - last_pitch);
        self.lag *= (-LAG_RATE * dt).exp();
        self.sway[0].step(soft_limit(self.lag.x * 0.09, MAX_SWAY), SWAY_SPRING, dt);
        self.sway[1].step(soft_limit(self.lag.y * 0.09, MAX_SWAY), SWAY_SPRING, dt);

        self.eat = approach(self.eat, if input.eating { 1.0 } else { 0.0 }, 12.0, dt);
    }

    /// The pose offset from rest this frame.
    fn pose(&self) -> Pose {
        let mut pose = self.carry;
        if let Some(t) = self.swing {
            pose = pose.add(swing_pose(t));
        }
        if let Some(t) = self.using {
            pose = pose.add(use_pose(t));
        }
        // Eating: up to the visor, with small chewing bobs.
        let chew = (self.time * 20.0).sin() * 0.008;
        pose = pose.add(
            Pose {
                pitch: 0.2,
                yaw: 0.5,
                shift: Vec3::new(-0.14, 0.08 + chew, 0.1),
                wrist: 0.3,
                ..Pose::default()
            }
            .scale(ease_out(self.eat)),
        );
        // Lowered while switching.
        let x = self.raise.clamp(0.0, 1.0);
        let down = 1.0 - x * x * (3.0 - 2.0 * x);
        pose = pose.add(Pose {
            pitch: -0.5 * down,
            shift: Vec3::new(0.0, -0.34 * down, 0.0),
            ..Pose::default()
        });
        // Walking (a smooth figure of eight), landing, breathing and sway.
        let (s, c) = self.bob_phase.sin_cos();
        let breath = (self.time * 1.8).sin();
        let sway = Vec2::new(self.sway[0].x, self.sway[1].x);
        pose.add(Pose {
            pitch: breath * 0.008 + sway.y * 1.5,
            yaw: -sway.x * 1.5,
            roll: s * 0.015 * self.bob,
            shift: Vec3::new(
                s * 0.012 * self.bob + sway.x,
                -c * c * 0.013 * self.bob + self.landing.x + breath * 0.002 + sway.y,
                0.0,
            ),
            wrist: 0.0,
        })
    }

    /// Rebuilds the mesh for this frame and returns its draw (`None` when hidden).
    pub fn draw(
        &mut self,
        renderer: &mut Renderer,
        camera: DVec3,
        orientation: Quat,
        light: Light,
        visible: bool,
    ) -> Result<Option<DrawItem>> {
        if !visible {
            if let Some(old) = self.mesh.take() {
                renderer.destroy_mesh(old);
            }
            return Ok(None);
        }
        let pose = self.pose();

        let rest = Affine3A::from_translation(REST)
            * Affine3A::from_quat(arm_base())
            * Affine3A::from_scale(Vec3::splat(ARM_UNIT));
        let shoulder = rest.transform_point3(SHOULDER);
        let motion = Affine3A::from_translation(shoulder + pose.shift)
            * Affine3A::from_quat(
                Quat::from_rotation_y(pose.yaw)
                    * Quat::from_rotation_x(pose.pitch)
                    * Quat::from_rotation_z(pose.roll),
            )
            * Affine3A::from_translation(-shoulder);
        let arm = motion * rest;
        let held = arm * Affine3A::from_rotation_x(pose.wrist) * self.grip();

        let world = |m: Affine3A| {
            move |p: [f32; 3]| (orientation * m.transform_point3(Vec3::from(p))).as_dvec3()
        };
        let mesh = models::view_model(self.shape.as_ref(), world(arm), world(held), light);
        if let Some(handle) = renderer.create_mesh(&mesh)?
            && let Some(old) = self.mesh.replace(handle)
        {
            renderer.destroy_mesh(old);
        }
        Ok(self.mesh.map(|mesh| DrawItem {
            rotation: glam::Quat::IDENTITY,
            mesh,
            origin: camera,
            radius: 2.0,
            fade: Fade::Visible,
        }))
    }

    /// Model units of the held item -> arm units: how it sits in the hand. Its orientation
    /// is chosen in camera space (what you see), then expressed relative to the arm.
    fn grip(&self) -> Affine3A {
        let in_view = |turn: Quat| Affine3A::from_quat(arm_base().inverse() * turn);
        match &self.shape {
            // Tools and other sprites: the handle (the image's lower left to upper right
            // diagonal) through the fist and standing up, leaning forward; the picture
            // turned towards the eye, but not quite flat on, so it has depth.
            Some(HeldShape::Sprite(..)) => {
                let handle = Vec3::new(0.02, 0.86, -0.5).normalize();
                let face = Vec3::new(-0.68, 0.12, 0.72);
                let face = (face - handle * face.dot(handle)).normalize();
                let across = handle.cross(face);
                let sq = std::f32::consts::FRAC_1_SQRT_2;
                let turn = Quat::from_mat3(&Mat3::from_cols(
                    (handle + across) * sq,
                    -face,
                    (handle - across) * sq,
                ));
                Affine3A::from_translation(Vec3::new(0.0, 1.0, 2.5))
                    * in_view(turn)
                    * Affine3A::from_scale(Vec3::splat(1.08))
                    * Affine3A::from_translation(Vec3::new(-3.5, -8.0, -3.5))
            }
            // Blocks and models: a small cube held up on the hand, level, turned to show
            // two sides and the top.
            Some(HeldShape::Block(_)) => {
                let turn = Quat::from_rotation_x(0.42)
                    * Quat::from_rotation_y(0.35)
                    * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                Affine3A::from_translation(Vec3::new(0.0, 3.0, 7.0))
                    * in_view(turn)
                    * Affine3A::from_scale(Vec3::splat(0.5))
                    * Affine3A::from_translation(Vec3::new(-8.0, -8.0, -8.0))
            }
            None => Affine3A::IDENTITY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ViewInput {
        ViewInput {
            item: Some(Item::Stick),
            swings: 0,
            uses: 0,
            mining: false,
            eating: false,
            ground_speed: 0.0,
            on_ground: true,
            look: (0.0, 0.0),
        }
    }

    #[test]
    fn swings_and_switches() {
        let mut vm = ViewModel::default();
        let mut i = input();
        for _ in 0..60 {
            vm.update(&i, 1.0 / 60.0);
        }
        assert_eq!(vm.shown, Some(Item::Stick));
        assert!(
            vm.raise >= 1.0 && vm.swing.is_none(),
            "raised, no swing at start"
        );

        i.swings = 1;
        vm.update(&i, 1.0 / 60.0);
        assert!(vm.swing.is_some(), "a swing starts");
        for _ in 0..30 {
            vm.update(&i, 1.0 / 60.0);
        }
        assert!(vm.swing.is_none(), "and ends");
        let p = vm.pose();
        assert!(p.pitch.abs() < 0.05 && p.wrist.abs() < 0.05, "back at rest");

        // Mining keeps swinging.
        i.mining = true;
        for _ in 0..50 {
            vm.update(&i, 1.0 / 60.0);
        }
        assert!(vm.swing.is_some());

        // Switching lowers the stick, then raises the new item.
        i.mining = false;
        i.item = None;
        vm.update(&i, 1.0 / 60.0);
        assert_eq!(vm.shown, Some(Item::Stick));
        for _ in 0..30 {
            vm.update(&i, 1.0 / 60.0);
        }
        assert_eq!(vm.shown, None);
        assert!(vm.raise >= 1.0);
    }

    #[test]
    fn swing_returns_to_rest() {
        assert!(swing_pose(0.0).pitch.abs() < 1e-6);
        assert!(swing_pose(1.0).pitch.abs() < 1e-6);
        assert!(swing_pose(SWING_STRIKE).wrist < -1.0, "strikes down");
        assert!(use_pose(1.0).shift.length() < 1e-6);
    }

    /// The largest step of the pose between frames, for continuity checks.
    fn jump(a: Pose, b: Pose) -> f32 {
        (a.pitch - b.pitch)
            .abs()
            .max((a.yaw - b.yaw).abs())
            .max((a.wrist - b.wrist).abs())
    }

    #[test]
    fn swing_curve_has_no_jumps() {
        let mut last = swing_pose(0.0);
        for i in 1..=1000 {
            let p = swing_pose(i as f32 / 1000.0);
            assert!(jump(last, p) < 0.02, "at {i}");
            last = p;
        }
    }

    #[test]
    fn repeated_swings_do_not_jump() {
        let mut vm = ViewModel::default();
        let mut i = input();
        for _ in 0..60 {
            vm.update(&i, 1.0 / 120.0);
        }
        vm.raise = 1.0;
        let mut last = vm.pose();
        // The fastest an uninterrupted swing moves in one frame; a snap back would be far more.
        let step = 1.0 / 120.0 / SWING_TIME;
        let fastest = (0..200)
            .map(|i| i as f32 / 200.0)
            .map(|t| jump(swing_pose(t), swing_pose(t + step)))
            .fold(0.0, f32::max);
        // Clicking faster than a swing lasts (a new click every 0.1 s, at 120 FPS).
        for frame in 0..240 {
            if frame % 12 == 0 {
                i.swings += 1;
            }
            vm.update(&i, 1.0 / 120.0);
            let p = vm.pose();
            assert!(jump(last, p) < fastest * 1.3, "frame {frame}");
            last = p;
        }
    }

    #[test]
    fn sway_is_the_same_at_any_frame_rate() {
        let run = |fps: u32| {
            let mut vm = ViewModel::default();
            let mut i = input();
            vm.update(&i, 1.0 / fps as f32);
            // Turn at 2 rad/s for a quarter of a second.
            for frame in 1..=(fps / 4) {
                i.look.0 = 2.0 * frame as f32 / fps as f32;
                vm.update(&i, 1.0 / fps as f32);
            }
            vm.sway[0].x
        };
        let (slow, fast) = (run(60), run(480));
        assert!(slow < -0.005, "the hand lags behind a right turn: {slow}");
        assert!((slow - fast).abs() < 0.002, "{slow} vs {fast}");
    }

    #[test]
    fn stepping_up_a_block_does_not_dip_but_landing_does() {
        let mut vm = ViewModel::default();
        let mut i = input();
        vm.update(&i, 1.0 / 60.0);
        i.on_ground = false;
        for _ in 0..3 {
            vm.update(&i, 1.0 / 60.0);
        }
        i.on_ground = true;
        vm.update(&i, 1.0 / 60.0);
        assert_eq!(vm.landing.v, 0.0);

        i.on_ground = false;
        for _ in 0..30 {
            vm.update(&i, 1.0 / 60.0);
        }
        i.on_ground = true;
        vm.update(&i, 1.0 / 60.0);
        assert!(vm.landing.x < 0.0);
    }
}
