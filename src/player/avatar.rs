//! Other players as you see them: a blocky figure in a pale space suit with an orange band,
//! a backpack and a dark visor. Same box-building kit as the animals (`mobs::models::Rig`);
//! rebuilt every frame, since it moves.
//!
//! Units are model units (1/16 m): x right, y forward, z up, origin between the feet.

use std::ops::Range;

use glam::DVec3;

use crate::{
    mobs::models::{Rgb, Rig, multiply},
    player::HEIGHT,
    render::{
        ChunkMesh,
        models::{Light, build, pitch},
    },
    world::textures::Tex,
};

const SUIT: Rgb = [205, 208, 212];
const SHADE: Rgb = [150, 155, 162];
const BAND: Rgb = [226, 122, 40];
const DARK: Rgb = [46, 50, 58];
const VISOR: Rgb = [28, 34, 52];

/// How the figure stands right now.
#[derive(Clone, Copy, Debug, Default)]
pub struct AvatarPose {
    /// Walk cycle phase (radians) and how strongly the legs swing (0..1).
    pub walk: f32,
    pub swing: f32,
    /// How far the right arm is raised in a swing (0..1).
    pub arm: f32,
    /// Looking up (positive) or down (radians).
    pub look_pitch: f32,
    /// Holding something out in front.
    pub holding: bool,
    /// Just been hit: the figure flashes red.
    pub hurt: bool,
}

/// How far the figure reaches from its middle, arms included (m).
const HALF_WIDTH: f64 = 0.45;

/// Distance along a ray (unit direction) to the box around a figure standing at `feet`, if hit.
pub fn ray_hit(feet: DVec3, up: DVec3, origin: DVec3, dir: DVec3, reach: f64) -> Option<f64> {
    let right = up.any_orthonormal_vector();
    let axes = [right, up.cross(right), up];
    let rel = origin - (feet + up * (HEIGHT * 0.5));
    let (mut near, mut far) = (0.0f64, reach);
    for (axis, half) in axes.iter().zip([HALF_WIDTH, HALF_WIDTH, HEIGHT * 0.5]) {
        let (o, d) = (rel.dot(*axis), dir.dot(*axis));
        if d.abs() < 1e-9 {
            if o.abs() > half {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half - o) / d, (half - o) / d);
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// Where a held item goes, from the feet: (right, forward, up) in metres.
pub const HAND: [f64; 3] = [0.3, 0.42, 0.82];

fn limb(rig: &mut Rig, x: f32, z: (f32, f32), half: f32, boot: f32, color: Rgb) -> Range<usize> {
    let start = rig.mark();
    rig.part(
        [x - half, -half, z.0 + boot],
        [x + half, half, z.1],
        Tex::Hide,
        color,
    );
    if boot > 0.0 {
        let g = half + 0.3;
        rig.part(
            [x - g, -g, z.0],
            [x + g, g + 1.0, z.0 + boot],
            Tex::Hide,
            DARK,
        );
    }
    start.start..rig.mark().end
}

/// The figure's mesh around its feet, facing `forward` (with `up`).
pub fn avatar_model(pose: &AvatarPose, (forward, up): (DVec3, DVec3), light: Light) -> ChunkMesh {
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 0x5ca1_ab1e,
    };
    let swing = pose.walk.sin() * 0.75 * pose.swing;

    // Legs, swinging opposite each other from the hip.
    let hip = 12.0;
    let left = limb(&mut rig, -2.1, (0.0, hip), 1.8, 2.5, SUIT);
    rig.turn(left, |p| pitch(p, [0.0, hip], swing));
    let right = limb(&mut rig, 2.1, (0.0, hip), 1.8, 2.5, SUIT);
    rig.turn(right, |p| pitch(p, [0.0, hip], -swing));

    // The body.
    rig.part([-4.1, -2.4, 12.0], [4.1, 2.4, 21.0], Tex::Hide, SUIT);
    rig.part([-4.3, -2.6, 17.6], [4.3, 2.6, 18.8], Tex::Hide, BAND);
    rig.part([-4.3, -2.6, 11.8], [4.3, 2.6, 13.0], Tex::Hide, SHADE);
    rig.part([-3.2, -5.0, 13.5], [3.2, -2.4, 21.0], Tex::Hide, SHADE);
    rig.part([-1.2, 2.4, 15.0], [1.2, 2.7, 17.0], Tex::Hide, DARK);

    // Arms: the left swings with the walk; the right swings too, unless it's busy.
    let shoulder = 20.0;
    let left = limb(&mut rig, -5.7, (11.5, shoulder + 1.0), 1.5, 0.0, SUIT);
    rig.turn(left, |p| pitch(p, [0.0, shoulder], -swing));
    let raised = if pose.holding { 0.9 } else { 0.0 };
    let right_angle = swing * (1.0 - raised) + raised + 1.5 * pose.arm;
    let right = limb(&mut rig, 5.7, (11.5, shoulder + 1.0), 1.5, 0.0, SUIT);
    rig.turn(right, |p| pitch(p, [0.0, shoulder], right_angle));

    // The helmet turns up and down with the view.
    let head = rig.mark();
    rig.part([-1.6, -1.6, 21.0], [1.6, 1.6, 22.0], Tex::Hide, SHADE);
    rig.part([-3.6, -3.6, 22.0], [3.6, 3.6, 28.8], Tex::Hide, SUIT);
    rig.part([-2.9, 3.6, 23.4], [2.9, 3.9, 27.6], Tex::Hide, VISOR);
    let head = head.start..rig.mark().end;
    let look = pose.look_pitch.clamp(-1.2, 1.2);
    rig.turn(head, |p| pitch(p, [0.0, 22.0], look));

    if pose.hurt {
        for q in &mut rig.quads {
            q.tint = multiply(q.tint, [1.0, 0.3, 0.3]);
        }
    }

    let right_axis = forward.cross(up);
    let s = 1.0 / 16.0;
    let place = |[x, y, z]: [f32; 3]| {
        right_axis * (x as f64 * s) + forward * (y as f64 * s) + up * (z as f64 * s)
    };
    build(&rig.quads, place, DVec3::ZERO, light, 0xffff_ffff, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figure_is_about_as_tall_as_a_player_and_stays_upright_when_walking() {
        for walk in [0.0, 0.8, 1.6, 3.1] {
            let pose = AvatarPose {
                walk,
                swing: 1.0,
                arm: 0.5,
                look_pitch: 0.4,
                holding: true,
                hurt: walk > 1.0,
            };
            let mesh = avatar_model(&pose, (DVec3::Y, DVec3::Z), Light::default());
            assert!(!mesh.opaque.is_empty());
        }
    }

    #[test]
    fn a_figure_is_hit_where_it_stands() {
        let up = DVec3::Z;
        let feet = DVec3::new(0.0, 3.0, 0.0);
        let eye = DVec3::new(0.0, 0.0, 1.6);
        let ahead = DVec3::Y;
        let t = ray_hit(feet, up, eye, ahead, 4.5).expect("straight at it");
        assert!((t - 2.55).abs() < 1e-6, "{t}");
        assert!(ray_hit(feet, up, eye, ahead, 2.0).is_none(), "out of reach");
        let beside = DVec3::new(1.0, 3.0, 0.0);
        assert!(
            ray_hit(beside, up, eye, ahead, 4.5).is_none(),
            "off to the side"
        );
        let over = eye + DVec3::Z * 0.5;
        assert!(
            ray_hit(feet, up, over, ahead, 4.5).is_none(),
            "over its head"
        );
        assert!(
            ray_hit(feet, up, eye, -ahead, 4.5).is_none(),
            "behind the eye"
        );
    }
}
