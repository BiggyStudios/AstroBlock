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
use glam::{Affine3A, DVec3, Quat, Vec2, Vec3};

use crate::{
    anim::{approach, ease_out},
    items::Item,
    models::{self, HeldShape, Light},
    render::{DrawItem, Fade, MeshHandle, Renderer},
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

const SWING_TIME: f32 = 0.3;
const USE_TIME: f32 = 0.22;
const LOWER_TIME: f32 = 0.1;
const RAISE_TIME: f32 = 0.16;
const LAND_TIME: f32 = 0.3;

/// Where the hand rests (camera space, meters: x right, y up, z backwards).
const REST: Vec3 = Vec3::new(0.24, -0.2, -0.46);
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

    /// Smoothly through keyframes `(time 0..1, pose)` at `t`.
    fn keyframes(keys: &[(f32, Pose)], t: f32) -> Pose {
        for pair in keys.windows(2) {
            let ((t0, a), (t1, b)) = (pair[0], pair[1]);
            if t <= t1 {
                let x = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
                let s = x * x * (3.0 - 2.0 * x);
                return a.scale(1.0 - s).add(b.scale(s));
            }
        }
        keys.last().map_or(Pose::default(), |k| k.1)
    }
}

/// The swing: a short wind-up, a fast strike down and across, and an easy recovery.
fn swing_pose(t: f32) -> Pose {
    let wind = Pose {
        pitch: 0.2,
        yaw: -0.08,
        shift: Vec3::new(0.015, 0.01, 0.02),
        wrist: 0.4,
        ..Pose::default()
    };
    let strike = Pose {
        pitch: -0.05,
        yaw: 0.26,
        roll: 0.08,
        shift: Vec3::new(-0.04, 0.03, -0.06),
        wrist: -1.15,
    };
    Pose::keyframes(
        &[
            (0.0, Pose::default()),
            (0.2, wind),
            (0.45, strike),
            (1.0, Pose::default()),
        ],
        t,
    )
}

/// Using / placing: a push forward.
fn use_pose(t: f32) -> Pose {
    let push = Pose {
        pitch: -0.12,
        yaw: 0.08,
        shift: Vec3::new(-0.01, 0.015, -0.08),
        wrist: -0.2,
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
    /// Walk cycle phase (radians) and how much of the bob shows.
    bob_phase: f32,
    bob: f32,
    /// Lag behind turning (meters), and last frame's look.
    sway: Vec2,
    last_look: Option<(f32, f32)>,
    landing: Option<f32>,
    was_on_ground: bool,
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

        // Gestures start when their counters move.
        let (swings, uses) = self.seen.unwrap_or((input.swings, input.uses));
        if input.swings != swings {
            self.swing = Some(0.0);
        }
        let swing_time = self.slow_swing.unwrap_or(SWING_TIME);
        if input.uses != uses {
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
        if input.on_ground && !self.was_on_ground {
            self.landing = Some(0.0);
        }
        self.was_on_ground = input.on_ground;
        advance(&mut self.landing, LAND_TIME);

        // Sway: the arm lags behind the view when turning.
        let (yaw, pitch) = input.look;
        let (last_yaw, last_pitch) = self.last_look.unwrap_or(input.look);
        self.last_look = Some(input.look);
        let wrap = |a: f32| {
            (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
        };
        let rate = |d: f32| if dt > 0.0 { d / dt } else { 0.0 };
        let target = Vec2::new(
            -rate(wrap(yaw - last_yaw)) * 0.008,
            -rate(pitch - last_pitch) * 0.008,
        )
        .clamp(Vec2::splat(-0.05), Vec2::splat(0.05));
        self.sway = Vec2::new(
            approach(self.sway.x, target.x, 10.0, dt),
            approach(self.sway.y, target.y, 10.0, dt),
        );

        self.eat = approach(self.eat, if input.eating { 1.0 } else { 0.0 }, 12.0, dt);
    }

    /// The pose offset from rest this frame.
    fn pose(&self) -> Pose {
        let mut pose = Pose::default();
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
                pitch: 0.25,
                yaw: 0.55,
                shift: Vec3::new(-0.16, 0.1 + chew, 0.12),
                wrist: 0.3,
                ..Pose::default()
            }
            .scale(ease_out(self.eat)),
        );
        // Lowered while switching.
        let down = 1.0 - ease_out(self.raise);
        pose = pose.add(Pose {
            pitch: -0.5 * down,
            shift: Vec3::new(0.0, -0.32 * down, 0.0),
            ..Pose::default()
        });
        // Walking, landing, breathing and sway.
        let (s, c) = self.bob_phase.sin_cos();
        let land = self
            .landing
            .map_or(0.0, |t| (t * std::f32::consts::PI).sin());
        let breath = (self.time * 1.8).sin();
        pose.add(Pose {
            pitch: breath * 0.01 + self.sway.y * 1.5,
            yaw: -self.sway.x * 1.5,
            roll: s * 0.02 * self.bob,
            shift: Vec3::new(
                s * 0.014 * self.bob + self.sway.x,
                -c.abs() * 0.012 * self.bob - land * 0.045 + breath * 0.0025 + self.sway.y,
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

        // Arm units -> camera space: lay the arm along the view (its y forward = -z, its z
        // up = +y), angled in from the lower right, then animate about the shoulder.
        let base = Quat::from_rotation_y(0.26)
            * Quat::from_rotation_x(0.1)
            * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let rest = Affine3A::from_translation(REST)
            * Affine3A::from_quat(base)
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

    /// Model units of the held item -> arm units: how it sits in the hand.
    fn grip(&self) -> Affine3A {
        match &self.shape {
            // Tools and other sprites: the handle (lower left of the image) in the fist,
            // the image turned upright and tipped forward, its face turned towards the view.
            Some(HeldShape::Sprite(..)) => {
                Affine3A::from_translation(Vec3::new(0.0, 1.0, 1.0))
                    * Affine3A::from_rotation_z(0.55)
                    * Affine3A::from_rotation_x(-0.55)
                    * Affine3A::from_rotation_z(std::f32::consts::FRAC_PI_2)
                    * Affine3A::from_scale(Vec3::splat(1.2))
                    * Affine3A::from_rotation_y(-std::f32::consts::FRAC_PI_4)
                    * Affine3A::from_translation(Vec3::new(-4.0, -8.0, -4.0))
            }
            // Blocks and models: a small cube on the palm, turned to show two sides and
            // the top.
            Some(HeldShape::Block(_)) => {
                Affine3A::from_translation(Vec3::new(0.0, 3.0, 8.0))
                    * Affine3A::from_rotation_x(0.45)
                    * Affine3A::from_rotation_z(0.75)
                    * Affine3A::from_scale(Vec3::splat(0.62))
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
        assert!(swing_pose(0.45).wrist < -1.0, "strikes down");
        assert!(use_pose(1.0).shift.length() < 1e-6);
    }
}
