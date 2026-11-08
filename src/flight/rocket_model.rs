//! The test rocket's model: about 8 m tall and 1.75 m wide, built from boxes like the animals
//! (`mobs::models::Rig`): an engine bell, an octagonal tank with an orange and a dark band,
//! four red fins, and a tapering capsule with a window, a nose cone and an antenna.
//!
//! Model units are 1/16 m with z along the nose and the engine bell's bottom at z = 0.
//! Model x is the vessel's right (+X), model z its nose (+Y) and model y its belly (-Z), so
//! the window (on -y) faces the dorsal side.

use glam::DVec3;

use crate::{
    flight::vessel::COM_HEIGHT,
    mobs::models::{Rgb, Rig, color},
    render::{
        ChunkMesh,
        models::{Light, build},
    },
    world::textures::Tex,
};

const HULL: Rgb = [236, 236, 232];
const ORANGE: Rgb = [235, 120, 40];
const BAND: Rgb = [40, 42, 48];
const FIN: Rgb = [200, 52, 40];
const METAL: Rgb = [92, 94, 102];
const BELL: Rgb = [64, 66, 72];
const WINDOW: Rgb = [40, 70, 110];
/// What hot metal glows like.
const GLOW: Rgb = [255, 120, 50];

/// The rocket's mesh, positioned about its centre of mass. `axes` are the vessel's right,
/// nose and dorsal directions in the world (planet body) frame; `heat` 0..1 makes the hull
/// glow, most at the nose.
pub fn rocket_model(axes: (DVec3, DVec3, DVec3), heat: f32, light: Light) -> ChunkMesh {
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 0x0bad_5eed,
    };
    let octagon = |rig: &mut Rig, half: f32, cut: f32, z: (f32, f32), rgb: Rgb| {
        rig.part([-half, -cut, z.0], [half, cut, z.1], Tex::Hide, rgb);
        rig.part([-cut, -half, z.0], [cut, half, z.1], Tex::Hide, rgb);
    };

    // Engine bell and the plate it hangs from.
    rig.part([-5.0, -5.0, 0.0], [5.0, 5.0, 4.0], Tex::Hide, BELL);
    rig.part([-4.0, -4.0, 4.0], [4.0, 4.0, 8.0], Tex::Hide, BELL);
    rig.part([-3.0, -3.0, 8.0], [3.0, 3.0, 12.0], Tex::Hide, METAL);
    octagon(&mut rig, 11.0, 7.0, (12.0, 16.0), METAL);

    // The tank.
    octagon(&mut rig, 14.0, 9.0, (16.0, 22.0), ORANGE);
    octagon(&mut rig, 14.0, 9.0, (22.0, 58.0), HULL);
    octagon(&mut rig, 14.1, 9.1, (58.0, 61.0), BAND);
    octagon(&mut rig, 14.0, 9.0, (61.0, 86.0), HULL);

    // Fins, swept back.
    for (sx, sy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        // Span a..b out along the fin's axis, 2 units thick.
        let fin = |(a, b): (f32, f32), z: (f32, f32)| {
            let (lo, hi) = if sx + sy > 0.0 { (a, b) } else { (-b, -a) };
            if sx != 0.0 {
                ([lo, -1.0, z.0], [hi, 1.0, z.1])
            } else {
                ([-1.0, lo, z.0], [1.0, hi, z.1])
            }
        };
        for (span, z) in [((13.0, 23.0), (10.0, 28.0)), ((13.0, 18.0), (28.0, 38.0))] {
            let (from, to) = fin(span, z);
            rig.part(from, to, Tex::Hide, FIN);
        }
    }

    // Capsule.
    octagon(&mut rig, 12.0, 8.0, (86.0, 100.0), HULL);
    octagon(&mut rig, 9.0, 6.0, (100.0, 110.0), HULL);
    octagon(&mut rig, 5.0, 4.0, (110.0, 117.0), ORANGE);
    rig.part([-2.0, -2.0, 117.0], [2.0, 2.0, 122.0], Tex::Hide, BAND);
    rig.part([-0.4, -0.4, 122.0], [0.4, 0.4, 130.0], Tex::Hide, METAL);
    rig.part([-4.0, -12.4, 90.0], [4.0, -11.9, 96.0], Tex::Hide, WINDOW);

    // Heat: the hull glows, the nose most.
    if heat > 0.0 {
        let glow = color(GLOW);
        for q in &mut rig.quads {
            let z = q.corners.iter().map(|c| c[2]).sum::<f32>() / 4.0;
            let k = (heat * (0.4 + 0.6 * z / 130.0)).clamp(0.0, 1.0);
            q.tint = mix_tint(q.tint, glow, k);
        }
    }

    let (right, nose, dorsal) = axes;
    let s = 1.0 / 16.0;
    let place = |[x, y, z]: [f32; 3]| {
        right * (x as f64 * s) + nose * (z as f64 * s - COM_HEIGHT) - dorsal * (y as f64 * s)
    };
    build(&rig.quads, place, DVec3::ZERO, light, 0xffff_ffff, false)
}

fn mix_tint(a: u32, b: u32, k: f32) -> u32 {
    let channel = |i: u32| {
        let (x, y) = (
            ((a >> (8 * i)) & 0xff) as f32,
            ((b >> (8 * i)) & 0xff) as f32,
        );
        ((x + (y - x) * k).round() as u32) << (8 * i)
    };
    0xff00_0000 | channel(0) | channel(1) | channel(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flight::vessel::HEIGHT, world::mesher::POSITION_UNIT};

    #[test]
    fn rocket_fits_its_size() {
        let mesh = rocket_model((DVec3::X, DVec3::Y, DVec3::Z), 0.5, Light::default());
        assert!(!mesh.opaque.is_empty() && mesh.opaque.len().is_multiple_of(4));
        for v in &mesh.opaque {
            let p = DVec3::new(
                v.position[0] as f64,
                v.position[1] as f64,
                v.position[2] as f64,
            ) * POSITION_UNIT;
            assert!(p.x.abs() < 1.5 && p.z.abs() < 1.5, "{p}");
            assert!(
                p.y >= -COM_HEIGHT - 0.01 && p.y <= HEIGHT - COM_HEIGHT + 0.2,
                "{p}"
            );
        }
    }
}
