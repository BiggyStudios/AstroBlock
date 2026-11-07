//! The animals' models: sculpted from many small boxes rather than a few skinned ones.
//!
//! - **Coats** are near-white tiled textures (`Tex::Fur`, `Patches`, `Wool`, `Hide`) at one
//!   texel per model unit, the same density as blocks, so nothing is stretched. Each part
//!   gets its colour from a per-face tint: coat, muzzle, hooves, eyes, horns.
//! - **Shapes:** a barrel body rounded off by a ridge, belly, chest and rump; a neck; a head
//!   with a separate muzzle or snout, eyes, ears and horns; a tail; legs with hooves.
//! - **Motion** (from `MobPose`): legs swing in diagonal pairs, the body bobs and the head
//!   nods with the walk; standing animals lower their heads to graze, look around, swish
//!   their tails and flick their ears. A hit flashes the whole animal red.
//!
//! Units are model units (1/16 m): x right, y forward (the way it faces), z up, with the
//! origin between the feet.

use std::ops::Range;

use glam::DVec3;

use crate::{
    mobs::{MobKind, MobPose},
    models::{Light, ModelBox, Quad, box_quads, build, cuboid, pitch},
    render::ChunkMesh,
    world::textures::Tex,
};

pub(crate) type Rgb = [u8; 3];

/// Hooves, eyes and nostrils.
const DARK: Rgb = [40, 35, 33];

/// A mob's mesh around its feet, facing `forward` (with `up`).
pub fn mob_model(
    kind: MobKind,
    pose: &MobPose,
    (forward, up): (DVec3, DVec3),
    light: Light,
) -> ChunkMesh {
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 0x2545_f491,
    };
    match kind {
        MobKind::Cow => cow(&mut rig, pose),
        MobKind::Pig => pig(&mut rig, pose),
        MobKind::Sheep => sheep(&mut rig, pose),
    }
    if pose.hurt {
        for q in &mut rig.quads {
            // Patches would stay cream: flash the whole coat.
            if q.tex == Tex::Patches {
                q.tex = Tex::Fur;
            }
            q.tint = multiply(q.tint, [1.0, 0.3, 0.3]);
        }
    }
    let right = forward.cross(up);
    let s = 1.0 / 16.0;
    let place = |[x, y, z]: [f32; 3]| {
        right * (x as f64 * s) + forward * (y as f64 * s) + up * (z as f64 * s)
    };
    build(&rig.quads, place, DVec3::ZERO, light, 0xffff_ffff, false)
}

/// What the animations come to for one frame.
struct Motion {
    /// Leg angles (radians, forward positive): front left, front right, back left, back right.
    legs: [f32; 4],
    /// Body lift (model units).
    bob: f32,
    /// Neck pitch (down negative), the head's own nod, and a turn to look around.
    neck: f32,
    nod: f32,
    look: f32,
    /// Tail swing and ear flick (radians).
    tail: f32,
    ears: f32,
}

impl Motion {
    /// `graze_depth`: how far the neck goes down to reach the grass.
    fn new(pose: &MobPose, graze_depth: f32) -> Self {
        let swing = pose.swing;
        let a = pose.walk.sin() * 0.55 * swing;
        let idle = (1.0 - swing) * (1.0 - pose.graze);
        let t = pose.age;
        // A twitch now and then: a sharp peak of a slow wave.
        let twitch = |speed: f32, offset: f32| ((t * speed + offset).sin().max(0.0)).powi(12);
        Self {
            legs: [a, -a, -a, a],
            bob: pose.walk.cos().abs() * 0.35 * swing,
            neck: -pose.graze * graze_depth + (pose.walk * 2.0).sin() * 0.05 * swing,
            nod: -pose.graze * 0.3
                + (pose.walk * 2.0).sin() * 0.06 * swing
                + twitch(0.7, 1.0) * 0.15 * pose.graze,
            look: idle * ((t * 0.31).sin() * 0.3 + (t * 0.13).sin() * 0.15),
            tail: (t * 2.3).sin() * (0.15 + 0.2 * swing) + twitch(0.9, 2.0) * 0.4,
            ears: twitch(1.1, 0.0) * 0.5,
        }
    }
}

fn cow(rig: &mut Rig, pose: &MobPose) {
    use Tex::*;
    let coat = [[118, 78, 50], [44, 40, 38], [150, 92, 58], [92, 64, 46]][pose.coat as usize % 4];
    let muzzle = [222, 184, 168];
    let horn = [228, 218, 194];
    let m = Motion::new(pose, 1.0);

    for (i, (x, y)) in [(-3.6, 7.0), (3.6, 7.0), (-3.6, -7.5), (3.6, -7.5)]
        .into_iter()
        .enumerate()
    {
        rig.leg(
            [x, y],
            3.2,
            11.5,
            2.0,
            (Patches, coat, DARK),
            None,
            m.legs[i],
        );
    }
    let body = rig.mark();
    rig.part([-5.5, -10.0, 10.0], [5.5, 9.0, 19.5], Patches, coat);
    rig.part([-4.5, -9.5, 19.5], [4.5, 8.5, 20.5], Patches, coat); // back
    rig.part([-4.5, -8.0, 9.0], [4.5, 7.0, 10.0], Patches, coat); // belly
    rig.part([-5.0, 9.0, 11.0], [5.0, 10.5, 19.0], Patches, coat); // chest
    rig.part([-5.0, -11.0, 12.0], [5.0, -10.0, 19.5], Patches, coat); // rump

    let tail = rig.mark();
    rig.part([-0.6, -11.8, 11.0], [0.6, -10.8, 19.5], Fur, coat);
    rig.part([-1.0, -12.2, 8.5], [1.0, -10.4, 11.5], Fur, DARK);
    rig.turn(tail, |p| yaw(p, [0.0, -11.0], m.tail));

    let neck = rig.mark();
    rig.part([-3.0, 9.5, 14.5], [3.0, 14.0, 20.0], Patches, coat);
    let head = rig.mark();
    rig.part([-3.2, 13.0, 14.0], [3.2, 19.0, 21.0], Patches, coat);
    rig.part([-2.8, 19.0, 13.5], [2.8, 21.5, 17.5], Hide, muzzle);
    for side in [-1.0, 1.0] {
        rig.part_x(
            side,
            [1.1, 21.5, 14.6],
            [1.7, 21.6, 15.2],
            Hide,
            [150, 104, 98],
        ); // nostril
        rig.part_x(side, [3.2, 16.4, 18.0], [3.5, 17.6, 19.2], Hide, DARK); // eye
        rig.part_x(side, [2.0, 14.8, 21.0], [3.0, 15.8, 22.6], Hide, horn);
        rig.part_x(side, [3.0, 14.8, 22.0], [4.0, 15.8, 23.0], Hide, horn);
        let ear = rig.mark();
        rig.part_x(side, [3.2, 14.8, 19.2], [6.0, 16.2, 20.4], Patches, coat);
        rig.turn(ear, |p| roll(p, [3.2 * side, 19.8], (m.ears - 0.25) * side));
    }
    rig.turn(head, |p| pitch(p, [16.0, 17.5], m.nod));
    rig.turn(neck.clone(), |p| pitch(p, [10.0, 17.0], m.neck));
    rig.turn(neck.clone(), |p| yaw(p, [0.0, 10.0], m.look));
    // Grazing stretches the neck down to the grass.
    let g = pose.graze;
    rig.turn(neck, |[x, y, z]| [x, y + 1.5 * g, z - 3.5 * g]);
    rig.turn(body, |[x, y, z]| [x, y, z + m.bob]);
}

fn pig(rig: &mut Rig, pose: &MobPose) {
    use Tex::*;
    let coat = [
        [236, 170, 164],
        [244, 192, 184],
        [206, 140, 130],
        [132, 104, 96],
    ][pose.coat as usize % 4];
    let snout = coat.map(|c| (c as f32 * 0.86) as u8);
    let m = Motion::new(pose, 0.55);

    for (i, (x, y)) in [(-3.2, 5.0), (3.2, 5.0), (-3.2, -5.5), (3.2, -5.5)]
        .into_iter()
        .enumerate()
    {
        rig.leg(
            [x, y],
            3.0,
            6.5,
            1.0,
            (Hide, coat, [104, 80, 76]),
            None,
            m.legs[i] * 0.8,
        );
    }
    let body = rig.mark();
    rig.part([-5.0, -8.5, 5.5], [5.0, 7.5, 13.0], Hide, coat);
    rig.part([-4.0, -8.0, 13.0], [4.0, 7.0, 14.0], Hide, coat); // back
    rig.part([-4.0, -7.0, 4.5], [4.0, 6.0, 5.5], Hide, coat); // belly
    rig.part([-4.0, 7.5, 6.0], [4.0, 8.5, 12.0], Hide, coat); // chest
    rig.part([-4.0, -9.5, 6.5], [4.0, -8.5, 12.0], Hide, coat); // rump

    // A curly tail: three little blocks.
    let tail = rig.mark();
    rig.part([-0.5, -10.3, 10.5], [0.5, -9.5, 12.5], Hide, coat);
    rig.part([-0.5, -11.1, 11.7], [0.5, -10.3, 12.5], Hide, coat);
    rig.part([-0.5, -11.1, 10.5], [0.5, -10.3, 11.7], Hide, coat);
    rig.turn(tail, |p| yaw(p, [0.0, -9.5], m.tail * 1.5));

    let head = rig.mark();
    rig.part([-4.0, 8.0, 5.5], [4.0, 13.5, 12.5], Hide, coat);
    rig.part([-2.3, 13.5, 6.3], [2.3, 15.3, 9.7], Hide, snout);
    for side in [-1.0, 1.0] {
        rig.part_x(side, [0.6, 15.3, 7.6], [1.5, 15.5, 8.6], Hide, [96, 54, 58]); // nostril
        rig.part_x(side, [1.8, 13.5, 10.3], [3.0, 13.7, 11.4], Hide, DARK); // eye
        // Floppy ears, folded forward over the eyes.
        let ear = rig.mark();
        rig.part_x(side, [1.2, 9.5, 12.5], [4.0, 12.0, 13.4], Hide, coat);
        rig.turn(ear, |p| pitch(p, [9.5, 13.0], -0.55 + m.ears * 0.6));
    }
    rig.turn(head.clone(), |p| pitch(p, [8.0, 10.0], m.neck + m.nod));
    rig.turn(head, |p| yaw(p, [0.0, 8.0], m.look));
    rig.turn(body, |[x, y, z]| [x, y, z + m.bob * 0.7]);
}

fn sheep(rig: &mut Rig, pose: &MobPose) {
    use Tex::*;
    let wool = [
        [238, 236, 228],
        [232, 228, 218],
        [214, 204, 186],
        [112, 88, 70],
    ][pose.coat as usize % 4];
    // Dark-faced or cream-faced; sheep have amber eyes, which show on a dark face.
    let (face, eye) = if pose.coat.is_multiple_of(2) {
        ([62, 56, 52], [214, 176, 96])
    } else {
        ([214, 198, 176], [44, 36, 30])
    };
    let m = Motion::new(pose, 1.0);

    for (i, (x, y)) in [(-3.0, 6.0), (3.0, 6.0), (-3.0, -6.5), (3.0, -6.5)]
        .into_iter()
        .enumerate()
    {
        let thigh = ([-1.9, -1.9, -3.0], [1.9, 1.9, 0.5], wool);
        rig.leg(
            [x, y],
            2.4,
            10.0,
            1.0,
            (Hide, face, DARK),
            Some(thigh),
            m.legs[i],
        );
    }
    let body = rig.mark();
    rig.part([-6.0, -9.0, 9.0], [6.0, 8.0, 19.0], Wool, wool);
    rig.part([-5.0, -8.0, 19.0], [5.0, 7.0, 20.5], Wool, wool); // back
    rig.part([-5.0, -8.0, 8.0], [5.0, 7.0, 9.0], Wool, wool); // belly
    rig.part([-5.0, 8.0, 10.0], [5.0, 9.5, 18.0], Wool, wool); // chest
    rig.part([-5.0, -10.0, 10.0], [5.0, -9.0, 18.0], Wool, wool); // rump
    for side in [-1.0, 1.0] {
        rig.part_x(side, [6.0, -6.0, 11.0], [6.8, 5.0, 17.0], Wool, wool); // flanks
    }
    let tail = rig.mark();
    rig.part([-1.2, -11.0, 13.5], [1.2, -10.0, 17.5], Wool, wool);
    rig.turn(tail, |p| yaw(p, [0.0, -10.0], m.tail));

    let head = rig.mark();
    rig.part([-2.8, 7.5, 13.0], [2.8, 10.0, 18.5], Wool, wool); // neck
    let face_start = rig.mark();
    rig.part([-2.4, 9.5, 12.5], [2.4, 14.5, 18.5], Hide, face);
    rig.part(
        [-1.8, 14.5, 12.8],
        [1.8, 15.4, 15.2],
        Hide,
        face.map(|c| (c as f32 * 0.85) as u8),
    );
    rig.part([-2.9, 9.0, 18.0], [2.9, 12.5, 20.2], Wool, wool); // cap
    for side in [-1.0, 1.0] {
        rig.part_x(side, [2.4, 11.6, 16.2], [2.7, 12.8, 17.3], Hide, eye);
        let ear = rig.mark();
        rig.part_x(side, [2.4, 10.5, 16.8], [5.0, 12.0, 17.8], Hide, face);
        rig.turn(ear, |p| roll(p, [2.4 * side, 17.3], (m.ears - 0.2) * side));
    }
    rig.turn(face_start, |p| pitch(p, [10.0, 16.0], m.nod));
    rig.turn(head.clone(), |p| pitch(p, [8.5, 15.0], m.neck));
    rig.turn(head.clone(), |p| yaw(p, [0.0, 8.5], m.look));
    let g = pose.graze;
    rig.turn(head, |[x, y, z]| [x, y + 2.0 * g, z - 4.5 * g]);
    rig.turn(body, |[x, y, z]| [x, y, z + m.bob]);
}

/// Builds a model part by part.
pub(crate) struct Rig {
    pub quads: Vec<Quad>,
    /// Varies where each face starts in its texture, so neighbouring parts don't repeat.
    pub salt: u32,
}

impl Rig {
    /// Where the next part starts (for `turn`).
    pub fn mark(&self) -> Range<usize> {
        self.quads.len()..usize::MAX
    }

    /// A box with `tex` tiled at one texel per unit, coloured `rgb`.
    pub fn part(&mut self, from: [f32; 3], to: [f32; 3], tex: Tex, rgb: Rgb) {
        let start = self.quads.len();
        let b: ModelBox = cuboid(from, to, tex, tex, tex);
        box_quads(&b, &mut self.quads);
        let tint = color(rgb);
        for q in &mut self.quads[start..] {
            for axis in 0..2 {
                let lo = q.uv.iter().map(|uv| uv[axis]).fold(f32::MAX, f32::min);
                let hi = q.uv.iter().map(|uv| uv[axis]).fold(f32::MIN, f32::max);
                // Start at a varying texel, staying within the 5-bit texel range.
                self.salt = self
                    .salt
                    .wrapping_mul(747_796_405)
                    .wrapping_add(2_891_336_453);
                let room = (31.0 - (hi - lo).ceil()).clamp(0.0, 15.0) as u32;
                let offset = ((self.salt >> 16) % (room + 1)) as f32;
                for uv in &mut q.uv {
                    uv[axis] += offset - lo;
                }
            }
            q.tint = tint;
        }
    }

    /// `part` on the right (`side` 1) or mirrored onto the left (`side` -1).
    pub fn part_x(&mut self, side: f32, from: [f32; 3], to: [f32; 3], tex: Tex, rgb: Rgb) {
        if side > 0.0 {
            self.part(from, to, tex, rgb);
        } else {
            self.part(
                [-to[0], from[1], from[2]],
                [-from[0], to[1], to[2]],
                tex,
                rgb,
            );
        }
    }

    /// A leg standing at `(x, y)`: `width` wide up to the hip at `hip`, a hoof of `hoof`
    /// units at the bottom (`(tex, rgb)` and `hoof_rgb` colour them), optionally a thigh box (offsets from the hip, colour),
    /// swung by `angle` about the hip.
    #[allow(clippy::too_many_arguments)]
    fn leg(
        &mut self,
        [x, y]: [f32; 2],
        width: f32,
        hip: f32,
        hoof: f32,
        (tex, rgb, hoof_rgb): (Tex, Rgb, Rgb),
        thigh: Option<([f32; 3], [f32; 3], Rgb)>,
        angle: f32,
    ) {
        let leg = self.mark();
        let h = width / 2.0;
        self.part([x - h, y - h, hoof], [x + h, y + h, hip], tex, rgb);
        let g = h + 0.25;
        self.part(
            [x - g, y - g, 0.0],
            [x + g, y + g, hoof],
            Tex::Hide,
            hoof_rgb,
        );
        if let Some((from, to, rgb)) = thigh {
            self.part(
                [x + from[0], y + from[1], hip + from[2]],
                [x + to[0], y + to[1], hip + to[2]],
                Tex::Wool,
                rgb,
            );
        }
        self.turn(leg, |p| pitch(p, [y, hip], angle));
    }

    /// Moves the parts in `range` (from a `mark`) with `f`.
    pub fn turn(&mut self, range: Range<usize>, f: impl Fn([f32; 3]) -> [f32; 3]) {
        let end = range.end.min(self.quads.len());
        for q in &mut self.quads[range.start..end] {
            q.corners = q.corners.map(&f);
        }
    }
}

/// Rotates `p` about the vertical axis through (x, y) = `pivot` (positive: to the left).
fn yaw(p: [f32; 3], pivot: [f32; 2], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (p[0] - pivot[0], p[1] - pivot[1]);
    [pivot[0] + dx * c - dy * s, pivot[1] + dx * s + dy * c, p[2]]
}

/// Rotates `p` about the forward axis through (x, z) = `pivot` (positive: right side up).
fn roll(p: [f32; 3], pivot: [f32; 2], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let (dx, dz) = (p[0] - pivot[0], p[2] - pivot[1]);
    [pivot[0] + dx * c - dz * s, p[1], pivot[1] + dx * s + dz * c]
}

/// An sRGB colour as the linear RGBA tint vertices take.
pub(crate) fn color(rgb: Rgb) -> u32 {
    let linear = |c: u8| {
        let c = c as f32 / 255.0;
        let l = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
        (l * 255.0).round() as u32
    };
    0xff00_0000 | linear(rgb[0]) | (linear(rgb[1]) << 8) | (linear(rgb[2]) << 16)
}

pub(crate) fn multiply(tint: u32, f: [f32; 3]) -> u32 {
    let channel = |i: usize| ((((tint >> (8 * i)) & 0xff) as f32 * f[i]).round() as u32) << (8 * i);
    (tint & 0xff00_0000) | channel(0) | channel(1) | channel(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{models::MODEL_FLAG, world::mesher::POSITION_UNIT};

    #[test]
    fn animals_fit_their_boxes_in_every_pose() {
        for kind in MobKind::ALL {
            let (half_width, height) = kind.size();
            for (walk, swing, graze, coat) in
                [(0.0, 0.0, 0.0, 0), (1.3, 1.0, 0.0, 1), (0.0, 0.0, 1.0, 2)]
            {
                let pose = MobPose {
                    walk,
                    swing,
                    graze,
                    age: 17.0,
                    coat,
                    hurt: coat == 1,
                };
                let mesh = mob_model(kind, &pose, (DVec3::Y, DVec3::Z), Light::default());
                assert!(!mesh.opaque.is_empty() && mesh.opaque.len().is_multiple_of(4));
                for v in &mesh.opaque {
                    let p = DVec3::new(
                        v.position[0] as f64,
                        v.position[1] as f64,
                        v.position[2] as f64,
                    ) * POSITION_UNIT;
                    assert!(v.data & MODEL_FLAG != 0);
                    // x right, y forward, z up; heads stick out ahead of the collision box.
                    assert!(p.x.abs() <= half_width + 0.01, "{kind:?} too wide: {p}");
                    assert!((-1.0..1.4).contains(&p.y), "{kind:?} too long: {p}");
                    assert!(
                        p.z >= -0.05 && p.z <= height + 0.1,
                        "{kind:?} too tall: {p}"
                    );
                }
            }
        }
    }
}
