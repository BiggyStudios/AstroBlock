//! What a vehicle looks like: one mesh in its own grid coordinates, rebuilt only when its parts
//! change, and drawn turned into place.
//!
//! Plain parts are cubes (only the faces nothing covers). Engines, wings, seats and wheels
//! are small models, turned to face the way their part does. Model units are 1/16 m around
//! the part's centre.
//!
//! What moves (propeller blades, wheels, the back of a flap) is left out of that mesh: each is
//! a mesh of its own around its axle or hinge (`Pivot`), drawn turned by what `Looks` says
//! it is doing, so nothing is rebuilt as it moves.

use glam::{DMat3, DQuat, DVec3};

use crate::{
    flight::craft::{Craft, Part, WHEEL_RADIUS, center, direction, is_cube},
    mobs::models::{Rgb, Rig},
    render::{
        ChunkMesh,
        models::{Light, Quad, build},
    },
    world::{block::Block, textures::Tex},
};

/// How a moving piece moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Propeller blades: spin with the throttle.
    Spin,
    /// A wheel: rolls with the ground going by. Front wheels steer too (about the vehicle's up).
    Roll { steers: bool },
    /// The back half of a flap, on its hinge: follows surface `n`'s deflection
    /// (`Stats::surfaces`, `Vessel::flaps`).
    Flap(usize),
}

/// A moving piece: turns about `axis` (grid axes, unit) through `at` (grid m), where its mesh's
/// origin is.
#[derive(Clone, Copy, Debug)]
pub struct Pivot {
    pub motion: Motion,
    pub at: DVec3,
    pub axis: DVec3,
}

/// How far a flap's back half swings at full deflection (rad): the angle whose sine is the
/// physics' `FLAP_TRAVEL`, a little more so it reads from a distance.
const FLAP_SWING: f64 = 0.45;
/// Propellers at full throttle (rad/s), and the most anything turns in one frame (rad): any
/// faster and the eye sees blades and spokes turning backwards.
const PROP_SPEED: f32 = 30.0;
const MOST_STEP: f32 = 0.6;

/// What the moving pieces are doing right now, eased from frame to frame.
#[derive(Clone, Debug, Default)]
pub struct Looks {
    prop_rate: f32,
    prop_angle: f32,
    wheel_rate: f32,
    wheel_angle: f32,
    steer: f32,
    flaps: Vec<f32>,
}

/// What the vehicle is doing, for `Looks::advance`.
pub struct Doing<'a> {
    /// The engines' throttle, if they are running.
    pub power: f32,
    /// Wheels on the ground, and how fast it goes forward over it (m/s).
    pub touching: bool,
    pub ground_speed: f32,
    /// The front wheels' angle (rad), and each surface's flap deflection (-1..1).
    pub steer: f32,
    pub flaps: &'a [f32],
}

/// Spins an angle on by `rate * dt`, never more than `MOST_STEP`, kept within a turn.
fn turn_on(angle: f32, rate: f32, dt: f32) -> f32 {
    (angle + (rate * dt).clamp(-MOST_STEP, MOST_STEP)).rem_euclid(std::f32::consts::TAU)
}

impl Looks {
    /// One frame on (`dt` in seconds of game time; 0 when paused).
    pub fn advance(&mut self, doing: &Doing, dt: f32) {
        use crate::ui::anim::approach;
        // Propellers spool up in a second or so, and run down slower.
        let target = doing.power * PROP_SPEED;
        let spool = if target > self.prop_rate { 2.0 } else { 0.8 };
        self.prop_rate = approach(self.prop_rate, target, spool, dt);
        self.prop_angle = turn_on(self.prop_angle, self.prop_rate, dt);
        // Wheels go with the ground; in the air they run down.
        let (target, grip) = if doing.touching {
            (doing.ground_speed / WHEEL_RADIUS as f32, 20.0)
        } else {
            (0.0, 0.6)
        };
        self.wheel_rate = approach(self.wheel_rate, target, grip, dt);
        self.wheel_angle = turn_on(self.wheel_angle, self.wheel_rate, dt);
        self.steer = approach(self.steer, doing.steer, 12.0, dt);
        self.flaps.resize(doing.flaps.len(), 0.0);
        for (shown, &wanted) in self.flaps.iter_mut().zip(doing.flaps) {
            *shown = approach(*shown, wanted.clamp(-1.0, 1.0), 14.0, dt);
        }
    }

    /// How a moving piece is turned now, in grid axes (`up`: the vehicle's).
    pub fn turn(&self, pivot: &Pivot, up: DVec3) -> DQuat {
        match pivot.motion {
            Motion::Spin => DQuat::from_axis_angle(pivot.axis, self.prop_angle as f64),
            Motion::Roll { steers } => {
                let roll = DQuat::from_axis_angle(pivot.axis, self.wheel_angle as f64);
                if steers {
                    DQuat::from_axis_angle(up, -self.steer as f64) * roll
                } else {
                    roll
                }
            }
            Motion::Flap(n) => {
                let d = self.flaps.get(n).copied().unwrap_or(0.0) as f64;
                DQuat::from_axis_angle(pivot.axis, d * FLAP_SWING)
            }
        }
    }
}

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

/// The boxes from `shape` turned by `turn`, around a pivot (model units).
fn piece(salt: u32, turn: DMat3, shape: impl FnOnce(&mut Rig)) -> Vec<Quad> {
    let mut rig = Rig {
        quads: Vec::new(),
        salt,
    };
    shape(&mut rig);
    for q in &mut rig.quads {
        for corner in &mut q.corners {
            let p = turn * DVec3::new(corner[0] as f64, corner[1] as f64, corner[2] as f64);
            *corner = [p.x as f32, p.y as f32, p.z as f32];
        }
    }
    rig.quads
}

/// The vehicle as a whole, for its moving pieces.
struct Whole<'a> {
    craft: &'a Craft,
    /// The centre of mass with full tanks: wheels ahead of it steer.
    com: DVec3,
}

/// Builds a shaped part; what moves goes into `moving`.
fn model(rig: &mut Rig, part: Part, whole: &Whole, moving: &mut Vec<(Pivot, Vec<Quad>)>) {
    let tex = Tex::Hide;
    let stats = &whole.craft.stats;
    let (right, up, forward) = (stats.right, stats.up, stats.forward);
    let facing = direction(part.facing);
    let c = center(part.at);
    let salt = rig.salt;
    let wheel = |moving: &mut Vec<_>, shape: &dyn Fn(&mut Rig)| {
        let pivot = Pivot {
            motion: Motion::Roll {
                steers: c.dot(forward) > whole.com.dot(forward),
            },
            at: c,
            axis: up.cross(forward).normalize_or(DVec3::X),
        };
        moving.push((pivot, piece(salt, axle_turn(right, up), shape)));
    };
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
        Block::Propeller => {
            // A hub against the body, and the blades (two crossing it) and spinner turning.
            let turn = turned_to(-facing);
            shaped(rig, part, turn, |r| {
                r.part([-3.0, -3.0, -8.0], [3.0, 3.0, 2.0], tex, METAL);
            });
            let blades = piece(salt, turn, |r| {
                r.part([-3.0, -3.0, 2.0], [3.0, 3.0, 5.0], tex, METAL);
                r.part([-2.0, -2.0, 5.0], [2.0, 2.0, 7.0], tex, RED);
                r.part([-22.0, -1.5, 2.0], [22.0, 1.5, 3.5], tex, WOOD);
                r.part([-1.5, -22.0, 2.0], [1.5, 22.0, 3.5], tex, WOOD);
                // (A tip painted on one blade, so the eye can follow the turning.)
                r.part([18.0, -1.6, 1.9], [22.1, 1.6, 3.6], tex, AMBER);
            });
            let pivot = Pivot {
                motion: Motion::Spin,
                at: c,
                axis: facing,
            };
            moving.push((pivot, blades));
        }
        Block::Wing => shaped(rig, part, turned_to(facing), |r| {
            r.part([-8.0, -8.0, -1.5], [8.0, 8.0, 1.5], tex, WHITE);
        }),
        Block::Flap => {
            // The front stays; the back (amber) swings on a hinge across the flow.
            let back = (-forward - facing * facing.dot(-forward))
                .try_normalize()
                .unwrap_or(turned_to(facing).y_axis);
            let turn = DMat3::from_cols(back.cross(facing), back, facing);
            shaped(rig, part, turn, |r| {
                r.part([-8.0, -8.0, -1.5], [8.0, 2.0, 1.5], tex, WHITE);
            });
            let flap = piece(salt, turn, |r| {
                r.part([-8.0, 0.0, -1.0], [8.0, 6.0, 1.0], tex, AMBER);
            });
            let n = stats
                .surfaces
                .iter()
                .position(|s| s.position == c)
                .unwrap_or(usize::MAX);
            let pivot = Pivot {
                motion: Motion::Flap(n),
                at: c + back * (2.0 / 16.0),
                // Turning this way takes the back edge away from the side it pushes towards.
                axis: facing.cross(back),
            };
            moving.push((pivot, flap));
        }
        Block::Seat => shaped(rig, part, seat_turn(facing), |r| {
            // Facing +Z: a base, a cushion and a back behind it.
            r.part([-7.0, -8.0, -6.0], [7.0, -5.0, 7.0], tex, METAL);
            r.part([-6.5, -5.0, -6.0], [6.5, -2.0, 7.0], tex, CLOTH);
            r.part([-7.0, -5.0, -8.0], [7.0, 8.0, -5.0], tex, CLOTH);
        }),
        Block::Wheel => wheel(moving, &|r| {
            drum(r, 5.0, 11.2, tex, TIRE);
            drum(r, 5.5, 6.0, tex, METAL);
            // A spoke across the hub, so it is seen to turn.
            r.part([-5.8, -1.0, -5.5], [5.8, 1.0, 5.5], tex, LIGHT_METAL);
        }),
        Block::LandingGear => {
            shaped(rig, part, axle_turn(right, up), |r| {
                r.part([-1.5, -2.0, -1.5], [1.5, 8.0, 1.5], tex, LIGHT_METAL);
                r.part([-5.0, 6.0, -5.0], [5.0, 8.0, 5.0], tex, METAL);
            });
            wheel(moving, &|r| {
                drum(r, 3.5, 11.2, tex, TIRE);
                drum(r, 4.0, 5.0, tex, AMBER);
                r.part([-4.3, -0.8, -4.6], [4.3, 0.8, 4.6], tex, METAL);
            });
        }
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

/// The whole vehicle's mesh, around the grid's origin (the core), and its moving pieces, each
/// around its own pivot.
pub fn craft_model(craft: &Craft, light: Light) -> (ChunkMesh, Vec<(Pivot, ChunkMesh)>) {
    let mut cubes = Vec::new();
    let mut glass = Vec::new();
    let mut rig = Rig {
        quads: Vec::new(),
        salt: 0x51ab_c0de,
    };
    let whole = Whole {
        craft,
        com: craft.massing(craft.stats.capacity).com,
    };
    let mut moving = Vec::new();
    for part in craft.parts() {
        if part.block == Block::Glass {
            cube(craft, *part, &mut glass);
        } else if is_cube(part.block) {
            cube(craft, *part, &mut cubes);
        } else {
            model(&mut rig, *part, &whole, &mut moving);
        }
    }
    cubes.append(&mut rig.quads);
    let place = |[x, y, z]: [f32; 3]| DVec3::new(x as f64, y as f64, z as f64) / 16.0;
    let mut mesh = build(&cubes, place, DVec3::ZERO, light, 0xffff_ffff, false);
    let see_through = build(&glass, place, DVec3::ZERO, light, 0xffff_ffff, true);
    mesh.cutout = see_through.opaque;
    let moving = moving
        .into_iter()
        .map(|(pivot, quads)| {
            let mesh = build(&quads, place, DVec3::ZERO, light, 0xffff_ffff, false);
            (pivot, mesh)
        })
        .collect();
    (mesh, moving)
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
        let two_cubes = craft_model(&c, Light::default()).0;
        // Two touching cubes show ten faces of four corners.
        assert_eq!(two_cubes.opaque.len(), 10 * 4);

        assert!(c.place(Part {
            at: [0, 0, 2],
            block: Block::RocketEngine,
            facing: facing_of(DVec3::Z),
        }));
        let mesh = craft_model(&c, Light::default()).0;
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
    fn propellers_wheels_and_flaps_move_on_their_own() {
        let mut c = Craft::new(4);
        let parts = [
            ([0, 0, 1], Block::Propeller, DVec3::Z),
            ([1, 0, 0], Block::Wheel, DVec3::X),
            ([0, 0, -1], Block::Flap, DVec3::Y),
        ];
        for (at, block, facing) in parts {
            assert!(c.place(Part {
                at,
                block,
                facing: facing_of(facing),
            }));
        }
        let (_, moving) = craft_model(&c, Light::default());
        let motions: Vec<Motion> = moving.iter().map(|(p, _)| p.motion).collect();
        assert_eq!(motions.len(), 3, "{motions:?}");
        assert!(motions.contains(&Motion::Spin));
        assert!(motions.iter().any(|m| matches!(m, Motion::Roll { .. })));
        let flap = moving
            .iter()
            .find_map(|(p, _)| matches!(p.motion, Motion::Flap(_)).then_some(*p))
            .expect("a flap");
        let Motion::Flap(n) = flap.motion else {
            unreachable!()
        };
        assert_eq!(c.stats.surfaces[n].position, center([0, 0, -1]));
        for (_, mesh) in &moving {
            assert!(!mesh.opaque.is_empty());
        }

        // A flap pushing towards its normal swings its back edge the other way.
        let mut looks = Looks::default();
        let flaps = vec![1.0];
        let doing = Doing {
            power: 1.0,
            touching: false,
            ground_speed: 0.0,
            steer: 0.0,
            flaps: &flaps,
        };
        for _ in 0..60 {
            looks.advance(&doing, 1.0 / 60.0);
        }
        let back = (flap.at - center([0, 0, -1])).normalize();
        let normal = c.stats.surfaces[n].normal;
        let swung = looks.turn(
            &Pivot {
                motion: Motion::Flap(0),
                ..flap
            },
            DVec3::Y,
        ) * back;
        assert!(swung.dot(normal) < -0.3, "{swung}");
        assert!(
            looks.prop_rate > 0.5 * PROP_SPEED,
            "spooling up: {}",
            looks.prop_rate
        );
    }

    #[test]
    fn glass_is_see_through() {
        let mut c = Craft::new(4);
        assert!(c.place(Part {
            at: [1, 0, 0],
            block: Block::Glass,
            facing: 0,
        }));
        let mesh = craft_model(&c, Light::default()).0;
        // The side against the core doesn't show.
        assert_eq!(mesh.cutout.len(), 5 * 4);
    }
}
