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
    items::{Icon, Item},
    mobs::MobKind,
    render::{BlockVertex, ChunkMesh},
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
struct ModelBox {
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

fn cuboid(from: [f32; 3], to: [f32; 3], top: Tex, side: Tex, bottom: Tex) -> ModelBox {
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

/// A box with one texture stretched over every face (mobs).
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
struct Quad {
    corners: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    tex: Tex,
}

fn box_quads(b: &ModelBox, out: &mut Vec<Quad>) {
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
        });
    }
}

/// Moves a group of quads (their corners) with `f`.
fn transform(quads: &mut [Quad], f: impl Fn([f32; 3]) -> [f32; 3]) {
    for q in quads {
        q.corners = q.corners.map(&f);
    }
}

/// Rotates `p` about the x axis through (y, z) = `pivot` by `angle` radians.
fn pitch(p: [f32; 3], pivot: [f32; 2], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let (dy, dz) = (p[1] - pivot[0], p[2] - pivot[1]);
    [p[0], pivot[0] + dy * c - dz * s, pivot[1] + dy * s + dz * c]
}

/// Turns quads into mesh vertices. `place` maps model units to planet-relative meters;
/// positions are stored relative to `origin`.
fn build(
    quads: &[Quad],
    place: impl Fn([f32; 3]) -> DVec3,
    origin: DVec3,
    light: Light,
    tint: u32,
    cutout: bool,
) -> ChunkMesh {
    let mut mesh = ChunkMesh::new(POSITION_UNIT as f32);
    let tint = BlockVertex::tint565(tint);
    for q in quads {
        for (corner, uv) in q.corners.iter().zip(q.uv) {
            let p = (place(*corner) - origin) / POSITION_UNIT;
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
                quads.push(Quad { corners, uv, tex });
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

/// A mob's mesh around its feet, facing `forward` (with `up`), legs swung by `walk` (the
/// walk cycle's phase, radians), flashing red while `hurt`. Units are Minecraft pixels.
pub fn mob_model(
    kind: MobKind,
    walk: f32,
    swing: f32,
    hurt: bool,
    (forward, up): (DVec3, DVec3),
    light: Light,
) -> ChunkMesh {
    use Tex::*;
    let right = forward.cross(up);
    let (body, head, legs, face, leg_height) = match kind {
        MobKind::Cow => (
            skin([-6.0, -9.0, 12.0], [6.0, 9.0, 22.0], CowSkin),
            skin([-4.0, 9.0, 16.0], [4.0, 15.0, 24.0], CowSkin),
            CowSkin,
            CowFace,
            12.0,
        ),
        MobKind::Pig => (
            skin([-5.0, -8.0, 6.0], [5.0, 8.0, 14.0], PigSkin),
            skin([-4.0, 8.0, 7.0], [4.0, 16.0, 15.0], PigSkin),
            PigSkin,
            PigFace,
            6.0,
        ),
        MobKind::Sheep => (
            skin([-6.0, -8.0, 11.0], [6.0, 8.0, 21.0], SheepWool),
            skin([-3.0, 8.0, 16.0], [3.0, 15.0, 23.0], SheepSkin),
            SheepSkin,
            SheepFace,
            12.0,
        ),
    };
    let mut quads = Vec::new();
    box_quads(&body, &mut quads);
    // The face is on the head's front (+y, the way the mob walks).
    box_quads(&ModelBox { back: face, ..head }, &mut quads);
    if kind == MobKind::Cow {
        // Horns.
        for x in [-5.0, 4.0] {
            box_quads(
                &skin([x, 11.0, 24.0], [x + 1.0, 12.0, 26.0], SheepWool),
                &mut quads,
            );
        }
    }
    if kind == MobKind::Pig {
        box_quads(
            &skin([-2.0, 16.0, 8.0], [2.0, 17.0, 11.0], PigFace),
            &mut quads,
        );
    }
    let (x_in, y_in) = match kind {
        MobKind::Pig => (5.0, 7.0),
        _ => (6.0, 8.0),
    };
    for (i, (x, y)) in [
        (-x_in, y_in - 4.0),
        (x_in - 4.0, y_in - 4.0),
        (-x_in, -y_in),
        (x_in - 4.0, -y_in),
    ]
    .into_iter()
    .enumerate()
    {
        let mut leg = Vec::new();
        box_quads(
            &skin([x, y, 0.0], [x + 4.0, y + 4.0, leg_height], legs),
            &mut leg,
        );
        // Diagonal pairs swing together, like Minecraft's quadrupeds.
        let sign = if i == 0 || i == 3 { 1.0 } else { -1.0 };
        let angle = walk.sin() * 0.7 * swing * sign;
        transform(&mut leg, |p| pitch(p, [y + 2.0, leg_height], angle));
        quads.extend(leg);
    }
    let s = 1.0 / 16.0;
    let place = |[x, y, z]: [f32; 3]| {
        right * (x as f64 * s) + forward * (y as f64 * s) + up * (z as f64 * s)
    };
    let tint = if hurt { 0xff_6060ff } else { 0xffff_ffff };
    build(&quads, place, DVec3::ZERO, light, tint, false)
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
    fn drops_and_mobs_are_small() {
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
        for kind in [MobKind::Cow, MobKind::Pig, MobKind::Sheep] {
            let mesh = mob_model(
                kind,
                1.0,
                1.0,
                false,
                (DVec3::X, DVec3::Y),
                Light::default(),
            );
            for v in &mesh.opaque {
                let p = DVec3::new(
                    v.position[0] as f64,
                    v.position[1] as f64,
                    v.position[2] as f64,
                ) * POSITION_UNIT;
                assert!(p.length() < 2.0, "{kind:?} vertex at {p}");
            }
        }
    }
}
