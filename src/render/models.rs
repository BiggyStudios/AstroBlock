//! Small 3D models made of textured boxes, Minecraft style (16 units = 1 block, 1 texel per
//! unit): the crafting table, furnace, chest and torch, dropped items, and mobs.
//!
//! Models become ordinary `ChunkMesh`es drawn with the block shaders. Their vertices set
//! `MODEL_FLAG`, so the shader takes the texture coordinates (in texels) from the vertex
//! instead of repeating the texture per block; `CUTOUT_FLAG` makes transparent sprite pixels
//! see-through. Placed models are mapped into their block cell's real (slightly curved and
//! skewed) shape by trilinear interpolation of its 8 corners, so they fit the grid exactly.

use glam::{DVec3, Vec3};

use crate::{
    factory::fluids::Fluid,
    render::{BlockVertex, ChunkMesh},
    survival::items::{Icon, Item},
    world::{
        block::{Block, DoorKind, Tint},
        chunk::BlockCell,
        mesher::POSITION_UNIT,
        planet::Planet,
        textures::{Tex, pixels},
    },
};

/// Vertex data bit: texture coordinates are per vertex, in texels (bits 16-20 and 21-25).
pub const MODEL_FLAG: u32 = 1 << 26;
/// Vertex data bit: texture alpha is transparency (discard below 0.5), no biome tint.
pub const CUTOUT_FLAG: u32 = 1 << 27;

/// Light at a model: the mesher's sky light estimate and block light (both 0-15).
#[derive(Clone, Copy, Debug, Default)]
pub struct Light {
    pub sky: u32,
    pub block: u32,
}

/// How a placed model looks right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelState {
    /// Turns the model's front (-y) towards -v, +u, +v, -u for 0..3.
    pub facing: u8,
    /// A furnace burns (its door glows).
    pub lit: bool,
    /// A torch hangs on a wall (behind it, +y) instead of standing.
    pub wall: bool,
    /// A chest's lid is up.
    pub open: bool,
    /// A conveyor bends round from its +x (1) or -x (2) side (see `factory`).
    #[serde(default)]
    pub bend: u8,
    /// How full an accumulator is, 0..=`LEVELS` (its gauge).
    #[serde(default)]
    pub level: u8,
    /// Which cell of a door this is: across + 3 x up, from the bottom left seen from its front.
    #[serde(default)]
    pub piece: u8,
    /// A pipe: which ways it joins (bits: +u, -u, +v, -v, up, down; see `factory::fluids`).
    #[serde(default)]
    pub links: u8,
    /// A cell of a big machine other than its first: how far from that one it is (grid steps
    /// u, v, k). Only the first draws the machine.
    #[serde(default)]
    pub offset: [i8; 3],
    /// A storage tank: which fluid fills its gauge (`Fluid` + 1; 0 none), to `level`.
    #[serde(default)]
    pub fluid: u8,
    /// A reactor overheating: its warning light is on.
    #[serde(default)]
    pub alarm: bool,
    /// A pipe: which of its joins (`links`) are machines (a flange there). Its line's fluid is
    /// `fluid` (a band of its colour).
    #[serde(default)]
    pub ports: u8,
    /// A pipe joined to nothing (or one way): which way it runs, along u (0), v (1) or up (2).
    #[serde(default)]
    pub axis: u8,
}

impl ModelState {
    /// The first cell of a big machine (the one that draws it).
    pub fn is_core(&self) -> bool {
        self.offset == [0; 3]
    }
}

/// Steps of an accumulator's gauge (`ModelState::level`).
pub const LEVELS: u8 = 20;

/// A box in model units (0..16 = one block; x, y across, z up), with a texture per face.
/// Texture coordinates follow the box's position in the block (like Minecraft's models),
/// or span each whole face when `full_uv` is set.
#[derive(Clone, Copy)]
pub(crate) struct ModelBox {
    from: [f32; 3],
    to: [f32; 3],
    top: Tex,
    bottom: Tex,
    side: Tex,
    /// The -y face, the model's front (and the +y face, its back).
    front: Tex,
    back: Tex,
    full_uv: bool,
}

pub(crate) fn cuboid(from: [f32; 3], to: [f32; 3], top: Tex, side: Tex, bottom: Tex) -> ModelBox {
    ModelBox {
        from,
        to,
        top,
        bottom,
        side,
        front: side,
        back: side,
        full_uv: false,
    }
}

/// A box with one texture stretched over every face (the first-person arm).
fn skin(from: [f32; 3], to: [f32; 3], tex: Tex) -> ModelBox {
    ModelBox {
        full_uv: true,
        ..cuboid(from, to, tex, tex, tex)
    }
}

fn crafting_table() -> Vec<ModelBox> {
    use Tex::*;
    let leg = |x: f32, y: f32| {
        cuboid(
            [x, y, 0.0],
            [x + 3.0, y + 3.0, 12.0],
            LogTop,
            OakLogSide,
            LogTop,
        )
    };
    vec![
        // Thick top with the crafting grid, and an apron below it.
        cuboid(
            [0.0, 0.0, 12.0],
            [16.0, 16.0, 16.0],
            CraftingTop,
            CraftingSide,
            OakPlanks,
        ),
        cuboid(
            [1.5, 1.5, 9.0],
            [14.5, 14.5, 12.0],
            OakPlanks,
            CraftingSide,
            OakPlanks,
        ),
        leg(1.0, 1.0),
        leg(12.0, 1.0),
        leg(1.0, 12.0),
        leg(12.0, 12.0),
        // A shelf between the legs.
        cuboid(
            [2.0, 2.0, 3.0],
            [14.0, 14.0, 4.5],
            OakPlanks,
            OakPlanks,
            OakPlanks,
        ),
    ]
}

fn furnace(lit: bool) -> Vec<ModelBox> {
    use Tex::*;
    let front = if lit { FurnaceFrontLit } else { FurnaceFront };
    vec![
        // A stone plinth, the firebox with its door at the front, a rim and a chimney.
        cuboid([0.0, 0.0, 0.0], [16.0, 16.0, 2.0], Stone, Stone, Stone),
        ModelBox {
            front,
            ..cuboid(
                [1.0, 1.0, 2.0],
                [15.0, 15.0, 12.0],
                Cobblestone,
                Cobblestone,
                Stone,
            )
        },
        cuboid(
            [0.5, 0.5, 12.0],
            [15.5, 15.5, 13.5],
            FurnaceTop,
            Stone,
            Stone,
        ),
        cuboid(
            [9.0, 9.0, 13.5],
            [13.0, 13.0, 16.0],
            FurnaceTop,
            Cobblestone,
            Stone,
        ),
    ]
}

/// A chest: the body, and the lid with its latch (true = part of the lid, which opens).
fn chest() -> Vec<(ModelBox, bool)> {
    use Tex::*;
    vec![
        (
            cuboid(
                [1.0, 1.0, 0.0],
                [15.0, 15.0, 10.0],
                ChestTop,
                ChestSide,
                ChestTop,
            ),
            false,
        ),
        (
            cuboid(
                [1.0, 1.0, 10.0],
                [15.0, 15.0, 14.0],
                ChestTop,
                ChestSide,
                ChestTop,
            ),
            true,
        ),
        (
            cuboid(
                [7.0, 0.0, 7.0],
                [9.0, 1.0, 11.0],
                InnerCore,
                InnerCore,
                InnerCore,
            ),
            true,
        ),
    ]
}

fn torch() -> ModelBox {
    cuboid(
        [7.0, 7.0, 0.0],
        [9.0, 9.0, 10.0],
        Tex::TorchTop,
        Tex::TorchSide,
        Tex::TorchSide,
    )
}

/// Height of a conveyor's belt (model units): what it carries rides on it.
pub const BELT_TOP: f32 = 3.5;
/// Where cables meet a power pole and a machine (model units above the cell's floor, at the
/// middle of the cell).
pub const POLE_TOP: f32 = 33.0;
pub const MACHINE_TOP: f32 = 15.0;
pub const MINER_TOP: f32 = 78.0;
pub const REFINERY_TOP: f32 = 13.0;
/// How far a miner's drill goes down and comes back (model units).
const DRILL_STROKE: f32 = 50.0;

/// A conveyor: a belt between two rails, its arrow towards the front (-y).
fn conveyor() -> Vec<ModelBox> {
    use Tex::*;
    let rail = |x: f32| {
        cuboid(
            [x, 0.0, 0.0],
            [x + 1.0, 16.0, 5.0],
            MachineTop,
            MachineSide,
            MachineTop,
        )
    };
    let roller = |y: f32| {
        cuboid(
            [1.0, y, 0.5],
            [15.0, y + 1.5, 2.0],
            MachineTop,
            MachineTop,
            MachineTop,
        )
    };
    vec![
        ModelBox {
            full_uv: true,
            ..cuboid(
                [1.0, 0.0, 2.0],
                [15.0, 16.0, BELT_TOP],
                ConveyorBelt,
                CableSkin,
                MachineSide,
            )
        },
        rail(0.0),
        rail(15.0),
        roller(0.5),
        roller(14.0),
    ]
}

/// A wooden pole two blocks tall, with a cross arm and an insulator on top for the cables.
fn power_pole() -> Vec<ModelBox> {
    use Tex::*;
    vec![
        cuboid(
            [7.0, 7.0, 0.0],
            [9.0, 9.0, 31.0],
            LogTop,
            OakLogSide,
            LogTop,
        ),
        cuboid(
            [2.0, 7.25, 26.0],
            [14.0, 8.75, 27.5],
            LogTop,
            OakLogSide,
            LogTop,
        ),
        cuboid(
            [2.5, 7.0, 27.5],
            [4.0, 9.0, 29.0],
            Insulator,
            Insulator,
            Insulator,
        ),
        cuboid(
            [12.0, 7.0, 27.5],
            [13.5, 9.0, 29.0],
            Insulator,
            Insulator,
            Insulator,
        ),
        cuboid(
            [7.25, 7.25, 31.0],
            [8.75, 8.75, POLE_TOP + 0.5],
            Insulator,
            Insulator,
            Insulator,
        ),
    ]
}

/// A steel housing on a plinth with `front` at the front, and an insulator on top where
/// cables connect.
fn machine(front: Tex, chimney: bool) -> Vec<ModelBox> {
    use Tex::*;
    let mut boxes = housing(front, MachineSide, MachineTop);
    if chimney {
        boxes.push(cuboid(
            [10.5, 10.5, 13.5],
            [14.0, 14.0, 16.0],
            MachineTop,
            CableSkin,
            MachineTop,
        ));
    }
    boxes
}

/// A housing on a plinth, `side` and `top` its panels, `front` at the front, with an insulator
/// on top.
fn housing(front: Tex, side: Tex, top: Tex) -> Vec<ModelBox> {
    use Tex::*;
    vec![
        cuboid([0.0, 0.0, 0.0], [16.0, 16.0, 1.5], top, top, top),
        ModelBox {
            front,
            ..cuboid([1.0, 1.0, 1.5], [15.0, 15.0, 12.5], top, side, top)
        },
        cuboid([0.5, 0.5, 12.5], [15.5, 15.5, 13.5], top, top, top),
        cuboid(
            [7.0, 7.0, 13.5],
            [9.0, 9.0, MACHINE_TOP],
            Insulator,
            Insulator,
            Insulator,
        ),
    ]
}

/// A steel box from `from` to `to`.
fn steel(from: [f32; 3], to: [f32; 3]) -> ModelBox {
    cuboid(from, to, Tex::MachineTop, Tex::MachineSide, Tex::MachineTop)
}

/// A dark box (graphite, rods, rubber).
fn dark(from: [f32; 3], to: [f32; 3]) -> ModelBox {
    cuboid(from, to, Tex::CableSkin, Tex::CableSkin, Tex::CableSkin)
}

/// A face from 4 corners, wound so that it faces along `outward`.
fn oriented(corners: [[f32; 3]; 4], uv: [[f32; 2]; 4], tex: Tex, outward: Vec3) -> Quad {
    let c = corners.map(Vec3::from);
    let (mut corners, mut uv) = (corners, uv);
    if (c[1] - c[0]).cross(c[2] - c[0]).dot(outward) < 0.0 {
        corners.reverse();
        uv.reverse();
    }
    Quad {
        corners,
        uv,
        tex,
        tint: 0,
    }
}

/// A square beam of half-width `w` from `a` to `b` (model units; any direction), in pieces of
/// at most a block so its texels don't run out.
fn beam(a: [f32; 3], b: [f32; 3], w: f32, tex: Tex, out: &mut Vec<Quad>) {
    let (a, b) = (Vec3::from(a), Vec3::from(b));
    let d = b - a;
    let length = d.length();
    if length < 1e-4 {
        return;
    }
    let along = d / length;
    let across = if along.z.abs() < 0.9 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let s = along.cross(across).normalize() * w;
    let t = along.cross(s).normalize() * w;
    let pieces = (length / 16.0).ceil().max(1.0) as usize;
    let l = length / pieces as f32;
    let w2 = 2.0 * w;
    for i in 0..pieces {
        let p0 = a + along * (l * i as f32);
        let p1 = p0 + along * l;
        for (e, f) in [(s, t), (t, -s), (-s, -t), (-t, s)] {
            let corners = [p0 + e - f, p0 + e + f, p1 + e + f, p1 + e - f].map(|p| p.to_array());
            let uv = [[0.0, l], [w2, l], [w2, 0.0], [0.0, 0.0]];
            out.push(oriented(corners, uv, tex, e));
        }
    }
    for (p, n) in [(a, -along), (b, along)] {
        let corners = [p - s - t, p + s - t, p + s + t, p - s + t].map(|p| p.to_array());
        out.push(oriented(
            corners,
            [[0.0, w2], [w2, w2], [w2, 0.0], [0.0, 0.0]],
            tex,
            n,
        ));
    }
}

/// The faces of boxes.
fn quads_of(boxes: &[ModelBox]) -> Vec<Quad> {
    let mut quads = Vec::new();
    for b in boxes {
        box_quads(b, &mut quads);
    }
    quads
}

/// Turns quads by `angle` about the vertical line through (x, y).
fn turn_z(quads: &mut [Quad], [cx, cy]: [f32; 2], angle: f32) {
    let (s, c) = angle.sin_cos();
    transform(quads, |[x, y, z]| {
        let (dx, dy) = (x - cx, y - cy);
        [cx + dx * c - dy * s, cy + dx * s + dy * c, z]
    });
}

/// Gives quads their own linear tint (see `Quad::tint`) from an sRGB colour.
fn tinted(mut quads: Vec<Quad>, [r, g, b]: [u8; 3]) -> Vec<Quad> {
    let linear = |c: u8| ((c as f32 / 255.0).powi(2) * 255.0).round() as u32;
    let tint = 0xff00_0000 | linear(b) << 16 | linear(g) << 8 | linear(r);
    for q in &mut quads {
        q.tint = tint;
    }
    quads
}

/// A small lamp on a machine's front, lit or dark.
fn lamp(from: [f32; 3], to: [f32; 3], color: [u8; 3]) -> Vec<Quad> {
    let lamp = ModelBox {
        full_uv: true,
        ..cuboid(from, to, Tex::Lamp, Tex::Lamp, Tex::Lamp)
    };
    tinted(quads_of(&[lamp]), color)
}

/// What lights up on the front (-y) face at `y` of a box spanning `x0..x1` and `z0..z1` (its
/// texels positional) while the machine works: quads over the pixels of `lit` that are much
/// brighter than in `unlit` (windows, flames, screens), in exactly the texels the face shows.
fn glow_face(lit: Tex, unlit: Tex, y: f32, [x0, x1]: [f32; 2], [z0, z1]: [f32; 2]) -> Vec<Quad> {
    let (on, off) = (pixels(lit), pixels(unlit));
    let bright = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
    let glows = |x: usize, y: usize| bright(on[y * 16 + x]) - bright(off[y * 16 + x]) > 60;
    // The face's texels as `build` rounds them, and back to model units.
    let (u0, u1) = (x0.round(), x1.round());
    let (v0, v1) = ((16.0 - z1).round(), (16.0 - z0).round());
    let x_at = |u: f32| x0 + (u - u0) / (u1 - u0) * (x1 - x0);
    let z_at = |v: f32| z1 - (v - v0) / (v1 - v0) * (z1 - z0);
    let mut quads = Vec::new();
    for py in (v0 as usize)..(v1 as usize).min(16) {
        let mut px = u0 as usize;
        while px < (u1 as usize).min(16) {
            if !glows(px, py) {
                px += 1;
                continue;
            }
            let start = px;
            while px < (u1 as usize).min(16) && glows(px, py) {
                px += 1;
            }
            let (ua, ub, va, vb) = (start as f32, px as f32, py as f32, py as f32 + 1.0);
            let (xa, xb, za, zb) = (x_at(ua), x_at(ub), z_at(vb), z_at(va));
            let y = y - 0.08;
            quads.push(Quad {
                corners: [[xa, y, za], [xb, y, za], [xb, y, zb], [xa, y, zb]],
                uv: [[ua, vb], [ub, vb], [ub, va], [ua, va]],
                tex: lit,
                tint: 0,
            });
        }
    }
    quads
}

/// The front of `housing`: (y, x range, z range) for `glow_face`.
const HOUSING_FRONT: (f32, [f32; 2], [f32; 2]) = (1.0, [1.0, 15.0], [1.5, 12.5]);

/// How a moving part of a machine moves (see `MovingPart`, `Motion::pose`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Turns about its axis, `0` radians per second of work.
    Spin(f32),
    /// A miner's drill string: down into the ground turning, then back up.
    Drill,
    /// What drives the drill: down and up with it.
    Carriage,
    /// An oil pump's beam, rocking on its frame.
    Rock,
    /// The pump's rod, up and down with the beam's head.
    Stroke,
    /// A beacon: blinks while the machine works.
    Blink,
    /// Fire or an arc behind a window: flickers while the machine works, `0` times a second.
    Flicker(f32),
    // Doors: `speed` is how far open (0..1), `work` 1 while it moves.
    /// A door's leaf, swinging a quarter turn on its hinge.
    Swing,
    /// A sliding door's leaf, along its axis.
    Slide,
    /// A slat of a big door's shutter (its bottom `0` model units up the door): it rolls up
    /// into the housing over the door, out of sight.
    Shutter(f32),
    /// A warning lamp, blinking while the door moves.
    Warn,
}

/// Where a moving part is drawn: moved `lift` model units up and `shift` along its axis,
/// turned `angle` about its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartPose {
    pub lift: f32,
    pub angle: f32,
    pub shown: bool,
    pub shift: f32,
}

/// How far a sliding door's leaf goes, and how high a big door rolls up (model units).
pub const SLIDE: f32 = 15.0;
pub const SHUTTER: f32 = 48.0;

/// Seconds of work a miner's drill takes to go down and come back.
const DRILL_CYCLE: f64 = 6.0;
/// How far the pump's beam rocks either way (radians), and how fast (radians a second).
const ROCK: f32 = 0.24;
const ROCK_RATE: f64 = 2.0;

impl Motion {
    /// The pose after `work` seconds of working (slowed by what it was given), working at
    /// `speed` (0..1) now; `clock` is real seconds and `seed` keeps machines out of step.
    pub fn pose(self, work: f64, speed: f32, clock: f64, seed: u64) -> PartPose {
        use std::f64::consts::TAU;
        let still = PartPose {
            lift: 0.0,
            angle: 0.0,
            shown: true,
            shift: 0.0,
        };
        let open = speed.clamp(0.0, 1.0);
        let rock = || ROCK * ((work * ROCK_RATE) % TAU).sin() as f32;
        match self {
            Motion::Spin(rate) => PartPose {
                angle: ((work * rate as f64) % TAU) as f32,
                ..still
            },
            Motion::Drill | Motion::Carriage => {
                // Slowly down, quickly back up.
                let u = (work / DRILL_CYCLE).fract() as f32;
                let down = if u < 0.82 {
                    u / 0.82
                } else {
                    0.5 + 0.5 * (std::f32::consts::PI * (u - 0.82) / 0.18).cos()
                };
                let turning = self == Motion::Drill;
                PartPose {
                    lift: -DRILL_STROKE * down,
                    angle: if turning {
                        ((work * 9.0) % TAU) as f32
                    } else {
                        0.0
                    },
                    ..still
                }
            }
            Motion::Rock => PartPose {
                angle: rock(),
                ..still
            },
            // (The head is 6 units in front of the beam's pivot: see `oil_pump_parts`.)
            Motion::Stroke => PartPose {
                lift: -6.0 * rock().sin(),
                ..still
            },
            Motion::Blink => PartPose {
                shown: speed > 0.5 && (clock + (seed % 97) as f64 * 0.01) % 1.4 < 0.45,
                ..still
            },
            Motion::Flicker(rate) => {
                let step = (clock * rate as f64).floor() as i64 as u64;
                let roll = crate::world::noise::mix64(step ^ seed.wrapping_mul(0x9e37)) % 100;
                PartPose {
                    shown: speed > 0.3 && roll < 72,
                    ..still
                }
            }
            Motion::Swing => PartPose {
                // (Eased, so it slows into its stops.)
                angle: std::f32::consts::FRAC_PI_2 * open * open * (3.0 - 2.0 * open),
                ..still
            },
            Motion::Slide => PartPose {
                shift: SLIDE * open * open * (3.0 - 2.0 * open),
                ..still
            },
            Motion::Shutter(bottom) => {
                let lift = SHUTTER * open;
                PartPose {
                    lift,
                    shown: bottom + lift < SHUTTER - 0.5,
                    ..still
                }
            }
            Motion::Warn => PartPose {
                shown: work > 0.5 && clock % 0.6 < 0.32,
                ..still
            },
        }
    }
}

/// A piece of a machine that moves: drawn on its own each frame (`part_model`), moved and
/// turned about `pivot` (model units) as its `motion` says. `glow`: lit by itself.
pub struct MovingPart {
    pub quads: Vec<Quad>,
    pub glow: bool,
    pub pivot: [f32; 3],
    pub axis: [f32; 3],
    pub motion: Motion,
}

impl MovingPart {
    fn new(quads: Vec<Quad>, pivot: [f32; 3], axis: [f32; 3], motion: Motion) -> Self {
        Self {
            quads,
            glow: false,
            pivot,
            axis,
            motion,
        }
    }

    /// A glowing overlay that shows only while `motion` says so.
    fn light(quads: Vec<Quad>, motion: Motion) -> Self {
        Self {
            glow: true,
            ..Self::new(quads, [8.0, 8.0, 8.0], [0.0, 0.0, 1.0], motion)
        }
    }
}

/// A miner: a drilling rig three blocks square and five tall (`Block::structure`), built about
/// its middle cell, where the drill goes down. A deck striped round its edge on corner jacks,
/// the control house at its front, the engine with its stacks at the back, mud tanks on one
/// side and a rack of drill pipe on the other; a derrick of four legs leaning in from the
/// deck's corners (rings and braces up each side) with the crown block on top, and the rail
/// the top drive runs down. The drill and what drives it move: `miner_parts`.
fn miner(front: Tex) -> Vec<Quad> {
    use Tex::*;
    let mut b = vec![
        cuboid(
            [-16.0, -16.0, 1.0],
            [32.0, 32.0, 3.0],
            MachineTop,
            Hazard,
            MachineTop,
        ),
        // The rotary table round the hole.
        dark([2.0, 2.0, 3.0], [14.0, 14.0, 4.5]),
        // The control house: its panel at the front (the front texture's lower rows, as the
        // one-block miner showed it), the cab over it, the roof.
        ModelBox {
            front,
            ..steel([0.0, -15.0, 3.0], [16.0, -6.0, 8.0])
        },
        cuboid(
            [0.5, -14.0, 8.0],
            [15.5, -6.0, 14.0],
            MachineTop,
            MachineSide,
            MachineTop,
        ),
        steel([-1.0, -15.5, 14.0], [17.0, -5.0, 15.0]),
        // The engine and its stacks.
        cuboid(
            [-12.0, 20.0, 3.0],
            [28.0, 31.0, 13.0],
            TankPaint,
            MachineSide,
            TankPaint,
        ),
        steel([-10.0, 22.0, 13.0], [6.0, 29.0, 15.0]),
        dark([18.0, 23.0, 13.0], [21.0, 26.0, 24.0]),
        dark([23.0, 23.0, 13.0], [26.0, 26.0, 21.0]),
        // The crown block, and where cables meet it.
        steel([3.0, 3.0, 72.0], [13.0, 13.0, 75.0]),
        dark([6.0, 7.0, 75.0], [10.0, 9.0, 76.5]),
        cuboid(
            [7.25, 7.25, 76.5],
            [8.75, 8.75, MINER_TOP],
            Insulator,
            Insulator,
            Insulator,
        ),
    ];
    // Jacks under the deck's corners.
    for (x, y) in [(-16.0, -16.0), (28.0, -16.0), (-16.0, 28.0), (28.0, 28.0)] {
        b.push(dark([x, y, 0.0], [x + 4.0, y + 4.0, 1.0]));
    }
    // Two mud tanks on the right.
    b.extend(drum([26.0, -6.0], 5.0, [3.0, 15.0], MachineTop, TankPaint));
    b.extend(drum([26.0, 8.0], 5.0, [3.0, 15.0], MachineTop, TankPaint));
    let mut q = tiled_quads(&b);
    // The beacon, dark (it blinks while the drill works).
    q.extend(lamp([3.25, 3.25, 75.0], [4.75, 4.75, 76.5], [70, 24, 22]));
    // A rack of drill pipe on the left: two trestles, the pipes lying along them.
    for y in [-8.0, 16.0] {
        beam([-14.0, y, 3.0], [-14.0, y, 8.0], 0.6, RigSteel, &mut q);
        beam([-6.0, y, 3.0], [-6.0, y, 8.0], 0.6, RigSteel, &mut q);
        beam([-14.0, y, 8.0], [-6.0, y, 8.0], 0.5, RigSteel, &mut q);
    }
    for i in 0..4 {
        let x = -13.0 + i as f32 * 2.2;
        beam([x, -12.0, 9.0], [x, 20.0, 9.0], 0.9, MachineTop, &mut q);
    }
    let corner = |i: usize, z: f32| {
        let half = 13.0 + (3.5 - 13.0) * ((z - 3.0) / 69.0);
        let (sx, sy) = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)][i];
        [8.0 + sx * half, 8.0 + sy * half, z]
    };
    for i in 0..4 {
        beam(corner(i, 3.0), corner(i, 72.0), 1.0, RigSteel, &mut q);
    }
    let rings = [14.0, 29.0, 44.0, 58.0, 71.0];
    for (n, &z) in rings.iter().enumerate() {
        for i in 0..4 {
            beam(corner(i, z), corner((i + 1) % 4, z), 0.55, RigSteel, &mut q);
        }
        let below = if n == 0 { 3.0 } else { rings[n - 1] };
        for i in 0..4 {
            let (a, c) = if (n + i) % 2 == 0 {
                (i, (i + 1) % 4)
            } else {
                ((i + 1) % 4, i)
            };
            beam(corner(a, below), corner(c, z), 0.4, RigSteel, &mut q);
        }
    }
    // The rail the top drive runs down.
    beam([8.0, 10.5, 4.5], [8.0, 10.5, 72.0], 0.5, MachineTop, &mut q);
    q
}

/// A miner's moving parts: the drill string (a rod with auger flights and the bit, turning as
/// it goes down), the top drive on its rail, and the beacon on the crown.
fn miner_parts() -> Vec<MovingPart> {
    use Tex::*;
    let mut drill = Vec::new();
    beam(
        [8.0, 8.0, 0.0],
        [8.0, 8.0, 62.0],
        0.6,
        MachineTop,
        &mut drill,
    );
    for i in 0..21 {
        let z = 5.0 + i as f32 * 2.6;
        let mut flight = quads_of(&[cuboid(
            [5.5, 7.4, z],
            [10.5, 8.6, z + 0.8],
            MachineSide,
            MachineTop,
            MachineSide,
        )]);
        turn_z(&mut flight, [8.0, 8.0], i as f32 * 0.7);
        drill.extend(flight);
    }
    drill.extend(quads_of(&[
        dark([6.25, 6.25, -2.0], [9.75, 9.75, 2.0]),
        steel([6.75, 6.75, -3.5], [9.25, 9.25, -2.0]),
        steel([7.25, 7.25, -5.0], [8.75, 8.75, -3.5]),
    ]));
    let drive = quads_of(&[
        cuboid(
            [6.25, 6.25, 62.0],
            [9.75, 9.75, 68.0],
            MachineTop,
            RigSteel,
            MachineTop,
        ),
        dark([6.75, 6.75, 68.0], [9.25, 9.25, 69.5]),
        steel([7.25, 9.75, 63.0], [8.75, 10.25, 67.0]),
    ]);
    vec![
        MovingPart::new(drill, [8.0, 8.0, 31.0], [0.0, 0.0, 1.0], Motion::Drill),
        MovingPart::new(drive, [8.0, 8.0, 65.0], [0.0, 0.0, 1.0], Motion::Carriage),
        MovingPart::light(
            lamp([4.15, 4.15, 74.9], [5.85, 5.85, 76.6], [255, 50, 40]),
            Motion::Blink,
        ),
    ]
}

/// An oil pump: a beam rocking on an A-frame over the well (the head at the front), its motor
/// at the back turning the cranks. The beam, the rod and the cranks move: `oil_pump_parts`.
fn oil_pump(front: Tex) -> Vec<ModelBox> {
    vec![
        steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5]),
        // The motor house, its gauge at the front.
        ModelBox {
            front,
            ..steel([3.0, 10.0, 1.5], [13.0, 15.5, 7.0])
        },
        // The A-frame, the wellhead, and the axle between the cranks.
        steel([3.5, 6.5, 1.5], [5.0, 8.5, 13.0]),
        steel([11.0, 6.5, 1.5], [12.5, 8.5, 13.0]),
        steel([3.5, 6.5, 12.0], [12.5, 8.5, 13.0]),
        dark([6.0, 0.5, 1.5], [10.0, 3.5, 2.5]),
        dark([3.0, 12.75, 10.0], [13.0, 13.75, 11.0]),
        steel([4.0, 12.25, 7.0], [12.0, 14.25, 10.0]),
        cuboid(
            [7.25, 7.25, 14.5],
            [8.75, 8.75, 15.5],
            Tex::Insulator,
            Tex::Insulator,
            Tex::Insulator,
        ),
    ]
}

/// The pump's beam with its horse head, the rod down into the well, and the two cranks with
/// their counterweights.
fn oil_pump_parts() -> Vec<MovingPart> {
    let beam = quads_of(&[
        steel([7.0, 1.0, 13.0], [9.0, 15.0, 14.5]),
        steel([6.5, 0.0, 9.5], [9.5, 3.0, 15.0]),
    ]);
    let rod = quads_of(&[dark([7.5, 1.0, 1.0], [8.5, 2.0, 9.5])]);
    let cranks = quads_of(&[
        dark([2.0, 12.25, 9.5], [3.0, 14.25, 13.5]),
        steel([1.75, 11.75, 12.5], [3.25, 14.75, 14.0]),
        dark([13.0, 12.25, 9.5], [14.0, 14.25, 13.5]),
        steel([12.75, 11.75, 12.5], [14.25, 14.75, 14.0]),
    ]);
    let x = [1.0, 0.0, 0.0];
    vec![
        MovingPart::new(beam, [8.0, 7.5, 13.75], x, Motion::Rock),
        MovingPart::new(rod, [8.0, 1.5, 5.0], x, Motion::Stroke),
        MovingPart::new(
            cranks,
            [8.0, 13.25, 10.5],
            x,
            Motion::Spin(ROCK_RATE as f32),
        ),
    ]
}

/// A coal generator's flywheel on its right side: a rim, four spokes and the hub.
fn flywheel() -> Vec<Quad> {
    use Tex::*;
    let (x, cy, cz, r) = (15.9, 8.0, 7.0, 4.6);
    let at = |a: f32, r: f32| [x, cy + r * a.cos(), cz + r * a.sin()];
    let mut q = Vec::new();
    let step = std::f32::consts::TAU / 12.0;
    for i in 0..12 {
        let (a0, a1) = (i as f32 * step, (i + 1) as f32 * step);
        beam(at(a0, r), at(a1, r), 0.6, MachineTop, &mut q);
    }
    for i in 0..4 {
        let a = i as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
        beam(at(a, 0.8), at(a, r - 0.4), 0.4, RigSteel, &mut q);
    }
    q.extend(quads_of(&[dark([15.0, 7.0, 6.0], [16.8, 9.0, 8.0])]));
    q
}

/// A refinery: a low house with its controls at the front, two columns of different heights
/// joined by a pipe, and a mast in the middle for the cable.
fn refinery(front: Tex) -> Vec<ModelBox> {
    use Tex::*;
    let tank = |from: [f32; 3], to: [f32; 3]| cuboid(from, to, MachineTop, MachineTop, MachineTop);
    vec![
        steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5]),
        ModelBox {
            front,
            ..steel([1.0, 1.0, 1.5], [15.0, 6.0, 9.0])
        },
        tank([1.5, 8.0, 1.5], [6.5, 13.0, 30.0]),
        tank([10.0, 9.0, 1.5], [14.5, 13.5, 22.0]),
        steel([6.5, 10.0, 18.0], [10.0, 11.0, 19.0]),
        steel([1.0, 7.5, 29.5], [7.0, 13.5, 30.5]),
        dark([2.5, 9.0, 30.5], [3.5, 10.0, 32.0]),
        steel([7.25, 7.25, 9.0], [8.75, 8.75, 11.5]),
        cuboid(
            [7.25, 7.25, 11.5],
            [8.75, 8.75, REFINERY_TOP],
            Insulator,
            Insulator,
            Insulator,
        ),
    ]
}

/// A solar panel: a plate of cells tilted a little towards its front, on a post.
fn solar_panel() -> Vec<Quad> {
    use Tex::*;
    let mut quads = quads_of(&[
        steel([5.0, 5.0, 0.0], [11.0, 11.0, 1.5]),
        steel([7.0, 7.0, 1.5], [9.0, 9.0, 7.5]),
    ]);
    let mut plate = quads_of(&[
        cuboid(
            [0.5, 0.5, 7.0],
            [15.5, 15.5, 8.5],
            SolarCell,
            MachineSide,
            MachineTop,
        ),
        steel([1.0, 7.0, 6.0], [15.0, 9.0, 7.0]),
    ]);
    transform(&mut plate, |p| pitch(p, [8.0, 7.5], 0.3));
    quads.extend(plate);
    quads
}

/// One cell of a door (`piece`: across + 3 x up, seen from its front, which is the model's
/// -y): what stays put (hinges, rails, the housing a big door rolls up into), and its leaf,
/// which moves (`Motion::Swing`, `Slide`, `Shutter`). In the model's units, as any model.
pub fn door_model(kind: DoorKind, piece: u8) -> (Vec<Quad>, Vec<MovingPart>) {
    use Tex::*;
    let (col, row) = ((piece % 3) as f32, (piece / 3) as f32);
    let up = [0.0, 0.0, 1.0];
    match kind {
        DoorKind::Hinged => {
            let fixed = quads_of(&[
                dark([0.0, 0.0, 2.0], [1.5, 2.5, 4.0]),
                dark([0.0, 0.0, 12.0], [1.5, 2.5, 14.0]),
            ]);
            let mut leaf = vec![ModelBox {
                full_uv: true,
                ..cuboid(
                    [0.5, 0.0, 0.0],
                    [15.5, 2.5, 16.0],
                    DoorPlanks,
                    DoorPlanks,
                    DoorPlanks,
                )
            }];
            if row == 0.0 {
                // The handle, at about the height of a hand.
                leaf.push(dark([12.5, -0.8, 14.0], [14.0, 0.0, 15.0]));
            }
            let leaf = MovingPart::new(quads_of(&leaf), [1.25, 1.25, 8.0], up, Motion::Swing);
            (fixed, vec![leaf])
        }
        DoorKind::Sliding => {
            let mut fixed = Vec::new();
            if row == 1.0 {
                fixed.push(steel([0.0, 0.0, 14.5], [16.0, 3.0, 16.0]));
            } else {
                fixed.push(steel([0.0, 0.0, 0.0], [16.0, 3.0, 0.5]));
            }
            let mut leaf = vec![ModelBox {
                full_uv: true,
                ..cuboid(
                    [0.0, 0.5, 0.5 * (1.0 - row)],
                    [16.0, 2.5, 16.0 - 1.5 * row],
                    SlidingPanel,
                    SlidingPanel,
                    SlidingPanel,
                )
            }];
            if row == 0.0 {
                leaf.push(dark([12.5, -0.3, 13.0], [13.5, 0.5, 15.5]));
            }
            let leaf = MovingPart::new(
                quads_of(&leaf),
                [8.0, 1.5, 8.0],
                [1.0, 0.0, 0.0],
                Motion::Slide,
            );
            (quads_of(&fixed), vec![leaf])
        }
        DoorKind::Big => {
            let mut fixed = Vec::new();
            // Rails up the sides, the housing over the top with a lamp that warns while it
            // moves.
            let (x0, x1) = match col as i32 {
                0 => (1.5, 16.0),
                2 => (0.0, 14.5),
                _ => (0.0, 16.0),
            };
            if col == 0.0 {
                fixed.push(steel([0.0, -0.5, 0.0], [1.5, 4.0, 16.0]));
            }
            if col == 2.0 {
                fixed.push(steel([14.5, -0.5, 0.0], [16.0, 4.0, 16.0]));
            }
            let mut moving = Vec::new();
            if row == 2.0 {
                fixed.push(cuboid(
                    [0.0, -1.0, 16.0],
                    [16.0, 5.0, 21.0],
                    MachineTop,
                    MachineSide,
                    MachineTop,
                ));
                fixed.push(cuboid(
                    [0.0, -1.1, 16.0],
                    [16.0, -1.0, 17.0],
                    Hazard,
                    Hazard,
                    Hazard,
                ));
                if col == 1.0 {
                    let mut dark_lamp = lamp([6.5, -1.6, 18.0], [9.5, -1.0, 20.0], [70, 44, 20]);
                    let warn = lamp([6.4, -1.7, 17.9], [9.6, -1.05, 20.1], [255, 170, 40]);
                    let mut quads = quads_of(&fixed);
                    quads.append(&mut dark_lamp);
                    moving.push(MovingPart::light(warn, Motion::Warn));
                    return (quads, slats(x0, x1, row, moving));
                }
            }
            (quads_of(&fixed), slats(x0, x1, row, moving))
        }
    }
}

/// A big door cell's four slats (each its own moving part), `x0..x1` wide, after `moving`.
fn slats(x0: f32, x1: f32, row: f32, mut moving: Vec<MovingPart>) -> Vec<MovingPart> {
    use Tex::*;
    for i in 0..4 {
        let z = i as f32 * 4.0;
        let tex = if row == 0.0 && i == 0 {
            Hazard
        } else {
            ShutterPanel
        };
        let slat = cuboid([x0, 1.0, z], [x1, 3.0, z + 4.0], tex, tex, tex);
        moving.push(MovingPart::new(
            quads_of(&[slat]),
            [8.0, 2.0, z + 2.0],
            [0.0, 0.0, 1.0],
            Motion::Shutter(row * 16.0 + z),
        ));
    }
    moving
}

// ---- Pipes, tanks, power and the big machines -------------------------------------------

/// Boxes cut along the block grid, so each piece's texels stay within one block (`box_quads`
/// gives positional uvs 0..16), the faces of the cuts left out. For models bigger than a block.
fn tiled_quads(boxes: &[ModelBox]) -> Vec<Quad> {
    let cuts = |lo: f32, hi: f32| {
        let mut out = Vec::new();
        let mut a = lo;
        while a < hi - 1e-4 {
            let b = (((a / 16.0).floor() + 1.0) * 16.0).min(hi);
            out.push((a, b));
            a = b;
        }
        out
    };
    let mut out = Vec::new();
    for b in boxes {
        if b.full_uv {
            box_quads(b, &mut out);
            continue;
        }
        for &(x0, x1) in &cuts(b.from[0], b.to[0]) {
            for &(y0, y1) in &cuts(b.from[1], b.to[1]) {
                for &(z0, z1) in &cuts(b.from[2], b.to[2]) {
                    let o = [x0, y0, z0].map(|v| (v / 16.0).floor() * 16.0);
                    let sub = ModelBox {
                        from: [x0 - o[0], y0 - o[1], z0 - o[2]],
                        to: [x1 - o[0], y1 - o[1], z1 - o[2]],
                        ..*b
                    };
                    let mut quads = Vec::new();
                    box_quads(&sub, &mut quads);
                    for mut q in quads {
                        q.corners = q.corners.map(|[x, y, z]| [x + o[0], y + o[1], z + o[2]]);
                        let cut = (0..3).any(|axis| {
                            let v = q.corners[0][axis];
                            q.corners.iter().all(|c| (c[axis] - v).abs() < 1e-4)
                                && v > b.from[axis] + 1e-4
                                && v < b.to[axis] - 1e-4
                        });
                        if !cut {
                            out.push(q);
                        }
                    }
                }
            }
        }
    }
    out
}

/// A drum standing about (cx, cy) of radius `r` from `z0` to `z1`: three boxes that round its
/// corners off.
fn drum([cx, cy]: [f32; 2], r: f32, [z0, z1]: [f32; 2], top: Tex, side: Tex) -> Vec<ModelBox> {
    let b = |hx: f32, hy: f32| {
        cuboid(
            [cx - hx, cy - hy, z0],
            [cx + hx, cy + hy, z1],
            top,
            side,
            top,
        )
    };
    vec![b(r, r * 0.55), b(r * 0.55, r), b(r * 0.83, r * 0.83)]
}

/// A drum lying along y about the line (x = cx, z = cz), from `y0` to `y1`.
fn lying_drum(
    [cx, cz]: [f32; 2],
    r: f32,
    [y0, y1]: [f32; 2],
    end: Tex,
    side: Tex,
) -> Vec<ModelBox> {
    let b = |hx: f32, hz: f32| ModelBox {
        front: end,
        back: end,
        ..cuboid(
            [cx - hx, y0, cz - hz],
            [cx + hx, y1, cz + hz],
            side,
            side,
            side,
        )
    };
    vec![b(r, r * 0.55), b(r * 0.55, r), b(r * 0.83, r * 0.83)]
}

/// An insulator for cables at (8, 8), its top at `top`.
fn insulator(top: f32) -> ModelBox {
    cuboid(
        [7.25, 7.25, top - 1.5],
        [8.75, 8.75, top],
        Tex::Insulator,
        Tex::Insulator,
        Tex::Insulator,
    )
}

/// Where cables meet a machine that isn't an ordinary box (model units up, at its middle).
pub fn structure_top(kind: crate::factory::MachineKind) -> f32 {
    use crate::factory::MachineKind::*;
    match kind {
        Blender => 66.0,
        Reactor => 76.0,
        SteamTurbine => 31.0,
        CoolingPool => 27.0,
        Reprocessing => 44.0,
        Accelerator => 22.0,
        Centrifuge => 47.0,
        Electrolyzer => 31.0,
        Boiler => 31.0,
        Tank => 32.0,
        Geothermal => 15.0,
        _ => MACHINE_TOP,
    }
}

/// A round bar along `axis` (0 x, 1 y, 2 z) through the middle of the cell, `r` thick each way,
/// from `a` to `b` along it (three boxes, like `drum`).
fn rod(axis: usize, r: f32, [a, b]: [f32; 2], tex: Tex) -> [ModelBox; 3] {
    let (p, q) = ((axis + 1) % 3, (axis + 2) % 3);
    [(r, r * 0.55), (r * 0.55, r), (r * 0.83, r * 0.83)].map(|(hp, hq)| {
        let (mut from, mut to) = ([8.0; 3], [8.0; 3]);
        from[axis] = a;
        to[axis] = b;
        (from[p], to[p]) = (8.0 - hp, 8.0 + hp);
        (from[q], to[q]) = (8.0 - hq, 8.0 + hq);
        cuboid(from, to, tex, tex, tex)
    })
}

/// Pipe sizes (model units out from the middle): the run, its collars, a fitting where it
/// turns or branches, the band showing its fluid, the flange on a machine.
const PIPE_R: f32 = 2.6;
const COLLAR_R: f32 = 3.3;
const FITTING_R: f32 = 3.6;
const BAND_R: f32 = 2.85;
const FLANGE_R: f32 = 4.4;

/// A pipe (`links`: which ways it joins, bits +x, -x, +y, -y, +z, -z; model x and y are the
/// grid's u and v, it is never turned). A straight run through the cell with a collar where
/// it meets the next, a capped end where it stops, a fitting where it turns or branches, a
/// flange where it meets a machine (`ports`), and a band in its fluid's colour (`fluid`).
/// On its own it runs along `axis`.
fn pipe(state: ModelState) -> Vec<Quad> {
    use Tex::*;
    let links = state.links & 0b11_1111;
    let joined = |side: usize| links & (1 << side) != 0;
    let mut boxes = Vec::new();
    // Each way it goes: along one axis (straight, or an end), or several (a fitting).
    let axes: Vec<usize> = (0..3)
        .filter(|&a| joined(2 * a) || joined(2 * a + 1))
        .collect();
    let straight = axes.len() <= 1;
    let mut bands = Vec::new();
    if straight {
        let axis = axes.first().copied().unwrap_or(state.axis.min(2) as usize);
        boxes.extend(rod(axis, PIPE_R, [0.0, 16.0], PipeSteel));
        bands.extend(rod(axis, BAND_R, [6.75, 9.25], Lamp));
        // Capped where it doesn't go on.
        for (side, [a, b]) in [(2 * axis, [14.6, 16.0]), (2 * axis + 1, [0.0, 1.4])] {
            if !joined(side) {
                boxes.extend(rod(axis, COLLAR_R, [a, b], PipeFitting));
            }
        }
    } else {
        boxes.extend(rod(
            2,
            FITTING_R,
            [8.0 - FITTING_R, 8.0 + FITTING_R],
            PipeFitting,
        ));
        for side in (0..6).filter(|&s| joined(s)) {
            let (axis, plus) = (side / 2, side % 2 == 0);
            let span = |a: f32, b: f32| if plus { [a, b] } else { [16.0 - b, 16.0 - a] };
            boxes.extend(rod(axis, PIPE_R, span(8.0, 16.0), PipeSteel));
            bands.extend(rod(axis, BAND_R, span(12.0, 13.6), Lamp));
        }
    }
    // A collar at each end it joins (the next pipe's makes it whole), a flange on a machine.
    for side in (0..6).filter(|&s| joined(s)) {
        let (axis, plus) = (side / 2, side % 2 == 0);
        let span = |a: f32, b: f32| if plus { [a, b] } else { [16.0 - b, 16.0 - a] };
        if state.ports & (1 << side) != 0 {
            let (mut from, mut to) = ([8.0 - FLANGE_R; 3], [8.0 + FLANGE_R; 3]);
            [from[axis], to[axis]] = span(14.6, 16.0);
            boxes.push(cuboid(from, to, PipeFitting, PipeFitting, PipeFitting));
            boxes.extend(rod(axis, COLLAR_R, span(13.4, 14.6), PipeFitting));
        } else {
            boxes.extend(rod(axis, COLLAR_R, span(15.0, 16.0), PipeFitting));
        }
    }
    let mut quads = quads_of(&boxes);
    // (An empty pipe that has carried nothing yet is bare.)
    if let Some(fluid) = state
        .fluid
        .checked_sub(1)
        .and_then(|f| Fluid::ALL.get(f as usize))
    {
        quads.extend(tinted(quads_of(&bands), fluid.color()));
    }
    quads
}

/// A storage tank: a white drum two blocks tall on a plinth, a gauge window up its front.
fn storage_tank() -> Vec<ModelBox> {
    use Tex::*;
    let mut b = vec![steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5])];
    b.extend(drum([8.0, 8.0], 7.0, [1.5, 29.0], TankPaint, TankPaint));
    b.extend(drum([8.0, 8.0], 5.0, [29.0, 30.5], MachineTop, MachineSide));
    b.push(steel([6.0, 0.6, 3.0], [10.0, 1.2, 3.8]));
    b.push(steel([6.0, 0.6, GAUGE_TOP], [10.0, 1.2, GAUGE_TOP + 0.8]));
    b.push(steel([6.0, 0.6, 3.8], [6.6, 1.2, GAUGE_TOP]));
    b.push(steel([9.4, 0.6, 3.8], [10.0, 1.2, GAUGE_TOP]));
    b.push(steel([3.0, 3.0, 30.5], [5.0, 5.0, 32.0]));
    b
}

/// A storage tank's gauge: from here up to `GAUGE_TOP`, filled to `level` of `LEVELS`.
const GAUGE_BOTTOM: f32 = 3.8;
const GAUGE_TOP: f32 = 27.0;

fn gauge_fill(level: u8) -> f32 {
    GAUGE_BOTTOM + (GAUGE_TOP - GAUGE_BOTTOM) * level.min(LEVELS) as f32 / LEVELS as f32
}

/// A water pump: its body on a plinth, the intake down into the water, the motor at the back,
/// and the outlet up out of its side.
fn water_pump() -> Vec<ModelBox> {
    use Tex::*;
    vec![
        steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5]),
        ModelBox {
            front: PumpFront,
            ..steel([2.0, 1.0, 1.5], [14.0, 11.0, 10.0])
        },
        dark([4.0, 11.0, 2.0], [12.0, 15.5, 9.0]),
        dark([6.5, 4.5, -14.0], [9.5, 7.5, 1.5]),
        cuboid(
            [10.0, 4.5, 10.0],
            [13.0, 7.5, 16.0],
            MachineSide,
            MachineTop,
            MachineSide,
        ),
        insulator(11.5),
    ]
}

/// An electrolyzer: a stack of cells in a white housing, two gas pipes up out of it (hydrogen
/// red, oxygen blue: `glow_quads` lights their bands), a panel at the front.
fn electrolyzer(lit: bool) -> Vec<ModelBox> {
    use Tex::*;
    let panel = if lit { ControlPanelLit } else { ControlPanel };
    let mut b = vec![steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5])];
    b.push(ModelBox {
        front: panel,
        ..cuboid(
            [1.5, 1.5, 1.5],
            [14.5, 14.5, 22.0],
            TankPaint,
            TankPaint,
            TankPaint,
        )
    });
    for z in [6.0, 11.0, 16.0] {
        b.push(dark([1.0, 1.0, z], [15.0, 15.0, z + 1.0]));
    }
    b.push(steel([2.5, 9.0, 22.0], [5.5, 12.0, 28.0]));
    b.push(steel([10.5, 9.0, 22.0], [13.5, 12.0, 28.0]));
    b.push(steel([6.0, 6.0, 22.0], [10.0, 10.0, 29.5]));
    b.push(insulator(31.0));
    b
}

/// A gas centrifuge: a tall slim drum in steel bands on a base, its drive on top.
fn centrifuge() -> Vec<ModelBox> {
    use Tex::*;
    let mut b = vec![steel([0.0, 0.0, 0.0], [16.0, 16.0, 2.0])];
    b.extend(drum([8.0, 8.0], 4.5, [2.0, 42.0], TankPaint, TankPaint));
    for z in [8.0, 18.0, 28.0, 38.0] {
        b.extend(drum([8.0, 8.0], 5.2, [z, z + 1.0], MachineTop, MachineSide));
    }
    b.push(dark([5.0, 5.0, 42.0], [11.0, 11.0, 44.0]));
    b.push(steel([1.0, 2.0, 2.0], [4.0, 5.0, 12.0]));
    b.push(steel([12.0, 11.0, 2.0], [15.0, 14.0, 12.0]));
    b.push(insulator(47.0));
    b
}

/// A boiler: a firebox (its door flickers: `moving_parts`) under a drum lying across it, and a
/// chimney.
fn boiler() -> Vec<ModelBox> {
    use Tex::*;
    let mut b = vec![
        steel([0.0, 0.0, 0.0], [16.0, 16.0, 1.5]),
        ModelBox {
            front: GeneratorFront,
            ..steel([1.0, 1.0, 1.5], [15.0, 15.0, 12.0])
        },
    ];
    b.extend(lying_drum(
        [8.0, 20.0],
        7.0,
        [1.5, 14.5],
        MachineTop,
        TankPaint,
    ));
    b.push(steel([11.0, 11.0, 12.0], [14.0, 14.0, 34.0]));
    b.push(dark([10.5, 10.5, 34.0], [14.5, 14.5, 35.0]));
    b.push(steel([3.0, 6.0, 26.0], [5.0, 8.0, 29.5]));
    b.push(insulator(31.0));
    b
}

/// A geothermal well: a concrete pad, the casing up out of the ground with its valves (red
/// wheels), the steam line off to the side.
fn geothermal() -> Vec<ModelBox> {
    use Tex::*;
    vec![
        cuboid(
            [0.0, 0.0, 0.0],
            [16.0, 16.0, 2.0],
            Concrete,
            Concrete,
            Concrete,
        ),
        steel([5.5, 5.5, 2.0], [10.5, 10.5, 11.0]),
        dark([4.5, 4.5, 4.0], [11.5, 11.5, 5.0]),
        dark([4.5, 4.5, 9.0], [11.5, 11.5, 10.0]),
        cuboid(
            [10.5, 6.5, 6.0],
            [16.0, 9.5, 9.0],
            MachineSide,
            MachineTop,
            MachineSide,
        ),
        steel([7.0, 7.0, 11.0], [9.0, 9.0, 13.5]),
        insulator(15.0),
    ]
}

/// An RTG: a dark finned drum on legs, a hazard band round it, its core glowing faintly at the
/// top (`glow_quads`).
fn rtg() -> Vec<ModelBox> {
    use Tex::*;
    let mut b = vec![steel([3.0, 3.0, 0.0], [13.0, 13.0, 1.0])];
    b.extend(drum([8.0, 8.0], 3.5, [1.0, 12.0], CableSkin, CableSkin));
    for (x0, y0, x1, y1) in [
        (0.5, 7.5, 4.5, 8.5),
        (11.5, 7.5, 15.5, 8.5),
        (7.5, 0.5, 8.5, 4.5),
        (7.5, 11.5, 8.5, 15.5),
    ] {
        b.push(dark([x0, y0, 2.0], [x1, y1, 11.0]));
    }
    b.push(cuboid(
        [4.2, 4.2, 5.0],
        [11.8, 11.8, 7.0],
        Hazard,
        Hazard,
        Hazard,
    ));
    b
}

/// A wind turbine: a tapering white tower eight blocks tall on a foundation, the nacelle on
/// top facing the model's front. The rotor turns: `moving_parts`.
fn wind_turbine() -> Vec<Quad> {
    use Tex::*;
    let mut q = tiled_quads(&[
        cuboid(
            [1.0, 1.0, 0.0],
            [15.0, 15.0, 2.0],
            Concrete,
            Concrete,
            Concrete,
        ),
        dark([6.0, 0.5, 2.0], [10.0, 2.0, 7.0]),
        insulator(6.0),
    ]);
    // The tower, in rings that narrow as it goes up.
    for i in 0..12 {
        let z0 = 2.0 + i as f32 * 10.0;
        let r = 4.8 - i as f32 * 0.22;
        q.extend(tiled_quads(&drum(
            [8.0, 8.0],
            r,
            [z0, z0 + 10.0],
            TankPaint,
            TankPaint,
        )));
    }
    q.extend(tiled_quads(&[
        cuboid(
            [4.5, 0.5, 120.0],
            [11.5, 17.0, 127.0],
            TankPaint,
            TankPaint,
            MachineSide,
        ),
        dark([5.5, 16.5, 121.0], [10.5, 17.5, 126.0]),
    ]));
    q
}

/// Where a wind turbine's rotor turns (model units), and how long its blades are.
const ROTOR: [f32; 3] = [8.0, -1.0, 123.5];
const BLADE: f32 = 46.0;

/// The rotor: a hub and three blades.
fn rotor() -> Vec<Quad> {
    use Tex::*;
    let [x, y, z] = ROTOR;
    let mut q = quads_of(&[cuboid(
        [x - 2.5, y - 1.0, z - 2.5],
        [x + 2.5, y + 2.0, z + 2.5],
        MachineTop,
        MachineSide,
        MachineTop,
    )]);
    for i in 0..3 {
        let a = i as f32 * std::f32::consts::TAU / 3.0;
        let (s, c) = a.sin_cos();
        let at = |r: f32| [x + c * r, y - 0.5, z + s * r];
        beam(at(2.0), at(BLADE * 0.35), 1.6, CleanPanel, &mut q);
        beam(at(BLADE * 0.35), at(BLADE * 0.75), 1.1, CleanPanel, &mut q);
        beam(at(BLADE * 0.75), at(BLADE), 0.6, CleanPanel, &mut q);
    }
    q
}

/// A blender, five blocks square: a hazard-edged slab, a great white vat on legs with a coned
/// foot, its lid, the gearbox and motor that turn its paddles, a ladder up its side, pipes
/// down from its top, and the control cabinet at the front.
fn blender(lit: bool) -> Vec<Quad> {
    use Tex::*;
    let c = [8.0, 8.0];
    let mut b = vec![
        cuboid(
            [-32.0, -32.0, 0.0],
            [48.0, 48.0, 2.0],
            MachineTop,
            Hazard,
            MachineTop,
        ),
        steel([-20.0, -20.0, 2.0], [-17.0, -17.0, 16.0]),
        steel([33.0, -20.0, 2.0], [36.0, -17.0, 16.0]),
        steel([-20.0, 33.0, 2.0], [-17.0, 36.0, 16.0]),
        steel([33.0, 33.0, 2.0], [36.0, 36.0, 16.0]),
    ];
    b.extend(drum(c, 12.0, [6.0, 12.0], MachineTop, MachineSide));
    b.extend(drum(c, 22.0, [12.0, 52.0], TankPaint, TankPaint));
    b.extend(drum(c, 23.0, [24.0, 25.5], MachineTop, MachineSide));
    b.extend(drum(c, 23.0, [40.0, 41.5], MachineTop, MachineSide));
    b.extend(drum(c, 20.0, [52.0, 54.0], MachineTop, MachineSide));
    b.extend([
        steel([0.0, 0.0, 54.0], [16.0, 16.0, 62.0]),
        dark([16.0, 3.0, 55.0], [26.0, 13.0, 61.0]),
        steel([7.0, 7.0, 62.0], [9.0, 9.0, 64.5]),
        insulator(66.0),
        // The control cabinet.
        ModelBox {
            front: if lit { ControlPanelLit } else { ControlPanel },
            ..steel([-10.0, -31.0, 2.0], [26.0, -24.0, 22.0])
        },
        // The ladder's rails.
        steel([29.0, -2.5, 2.0], [30.0, -1.5, 54.0]),
        steel([29.0, 3.5, 2.0], [30.0, 4.5, 54.0]),
    ]);
    let mut q = tiled_quads(&b);
    for i in 0..13 {
        let z = 5.0 + i as f32 * 4.0;
        beam([29.5, -2.0, z], [29.5, 4.0, z], 0.35, MachineTop, &mut q);
    }
    for (x, y) in [(-12.0, 20.0), (28.0, 20.0)] {
        beam([x, y, 54.0], [x, y, 60.0], 1.5, MachineSide, &mut q);
        let out = if x < 0.0 { -30.0 } else { 46.0 };
        beam([x, y, 60.0], [out, y, 60.0], 1.5, MachineSide, &mut q);
        beam([out, y, 60.0], [out, y, 2.0], 1.5, MachineSide, &mut q);
    }
    q
}

/// A steam turbine, three blocks square: its casing lying along the model's y on a plinth, the
/// steam line into its top, the generator behind it; the coupling between them turns.
fn steam_turbine() -> Vec<Quad> {
    use Tex::*;
    let mut b = vec![steel([-16.0, -16.0, 0.0], [32.0, 32.0, 3.0])];
    b.extend(lying_drum(
        [8.0, 14.0],
        10.0,
        [-14.0, 14.0],
        MachineTop,
        MachineSide,
    ));
    b.extend(lying_drum(
        [8.0, 14.0],
        11.0,
        [-14.0, -11.0],
        MachineTop,
        MachineTop,
    ));
    b.extend(lying_drum(
        [8.0, 14.0],
        11.0,
        [11.0, 14.0],
        MachineTop,
        MachineTop,
    ));
    b.extend([
        cuboid(
            [-6.0, 19.0, 3.0],
            [22.0, 31.0, 25.0],
            TankPaint,
            TankPaint,
            TankPaint,
        ),
        steel([5.0, -2.0, 24.0], [11.0, 4.0, 28.0]),
        steel([3.5, -3.5, 28.0], [12.5, 5.5, 29.5]),
        steel([7.0, 7.0, 24.0], [9.0, 9.0, 29.5]),
        insulator(31.0),
        steel([-4.0, -14.0, 3.0], [20.0, -10.0, 6.0]),
    ]);
    tiled_quads(&b)
}

/// The coupling between a steam turbine and its generator: a flanged disc on the shaft.
fn turbine_coupling() -> Vec<Quad> {
    let mut q = quads_of(&[
        steel([4.0, 15.0, 10.0], [12.0, 17.0, 18.0]),
        dark([7.0, 14.0, 13.0], [9.0, 19.0, 15.0]),
    ]);
    q.extend(quads_of(&[dark([3.5, 15.5, 13.0], [12.5, 16.5, 15.0])]));
    q
}

/// A nuclear reactor, five blocks square: a concrete containment rising to a stepped dome on a
/// slab, the steam lines down its side, and the control building at its front (its panel lit
/// while it runs). A warning light on the roof glows red while it overheats (`glow_quads`).
fn reactor(lit: bool) -> Vec<Quad> {
    use Tex::*;
    let c = [8.0, 8.0];
    let mut b = vec![cuboid(
        [-32.0, -32.0, 0.0],
        [48.0, 48.0, 4.0],
        Concrete,
        Concrete,
        Concrete,
    )];
    b.extend(drum(c, 30.0, [4.0, 56.0], Concrete, Concrete));
    b.extend(drum(c, 31.0, [52.0, 54.0], MachineTop, MachineSide));
    b.extend(drum(c, 26.0, [56.0, 62.0], Concrete, Concrete));
    b.extend(drum(c, 20.0, [62.0, 68.0], Concrete, Concrete));
    b.extend(drum(c, 12.0, [68.0, 73.0], Concrete, Concrete));
    b.extend([
        steel([6.0, 6.0, 73.0], [10.0, 10.0, 74.5]),
        insulator(76.0),
        // The control building.
        ModelBox {
            front: if lit { ControlPanelLit } else { ControlPanel },
            ..cuboid(
                [-20.0, -32.0, 4.0],
                [36.0, -22.0, 26.0],
                Concrete,
                Concrete,
                Concrete,
            )
        },
        cuboid(
            [-21.0, -33.0, 26.0],
            [37.0, -21.0, 27.5],
            Hazard,
            Hazard,
            Hazard,
        ),
        dark([-16.0, -32.5, 4.0], [-6.0, -31.5, 18.0]),
        // Steam lines.
        steel([36.0, 2.0, 4.0], [41.0, 7.0, 46.0]),
        steel([36.0, 10.0, 4.0], [41.0, 15.0, 46.0]),
    ]);
    tiled_quads(&b)
}

/// Where the reactor's warning light is.
const ALARM: ([f32; 3], [f32; 3]) = ([26.0, -30.0, 27.5], [30.0, -26.0, 30.0]);

/// A cooling pool, three blocks square: concrete walls round water lit blue (`glow_quads`), a
/// gantry crane over it.
fn cooling_pool() -> Vec<Quad> {
    use Tex::*;
    let wall = |from, to| cuboid(from, to, Concrete, Concrete, Concrete);
    let mut b = vec![
        wall([-16.0, -16.0, 0.0], [32.0, 32.0, 2.0]),
        wall([-16.0, -16.0, 2.0], [32.0, -13.0, 16.0]),
        wall([-16.0, 29.0, 2.0], [32.0, 32.0, 16.0]),
        wall([-16.0, -13.0, 2.0], [-13.0, 29.0, 16.0]),
        wall([29.0, -13.0, 2.0], [32.0, 29.0, 16.0]),
        // The racks under the water.
        dark([-10.0, -10.0, 2.0], [26.0, 26.0, 8.0]),
        // The gantry.
        steel([-15.5, 6.0, 16.0], [-13.5, 10.0, 26.0]),
        steel([29.5, 6.0, 16.0], [31.5, 10.0, 26.0]),
        steel([-15.5, 6.5, 24.0], [31.5, 9.5, 26.0]),
        dark([5.0, 5.5, 20.0], [11.0, 10.5, 24.0]),
        insulator(27.0),
    ];
    b.push(steel([-12.0, -16.5, 4.0], [-4.0, -15.5, 12.0]));
    tiled_quads(&b)
}

/// The pool's water (a quad over the racks).
fn pool_water() -> Vec<Quad> {
    let z = 13.5;
    vec![Quad {
        corners: [
            [-13.0, -13.0, z],
            [29.0, -13.0, z],
            [29.0, 29.0, z],
            [-13.0, 29.0, z],
        ],
        uv: [[0.0, 16.0], [16.0, 16.0], [16.0, 0.0], [0.0, 0.0]],
        tex: Tex::Cherenkov,
        tint: 0,
    }]
}

/// A reprocessing plant, three blocks square: a heavy concrete block with two hot cell windows
/// at its front (lit while it works), a stack on its roof, pipes down its side.
fn reprocessing(lit: bool) -> Vec<Quad> {
    use Tex::*;
    let window = if lit { HotCellLit } else { HotCell };
    let b = vec![
        cuboid(
            [-16.0, -15.0, 0.0],
            [32.0, 32.0, 40.0],
            Concrete,
            Concrete,
            Concrete,
        ),
        ModelBox {
            front: window,
            full_uv: true,
            ..cuboid(
                [-12.0, -16.0, 10.0],
                [2.0, -15.0, 24.0],
                Concrete,
                Concrete,
                Concrete,
            )
        },
        ModelBox {
            front: window,
            full_uv: true,
            ..cuboid(
                [14.0, -16.0, 10.0],
                [28.0, -15.0, 24.0],
                Concrete,
                Concrete,
                Concrete,
            )
        },
        dark([-11.0, -18.0, 25.0], [-9.0, -15.0, 27.0]),
        dark([25.0, -18.0, 25.0], [27.0, -15.0, 27.0]),
        steel([19.0, 19.0, 40.0], [26.0, 26.0, 47.5]),
        cuboid(
            [18.5, 18.5, 43.0],
            [26.5, 26.5, 44.5],
            Hazard,
            Hazard,
            Hazard,
        ),
        steel([7.0, 7.0, 40.0], [9.0, 9.0, 42.5]),
        insulator(44.0),
        steel([32.0, -6.0, 0.0], [35.0, -3.0, 36.0]),
        steel([32.0, 4.0, 0.0], [35.0, 7.0, 36.0]),
    ];
    tiled_quads(&b)
}

/// The particle accelerator's ring: where its magnets stand (centre, radius).
const RING: (f32, f32) = (8.0, 40.0);
const MAGNETS: usize = 16;

/// A particle accelerator, seven blocks square: a ring of blue magnets on a floor, the beam
/// line between them (lit while it runs: `glow_quads`), the target chamber in the middle and
/// the control desk at the front.
fn accelerator(lit: bool) -> Vec<Quad> {
    use Tex::*;
    let mut q = tiled_quads(&[
        cuboid(
            [-48.0, -48.0, 0.0],
            [64.0, 64.0, 2.0],
            MachineTop,
            Hazard,
            MachineTop,
        ),
        steel([-2.0, -2.0, 2.0], [18.0, 18.0, 16.0]),
        cuboid(
            [-3.0, -3.0, 9.0],
            [19.0, 19.0, 11.0],
            Hazard,
            Hazard,
            Hazard,
        ),
        steel([7.0, 7.0, 16.0], [9.0, 9.0, 20.5]),
        insulator(22.0),
        ModelBox {
            front: if lit { ControlPanelLit } else { ControlPanel },
            ..steel([-8.0, -46.0, 2.0], [24.0, -40.0, 14.0])
        },
    ]);
    let (c, r) = RING;
    for i in 0..MAGNETS {
        let a = i as f32 / MAGNETS as f32 * std::f32::consts::TAU;
        let mut magnet = quads_of(&[
            cuboid(
                [c - 6.0, c - 4.0, 2.0],
                [c + 6.0, c + 4.0, 14.0],
                TankPaint,
                TankPaint,
                TankPaint,
            ),
            dark([c - 6.5, c - 4.5, 6.5], [c + 6.5, c + 4.5, 9.5]),
        ]);
        transform(&mut magnet, |[x, y, z]| [x, y + r, z]);
        turn_z(&mut magnet, [c, c], a);
        q.extend(tinted(magnet, [90, 140, 255]));
        // The beam pipe on to the next magnet.
        let b = (i as f32 + 0.5) / MAGNETS as f32 * std::f32::consts::TAU;
        let at = |angle: f32| [c - r * angle.sin(), c + r * angle.cos(), 8.0];
        let a0 = a + 0.17;
        let a1 = (i + 1) as f32 / MAGNETS as f32 * std::f32::consts::TAU - 0.17;
        beam(at(a0), at(b), 1.0, MachineTop, &mut q);
        beam(at(b), at(a1), 1.0, MachineTop, &mut q);
    }
    q
}

/// The accelerator's beam line, glowing (as quads to light up).
fn beam_glow() -> Vec<Quad> {
    let (c, r) = RING;
    let mut q = Vec::new();
    for i in 0..MAGNETS * 2 {
        let a0 = i as f32 / (MAGNETS * 2) as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / (MAGNETS * 2) as f32 * std::f32::consts::TAU;
        let at = |angle: f32| [c - r * angle.sin(), c + r * angle.cos(), 8.0];
        beam(at(a0), at(a1), 1.15, Tex::Lamp, &mut q);
    }
    tinted(q, [110, 230, 255])
}

/// A small lamp a big machine shows its state by (green while it works), or None for one in
/// the usual place (`LAMP_FROM`).
fn lamp_place(block: Block) -> ([f32; 3], [f32; 3]) {
    match block {
        Block::Blender => ([20.0, -31.5, 18.0], [23.0, -31.0, 20.0]),
        Block::SteamTurbine => ([14.0, -14.5, 3.5], [17.0, -14.0, 5.5]),
        Block::ReprocessingPlant => ([4.0, -15.5, 30.0], [8.0, -15.0, 33.0]),
        Block::ParticleAccelerator => ([18.0, -46.5, 10.0], [22.0, -46.0, 12.0]),
        Block::Centrifuge | Block::Electrolyzer => ([10.0, 0.9, 18.0], [12.0, 1.4, 19.5]),
        Block::WaterPump => ([10.5, 0.5, 7.5], [12.5, 1.0, 9.0]),
        _ => (LAMP_FROM, LAMP_TO),
    }
}

/// The boxes of a model block that is only boxes (the chest's lid separately: see `chest`).
fn model_boxes(block: Block, state: ModelState) -> Vec<ModelBox> {
    use Tex::*;
    let lit = |on: Tex, off: Tex| if state.lit { on } else { off };
    match block {
        Block::Furnace => furnace(state.lit),
        Block::Chest => chest().into_iter().map(|(b, _)| b).collect(),
        Block::Torch => vec![torch()],
        Block::Conveyor => conveyor(),
        Block::PowerPole => power_pole(),
        // (Its fire flickers: `moving_parts`.)
        Block::CoalGenerator => machine(GeneratorFront, true),
        Block::ElectricFurnace => machine(lit(ElectricFrontLit, ElectricFront), false),
        Block::OilPump => oil_pump(lit(PumpFrontLit, PumpFront)),
        Block::Refinery => refinery(RefineryFront),
        Block::ArcFurnace => {
            // Three graphite electrodes down through the roof.
            let mut boxes = machine(ArcFront, false);
            for x in [3.0, 7.0, 11.0] {
                boxes.push(dark([x, 2.5, 13.5], [x + 2.0, 4.5, 21.0]));
            }
            boxes
        }
        Block::Assembler => {
            // A hopper on top for parts.
            let mut boxes = machine(lit(AssemblerFrontLit, AssemblerFront), false);
            boxes.push(steel([2.5, 9.5, 13.5], [6.5, 13.5, 16.5]));
            boxes
        }
        Block::ChipFab => {
            // Clean white, with a filter unit on the roof (its fan turns).
            let mut boxes = housing(lit(FabFrontLit, FabFront), CleanPanel, CleanPanel);
            boxes.push(cuboid(
                [9.5, 9.5, 13.5],
                [14.0, 14.0, 16.0],
                CleanPanel,
                MachineSide,
                CleanPanel,
            ));
            boxes
        }
        Block::Accumulator => {
            // Two terminals on top; the gauge at the front is `accumulator_gauge`.
            let mut boxes = machine(AccumulatorFront, false);
            boxes.push(cuboid(
                [2.5, 10.5, 13.5],
                [4.5, 12.5, 15.0],
                Insulator,
                Insulator,
                Insulator,
            ));
            boxes.push(dark([11.5, 10.5, 13.5], [13.5, 12.5, 15.0]));
            boxes
        }
        _ => crafting_table(),
    }
}

/// The machines with a status lamp on their front (green while they work; see `lamp_place`).
fn has_lamp(block: Block) -> bool {
    matches!(
        block,
        Block::CoalGenerator
            | Block::ElectricFurnace
            | Block::ArcFurnace
            | Block::Assembler
            | Block::ChipFab
            | Block::Accumulator
            | Block::WaterPump
            | Block::Electrolyzer
            | Block::Centrifuge
            | Block::FuelFabricator
            | Block::Blender
            | Block::SteamTurbine
            | Block::ReprocessingPlant
            | Block::ParticleAccelerator
    )
}

const LAMP_FROM: [f32; 3] = [12.5, 0.25, 12.65];
const LAMP_TO: [f32; 3] = [14.0, 0.5, 13.35];

/// The x of an accumulator's three charge tubes (each 2.5 wide), and their bottom and top.
const TUBES: [f32; 3] = [3.0, 6.75, 10.5];
const TUBE_BOTTOM: f32 = 3.0;
const TUBE_TOP: f32 = 11.5;

/// An accumulator's three charge tubes: steel caps, and what of them is empty (dark glass).
/// What is full glows: `glow_quads`.
fn accumulator_gauge(level: u8) -> Vec<Quad> {
    let full = TUBE_BOTTOM + (TUBE_TOP - TUBE_BOTTOM) * level.min(LEVELS) as f32 / LEVELS as f32;
    let mut boxes = Vec::new();
    for x in TUBES {
        boxes.push(steel(
            [x - 0.4, 0.2, TUBE_BOTTOM - 0.6],
            [x + 2.9, 1.0, TUBE_BOTTOM],
        ));
        boxes.push(steel(
            [x - 0.4, 0.2, TUBE_TOP],
            [x + 2.9, 1.0, TUBE_TOP + 0.6],
        ));
        if full < TUBE_TOP {
            boxes.push(dark([x, 0.4, full], [x + 2.5, 1.0, TUBE_TOP]));
        }
    }
    quads_of(&boxes)
}

/// Every face of a model block that doesn't glow or move (see `glow_quads`, `moving_parts`).
fn model_quads(block: Block, state: ModelState) -> Vec<Quad> {
    use Tex::*;
    if !state.is_core() {
        return Vec::new();
    }
    let lit = state.lit;
    let mut quads = match block {
        Block::Miner => miner(if state.lit { MinerFrontLit } else { MinerFront }),
        Block::Conveyor if state.bend != 0 => conveyor_bend(state.bend == 1),
        Block::SolarPanel => solar_panel(),
        Block::Pipe => pipe(state),
        Block::StorageTank => {
            let mut b = storage_tank();
            let fill = gauge_fill(if state.fluid == 0 { 0 } else { state.level });
            if fill < GAUGE_TOP {
                b.push(dark([6.6, 0.8, fill], [9.4, 1.2, GAUGE_TOP]));
            }
            tiled_quads(&b)
        }
        Block::WaterPump => tiled_quads(&water_pump()),
        Block::Electrolyzer => tiled_quads(&electrolyzer(lit)),
        Block::Centrifuge => tiled_quads(&centrifuge()),
        Block::FuelFabricator => {
            let mut b = housing(
                if lit { ControlPanelLit } else { ControlPanel },
                MachineSide,
                MachineTop,
            );
            b.push(dark([2.5, 9.5, 13.5], [5.5, 13.5, 15.0]));
            b.push(steel([10.0, 9.0, 13.5], [14.0, 14.0, 14.5]));
            quads_of(&b)
        }
        Block::Boiler => tiled_quads(&boiler()),
        Block::GeothermalWell => tiled_quads(&geothermal()),
        Block::WindTurbine => wind_turbine(),
        Block::Rtg => tiled_quads(&rtg()),
        Block::Blender => blender(lit),
        Block::SteamTurbine => steam_turbine(),
        Block::NuclearReactor => reactor(lit),
        Block::CoolingPool => {
            let mut q = cooling_pool();
            if !lit {
                q.extend(pool_water());
            }
            q
        }
        Block::ReprocessingPlant => reprocessing(lit),
        Block::ParticleAccelerator => accelerator(lit),
        b if b.door().is_some() => {
            let (kind, _) = b.door().unwrap_or((DoorKind::Hinged, false));
            door_model(kind, state.piece).0
        }
        _ => quads_of(&model_boxes(block, state)),
    };
    match block {
        Block::CoalGenerator => {
            // The flywheel's axle out of the side.
            quads.extend(quads_of(&[dark([14.5, 7.25, 6.25], [15.2, 8.75, 7.75])]));
        }
        Block::Accumulator => quads.extend(accumulator_gauge(state.level)),
        _ => {}
    }
    if has_lamp(block) && !state.lit {
        let (from, to) = lamp_place(block);
        quads.extend(lamp(from, to, [30, 44, 34]));
    }
    quads
}

/// What lights up of a model block (drawn lit by itself): windows and screens while it works,
/// its status lamp, an accumulator's charge.
fn glow_quads(block: Block, state: ModelState) -> Vec<Quad> {
    use Tex::*;
    let mut quads = Vec::new();
    if !state.is_core() {
        return quads;
    }
    let (y, xs, zs) = HOUSING_FRONT;
    if state.lit {
        match block {
            Block::ElectricFurnace => quads = glow_face(ElectricFrontLit, ElectricFront, y, xs, zs),
            Block::Assembler => quads = glow_face(AssemblerFrontLit, AssemblerFront, y, xs, zs),
            Block::ChipFab => quads = glow_face(FabFrontLit, FabFront, y, xs, zs),
            Block::FuelFabricator => quads = glow_face(ControlPanelLit, ControlPanel, y, xs, zs),
            Block::Miner => {
                quads = glow_face(MinerFrontLit, MinerFront, -15.0, [0.0, 16.0], [3.0, 8.0])
            }
            Block::OilPump => {
                quads = glow_face(PumpFrontLit, PumpFront, 10.0, [3.0, 13.0], [1.5, 7.0])
            }
            Block::CoolingPool => quads = pool_water(),
            Block::ParticleAccelerator => quads = beam_glow(),
            Block::Electrolyzer => {
                quads.extend(lamp([2.3, 8.8, 25.0], [5.7, 12.2, 26.5], [255, 90, 80]));
                quads.extend(lamp([10.3, 8.8, 25.0], [13.7, 12.2, 26.5], [100, 160, 255]));
            }
            Block::NuclearReactor => {
                // The core seen through a port on the dome, Cherenkov blue.
                quads.extend(lamp([5.0, 5.0, 73.0], [11.0, 11.0, 73.2], [90, 170, 255]));
            }
            _ => {}
        }
        if has_lamp(block) {
            let (from, to) = lamp_place(block);
            quads.extend(lamp(from, to, [90, 255, 120]));
        }
    }
    match block {
        Block::NuclearReactor if state.alarm => {
            quads.extend(lamp(ALARM.0, ALARM.1, [255, 50, 30]));
        }
        Block::Rtg => quads.extend(lamp([6.0, 6.0, 12.0], [10.0, 10.0, 12.6], [255, 120, 50])),
        Block::StorageTank if state.fluid > 0 && state.level > 0 => {
            let color = crate::factory::fluids::Fluid::ALL
                .get(state.fluid as usize - 1)
                .map_or([255, 255, 255], |f| f.color());
            let fill = gauge_fill(state.level);
            quads.extend(lamp([6.6, 0.8, GAUGE_BOTTOM], [9.4, 1.2, fill], color));
        }
        _ => {}
    }
    if block == Block::Accumulator && state.level > 0 {
        let full =
            TUBE_BOTTOM + (TUBE_TOP - TUBE_BOTTOM) * state.level.min(LEVELS) as f32 / LEVELS as f32;
        for x in TUBES {
            quads.extend(lamp(
                [x, 0.4, TUBE_BOTTOM],
                [x + 2.5, 1.0, full],
                [80, 235, 190],
            ));
        }
    }
    quads
}

/// The parts of a model block that move while it works, or a door's leaves (drawn each frame:
/// `part_model`).
pub fn moving_parts(block: Block, state: ModelState) -> Vec<MovingPart> {
    use Tex::*;
    let (y, xs, zs) = HOUSING_FRONT;
    if let Some((kind, _)) = block.door() {
        return door_model(kind, state.piece).1;
    }
    if !state.is_core() {
        return Vec::new();
    }
    let fan = |center: [f32; 3], axis: usize, r: f32| {
        // Two crossed blades across `axis`.
        let (a, b) = ([0, 1, 2][(axis + 1) % 3], [0, 1, 2][(axis + 2) % 3]);
        let mut q = Vec::new();
        for (u, v) in [(a, b), (b, a)] {
            let mut from = center;
            let mut to = center;
            from[u] -= r;
            to[u] += r;
            from[v] -= 0.3;
            to[v] += 0.3;
            from[axis] -= 0.2;
            to[axis] += 0.2;
            q.extend(quads_of(&[steel(from, to)]));
        }
        let mut dir = [0.0; 3];
        dir[axis] = 1.0;
        MovingPart::new(q, center, dir, Motion::Spin(9.0))
    };
    match block {
        Block::WindTurbine => vec![MovingPart::new(
            rotor(),
            ROTOR,
            [0.0, 1.0, 0.0],
            Motion::Spin(1.8),
        )],
        Block::SteamTurbine => vec![MovingPart::new(
            turbine_coupling(),
            [8.0, 16.0, 14.0],
            [0.0, 1.0, 0.0],
            Motion::Spin(14.0),
        )],
        Block::Blender => vec![fan([26.3, 8.0, 58.0], 0, 2.6)],
        Block::Centrifuge => vec![fan([8.0, 8.0, 44.4], 2, 2.6)],
        Block::WaterPump => vec![fan([8.0, 15.8, 5.5], 1, 2.6)],
        Block::Boiler => vec![MovingPart::light(
            glow_face(GeneratorFrontLit, GeneratorFront, y, xs, [1.5, 12.0]),
            Motion::Flicker(8.0),
        )],
        Block::Miner => miner_parts(),
        Block::OilPump => oil_pump_parts(),
        Block::CoalGenerator => vec![
            MovingPart::new(
                flywheel(),
                [15.9, 8.0, 7.0],
                [1.0, 0.0, 0.0],
                Motion::Spin(5.0),
            ),
            MovingPart::light(
                glow_face(GeneratorFrontLit, GeneratorFront, y, xs, zs),
                Motion::Flicker(9.0),
            ),
        ],
        Block::Refinery => vec![MovingPart::light(
            glow_face(
                RefineryFrontLit,
                RefineryFront,
                1.0,
                [1.0, 15.0],
                [1.5, 9.0],
            ),
            Motion::Flicker(6.0),
        )],
        Block::ArcFurnace => vec![MovingPart::light(
            glow_face(ArcFrontLit, ArcFront, y, xs, zs),
            Motion::Flicker(22.0),
        )],
        Block::ChipFab => {
            // The filter unit's fan: two blades.
            let blades = quads_of(&[
                steel([9.9, 11.45, 16.0], [13.6, 12.05, 16.4]),
                steel([11.45, 9.9, 16.0], [12.05, 13.6, 16.4]),
            ]);
            vec![MovingPart::new(
                blades,
                [11.75, 11.75, 16.2],
                [0.0, 0.0, 1.0],
                Motion::Spin(7.0),
            )]
        }
        _ => Vec::new(),
    }
}

/// A conveyor coming round a quarter bend from its model's +x side (`plus_x`) or -x side, to its
/// front: the belt, its edges and the two rails, as quads on arcs about the front corner on that
/// side. (Items follow the same curve: `factory::Place`.)
fn conveyor_bend(plus_x: bool) -> Vec<Quad> {
    use Tex::*;
    const SEGMENTS: usize = 16;
    // Built for the +x side, about the corner (16, 0): the belt's middle has radius 8.
    let point = |r: f32, s: f32, z: f32| {
        let a = std::f32::consts::FRAC_PI_2 * (1.0 + s);
        [16.0 + r * a.cos(), r * a.sin(), z]
    };
    let mut quads = Vec::new();
    // A quad, its corners put in the order that faces it along `normal` (outwards).
    let mut face = |corners: [[f32; 3]; 4], uv: [[f32; 2]; 4], tex: Tex, normal: [f32; 3]| {
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let (e1, e2) = (sub(corners[1], corners[0]), sub(corners[2], corners[0]));
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let facing = n[0] * normal[0] + n[1] * normal[1] + n[2] * normal[2];
        let (mut corners, mut uv) = (corners, uv);
        if facing < 0.0 {
            corners.reverse();
            uv.reverse();
        }
        quads.push(Quad {
            corners,
            uv,
            tex,
            tint: 0,
        });
    };
    let radial = |s: f32, sign: f32| {
        let a = std::f32::consts::FRAC_PI_2 * (1.0 + s);
        [a.cos() * sign, a.sin() * sign, 0.0]
    };
    for i in 0..SEGMENTS {
        let (s0, s1) = (i as f32 / SEGMENTS as f32, (i + 1) as f32 / SEGMENTS as f32);
        let sm = (s0 + s1) * 0.5;
        let v = |s: f32| 16.0 * (1.0 - s);
        let u = |r: f32| (r - 1.0) / 14.0 * 16.0;
        // The belt's top: chevrons running round the bend.
        let (r0, r1, top) = (1.0, 15.0, BELT_TOP);
        face(
            [
                point(r0, s0, top),
                point(r1, s0, top),
                point(r1, s1, top),
                point(r0, s1, top),
            ],
            [
                [u(r0), v(s0)],
                [u(r1), v(s0)],
                [u(r1), v(s1)],
                [u(r0), v(s1)],
            ],
            ConveyorBelt,
            [0.0, 0.0, 1.0],
        );
        // Its edges, and the rails' faces and tops: (radius, from z, to z, outward, texture).
        let walls = [
            (15.0, 2.0, BELT_TOP, 1.0, CableSkin),
            (1.0, 2.0, BELT_TOP, -1.0, CableSkin),
            (16.0, 0.0, 5.0, 1.0, MachineSide),
            (15.0, BELT_TOP, 5.0, -1.0, MachineSide),
            (1.0, BELT_TOP, 5.0, 1.0, MachineSide),
            (0.01, 0.0, 5.0, -1.0, MachineSide),
        ];
        for (r, z0, z1, sign, tex) in walls {
            face(
                [
                    point(r, s0, z0),
                    point(r, s1, z0),
                    point(r, s1, z1),
                    point(r, s0, z1),
                ],
                [
                    [s0 * 16.0, 16.0 - z0],
                    [s1 * 16.0, 16.0 - z0],
                    [s1 * 16.0, 16.0 - z1],
                    [s0 * 16.0, 16.0 - z1],
                ],
                tex,
                radial(sm, sign),
            );
        }
        for (ra, rb) in [(15.0, 16.0), (0.01, 1.0)] {
            face(
                [
                    point(ra, s0, 5.0),
                    point(rb, s0, 5.0),
                    point(rb, s1, 5.0),
                    point(ra, s1, 5.0),
                ],
                [[0.0, v(s0)], [1.0, v(s0)], [1.0, v(s1)], [0.0, v(s1)]],
                MachineTop,
                [0.0, 0.0, 1.0],
            );
        }
    }
    if !plus_x {
        // Mirrored across the middle (which turns every face inside out: put them back).
        for q in &mut quads {
            q.corners = q.corners.map(|[x, y, z]| [16.0 - x, y, z]);
            q.corners.reverse();
            q.uv.reverse();
        }
    }
    quads
}

/// A point in a cell given in model units (x, y across, z up, 16 = one block; z may go past
/// the top), planet-relative.
pub fn cell_point(planet: &Planet, cell: BlockCell, [x, y, z]: [f32; 3]) -> DVec3 {
    let c = planet.cell_corners(cell);
    let [u, v, k] = [x, y, z].map(|t| t as f64 / 16.0);
    let lerp = |a: DVec3, b: DVec3, t: f64| a + (b - a) * t;
    let bottom = lerp(lerp(c[0], c[1], u), lerp(c[2], c[3], u), v);
    let top = lerp(lerp(c[4], c[5], u), lerp(c[6], c[7], u), v);
    lerp(bottom, top, k)
}

/// `cell_point` of a point of a model turned by `facing` (see `ModelState`).
pub fn model_point(planet: &Planet, cell: BlockCell, facing: u8, p: [f32; 3]) -> DVec3 {
    cell_point(planet, cell, rotate(p, facing))
}

/// A box face: its corners, texture, and how a corner maps to texel uvs.
type BoxFace<'a> = ([[f32; 3]; 4], Tex, &'a dyn Fn([f32; 3]) -> [f32; 2]);

/// A face as 4 corners (model units, counter-clockwise seen from outside) with texel uvs.
pub struct Quad {
    pub corners: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub tex: Tex,
    /// This face's own linear RGBA tint, or 0 for the mesh's.
    pub tint: u32,
}

pub(crate) fn box_quads(b: &ModelBox, out: &mut Vec<Quad>) {
    let ([x0, y0, z0], [x1, y1, z1]) = (b.from, b.to);
    // Texel coordinates of a point on a face: (horizontal, from the top).
    let span = |v: f32, lo: f32, hi: f32| {
        if b.full_uv {
            (v - lo) / (hi - lo) * 16.0
        } else {
            v
        }
    };
    // (Boxes high up are textured as if they were in the bottom block, so their texels stay
    // in range.)
    let lift = if b.full_uv {
        0.0
    } else {
        (z0 / 16.0).floor() * 16.0
    };
    let side_uv = |h: f32, lo: f32, hi: f32, z: f32| {
        [span(h, lo, hi), 16.0 - span(z - lift, z0 - lift, z1 - lift)]
    };
    let flip = |h: f32, lo: f32, hi: f32| if b.full_uv { lo + hi - h } else { 16.0 - h };
    let faces: [BoxFace; 6] = [
        // +z (top), -z (bottom)
        (
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            b.top,
            &|p| [span(p[0], x0, x1), span(p[1], y0, y1)],
        ),
        (
            [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
            b.bottom,
            &|p| [span(p[0], x0, x1), span(p[1], y0, y1)],
        ),
        // -y (front), +y (back)
        (
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            b.front,
            &|p| side_uv(p[0], x0, x1, p[2]),
        ),
        (
            [[x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1]],
            b.back,
            &|p| side_uv(flip(p[0], x0, x1), x0, x1, p[2]),
        ),
        // +x (right), -x (left)
        (
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            b.side,
            &|p| side_uv(p[1], y0, y1, p[2]),
        ),
        (
            [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
            b.side,
            &|p| side_uv(flip(p[1], y0, y1), y0, y1, p[2]),
        ),
    ];
    for (corners, tex, uv) in faces {
        out.push(Quad {
            corners,
            uv: corners.map(uv),
            tex,
            tint: 0,
        });
    }
}

/// Moves a group of quads (their corners) with `f`.
pub(crate) fn transform(quads: &mut [Quad], f: impl Fn([f32; 3]) -> [f32; 3]) {
    for q in quads {
        q.corners = q.corners.map(&f);
    }
}

/// Rotates `p` about the x axis through (y, z) = `pivot` by `angle` radians.
pub(crate) fn pitch(p: [f32; 3], pivot: [f32; 2], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let (dy, dz) = (p[1] - pivot[0], p[2] - pivot[1]);
    [p[0], pivot[0] + dy * c - dz * s, pivot[1] + dy * s + dz * c]
}

/// Turns quads into mesh vertices. `place` maps model units to planet-relative meters;
/// positions are stored relative to `origin`.
pub fn build(
    quads: &[Quad],
    place: impl Fn([f32; 3]) -> DVec3,
    origin: DVec3,
    light: Light,
    tint: u32,
    cutout: bool,
) -> ChunkMesh {
    build_with_unit(quads, place, origin, light, tint, cutout, POSITION_UNIT)
}

/// `build` with `unit` meters per position unit.
fn build_with_unit(
    quads: &[Quad],
    place: impl Fn([f32; 3]) -> DVec3,
    origin: DVec3,
    light: Light,
    tint: u32,
    cutout: bool,
    unit: f64,
) -> ChunkMesh {
    let mut mesh = ChunkMesh::new(unit as f32);
    let mesh_tint = BlockVertex::tint565(tint);
    for q in quads {
        let tint = if q.tint != 0 {
            BlockVertex::tint565(q.tint)
        } else {
            mesh_tint
        };
        for (corner, uv) in q.corners.iter().zip(q.uv) {
            let p = (place(*corner) - origin) / unit;
            let texel = |t: f32| (t.round().max(0.0) as u32).min(31);
            let data = q.tex as u32
                | (3 << 10)
                | (light.sky.min(15) << 12)
                | (texel(uv[0]) << 16)
                | (texel(uv[1]) << 21)
                | MODEL_FLAG
                | if cutout { CUTOUT_FLAG } else { 0 }
                | (light.block.min(15) << 28);
            let position = [p.x.round() as i16, p.y.round() as i16, p.z.round() as i16];
            mesh.opaque.push(BlockVertex::new(position, tint, data));
        }
    }
    mesh
}

/// Rotates model coordinates by `facing` quarter turns about the vertical axis.
fn rotate([x, y, z]: [f32; 3], facing: u8) -> [f32; 3] {
    match facing % 4 {
        0 => [x, y, z],
        1 => [16.0 - y, x, z],
        2 => [16.0 - x, 16.0 - y, z],
        _ => [y, 16.0 - x, z],
    }
}

/// The mesh of a model block in `cell`: (mesh, planet-relative origin, bounding radius).
/// Its moving parts are separate (`part_model`).
pub fn block_model(
    planet: &Planet,
    cell: BlockCell,
    block: Block,
    state: ModelState,
    light: Light,
) -> (ChunkMesh, DVec3, f32) {
    let mut quads = Vec::new();
    match block {
        Block::Chest => {
            for (b, lid) in chest() {
                let mut part = Vec::new();
                box_quads(&b, &mut part);
                if lid && state.open {
                    // Hinged at the back edge.
                    transform(&mut part, |p| pitch(p, [15.0, 10.0], -1.1));
                }
                quads.extend(part);
            }
        }
        Block::Torch => {
            box_quads(&torch(), &mut quads);
            if state.wall {
                // Leaning out from the wall behind it (+y), its foot against the wall.
                transform(&mut quads, |[x, y, z]| [x, y + 6.5 - 0.45 * z, z + 3.5]);
            }
        }
        _ => quads = model_quads(block, state),
    }
    let c = planet.cell_corners(cell);
    let origin = c.iter().sum::<DVec3>() / 8.0;
    let place = cell_place(planet, cell, state.facing);
    // A torch's flame lights the torch itself.
    let light = if block == Block::Torch {
        Light { block: 15, ..light }
    } else {
        light
    };
    let mut mesh = build(&quads, &place, origin, light, 0xffff_ffff, false);
    let glow = glow_quads(block, state);
    let lit = Light { block: 15, ..light };
    mesh.opaque
        .extend(build(&glow, &place, origin, lit, 0xffff_ffff, false).opaque);
    let radius = quads
        .iter()
        .chain(&glow)
        .flat_map(|q| q.corners)
        .map(|p| (place(p) - origin).length())
        .fold(0.0, f64::max);
    (mesh, origin, radius as f32 + 0.05)
}

/// Where a point of a model (model units, turned by `facing`) is in `cell`, planet-relative.
fn cell_place(planet: &Planet, cell: BlockCell, facing: u8) -> impl Fn([f32; 3]) -> DVec3 {
    let c = planet.cell_corners(cell);
    move |p: [f32; 3]| {
        let [u, v, k] = rotate(p, facing).map(|x| x as f64 / 16.0);
        let lerp = |a: DVec3, b: DVec3, t: f64| a + (b - a) * t;
        let bottom = lerp(lerp(c[0], c[1], u), lerp(c[2], c[3], u), v);
        let top = lerp(lerp(c[4], c[5], u), lerp(c[6], c[7], u), v);
        lerp(bottom, top, k)
    }
}

/// A moving part's mesh, built in place about its pivot (see `MovingPart`).
pub struct PartMesh {
    pub mesh: ChunkMesh,
    /// Where the pivot is (planet-relative), the axis it turns about (unit), one model unit
    /// up (for `PartPose::lift`), and how far the part reaches from the pivot.
    pub pivot: DVec3,
    pub axis: DVec3,
    pub up: DVec3,
    pub radius: f32,
    /// One model unit along the axis (for `PartPose::shift`).
    pub step: DVec3,
}

/// A model block as it is held: all of it, its moving parts at rest; one taller than two
/// blocks (a miner's rig) shrunk to that. A door: all its cells.
pub fn held_model(block: Block) -> Vec<Quad> {
    let mut quads = Vec::new();
    let (w, h) = block.door().map_or((1, 1), |(kind, _)| kind.size());
    for row in 0..h {
        for col in 0..w {
            let state = ModelState {
                piece: (col + 3 * row) as u8,
                ..Default::default()
            };
            let mut cell = model_quads(block, state);
            cell.extend(moving_parts(block, state).into_iter().flat_map(|p| p.quads));
            let (dx, dz) = (col as f32 * 16.0, row as f32 * 16.0);
            transform(&mut cell, |[x, y, z]| [x + dx, y, z + dz]);
            quads.extend(cell);
        }
    }
    if w > 1 {
        // (A big door: about the middle cell.)
        transform(&mut quads, |[x, y, z]| [x - 16.0, y, z]);
    }
    let top = quads
        .iter()
        .flat_map(|q| q.corners)
        .fold(0.0f32, |m, p| m.max(p[2]));
    // (Big machines too: as wide as a block and a half at most.)
    let wide = quads.iter().flat_map(|q| q.corners).fold(0.0f32, |m, p| {
        m.max((p[0] - 8.0).abs()).max((p[1] - 8.0).abs())
    });
    let s = (34.0 / top).min(12.0 / wide).min(1.0);
    if s < 1.0 {
        transform(&mut quads, |[x, y, z]| {
            [8.0 + (x - 8.0) * s, 8.0 + (y - 8.0) * s, z.max(0.0) * s]
        });
    }
    quads
}

/// The mesh of moving part `part` of the model in `cell` (facing `facing`).
pub fn part_model(
    planet: &Planet,
    cell: BlockCell,
    facing: u8,
    part: &MovingPart,
    light: Light,
) -> PartMesh {
    let place = cell_place(planet, cell, facing);
    let pivot = place(part.pivot);
    let offset = |d: [f32; 3]| {
        let [x, y, z] = part.pivot;
        place([x + d[0], y + d[1], z + d[2]]) - pivot
    };
    let light = if part.glow {
        Light { block: 15, ..light }
    } else {
        light
    };
    let radius = part
        .quads
        .iter()
        .flat_map(|q| q.corners)
        .map(|p| (place(p) - pivot).length())
        .fold(0.0, f64::max);
    PartMesh {
        mesh: build(&part.quads, &place, pivot, light, 0xffff_ffff, false),
        pivot,
        axis: offset(part.axis).normalize_or(DVec3::Z),
        up: offset([0.0, 0.0, 1.0]),
        radius: radius as f32 + 0.05,
        step: offset(part.axis),
    }
}

/// The facing (see `ModelState`) that turns a model's front towards `towards` (a
/// planet-relative direction, e.g. from the block to the player).
pub fn facing_towards(planet: &Planet, cell: BlockCell, towards: DVec3) -> u8 {
    let c = planet.cell_corners(cell);
    let u = (c[1] - c[0]).normalize();
    let v = (c[2] - c[0]).normalize();
    let (du, dv) = (towards.dot(u), towards.dot(v));
    if du.abs() > dv.abs() {
        if du > 0.0 { 1 } else { 3 }
    } else if dv > 0.0 {
        2
    } else {
        0
    }
}

/// (east, north, up) at a spot: the frame dropped items and mobs are built in.
pub type Frame = (DVec3, DVec3, DVec3);

/// The mesh of a dropped item lying at the origin: a small cube for blocks, the sprite one
/// pixel thick for other items, standing up (drawn spinning about the origin: `Drop::spin`).
pub fn drop_model(item: Item, frame: Frame, light: Light) -> ChunkMesh {
    item_model(item, frame, light, 1.0)
}

/// An item riding on a conveyor: a dropped item's model, smaller, built along +x (the way the
/// belt runs) with +z up, to be turned into place when drawn (`DrawItem::rotation`).
pub fn belt_item_model(item: Item, light: Light) -> ChunkMesh {
    item_model(item, (DVec3::X, DVec3::Y, DVec3::Z), light, 0.75)
}

/// `drop_model` with its size scaled by `scale`.
fn item_model(item: Item, (east, north, up): Frame, light: Light, scale: f64) -> ChunkMesh {
    let mut quads = Vec::new();
    let mut edges = Vec::new();
    let (size, cutout, tint) = match item.icon() {
        Icon::Block(block) => {
            let [top, side, bottom] = block.textures();
            box_quads(
                &ModelBox {
                    full_uv: true,
                    ..cuboid([0.0; 3], [16.0; 3], top, side, bottom)
                },
                &mut quads,
            );
            let tint = match block.tint() {
                Tint::Grass | Tint::Foliage => 0xff_3c9a4c,
                Tint::Spruce => 0xff_3c5a3c,
                _ => 0xffff_ffff,
            };
            (0.25, block.is_cutout(), tint)
        }
        Icon::Sprite(tex) => {
            // The sprite standing up, one pixel thick (as held: `extruded_sprite`).
            let (faces, rim) = extruded_sprite(tex, &crate::world::textures::pixels(tex));
            quads = faces;
            edges = rim;
            (0.4, true, 0xffff_ffff)
        }
        Icon::Picture(_) => {
            // A machine whose picture only the interface has: its model, made small (as held,
            // then shrunk to fit a block).
            if let Some(block) = item.block() {
                quads = held_model(block);
                let (top, wide) =
                    quads
                        .iter()
                        .flat_map(|q| q.corners)
                        .fold((16.0f32, 8.0f32), |(t, w), p| {
                            (
                                t.max(p[2]),
                                w.max((p[0] - 8.0).abs()).max((p[1] - 8.0).abs()),
                            )
                        });
                let s = 16.0 / top.max(2.0 * wide);
                transform(&mut quads, |[x, y, z]| {
                    [8.0 + (x - 8.0) * s, 8.0 + (y - 8.0) * s, z * s]
                });
            }
            (0.32, false, 0xffff_ffff)
        }
    };
    let place = |[x, y, z]: [f32; 3]| {
        let s = size * scale / 16.0;
        east * ((x as f64 - 8.0) * s) + north * ((y as f64 - 8.0) * s) + up * (z as f64 * s)
    };
    let mut mesh = build(&quads, place, DVec3::ZERO, light, tint, cutout);
    mesh.opaque
        .extend(build(&edges, place, DVec3::ZERO, light, tint, false).opaque);
    mesh
}

/// A cable hanging from `a` to `b` (planet-relative): a thin dark tube sagging `sag` metres in
/// the middle. Returns the mesh and its origin (the midpoint).
pub fn cable_model(a: DVec3, b: DVec3, sag: f64, light: Light) -> (ChunkMesh, DVec3) {
    const SEGMENTS: usize = 10;
    const HALF_WIDTH: f64 = 0.022;
    let origin = (a + b) * 0.5;
    let up = origin.normalize_or(DVec3::Y);
    let point = |i: usize| {
        let t = i as f64 / SEGMENTS as f64;
        a + (b - a) * t - up * (sag * 4.0 * t * (1.0 - t)) - origin
    };
    let mut quads = Vec::new();
    for i in 0..SEGMENTS {
        let (p0, p1) = (point(i), point(i + 1));
        let Some(along) = (p1 - p0).try_normalize() else {
            continue;
        };
        let side = along
            .cross(up)
            .try_normalize()
            .unwrap_or(along.any_orthonormal_vector());
        let over = side.cross(along);
        for n in [side, over, -side, -over] {
            // Corners counter-clockwise seen from outside.
            let (e, t) = (n * HALF_WIDTH, n.cross(along) * HALF_WIDTH);
            let corners = [p0 + e - t, p1 + e - t, p1 + e + t, p0 + e + t];
            quads.push(Quad {
                corners: corners.map(|c| c.as_vec3().to_array()),
                uv: [[0.0, 0.0], [16.0, 0.0], [16.0, 1.0], [0.0, 1.0]],
                tex: Tex::CableSkin,
                tint: 0,
            });
        }
    }
    let place = |p: [f32; 3]| origin + DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
    (
        build(&quads, place, origin, light, 0xffff_ffff, false),
        origin,
    )
}

/// A whole block as a loose cube of edge `size` metres, centred on the origin (flying debris).
pub fn block_cube(block: Block, size: f64, light: Light) -> ChunkMesh {
    let [top, side, bottom] = block.textures();
    let mut quads = Vec::new();
    box_quads(
        &ModelBox {
            full_uv: true,
            ..cuboid([0.0; 3], [16.0; 3], top, side, bottom)
        },
        &mut quads,
    );
    let tint = match block.tint() {
        Tint::Grass | Tint::Foliage => 0xff_3c9a4c,
        Tint::Spruce => 0xff_3c5a3c,
        _ => 0xffff_ffff,
    };
    let place = |[x, y, z]: [f32; 3]| DVec3::new(x as f64, y as f64, z as f64) * (size / 16.0);
    let origin = DVec3::splat(size * 0.5);
    build(&quads, place, origin, light, tint, block.is_cutout())
}

/// Meters per position unit of first-person meshes: fine, since they're seen from a few
/// centimetres (`POSITION_UNIT` would make them jitter by about a pixel as you turn).
pub const VIEW_UNIT: f64 = 1.0 / 16384.0;

/// The player's arm in its own units (`viewmodel::ARM_UNIT` m each): x right, y along the arm
/// towards the hand, z up. Deliberately plain and blocky: one skin block for hand and forearm
/// (its grip at the origin), running back along -y into a shirt sleeve. (A suit can replace it
/// later.)
fn bare_arm() -> Vec<ModelBox> {
    vec![
        skin([-3.0, -18.0, -2.6], [3.0, 3.5, 3.0], Tex::Skin),
        skin([-3.4, -46.0, -3.0], [3.4, -18.0, 3.4], Tex::Sleeve),
    ]
}

/// How a held item is shaped, in model units (0..16 = one block).
pub enum HeldShape {
    /// A block as a cube, or a model block (table, furnace, chest, torch) as its model.
    Block(Block),
    /// A flat sprite extruded one pixel thick, lying in the x-z plane (x right, z up, the
    /// image's top at z = 16) between y = 7.5 and 8.5. Its pixels come along.
    Sprite(Tex, Vec<[u8; 4]>),
}

impl HeldShape {
    pub fn of(item: Item) -> HeldShape {
        match (item.block(), item.icon()) {
            (Some(block), _) if block.is_model() => HeldShape::Block(block),
            (_, Icon::Block(block)) => HeldShape::Block(block),
            (_, Icon::Sprite(tex)) => HeldShape::Sprite(tex, crate::world::textures::pixels(tex)),
            // (Only machines have pictures, and they are models, caught above.)
            (block, Icon::Picture(_)) => HeldShape::Block(block.unwrap_or(Block::Stone)),
        }
    }
}

/// The first-person view: the arm (mapped by `arm` from arm units) and the held item
/// (mapped by `held` from model units), both to camera-relative meters.
pub fn view_model(
    shape: Option<&HeldShape>,
    arm: impl Fn([f32; 3]) -> DVec3,
    held: impl Fn([f32; 3]) -> DVec3,
    light: Light,
) -> ChunkMesh {
    let mut quads = Vec::new();
    for b in bare_arm() {
        box_quads(&b, &mut quads);
    }
    let mut mesh = build_with_unit(
        &quads,
        arm,
        DVec3::ZERO,
        light,
        0xffff_ffff,
        false,
        VIEW_UNIT,
    );
    let Some(shape) = shape else { return mesh };

    let mut quads = Vec::new();
    let mut edges = Vec::new();
    let mut tint = 0xffff_ffff;
    let mut cutout = false;
    match shape {
        HeldShape::Block(block) => match block {
            b if b.is_model() => quads = held_model(*b),
            _ => {
                let [top, side, bottom] = block.textures();
                tint = match block.tint() {
                    Tint::Grass | Tint::Foliage => 0xff_3c9a4c,
                    Tint::Spruce => 0xff_3c5a3c,
                    _ => 0xffff_ffff,
                };
                cutout = block.is_cutout();
                let cube = ModelBox {
                    full_uv: true,
                    ..cuboid([0.0; 3], [16.0; 3], top, side, bottom)
                };
                box_quads(&cube, &mut quads);
            }
        },
        HeldShape::Sprite(tex, pixels) => {
            cutout = true;
            (quads, edges) = extruded_sprite(*tex, pixels);
        }
    }
    let item = build_with_unit(&quads, &held, DVec3::ZERO, light, tint, cutout, VIEW_UNIT);
    mesh.opaque.extend(item.opaque);
    // Rim faces sample a single texel each; no cutout (mipmaps could fade them out).
    let rim = build_with_unit(&edges, &held, DVec3::ZERO, light, tint, false, VIEW_UNIT);
    mesh.opaque.extend(rim.opaque);
    mesh
}

/// A sprite extruded one pixel thick, lying in the x-z plane (x right, z up, the image's top
/// at z = 16) between y = 7.5 and 8.5: its front and back (cut out where it is clear), and its
/// rim, a one-texel face wherever an opaque pixel borders a clear one, coloured by that pixel
/// (returned apart: drawn without the cutout, which mipmaps could fade out). `pixels`: the
/// sprite's (`textures::pixels`).
pub fn extruded_sprite(tex: Tex, pixels: &[[u8; 4]]) -> (Vec<Quad>, Vec<Quad>) {
    let mut quads = Vec::new();
    let mut edges = Vec::new();
    let (y0, y1) = (7.5, 8.5);
    let full = [[0.0, 16.0], [16.0, 16.0], [16.0, 0.0], [0.0, 0.0]];
    // Front and back: the whole image, transparent pixels cut out.
    quads.push(Quad {
        corners: [
            [0.0, y0, 0.0],
            [16.0, y0, 0.0],
            [16.0, y0, 16.0],
            [0.0, y0, 16.0],
        ],
        uv: full,
        tex,
        tint: 0,
    });
    quads.push(Quad {
        corners: [
            [16.0, y1, 0.0],
            [0.0, y1, 0.0],
            [0.0, y1, 16.0],
            [16.0, y1, 16.0],
        ],
        uv: [[16.0, 16.0], [0.0, 16.0], [0.0, 0.0], [16.0, 0.0]],
        tex,
        tint: 0,
    });
    // The rim: a one-pixel face wherever an opaque pixel borders a clear one,
    // coloured by that pixel.
    let solid = |x: i32, y: i32| {
        (0..16).contains(&x) && (0..16).contains(&y) && pixels[(y * 16 + x) as usize][3] >= 128
    };
    for py in 0..16 {
        for px in 0..16 {
            if !solid(px, py) {
                continue;
            }
            let (x0, x1) = (px as f32, px as f32 + 1.0);
            let (z0, z1) = (15.0 - py as f32, 16.0 - py as f32);
            let (u0, u1, v0, v1) = (x0, x1, py as f32, py as f32 + 1.0);
            let uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
            if !solid(px - 1, py) {
                let corners = [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]];
                edges.push(Quad {
                    corners,
                    uv,
                    tex,
                    tint: 0,
                });
            }
            if !solid(px + 1, py) {
                let corners = [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]];
                edges.push(Quad {
                    corners,
                    uv,
                    tex,
                    tint: 0,
                });
            }
            if !solid(px, py - 1) {
                let corners = [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]];
                edges.push(Quad {
                    corners,
                    uv,
                    tex,
                    tint: 0,
                });
            }
            if !solid(px, py + 1) {
                let corners = [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]];
                edges.push(Quad {
                    corners,
                    uv,
                    tex,
                    tint: 0,
                });
            }
        }
    }
    (quads, edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::planet::Face;

    #[test]
    fn models_fit_their_cell() {
        let planet = Planet::new(DVec3::ZERO, 64, 128, 1);
        let cell = BlockCell {
            face: Face(2),
            u: 700,
            v: 900,
            k: planet.sea_layer + 3,
        };
        let corners = planet.cell_corners(cell);
        let centre = corners.iter().sum::<DVec3>() / 8.0;
        for block in [
            Block::CraftingTable,
            Block::Furnace,
            Block::Chest,
            Block::Torch,
            Block::Conveyor,
            Block::PowerPole,
            Block::CoalGenerator,
            Block::ElectricFurnace,
            Block::Miner,
            Block::OilPump,
            Block::Refinery,
            Block::ArcFurnace,
            Block::Assembler,
            Block::ChipFab,
            Block::Accumulator,
            Block::SolarPanel,
            Block::Door,
            Block::SlidingDoorOpen,
            Block::BigDoor,
            Block::Pipe,
            Block::StorageTank,
            Block::WaterPump,
            Block::Electrolyzer,
            Block::Centrifuge,
            Block::FuelFabricator,
            Block::Boiler,
            Block::GeothermalWell,
            Block::WindTurbine,
            Block::Rtg,
            Block::Blender,
            Block::SteamTurbine,
            Block::NuclearReactor,
            Block::CoolingPool,
            Block::ReprocessingPlant,
            Block::ParticleAccelerator,
        ] {
            for facing in 0..4 {
                for (i, (wall, open, bend)) in
                    [(false, false, 0), (true, true, 1), (false, true, 2)]
                        .into_iter()
                        .enumerate()
                {
                    let state = ModelState {
                        facing,
                        lit: bend != 2,
                        wall,
                        open,
                        bend,
                        level: 13,
                        // (Every cell of a big door, in turn.)
                        piece: if block == Block::BigDoor {
                            (facing * 3 + i as u8) % 9
                        } else {
                            i as u8 % 2 * 3
                        },
                        // (A pipe joined various ways, a tank's gauge part full.)
                        links: [0, 0b10_0101, 0b11_1111][i],
                        ports: [0, 0b00_0100, 0b10_0000][i],
                        axis: i as u8,
                        fluid: i as u8,
                        alarm: i == 1,
                        ..Default::default()
                    };
                    let (mesh, origin, radius) =
                        block_model(&planet, cell, block, state, Light::default());
                    assert_eq!(mesh.opaque.len() % 4, 0);
                    // (A big door's middle cells are only their moving slats.)
                    assert!(!mesh.opaque.is_empty() || block == Block::BigDoor);
                    assert!((origin - centre).length() < 1e-9);
                    // In its cell; a pole two blocks tall, a big machine
                    // in the cells it fills (and a water pump's intake in the one below).
                    let (r, h) = block.structure();
                    let most = match block {
                        Block::PowerPole | Block::Refinery => 1.9,
                        Block::WaterPump => 1.8,
                        b if b.is_structure() => {
                            let across = r as f64 + 0.5;
                            (2.0 * across * across + (h as f64 - 0.5).powi(2)).sqrt() as f32 + 0.3
                        }
                        _ => 1.3,
                    };
                    assert!(radius < most, "{block:?} reaches {radius} m");
                    for v in &mesh.opaque {
                        let p = DVec3::new(
                            v.position[0] as f64,
                            v.position[1] as f64,
                            v.position[2] as f64,
                        ) * POSITION_UNIT;
                        assert!(p.length() <= radius as f64, "vertex outside the cell: {p}");
                        assert!(v.data & MODEL_FLAG != 0 && v.data & CUTOUT_FLAG == 0);
                    }
                    for part in moving_parts(block, state) {
                        let made = part_model(&planet, cell, facing, &part, Light::default());
                        assert!(
                            !made.mesh.opaque.is_empty()
                                && made.mesh.opaque.len().is_multiple_of(4)
                        );
                        assert!((made.axis.length() - 1.0).abs() < 1e-9);
                        assert!(made.radius < 4.6);
                    }
                }
            }
        }
        // Facing towards +u turns the model a quarter.
        let u_dir = corners[1] - corners[0];
        assert_eq!(facing_towards(&planet, cell, u_dir), 1);
        assert_eq!(facing_towards(&planet, cell, -u_dir), 3);
    }

    /// A miner's drill goes all the way down turning and comes back up; the beacon blinks and
    /// a fire flickers only while the machine works; an accumulator's tubes fill.
    #[test]
    fn machines_move_while_they_work() {
        let lifts: Vec<f32> = (0..600)
            .map(|i| Motion::Drill.pose(i as f64 * 0.01, 1.0, 0.0, 1).lift)
            .collect();
        assert_eq!(lifts[0], 0.0);
        let deepest = lifts.iter().cloned().fold(0.0, f32::min);
        assert!((-DRILL_STROKE..-0.95 * DRILL_STROKE).contains(&deepest));
        assert!(lifts.windows(2).filter(|w| w[1] > w[0] + 1e-4).count() > 30);
        assert!(Motion::Drill.pose(1.0, 1.0, 0.0, 1).angle != 0.0);
        assert_eq!(Motion::Carriage.pose(1.0, 1.0, 0.0, 1).angle, 0.0);
        let shown = |m: Motion, speed: f32| {
            (0..200)
                .filter(|&i| m.pose(0.0, speed, i as f64 * 0.05, 7).shown)
                .count()
        };
        for m in [Motion::Blink, Motion::Flicker(9.0)] {
            assert_eq!(shown(m, 0.0), 0);
            assert!((20..200).contains(&shown(m, 1.0)), "{m:?}");
        }
        let lit = |level| {
            glow_quads(
                Block::Accumulator,
                ModelState {
                    level,
                    ..Default::default()
                },
            )
        };
        assert!(lit(0).is_empty());
        let top = |level| {
            lit(level)
                .iter()
                .flat_map(|q| q.corners)
                .fold(0.0f32, |m, p| m.max(p[2]))
        };
        assert!(top(5) < top(15) && top(15) < top(LEVELS));
    }

    /// Doors open by how far open they are: a leaf swings a quarter turn, a sliding one moves
    /// most of a block, a big door's slats roll up out of sight (the bottom one last), and its
    /// lamp warns only while it moves.
    #[test]
    fn doors_open() {
        let pose = |m: Motion, open: f32, moving: f64| m.pose(moving, open, 0.1, 1);
        assert_eq!(pose(Motion::Swing, 0.0, 0.0).angle, 0.0);
        assert!((pose(Motion::Swing, 1.0, 0.0).angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((pose(Motion::Slide, 1.0, 0.0).shift - SLIDE).abs() < 1e-6);
        for (bottom, gone_by) in [(44.0, 0.1), (0.0, 1.0)] {
            assert!(pose(Motion::Shutter(bottom), 0.0, 0.0).shown);
            assert!(
                !pose(Motion::Shutter(bottom), gone_by, 0.0).shown,
                "{bottom}"
            );
        }
        assert!(pose(Motion::Shutter(0.0), 0.9, 0.0).shown);
        assert!(!pose(Motion::Warn, 0.5, 0.0).shown && pose(Motion::Warn, 0.5, 1.0).shown);
        // Every cell of a big door has its slats; a hinged door's two cells their leaf.
        for piece in 0..9 {
            let (_, moving) = door_model(DoorKind::Big, piece);
            let slats = moving
                .iter()
                .filter(|p| matches!(p.motion, Motion::Shutter(_)))
                .count();
            assert_eq!(slats, 4);
        }
        assert_eq!(door_model(DoorKind::Hinged, 3).1.len(), 1);
        // Held, a door shows all of it.
        assert!(held_model(Block::Door).len() > 2 * door_model(DoorKind::Hinged, 0).0.len());
    }

    #[test]
    fn drops_are_small() {
        let frame = (DVec3::X, DVec3::Z, DVec3::Y);
        for item in [Item::Block(Block::Dirt), Item::Stick] {
            let mesh = drop_model(item, frame, Light::default());
            assert!(!mesh.opaque.is_empty() && mesh.opaque.len().is_multiple_of(4));
            for v in &mesh.opaque {
                let size = v.position[..3]
                    .iter()
                    .map(|c| c.unsigned_abs())
                    .max()
                    .unwrap();
                assert!((size as f64) * POSITION_UNIT <= 0.41);
            }
        }
    }
}
