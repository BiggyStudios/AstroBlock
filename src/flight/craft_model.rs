//! What a vehicle looks like: one mesh in its own grid coordinates, rebuilt only when its parts
//! change, and drawn turned into place.
//!
//! Plain parts are cubes (only the faces nothing covers). Engines, wings, seats and wheels
//! are small models, turned to face the way their part does. Model units are 1/16 m around
//! the part's centre.

use glam::{DMat3, DVec3};

use crate::{
    flight::craft::{Craft, Part, center, direction, is_cube},
    mobs::models::{Rgb, Rig},
    render::{
        ChunkMesh,
        models::{Light, Quad, build},
    },
    world::{block::Block, textures::Tex},
};

const DARK: Rgb = [46, 48, 54];
const METAL: Rgb = [132, 136, 146];
const LIGHT_METAL: Rgb = [196, 200, 208];
const WHITE: Rgb = [236, 238, 240];
const RED: Rgb = [200, 58, 46];
const AMBER: Rgb = [240, 170, 50];
const TIRE: Rgb = [38, 38, 42];
const CLOTH: Rgb = [58, 84, 128];
const GOLD: Rgb = [206, 168, 72];
const WOOD: Rgb = [120, 84, 50];

/// Takes a model's +Z to `facing` (keeping everything right-handed).
fn turned_to(facing: DVec3) -> DMat3 {
    let z = facing.normalize();
    let helper = if z.y.abs() > 0.9 { DVec3::Z } else { DVec3::Y };
    let x = helper.cross(z).normalize();
    DMat3::from_cols(x, z.cross(x), z)
}

/// A cube's six faces: (the way it looks, four corners seen from outside, which of the
/// block's three textures).
#[allow(clippy::type_complexity)]
const FACES: [([i16; 3], [[f32; 3]; 4], usize); 6] = [
    (
        [1, 0, 0],
        [
            [8.0, -8.0, 8.0],
            [8.0, -8.0, -8.0],
            [8.0, 8.0, -8.0],
            [8.0, 8.0, 8.0],
        ],
        1,
    ),
    (
        [-1, 0, 0],
        [
            [-8.0, -8.0, -8.0],
            [-8.0, -8.0, 8.0],
            [-8.0, 8.0, 8.0],
            [-8.0, 8.0, -8.0],
        ],
        1,
    ),
    (
        [0, 1, 0],
        [
            [-8.0, 8.0, 8.0],
            [8.0, 8.0, 8.0],
            [8.0, 8.0, -8.0],
            [-8.0, 8.0, -8.0],
        ],
        0,
    ),
    (
        [0, -1, 0],
        [
            [-8.0, -8.0, -8.0],
            [8.0, -8.0, -8.0],
            [8.0, -8.0, 8.0],
            [-8.0, -8.0, 8.0],
        ],
        2,
    ),
    (
        [0, 0, 1],
        [
            [-8.0, -8.0, 8.0],
            [8.0, -8.0, 8.0],
            [8.0, 8.0, 8.0],
            [-8.0, 8.0, 8.0],
        ],
        1,
    ),
    (
        [0, 0, -1],
        [
            [8.0, -8.0, -8.0],
            [-8.0, -8.0, -8.0],
            [-8.0, 8.0, -8.0],
            [8.0, 8.0, -8.0],
        ],
        1,
    ),
];

/// The quads of a cube part, leaving out the faces something covers.
fn cube(craft: &Craft, part: Part, into: &mut Vec<Quad>) {
    let textures = part.block.textures();
    let at = part.at;
    let c = center(at).as_vec3() * 16.0;
    for (normal, corners, which) in FACES {
        let next = [at[0] + normal[0], at[1] + normal[1], at[2] + normal[2]];
        if craft.get(next).is_some_and(|p| is_cube(p.block)) {
            continue;
        }
        let uv = [[0.0, 16.0], [16.0, 16.0], [16.0, 0.0], [0.0, 0.0]];
        into.push(Quad {
            corners: corners.map(|[x, y, z]| [x + c.x, y + c.y, z + c.z]),
            uv,
            tex: textures[which],
            tint: 0,
        });
    }
}

/// Adds boxes (from `shape`, around the part's centre) turned by `turn` and moved into place.
fn shaped(rig: &mut Rig, part: Part, turn: DMat3, shape: impl FnOnce(&mut Rig)) {
    let start = rig.quads.len();
    shape(rig);
    let c = center(part.at) * 16.0;
    for q in &mut rig.quads[start..] {
        for corner in &mut q.corners {
            let p = turn * DVec3::new(corner[0] as f64, corner[1] as f64, corner[2] as f64) + c;
            *corner = [p.x as f32, p.y as f32, p.z as f32];
        }
    }
}

/// A round thing made of boxes around the x axis (wheels): `half` thick, `radius` out.
fn drum(rig: &mut Rig, half: f32, radius: f32, tex: Tex, rgb: Rgb) {
    let cut = radius * 0.42;
    rig.part([-half, -radius, -cut], [half, radius, cut], tex, rgb);
    rig.part([-half, -cut, -radius], [half, cut, radius], tex, rgb);
    let mid = radius * 0.72;
    rig.part([-half, -mid, -mid], [half, mid, mid], tex, rgb);
}

/// Builds a shaped part.
fn model(rig: &mut Rig, part: Part, right: DVec3, up: DVec3) {
    let tex = Tex::Hide;
    let facing = direction(part.facing);
    match part.block {
        Block::RocketEngine => shaped(rig, part, turned_to(facing), |r| {
            r.part([-7.0, -7.0, -8.0], [7.0, 7.0, 0.0], tex, METAL);
            r.part([-4.0, -4.0, 0.0], [4.0, 4.0, 3.0], tex, DARK);
            r.part([-6.0, -6.0, 3.0], [6.0, 6.0, 6.0], tex, DARK);
            r.part([-7.5, -7.5, 6.0], [7.5, 7.5, 9.0], tex, DARK);
        }),
        Block::OrbitalEngine => shaped(rig, part, turned_to(facing), |r| {
            r.part([-5.0, -5.0, -8.0], [5.0, 5.0, -2.0], tex, GOLD);
            r.part([-2.5, -2.5, -2.0], [2.5, 2.5, 2.0], tex, DARK);
            r.part([-5.5, -5.5, 2.0], [5.5, 5.5, 9.0], tex, METAL);
        }),
        Block::JetEngine => shaped(rig, part, turned_to(facing), |r| {
            r.part([-7.0, -7.0, -8.0], [7.0, 7.0, 4.0], tex, LIGHT_METAL);
            r.part([-5.0, -5.0, 4.0], [5.0, 5.0, 9.0], tex, DARK);
            r.part([-7.5, -7.5, -9.0], [7.5, 7.5, -7.0], tex, METAL);
        }),
        Block::Propeller => shaped(rig, part, turned_to(-facing), |r| {
            // A hub against the body and two long blades crossing it.
            r.part([-3.0, -3.0, -8.0], [3.0, 3.0, 5.0], tex, METAL);
            r.part([-2.0, -2.0, 5.0], [2.0, 2.0, 7.0], tex, RED);
            r.part([-22.0, -1.5, 2.0], [22.0, 1.5, 3.5], tex, WOOD);
            r.part([-1.5, -22.0, 2.0], [1.5, 22.0, 3.5], tex, WOOD);
        }),
        Block::Wing => shaped(rig, part, turned_to(facing), |r| {
            r.part([-8.0, -8.0, -1.5], [8.0, 8.0, 1.5], tex, WHITE);
        }),
        Block::Flap => shaped(rig, part, turned_to(facing), |r| {
            r.part([-8.0, -8.0, -1.5], [8.0, 2.0, 1.5], tex, WHITE);
            r.part([-8.0, 2.0, -1.0], [8.0, 8.0, 1.0], tex, AMBER);
        }),
        Block::Seat => shaped(rig, part, seat_turn(facing), |r| {
            // Facing +Z: a base, a cushion and a back behind it.
            r.part([-7.0, -8.0, -6.0], [7.0, -5.0, 7.0], tex, METAL);
            r.part([-6.5, -5.0, -6.0], [6.5, -2.0, 7.0], tex, CLOTH);
            r.part([-7.0, -5.0, -8.0], [7.0, 8.0, -5.0], tex, CLOTH);
        }),
        Block::Wheel => shaped(rig, part, axle_turn(right, up), |r| {
            drum(r, 5.0, 11.2, tex, TIRE);
            drum(r, 5.5, 6.0, tex, METAL);
        }),
        Block::LandingGear => shaped(rig, part, axle_turn(right, up), |r| {
            r.part([-1.5, -2.0, -1.5], [1.5, 8.0, 1.5], tex, LIGHT_METAL);
            r.part([-5.0, 6.0, -5.0], [5.0, 8.0, 5.0], tex, METAL);
            drum(r, 3.5, 11.2, tex, TIRE);
            drum(r, 4.0, 5.0, tex, AMBER);
        }),
        _ => {}
    }
}

/// A seat is upright in the grid and looks along `facing` (+Z of its model).
fn seat_turn(facing: DVec3) -> DMat3 {
    let z = if facing.y.abs() > 0.5 {
        DVec3::Z
    } else {
        facing
    };
    DMat3::from_cols(DVec3::Y.cross(z), DVec3::Y, z)
}

/// Takes the model's x axle onto the vehicle's right, its y onto up.
fn axle_turn(right: DVec3, up: DVec3) -> DMat3 {
    let y = if up.y.abs() > 0.5 { up } else { DVec3::Y };
    let x = if right.y.abs() > 0.5 { DVec3::X } else { right };
    DMat3::from_cols(x, y, x.cross(y))
}

/// The whole vehicle's mesh, around the grid's origin (the core).
pub fn craft_model(craft: &Craft, light: Light) -> ChunkMesh {
    let mut cubes = Vec::new();
    let mut glass = Vec::new();
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 0x51ab_c0de,
    };
    let stats = &craft.stats;
    for part in craft.parts() {
        if part.block == Block::Glass {
            cube(craft, *part, &mut glass);
        } else if is_cube(part.block) {
            cube(craft, *part, &mut cubes);
        } else {
            model(&mut rig, *part, stats.right, stats.up);
        }
    }
    cubes.append(&mut rig.quads);
    let place = |[x, y, z]: [f32; 3]| DVec3::new(x as f64, y as f64, z as f64) / 16.0;
    let mut mesh = build(&cubes, place, DVec3::ZERO, light, 0xffff_ffff, false);
    let see_through = build(&glass, place, DVec3::ZERO, light, 0xffff_ffff, true);
    mesh.cutout = see_through.opaque;
    mesh
}

/// An outline around a part, for the crosshair: twelve thin bars.
pub fn outline(at: [i16; 3], light: Light) -> ChunkMesh {
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 7,
    };
    let (e, t) = (8.05f32, 0.35f32);
    let color: Rgb = [250, 250, 250];
    let tex = Tex::Hide;
    for a in [-1.0f32, 1.0] {
        for b in [-1.0f32, 1.0] {
            rig.part(
                [-e, a * e - t, b * e - t],
                [e, a * e + t, b * e + t],
                tex,
                color,
            );
            rig.part(
                [a * e - t, -e, b * e - t],
                [a * e + t, e, b * e + t],
                tex,
                color,
            );
            rig.part(
                [a * e - t, b * e - t, -e],
                [a * e + t, b * e + t, e],
                tex,
                color,
            );
        }
    }
    let c = center(at).as_vec3() * 16.0;
    let place = |[x, y, z]: [f32; 3]| {
        DVec3::new((x + c.x) as f64, (y + c.y) as f64, (z + c.z) as f64) / 16.0
    };
    build(&rig.quads, place, DVec3::ZERO, light, 0xffff_ffff, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::craft::facing_of;

    #[test]
    fn hidden_faces_are_left_out_and_shapes_stay_near_their_cell() {
        let mut c = Craft::new(4);
        assert!(c.place(Part {
            at: [0, 0, 1],
            block: Block::Hull,
            facing: 0,
        }));
        let two_cubes = craft_model(&c, Light::default());
        // Two touching cubes show ten faces of four corners.
        assert_eq!(two_cubes.opaque.len(), 10 * 4);

        assert!(c.place(Part {
            at: [0, 0, 2],
            block: Block::RocketEngine,
            facing: facing_of(DVec3::Z),
        }));
        let mesh = craft_model(&c, Light::default());
        assert!(mesh.opaque.len().is_multiple_of(4));
        let unit = crate::world::mesher::POSITION_UNIT;
        for v in &mesh.opaque {
            let z = v.position[2] as f64 * unit;
            assert!((-0.6..3.0).contains(&z), "{z}");
        }
        // The nozzle sticks out past its cell, on the side the exhaust leaves.
        let tip = mesh
            .opaque
            .iter()
            .map(|v| v.position[2] as f64 * unit)
            .fold(f64::MIN, f64::max);
        assert!(tip > 2.5, "{tip}");
    }

    #[test]
    fn glass_is_see_through() {
        let mut c = Craft::new(4);
        assert!(c.place(Part {
            at: [1, 0, 0],
            block: Block::Glass,
            facing: 0,
        }));
        let mesh = craft_model(&c, Light::default());
        // The side against the core doesn't show.
        assert_eq!(mesh.cutout.len(), 5 * 4);
    }
}
