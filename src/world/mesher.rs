//! Turns a padded chunk into quads on the curved planet surface.
//!
//! * Faces are emitted only between a block and a non-opaque neighbour (water only against
//!   air).
//! * **Greedy meshing**: neighbouring faces with the same texture, light and (uniform)
//!   ambient occlusion are merged into one bigger quad; the texture repeats per block.
//!   Merges are capped at [`MAX_TANGENT`] blocks along the curved surface so the flat quad
//!   never deviates from the sphere by more than a fraction of a millimetre, but can span
//!   the whole chunk radially (radial edges are exactly straight).
//! * Each vertex gets Minecraft-style ambient occlusion from its 3 neighbouring blocks, plus
//!   a cheap "sky light" based on how far below the column's surface it is (so caves are
//!   dark without a full light-propagation system).

use glam::DVec3;

use super::{
    block::{Block, Tint},
    chunk::{ChunkKey, PaddedBlocks},
    planet::{CHUNK_SIZE, Planet},
    terrain::{ColumnData, WHITE},
};
use crate::render::{BlockVertex, ChunkMesh};

/// Max merged quad size along the planet surface. A flat quad of length L sags
/// L^2 / (8 R) below the sphere: 8 m on a 40 km planet = 0.2 mm.
const MAX_TANGENT: i32 = 8;
/// Max merged quad size radially (u, v, k: axis 2 is radial).
const MAX_RADIAL: i32 = CHUNK_SIZE;
/// Vertex positions snap to a global lattice of this many meters (2 mm).
pub const POSITION_UNIT: f64 = 1.0 / 512.0;
/// How far (lattice units) opaque quads are grown to hide T-junction cracks.
const QUAD_GROW: f64 = 1.25;

struct Face {
    /// Normal axis (0 = u, 1 = v, 2 = k) and direction.
    axis: usize,
    dir: [i32; 3],
    /// The two in-plane axes, ascending. These are also the texture u/v axes.
    tangents: [usize; 2],
    /// Unit-cube corners in (u, v, k), counter-clockwise seen from outside.
    corners: [[i32; 3]; 4],
    /// Texture corner bits (u | v << 1) per corner; texture "up" points to +k on side faces.
    uv: [u32; 4],
    /// Index into `Block::textures()`: 0 top, 1 side, 2 bottom.
    texture_slot: usize,
}

const FACES: [Face; 6] = [
    Face {
        axis: 0,
        dir: [1, 0, 0],
        tangents: [1, 2],
        corners: [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]],
        uv: [2, 3, 1, 0],
        texture_slot: 1,
    },
    Face {
        axis: 0,
        dir: [-1, 0, 0],
        tangents: [1, 2],
        corners: [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]],
        uv: [2, 0, 1, 3],
        texture_slot: 1,
    },
    Face {
        axis: 1,
        dir: [0, 1, 0],
        tangents: [0, 2],
        corners: [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]],
        uv: [2, 0, 1, 3],
        texture_slot: 1,
    },
    Face {
        axis: 1,
        dir: [0, -1, 0],
        tangents: [0, 2],
        corners: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
        uv: [2, 3, 1, 0],
        texture_slot: 1,
    },
    Face {
        axis: 2,
        dir: [0, 0, 1],
        tangents: [0, 1],
        corners: [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
        uv: [0, 1, 3, 2],
        texture_slot: 0,
    },
    Face {
        axis: 2,
        dir: [0, 0, -1],
        tangents: [0, 1],
        corners: [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]],
        uv: [0, 2, 3, 1],
        texture_slot: 2,
    },
];

/// Precomputed corner geometry for one chunk: the curved grid of block corners.
pub struct ChunkGeometry {
    dirs: Vec<DVec3>,
    radii: Vec<f64>,
    /// Mesh origin in lattice units (planet-relative).
    origin_units: [i64; 3],
    /// Mesh origin in meters, exactly on the lattice.
    pub origin: DVec3,
    pub radius: f32,
}

const CORNERS: i32 = CHUNK_SIZE + 1;

impl ChunkGeometry {
    pub fn new(planet: &Planet, key: ChunkKey) -> Self {
        let (u0, v0, k0) = (
            key.cu * CHUNK_SIZE,
            key.cv * CHUNK_SIZE,
            key.ck * CHUNK_SIZE,
        );
        let n = planet.res_at(k0);
        let mut dirs = Vec::with_capacity((CORNERS * CORNERS) as usize);
        for j in 0..CORNERS {
            for i in 0..CORNERS {
                dirs.push(planet.corner_dir(key.face, u0 + i, v0 + j, n));
            }
        }
        let radii: Vec<f64> = (0..CORNERS)
            .map(|l| planet.layer_radius((k0 + l) as f64))
            .collect();
        let centre = planet.chunk_center(key) / POSITION_UNIT;
        let origin_units = [
            centre.x.round() as i64,
            centre.y.round() as i64,
            centre.z.round() as i64,
        ];
        let origin = DVec3::new(
            origin_units[0] as f64,
            origin_units[1] as f64,
            origin_units[2] as f64,
        ) * POSITION_UNIT;

        let mut geo = Self {
            dirs,
            radii,
            origin_units,
            origin,
            radius: 0.0,
        };
        let m = CHUNK_SIZE;
        geo.radius = [(0, 0), (m, 0), (0, m), (m, m)]
            .iter()
            .flat_map(|&(i, j)| [(i, j, 0), (i, j, m)])
            .map(|(i, j, l)| {
                let c = geo.corner([i, j, l]);
                ((c[0] as f32).powi(2) + (c[1] as f32).powi(2) + (c[2] as f32).powi(2)).sqrt()
                    * POSITION_UNIT as f32
            })
            .fold(0.0, f32::max);
        geo
    }

    /// Corner (u, v, k) of the chunk's block grid in lattice units relative to the origin.
    /// Snapping happens on the *planet-relative* position, so every chunk sharing this corner
    /// gets exactly the same lattice point.
    #[inline]
    pub fn corner(&self, p: [i32; 3]) -> [i16; 3] {
        self.snap(self.corner_units(p))
    }

    /// Exact (unsnapped) corner position in lattice units, planet-relative.
    #[inline]
    fn corner_units(&self, [i, j, l]: [i32; 3]) -> DVec3 {
        self.dirs[(i + j * CORNERS) as usize] * (self.radii[l as usize] / POSITION_UNIT)
    }

    /// Snaps a planet-relative position (lattice units) to the lattice, relative to the origin.
    #[inline]
    fn snap(&self, p: DVec3) -> [i16; 3] {
        let o = self.origin_units;
        [
            (p.x.round() as i64 - o[0]) as i16,
            (p.y.round() as i64 - o[1]) as i16,
            (p.z.round() as i64 - o[2]) as i16,
        ]
    }
}

/// One potential face in the greedy-meshing mask.
#[derive(Clone, Copy, Default)]
struct Cell {
    /// 0 = no face. Otherwise texture, light, AO and water flag; faces merge only when their
    /// keys are equal. Faces with non-uniform AO get a key with `NO_MERGE` set.
    key: u32,
    ao: [u32; 4],
    /// Block light (0-15) at each corner.
    block_light: [u32; 4],
    /// RGB565 tint.
    tint: u16,
}

const VALID: u32 = 1 << 31;
const NO_MERGE: u32 = 1 << 30;
const WATER: u32 = 1 << 29;
const CUTOUT: u32 = 1 << 28;

/// What the mesher knows about the columns of a chunk.
#[derive(Clone, Copy)]
pub enum Columns<'a> {
    /// Near the surface: per-column surface height and biome tints.
    Surface(&'a ColumnData),
    /// Deep interior: no surface data; "surface" is taken to be the given sea layer.
    Deep(i32),
}

impl Columns<'_> {
    fn surface_height(self, u: i32, v: i32) -> i32 {
        match self {
            Columns::Surface(c) => c.get(u, v).height,
            Columns::Deep(sea_layer) => sea_layer,
        }
    }
}

/// Returns (mesh, origin relative to planet centre, bounding radius).
pub fn mesh_chunk(
    planet: &Planet,
    key: ChunkKey,
    blocks: &PaddedBlocks,
    cols: Columns,
    block_light: &dyn Fn([i32; 3]) -> u8,
) -> (ChunkMesh, DVec3, f32) {
    let geo = ChunkGeometry::new(planet, key);
    let k0 = key.ck * CHUNK_SIZE;
    let mut mesh = ChunkMesh::new(POSITION_UNIT as f32);
    let opaque = |p: [i32; 3]| blocks.get(p[0], p[1], p[2]).is_opaque() as u32;
    let n = CHUNK_SIZE;
    let mut mask = vec![Cell::default(); (n * n) as usize];

    for face in &FACES {
        let [ta, tb] = face.tangents;
        let limit = |axis: usize| if axis == 2 { MAX_RADIAL } else { MAX_TANGENT };
        let (max_w, max_h) = (limit(ta), limit(tb));

        for slice in 0..n {
            // 1. Which faces exist in this slice, and what do they look like?
            for j in 0..n {
                for i in 0..n {
                    let mut pos = [0; 3];
                    pos[face.axis] = slice;
                    pos[ta] = i;
                    pos[tb] = j;
                    mask[(i + j * n) as usize] =
                        face_cell(face, pos, blocks, cols, k0, &opaque, block_light);
                }
            }

            // 2. Greedily merge equal faces into rectangles.
            for j in 0..n {
                let mut i = 0;
                while i < n {
                    let cell = mask[(i + j * n) as usize];
                    if cell.key == 0 {
                        i += 1;
                        continue;
                    }
                    let (mut w, mut h) = (1, 1);
                    if cell.key & NO_MERGE == 0 {
                        while i + w < n
                            && w < max_w
                            && mask[(i + w + j * n) as usize].key == cell.key
                        {
                            w += 1;
                        }
                        'grow: while j + h < n && h < max_h {
                            for x in i..i + w {
                                if mask[(x + (j + h) * n) as usize].key != cell.key {
                                    break 'grow;
                                }
                            }
                            h += 1;
                        }
                    }
                    for y in j..j + h {
                        for x in i..i + w {
                            mask[(x + y * n) as usize].key = 0;
                        }
                    }

                    let mut base = [0; 3];
                    base[face.axis] = slice;
                    base[ta] = i;
                    base[tb] = j;
                    emit_quad(&mut mesh, &geo, face, base, [w, h], cell);
                    i += w;
                }
            }
        }
    }
    (mesh, geo.origin, geo.radius)
}

fn face_cell(
    face: &Face,
    pos: [i32; 3],
    blocks: &PaddedBlocks,
    cols: Columns,
    k0: i32,
    opaque: &impl Fn([i32; 3]) -> u32,
    block_light: &dyn Fn([i32; 3]) -> u8,
) -> Cell {
    let block = blocks.get(pos[0], pos[1], pos[2]);
    // Model blocks (crafting table, furnace) are drawn as separate models, not cubes.
    if block == Block::Air || block.is_model() {
        return Cell::default();
    }
    let out = [
        pos[0] + face.dir[0],
        pos[1] + face.dir[1],
        pos[2] + face.dir[2],
    ];
    let neighbour = blocks.get(out[0], out[1], out[2]);
    let water = block == Block::Water;
    let visible = if water {
        !neighbour.is_opaque() && neighbour != Block::Water
    } else if block.is_cutout() {
        // Glass doesn't show its faces against more glass.
        !neighbour.is_opaque() && neighbour != block
    } else {
        !neighbour.is_opaque()
    };
    if !visible {
        return Cell::default();
    }

    let tex = block.textures()[face.texture_slot] as u32;
    let tint = match (block.tint(), cols) {
        (Tint::None, _) | (_, Columns::Deep(_)) => WHITE,
        (Tint::Grass, Columns::Surface(c)) => c.get(pos[0], pos[1]).grass_tint,
        (Tint::Foliage, Columns::Surface(c)) => c.get(pos[0], pos[1]).foliage_tint,
        (Tint::Spruce, _) => 0xff_3c5a3c,
        (Tint::Water, Columns::Surface(c)) => c.get(pos[0], pos[1]).water_tint,
    };

    // Fake sky light: darker the deeper the open cell is below the surface.
    let depth = cols.surface_height(out[0], out[1]) - (k0 + out[2]);
    let light = if depth < 0 {
        15
    } else {
        (13 - depth).clamp(1, 13) as u32
    };

    // Ambient occlusion from the 3 blocks around each corner, in the layer of cells just
    // outside this face.
    let ao = if water {
        [3; 4]
    } else {
        face.corners.map(|c| {
            let [ta, tb] = face.tangents;
            let step = |axis: usize| if c[axis] == 1 { 1 } else { -1 };
            let mut side1 = out;
            side1[ta] += step(ta);
            let mut side2 = out;
            side2[tb] += step(tb);
            let mut corner = side1;
            corner[tb] += step(tb);
            let (s1, s2) = (opaque(side1), opaque(side2));
            if s1 == 1 && s2 == 1 {
                0
            } else {
                3 - (s1 + s2 + opaque(corner))
            }
        })
    };

    // Block light (torches): smooth, like Minecraft - each corner averages the open cells
    // around it in the layer just outside the face. Glowing blocks light their own faces.
    let block_light = if block.glow() > 0 {
        [block.glow(); 4]
    } else {
        face.corners.map(|c| {
            let [ta, tb] = face.tangents;
            let step = |axis: usize| if c[axis] == 1 { 1 } else { -1 };
            let mut side1 = out;
            side1[ta] += step(ta);
            let mut side2 = out;
            side2[tb] += step(tb);
            let mut corner = side1;
            corner[tb] += step(tb);
            let (mut sum, mut count) = (block_light(out) as u32, 1);
            for p in [side1, side2, corner] {
                if opaque(p) == 0 {
                    sum += block_light(p) as u32;
                    count += 1;
                }
            }
            sum / count
        })
    };

    let uniform =
        ao.iter().all(|&a| a == ao[0]) && block_light.iter().all(|&l| l == block_light[0]);
    let key = VALID
        | tex
        | (light << 8)
        | (ao[0] << 12)
        | (block_light[0] << 16)
        | if water { WATER } else { 0 }
        | if block.is_cutout() { CUTOUT } else { 0 }
        | if uniform { 0 } else { NO_MERGE };
    Cell {
        key,
        ao,
        block_light,
        tint: BlockVertex::tint565(tint),
    }
}

fn emit_quad(
    mesh: &mut ChunkMesh,
    geo: &ChunkGeometry,
    face: &Face,
    base: [i32; 3],
    size: [i32; 2],
    cell: Cell,
) {
    let tex = cell.key & 0xff;
    let light = (cell.key >> 8) & 0xf;
    let ao = cell.ao;
    let [ta, tb] = face.tangents;

    // Pick the quad diagonal that interpolates AO without artifacts.
    let order = if ao[0] + ao[2] < ao[1] + ao[3] {
        [1, 2, 3, 0]
    } else {
        [0, 1, 2, 3]
    };
    let (target, cutout) = if cell.key & WATER != 0 {
        (&mut mesh.water, 0)
    } else if cell.key & CUTOUT != 0 {
        (&mut mesh.cutout, crate::render::models::CUTOUT_FLAG)
    } else {
        (&mut mesh.opaque, 0)
    };
    let tex_size = [size[0] as u32, size[1] as u32];

    let corners: [DVec3; 4] = std::array::from_fn(|ci| {
        let c = face.corners[ci];
        let mut p = base;
        p[face.axis] += c[face.axis];
        p[ta] += c[ta] * size[0];
        p[tb] += c[tb] * size[1];
        geo.corner_units(p)
    });

    for ci in order {
        let mut p = corners[ci];
        if cell.key & WATER == 0 {
            // Grow opaque quads by ~1 lattice unit in their own plane. Merged quads create
            // T-junctions whose vertices snap to the lattice independently, which would leave
            // sub-pixel cracks ("sparkles"); the tiny overlap hides them.
            let prev = corners[(ci + 3) % 4];
            let next = corners[(ci + 1) % 4];
            p += ((p - prev).normalize() + (p - next).normalize()) * QUAD_GROW;
        }
        let data = BlockVertex::pack(tex, face.uv[ci], ao[ci], light, tex_size)
            | cutout
            | (cell.block_light[ci] << 28);
        target.push(BlockVertex::new(geo.snap(p), cell.tint, data));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{DVec3, Vec3};

    use super::*;
    use crate::world::{planet::Face, terrain::TerrainGenerator};

    /// Deep in the planet (here: the chunks on both sides of the first shell boundary,
    /// ~20 km down) there must still be caves, and their meshes must be well-formed.
    #[test]
    fn deep_caves_exist_and_mesh_cleanly() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let terrain = TerrainGenerator::new(planet.clone());
        let boundary_ck = planet.shells[0].bottom / CHUNK_SIZE;

        let (mut meshed, mut quads) = (0, 0);
        for ck in [boundary_ck - 1, boundary_ck] {
            let per_face = planet.res_at(ck * CHUNK_SIZE) / CHUNK_SIZE;
            for i in 0..48 {
                let key = ChunkKey {
                    face: Face::ALL[i % 6],
                    cu: (i as i32 * 37) % per_face,
                    cv: (i as i32 * 91) % per_face,
                    ck,
                };
                let Some(mesh) = terrain
                    .generate_chunk(key, &Default::default(), &Default::default())
                    .mesh
                else {
                    continue;
                };
                meshed += 1;
                for quad in mesh.opaque.chunks(4) {
                    quads += 1;
                    let p: Vec<Vec3> = quad
                        .iter()
                        .map(|v| {
                            Vec3::new(
                                v.position[0] as f32,
                                v.position[1] as f32,
                                v.position[2] as f32,
                            )
                        })
                        .collect();
                    let n1 = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
                    let n2 = (p[2] - p[0]).cross(p[3] - p[0]).normalize();
                    assert!(n1.dot(n2) > 0.99, "twisted deep quad in {key:?}");
                }
            }
        }
        assert!(meshed > 10, "only {meshed} of 96 deep chunks have caves");
        assert!(quads > 1000);
    }

    /// Every quad must be planar-ish and its two triangles must face the same way;
    /// a misplaced corner shows up in-game as a "spike" where one triangle is culled.
    #[test]
    fn quads_are_not_twisted() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let terrain = TerrainGenerator::new(planet.clone());
        let (lat, lon) = ((-41.3f64).to_radians(), 36f64.to_radians());
        let dir = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
        let surface = planet.sea_radius + terrain.sample_column(dir).height;
        let centre = planet.chunk_at(dir * surface);

        let mut bad = 0;
        let mut total = 0;
        for dk in -1..=1 {
            for du in -2..=2 {
                for dv in -2..=2 {
                    let key = ChunkKey {
                        cu: centre.cu + du,
                        cv: centre.cv + dv,
                        ck: centre.ck + dk,
                        ..centre
                    };
                    let Some(mesh) = terrain
                        .generate_chunk(key, &Default::default(), &Default::default())
                        .mesh
                    else {
                        continue;
                    };
                    for quad in mesh.opaque.chunks(4).chain(mesh.water.chunks(4)) {
                        total += 1;
                        let p: Vec<Vec3> = quad
                            .iter()
                            .map(|v| {
                                Vec3::new(
                                    v.position[0] as f32,
                                    v.position[1] as f32,
                                    v.position[2] as f32,
                                )
                            })
                            .collect();
                        let n1 = (p[1] - p[0]).cross(p[2] - p[0]);
                        let n2 = (p[2] - p[0]).cross(p[3] - p[0]);
                        if n1.normalize().dot(n2.normalize()) < 0.99 {
                            if bad < 5 {
                                eprintln!(
                                    "twisted quad in {key:?}: {p:?} data {:#x}",
                                    quad[0].data
                                );
                            }
                            bad += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(bad, 0, "{bad} of {total} quads are twisted");
    }
}

#[cfg(test)]
mod layer_tests {
    use std::sync::Arc;

    use glam::DVec3;

    use super::*;
    use crate::world::terrain::TerrainGenerator;

    /// Corners of every quad must sit on block-layer radii: all 4 on one layer (top/bottom
    /// faces) or 2 + 2 on two layers (side faces).
    #[test]
    fn quad_corners_sit_on_layers() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let terrain = TerrainGenerator::new(planet.clone());
        let (lat, lon) = ((-41.3f64).to_radians(), 36f64.to_radians());
        let dir = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
        let surface = planet.sea_radius + terrain.sample_column(dir).height;
        let centre = planet.chunk_at(dir * surface);

        let mut bad = 0;
        for du in -2..=2 {
            for dv in -2..=2 {
                for dk in -1..=1 {
                    let key = ChunkKey {
                        cu: centre.cu + du,
                        cv: centre.cv + dv,
                        ck: centre.ck + dk,
                        ..centre
                    };
                    let result =
                        terrain.generate_chunk(key, &Default::default(), &Default::default());
                    let Some(mesh) = result.mesh else { continue };
                    for quad in mesh.opaque.chunks(4) {
                        let layers: Vec<f64> = quad
                            .iter()
                            .map(|v| {
                                let p = result.origin
                                    + DVec3::new(
                                        v.position[0] as f64,
                                        v.position[1] as f64,
                                        v.position[2] as f64,
                                    ) * POSITION_UNIT;
                                p.length() - planet.sea_radius + planet.sea_layer as f64
                            })
                            .collect();
                        let rounded: Vec<i64> = layers.iter().map(|l| l.round() as i64).collect();
                        let off = layers.iter().any(|l| (l - l.round()).abs() > 0.01);
                        let mut distinct = rounded.clone();
                        distinct.sort();
                        distinct.dedup();
                        let ok = !off
                            && match distinct.len() {
                                1 => true,
                                2 => rounded.iter().filter(|&&r| r == distinct[0]).count() == 2,
                                _ => false,
                            };
                        if !ok {
                            if bad < 6 {
                                eprintln!(
                                    "bad quad {key:?} layers {layers:?} data {:#x}",
                                    quad[0].data
                                );
                            }
                            bad += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(bad, 0);
    }
}
