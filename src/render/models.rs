//! Small 3D models made of textured boxes, Minecraft style (16 units = 1 block, 1 texel per
//! unit): the crafting table, furnace, chest and torch, dropped items, and mobs.
//!
//! Models become ordinary `ChunkMesh`es drawn with the block shaders. Their vertices set
//! `MODEL_FLAG`, so the shader takes the texture coordinates (in texels) from the vertex
//! instead of repeating the texture per block; `CUTOUT_FLAG` makes transparent sprite pixels
//! see-through. Placed models are mapped into their block cell's real (slightly curved and
//! skewed) shape by trilinear interpolation of its 8 corners, so they fit the grid exactly.

use glam::DVec3;

use crate::{
    render::{BlockVertex, ChunkMesh},
    survival::items::{Icon, Item},
    world::{
        block::{Block, Tint},
        chunk::BlockCell,
        mesher::POSITION_UNIT,
        planet::Planet,
        textures::Tex,
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
}

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
    let side_uv = |h: f32, lo: f32, hi: f32, z: f32| [span(h, lo, hi), 16.0 - span(z, z0, z1)];
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
pub fn block_model(
    planet: &Planet,
    cell: BlockCell,
    block: Block,
    state: ModelState,
    light: Light,
) -> (ChunkMesh, DVec3, f32) {
    let mut quads = Vec::new();
    match block {
        Block::Furnace => furnace(state.lit)
            .iter()
            .for_each(|b| box_quads(b, &mut quads)),
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
        _ => crafting_table()
            .iter()
            .for_each(|b| box_quads(b, &mut quads)),
    }
    let c = planet.cell_corners(cell);
    let origin = c.iter().sum::<DVec3>() / 8.0;
    let place = |p: [f32; 3]| {
        let [u, v, k] = rotate(p, state.facing).map(|x| x as f64 / 16.0);
        let lerp = |a: DVec3, b: DVec3, t: f64| a + (b - a) * t;
        let bottom = lerp(lerp(c[0], c[1], u), lerp(c[2], c[3], u), v);
        let top = lerp(lerp(c[4], c[5], u), lerp(c[6], c[7], u), v);
        lerp(bottom, top, k)
    };
    // A torch's flame lights the torch itself.
    let light = if block == Block::Torch {
        Light { block: 15, ..light }
    } else {
        light
    };
    let mesh = build(&quads, place, origin, light, 0xffff_ffff, false);
    (mesh, origin, 1.2)
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

/// The mesh of a dropped item lying at the origin: a small cube for blocks, a crossed pair
/// of sprites for other items.
pub fn drop_model(item: Item, (east, north, up): Frame, light: Light) -> ChunkMesh {
    let mut quads = Vec::new();
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
            // Two crossed planes, each seen from both sides.
            let plane = |a: [f32; 3], b: [f32; 3]| [a, b, [b[0], b[1], 16.0], [a[0], a[1], 16.0]];
            for corners in [
                plane([0.0, 8.0, 0.0], [16.0, 8.0, 0.0]),
                plane([16.0, 8.0, 0.0], [0.0, 8.0, 0.0]),
                plane([8.0, 16.0, 0.0], [8.0, 0.0, 0.0]),
                plane([8.0, 0.0, 0.0], [8.0, 16.0, 0.0]),
            ] {
                let horizontal = |p: [f32; 3]| if p[1] == 8.0 { p[0] } else { 16.0 - p[1] };
                let (h0, h1) = (horizontal(corners[0]), horizontal(corners[1]));
                let uv = corners.map(|p| {
                    let h = horizontal(p);
                    // Each side shows the sprite the right way round.
                    let u = if h0 < h1 { h } else { 16.0 - h };
                    [u, 16.0 - p[2]]
                });
                quads.push(Quad {
                    corners,
                    uv,
                    tex,
                    tint: 0,
                });
            }
            (0.4, true, 0xffff_ffff)
        }
    };
    let place = |[x, y, z]: [f32; 3]| {
        let s = size / 16.0;
        east * ((x as f64 - 8.0) * s) + north * ((y as f64 - 8.0) * s) + up * (z as f64 * s)
    };
    build(&quads, place, DVec3::ZERO, light, tint, cutout)
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
        HeldShape::Block(block) => {
            let boxes = match block {
                Block::CraftingTable => crafting_table(),
                Block::Furnace => furnace(false),
                Block::Chest => chest().into_iter().map(|(b, _)| b).collect(),
                Block::Torch => vec![torch()],
                _ => {
                    let [top, side, bottom] = block.textures();
                    tint = match block.tint() {
                        Tint::Grass | Tint::Foliage => 0xff_3c9a4c,
                        Tint::Spruce => 0xff_3c5a3c,
                        _ => 0xffff_ffff,
                    };
                    cutout = block.is_cutout();
                    vec![ModelBox {
                        full_uv: true,
                        ..cuboid([0.0; 3], [16.0; 3], top, side, bottom)
                    }]
                }
            };
            for b in &boxes {
                box_quads(b, &mut quads);
            }
        }
        HeldShape::Sprite(tex, pixels) => {
            let tex = *tex;
            cutout = true;
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
                (0..16).contains(&x)
                    && (0..16).contains(&y)
                    && pixels[(y * 16 + x) as usize][3] >= 128
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
        }
    }
    let item = build_with_unit(&quads, &held, DVec3::ZERO, light, tint, cutout, VIEW_UNIT);
    mesh.opaque.extend(item.opaque);
    // Rim faces sample a single texel each; no cutout (mipmaps could fade them out).
    let rim = build_with_unit(&edges, &held, DVec3::ZERO, light, tint, false, VIEW_UNIT);
    mesh.opaque.extend(rim.opaque);
    mesh
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
        ] {
            for facing in 0..4 {
                for (wall, open) in [(false, false), (true, true)] {
                    let state = ModelState {
                        facing,
                        lit: true,
                        wall,
                        open,
                    };
                    let (mesh, origin, radius) =
                        block_model(&planet, cell, block, state, Light::default());
                    assert_eq!(mesh.opaque.len() % 4, 0);
                    assert!(!mesh.opaque.is_empty());
                    assert!((origin - centre).length() < 1e-9);
                    for v in &mesh.opaque {
                        let p = DVec3::new(
                            v.position[0] as f64,
                            v.position[1] as f64,
                            v.position[2] as f64,
                        ) * POSITION_UNIT;
                        assert!(p.length() <= radius as f64, "vertex outside the cell: {p}");
                        assert!(v.data & MODEL_FLAG != 0 && v.data & CUTOUT_FLAG == 0);
                    }
                }
            }
        }
        // Facing towards +u turns the model a quarter.
        let u_dir = corners[1] - corners[0];
        assert_eq!(facing_towards(&planet, cell, u_dir), 1);
        assert_eq!(facing_towards(&planet, cell, -u_dir), 3);
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
