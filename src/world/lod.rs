//! Level-of-detail terrain: see all the way to the horizon.
//!
//! Every cube face is covered by a quadtree of **LOD tiles**. A tile at level `L` is a grid of
//! `LOD_CELLS x LOD_CELLS` cells, each `2^L x 2^L` surface columns wide (so 2 m cells at
//! level 1, 4 m at level 2, ... up to the whole face as one tile at the root). The quadtree is
//! refined around the camera: a tile splits into 4 children while the camera is closer than
//! `LOD_SPLIT_DISTANCE` tile-widths, so detail falls off smoothly with distance. Tiles behind
//! the planet's horizon are skipped entirely.
//!
//! Tiles are generated from the same terrain function as real chunks (one column sample per
//! cell), and are meshed in the same voxel style: every cell is a block column with a top face
//! and walls down to its neighbours, textured with its surface material (grass, snow, sand,
//! rock, forest canopy...) - so distant mountains keep their blocky silhouettes. Water is a
//! separate translucent layer. Tile edges get "skirts" hanging down, which hide the cracks
//! between tiles of different levels.
//!
//! The finest levels (`TREE_LEVELS`) draw the *actual* trees - same positions, heights and
//! (simplified) crowns as the real chunks will have - so trees don't pop in when chunks load.
//! Coarser levels show forests as canopy-covered cells instead.
//!
//! Near the camera the real voxel chunks take over: the LOD shader discards fragments inside
//! the fully-loaded chunk radius.

use glam::DVec3;

use super::{
    noise::{hash3, unit},
    planet::{CHUNK_SIZE, Face, Planet},
    terrain::{
        Biome, ColumnSample, TerrainGenerator, TreeInfo, TreeKind, WHITE, grass_color, pack_rgb,
        water_color,
    },
    textures::Tex,
};
use crate::render::{BlockVertex, ChunkMesh};

/// Cells per tile edge.
pub const LOD_CELLS: i32 = 32;
/// A tile is split when the camera is closer than this many tile widths.
pub const LOD_SPLIT_DISTANCE: f64 = 5.0;
/// Edge skirts hang this many cells below the tile edge.
const SKIRT_CELLS: i32 = 2;
/// Levels up to this one draw individual trees (level 1 with detailed crowns).
const TREE_LEVELS: u8 = 2;
/// LOD terrain sits this far below the real terrain, so the real chunks win the depth test
/// where both are drawn (just outside the fully loaded area).
const SINK: f64 = 0.25;
/// LOD trees are shrunk by this much (m) on every side, so they hide inside the real trees.
const TREE_SHRINK: f64 = 0.08;

/// A tile in the LOD quadtree: cube face, level, and tile coordinates within the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LodKey {
    pub face: Face,
    pub level: u8,
    pub tu: i32,
    pub tv: i32,
}

impl LodKey {
    /// Columns per cell.
    pub fn step(self) -> i32 {
        1 << self.level
    }

    /// Columns per tile edge.
    pub fn columns(self) -> i32 {
        LOD_CELLS * self.step()
    }

    pub fn parent(self) -> LodKey {
        LodKey {
            level: self.level + 1,
            tu: self.tu / 2,
            tv: self.tv / 2,
            ..self
        }
    }

    pub fn children(self) -> [LodKey; 4] {
        let level = self.level - 1;
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(a, b)| LodKey {
            level,
            tu: self.tu * 2 + a,
            tv: self.tv * 2 + b,
            ..self
        })
    }
}

/// Root level: one tile covers a whole cube face.
pub fn root_level(planet: &Planet) -> u8 {
    (planet.face_blocks / LOD_CELLS).trailing_zeros() as u8
}

/// Unit direction through the centre of a tile, and its angular radius (half diagonal).
pub fn tile_bounds(planet: &Planet, key: LodKey) -> (DVec3, f64) {
    let n = planet.face_blocks;
    let half = key.columns() as f64 / 2.0;
    let centre = planet.dir(
        key.face,
        key.tu as f64 * key.columns() as f64 + half,
        key.tv as f64 * key.columns() as f64 + half,
        n,
    );
    // Surface columns are at most 1.08 m wide (see `planet::BLOCK_WIDTH`); 1.3 leaves a margin.
    let angular_radius =
        key.columns() as f64 * std::f64::consts::FRAC_1_SQRT_2 * 1.3 / planet.sea_radius;
    (centre, angular_radius)
}

pub struct LodResult {
    pub key: LodKey,
    pub mesh: Option<ChunkMesh>,
    /// Mesh origin relative to the planet centre.
    pub origin: DVec3,
    pub radius: f32,
}

/// What one LOD cell looks like.
#[derive(Clone, Copy)]
struct Cell {
    /// Layer of the topmost solid block (terrain, ice or tree canopy).
    top: i32,
    /// Liquid water above the ground (open ocean).
    water: bool,
    top_tex: Tex,
    side_tex: Tex,
    tint: u16,
    water_tint: u16,
}

/// Generates and meshes one LOD tile.
pub fn build_tile(terrain: &TerrainGenerator, key: LodKey) -> LodResult {
    let planet: &Planet = &terrain.planet;
    let n = planet.face_blocks;
    let sea = planet.sea_layer;
    let step = key.step();
    let (u0, v0) = (key.tu * key.columns(), key.tv * key.columns());
    let g = (LOD_CELLS + 2) as usize;
    let idx = |i: i32, j: i32| ((i + 1) + (j + 1) * (LOD_CELLS + 2)) as usize;

    // 1. Sample one column per cell (plus a border ring for walls and skirts).
    let mut samples: Vec<(ColumnSample, i32)> = Vec::with_capacity(g * g);
    for j in -1..=LOD_CELLS {
        for i in -1..=LOD_CELLS {
            let (gu, gv) = (u0 + i * step + step / 2, v0 + j * step + step / 2);
            let (f, wu, wv) = planet.wrap_column(key.face, gu, gv, n);
            let s = terrain.sample_column(planet.column_dir(f, wu, wv));
            samples.push((s, sea + s.height.floor() as i32));
        }
    }

    // 2. Decide what each cell looks like.
    let mut cells: Vec<Cell> = Vec::with_capacity(g * g);
    for j in -1..=LOD_CELLS {
        for i in -1..=LOD_CELLS {
            let (s, ground) = samples[idx(i, j)];
            let slope = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .map(|&(di, dj)| {
                    let (ni, nj) = ((i + di).clamp(-1, LOD_CELLS), (j + dj).clamp(-1, LOD_CELLS));
                    (samples[idx(ni, nj)].1 - ground).abs()
                })
                .max()
                .unwrap_or(0) as f64
                / step as f64;
            let seed = hash3(
                planet.seed ^ 0x10d,
                key.face.0 as i64 * 1_000_003 + ((u0 + i * step) >> key.level) as i64,
                ((v0 + j * step) >> key.level) as i64,
                key.level as i64,
            );
            let canopy = key.level > TREE_LEVELS;
            cells.push(describe_cell(&s, ground, sea, slope > 1.2, seed, canopy));
        }
    }

    // 3. Mesh. The origin sits at the tile's mid height, so the i16 vertex range (+-128
    // cells) covers tall mountains too.
    let unit = (1u64 << (key.level + 1)) as f64 / 512.0;
    let (centre, _) = tile_bounds(planet, key);
    let (lo, hi) = cells.iter().fold((i32::MAX, i32::MIN), |(lo, hi), c| {
        (lo.min(c.top), hi.max(c.top))
    });
    let mid = planet.layer_radius(((lo as f64 + hi as f64) / 2.0).max(sea as f64));
    let origin_units = (centre * mid / unit).round();
    let mut builder = TileMesher {
        mesh: ChunkMesh::new(unit as f32),
        dirs: (0..=LOD_CELLS)
            .flat_map(|j| (0..=LOD_CELLS).map(move |i| (i, j)))
            .map(|(i, j)| planet.corner_dir(key.face, u0 + i * step, v0 + j * step, n))
            .collect(),
        planet,
        face: key.face,
        unit,
        origin_units,
        sink: SINK,
        step,
        max_merge: max_merge(planet, step),
        radius: 0.0,
    };
    let cell = |i: i32, j: i32| cells[idx(i, j)];

    // Tops (greedy-merged rectangles of equal height and material) and water.
    builder.tops(
        &|i, j| {
            let c = cell(i, j);
            Some((c.top, c.top_tex, c.tint))
        },
        false,
    );
    // Water is never drawn together with the real chunks' water (they hand over at one
    // radius), so it sits exactly at sea level.
    builder.sink = 0.0;
    builder.tops(
        &|i, j| {
            let c = cell(i, j);
            c.water.then_some((sea, Tex::Water, c.water_tint))
        },
        true,
    );
    builder.sink = SINK;

    // Walls between cells, and skirts along the tile edges.
    for j in 0..LOD_CELLS {
        for i in 0..LOD_CELLS {
            let c = cell(i, j);
            for (dir, (di, dj)) in [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().enumerate() {
                let (ni, nj) = (i + di, j + dj);
                let neighbour = cell(ni, nj).top;
                let edge = !(0..LOD_CELLS).contains(&ni) || !(0..LOD_CELLS).contains(&nj);
                let bottom = if edge {
                    neighbour.min(c.top) - SKIRT_CELLS * step
                } else if neighbour < c.top {
                    neighbour
                } else {
                    continue;
                };
                builder.wall(i, j, dir, bottom, c.top, c.side_tex, c.tint);
            }
        }
    }

    // The real trees.
    if key.level <= TREE_LEVELS {
        for gv in v0..v0 + key.columns() {
            for gu in u0..u0 + key.columns() {
                if let Some(tree) = terrain.tree_at(key.face, gu, gv) {
                    builder.tree(gu, gv, &tree, key.level == 1);
                }
            }
        }
    }

    let radius = builder.radius;
    let origin = origin_units * unit;
    let mesh = builder.mesh;
    LodResult {
        key,
        mesh: (!mesh.is_empty()).then_some(mesh),
        origin,
        radius,
    }
}

/// Merged faces are flat, but the planet is round: a flat quad of width L sags L^2 / (8 R)
/// below the sphere in its middle. Cap merges so the sag stays under ~5% of a cell (and at
/// least 1 m) - otherwise e.g. merged ocean surfaces of coarse tiles dip under the sea floor.
fn max_merge(planet: &Planet, step: i32) -> i32 {
    let cell = step as f64;
    let tolerance = (0.05 * cell).max(1.0);
    let max_width = (8.0 * planet.sea_radius * tolerance).sqrt();
    ((max_width / cell) as i32).clamp(1, LOD_CELLS)
}

fn describe_cell(
    s: &ColumnSample,
    ground: i32,
    sea: i32,
    steep: bool,
    seed: u64,
    canopy: bool,
) -> Cell {
    let grass = BlockVertex::tint565(pack_rgb(grass_color(s.temperature, s.humidity)));
    let foliage = BlockVertex::tint565(pack_rgb(
        grass_color(s.temperature, s.humidity).map(|c| c * 0.82),
    ));
    let white = BlockVertex::tint565(WHITE);
    let lunar = matches!(s.biome, Biome::Highlands | Biome::Mare);
    let ocean = ground < sea && !lunar;
    let water_tint = BlockVertex::tint565(pack_rgb(water_color(s.temperature)));

    // Forest canopy (coarse levels only): a share of the cells is drawn as tree tops, so
    // forests read as forests from a distance. The share is the ground area the real crowns
    // cover, 1 - exp(-density * crown footprint), so it looks the same as the individual
    // trees of the finer levels (see `tree_kind` in terrain.rs for the densities).
    let spruce = BlockVertex::tint565(0xff_3c5a3c);
    let canopy = if ocean || steep || !canopy {
        None
    } else {
        match s.biome {
            Biome::Forest => Some((0.65, 6, Tex::OakLeaves, foliage)),
            Biome::Jungle => Some((0.84, 12, Tex::JungleLeaves, foliage)),
            Biome::Taiga => Some((0.52, 8, Tex::SpruceLeaves, spruce)),
            Biome::Tundra => Some((0.04, 8, Tex::SpruceLeaves, spruce)),
            Biome::Mountains if s.temperature >= 0.1 => Some((0.19, 8, Tex::SpruceLeaves, spruce)),
            Biome::Savanna => Some((0.2, 5, Tex::OakLeaves, foliage)),
            Biome::Plains => Some((0.07, 6, Tex::OakLeaves, foliage)),
            _ => None,
        }
    };
    if let Some((coverage, height, tex, tint)) = canopy
        && unit(seed) < coverage
    {
        return Cell {
            top: ground + height,
            water: false,
            top_tex: tex,
            side_tex: tex,
            tint,
            water_tint,
        };
    }

    let (top_tex, side_tex, tint) = match s.biome {
        Biome::Caldera => (Tex::Lava, Tex::Basalt, white),
        Biome::Volcanic if steep => (Tex::Basalt, Tex::Basalt, white),
        Biome::Volcanic => (Tex::Ash, Tex::Basalt, white),
        Biome::Highlands if steep => (Tex::MoonRock, Tex::MoonRock, white),
        Biome::Mare if steep => (Tex::Basalt, Tex::Basalt, white),
        Biome::Highlands => (Tex::Regolith, Tex::MoonRock, white),
        Biome::Mare => (Tex::Regolith, Tex::Basalt, white),
        Biome::FrozenOcean => (Tex::Ice, Tex::Ice, white),
        Biome::Ocean if s.temperature > 0.25 => (Tex::Sand, Tex::Sandstone, white),
        Biome::Ocean | Biome::DeepOcean => (Tex::Gravel, Tex::Stone, white),
        Biome::Beach if s.temperature < 0.15 => (Tex::Gravel, Tex::Stone, white),
        Biome::Beach | Biome::Desert => (Tex::Sand, Tex::Sandstone, white),
        Biome::Badlands if steep => (Tex::TerracottaOrange, Tex::TerracottaRed, white),
        Biome::Badlands => (Tex::RedSand, Tex::TerracottaOrange, white),
        Biome::IceSheet => (Tex::Snow, Tex::Ice, white),
        Biome::Mountains if s.temperature < 0.1 => (Tex::Snow, Tex::Stone, white),
        _ if steep => (Tex::Stone, Tex::Stone, white),
        Biome::Tundra | Biome::Taiga if s.temperature < 0.28 => (Tex::Snow, Tex::Dirt, white),
        _ => (Tex::GrassTop, Tex::Dirt, grass),
    };

    // Frozen seas are a solid ice sheet at sea level.
    if s.biome == Biome::FrozenOcean {
        return Cell {
            top: sea,
            water: false,
            top_tex,
            side_tex,
            tint,
            water_tint,
        };
    }
    Cell {
        top: ground,
        water: ocean,
        top_tex,
        side_tex,
        tint,
        water_tint,
    }
}

struct TileMesher<'a> {
    mesh: ChunkMesh,
    /// Corner directions, (LOD_CELLS + 1)^2.
    dirs: Vec<DVec3>,
    planet: &'a Planet,
    face: Face,
    unit: f64,
    origin_units: DVec3,
    /// LOD geometry sits slightly below the real terrain so it never z-fights the real
    /// chunks where both overlap.
    sink: f64,
    step: i32,
    /// Largest merged top face, in cells (see [`max_merge`]).
    max_merge: i32,
    radius: f32,
}

/// One face of a [`TreeBox`]: corners, uv corners, texture size (blocks) and texture.
type BoxFace = ([DVec3; 4], [u32; 4], [u32; 2], Tex);

/// An axis-aligned (in grid terms) box of a simplified tree.
struct TreeBox {
    /// Extent in surface-grid columns (fractional).
    u: [f64; 2],
    v: [f64; 2],
    /// Bottom and top radius (m from the planet centre).
    r: [f64; 2],
    /// Block counts (u, v, height), for texture repetition.
    blocks: [u32; 3],
    side: Tex,
    top: Option<Tex>,
    bottom: bool,
    tint: u16,
}

impl TileMesher<'_> {
    /// A planet-relative position in this tile's lattice units.
    fn lattice(&mut self, p: DVec3) -> [i16; 3] {
        let p = (p / self.unit).round() - self.origin_units;
        self.radius = self.radius.max((p.length() * self.unit) as f32);
        debug_assert!(p.abs().max_element() < 32767.0, "LOD vertex out of range");
        [p.x as i16, p.y as i16, p.z as i16]
    }

    /// Position of grid corner (i, j) at the top of layer `k`, in lattice units.
    fn corner(&mut self, i: i32, j: i32, k: i32) -> [i16; 3] {
        let dir = self.dirs[(i + j * (LOD_CELLS + 1)) as usize];
        let r = self.planet.layer_radius(k as f64 + 1.0) - self.sink;
        self.lattice(dir * r)
    }

    /// One tree standing on column (gu, gv): the trunk plus a crown of a few boxes that
    /// approximates the real block crown. `detailed` crowns follow the real shape more
    /// closely; otherwise the crown is a single box.
    fn tree(&mut self, gu: i32, gv: i32, tree: &TreeInfo, detailed: bool) {
        let top = tree.base + tree.height;
        let (leaves, log, tint) = match tree.kind {
            TreeKind::Spruce => (
                Tex::SpruceLeaves,
                Tex::SpruceLogSide,
                BlockVertex::tint565(0xff_3c5a3c),
            ),
            TreeKind::Jungle => (
                Tex::JungleLeaves,
                Tex::OakLogSide,
                BlockVertex::tint565(tree.foliage_tint),
            ),
            _ => (
                Tex::OakLeaves,
                Tex::OakLogSide,
                BlockVertex::tint565(tree.foliage_tint),
            ),
        };
        let white = BlockVertex::tint565(WHITE);

        // Crown boxes: (half-width u, half-width v beyond the trunk column, lowest and
        // highest layer relative to `top`).
        let crown: &[(f64, f64, i32, i32)] = match (tree.kind, detailed) {
            (TreeKind::Cactus, _) => &[],
            (TreeKind::Oak, true) => &[(2.0, 2.0, -2, -1), (1.0, 1.0, 0, 1)],
            (TreeKind::Oak, false) => &[(2.0, 2.0, -2, 1)],
            (TreeKind::Acacia, true) => &[(2.5, 2.5, 0, 0), (1.0, 1.0, 1, 1)],
            (TreeKind::Acacia, false) => &[(2.5, 2.5, 0, 1)],
            (TreeKind::Jungle, true) => &[(2.0, 2.0, -2, 1), (3.0, 1.0, -1, 0), (1.0, 3.0, -1, 0)],
            (TreeKind::Jungle, false) => &[(2.5, 2.5, -2, 1)],
            (TreeKind::Spruce, _) => &[],
        };
        let mut boxes: Vec<(f64, f64, i32, i32)> = crown.to_vec();
        if tree.kind == TreeKind::Spruce {
            // A narrow core from the lowest branches to the top, with wider tiers on every
            // other layer (and a single block on the very top).
            let lowest = 3 - tree.height;
            boxes.push((1.0, 1.0, lowest, 0));
            boxes.push((0.0, 0.0, 1, 1));
            if detailed {
                boxes.extend(
                    (lowest..=-2)
                        .filter(|dk| dk % 2 == 0)
                        .map(|dk| (1.8, 1.8, dk, dk)),
                );
            } else {
                boxes[0] = (1.5, 1.5, lowest, 0);
            }
        }

        // The trunk, up to where the crown starts (or the whole way for cacti).
        let crown_bottom = boxes
            .iter()
            .map(|b| b.2)
            .min()
            .map_or(tree.height + 1, |b| b + tree.height);
        let trunk_top = (tree.base + crown_bottom - 1).max(tree.base + 1);
        let (trunk_side, trunk_top_tex) = if tree.kind == TreeKind::Cactus {
            (Tex::CactusSide, Some(Tex::CactusTop))
        } else {
            (log, None)
        };
        let s = TREE_SHRINK;
        self.tree_box(&TreeBox {
            u: [gu as f64 + s, gu as f64 + 1.0 - s],
            v: [gv as f64 + s, gv as f64 + 1.0 - s],
            r: [
                self.planet.layer_radius(tree.base as f64 + 1.0) - SINK,
                self.planet.layer_radius(trunk_top as f64 + 1.0)
                    + if trunk_top_tex.is_some() { -s } else { 0.5 },
            ],
            blocks: [1, 1, (trunk_top - tree.base) as u32],
            side: trunk_side,
            top: trunk_top_tex,
            bottom: false,
            tint: white,
        });

        for (i, &(hu, hv, k0, k1)) in boxes.iter().enumerate() {
            let (k0, k1) = (top + k0, top + k1);
            // Boxes stacked on another box of the crown don't need a bottom face.
            let on_top = boxes[..i].iter().any(|b| top + b.3 + 1 == k0);
            self.tree_box(&TreeBox {
                u: [gu as f64 - hu + s, gu as f64 + 1.0 + hu - s],
                v: [gv as f64 - hv + s, gv as f64 + 1.0 + hv - s],
                r: [
                    self.planet.layer_radius(k0 as f64) + s,
                    self.planet.layer_radius(k1 as f64 + 1.0) - s,
                ],
                blocks: [
                    (1.0 + 2.0 * hu).round() as u32,
                    (1.0 + 2.0 * hv).round() as u32,
                    (k1 - k0 + 1) as u32,
                ],
                side: leaves,
                top: Some(leaves),
                bottom: !on_top,
                tint,
            });
        }
    }

    fn tree_box(&mut self, b: &TreeBox) {
        let n = self.planet.face_blocks;
        let face = self.face;
        let planet = self.planet;
        let p = |u: f64, v: f64, r: f64| planet.dir(face, u, v, n) * r;
        let ([u0, u1], [v0, v1], [r0, r1]) = (b.u, b.v, b.r);
        let [bu, bv, bk] = b.blocks;
        // (corners, uv corners, texture size); corners wind so that the face normal points
        // outwards, like the terrain walls and tops.
        let mut faces: Vec<BoxFace> = vec![
            (
                [p(u1, v0, r0), p(u1, v1, r0), p(u1, v1, r1), p(u1, v0, r1)],
                [2, 3, 1, 0],
                [bv, bk],
                b.side,
            ),
            (
                [p(u0, v0, r0), p(u0, v0, r1), p(u0, v1, r1), p(u0, v1, r0)],
                [2, 0, 1, 3],
                [bv, bk],
                b.side,
            ),
            (
                [p(u0, v1, r0), p(u0, v1, r1), p(u1, v1, r1), p(u1, v1, r0)],
                [2, 0, 1, 3],
                [bu, bk],
                b.side,
            ),
            (
                [p(u0, v0, r0), p(u1, v0, r0), p(u1, v0, r1), p(u0, v0, r1)],
                [2, 3, 1, 0],
                [bu, bk],
                b.side,
            ),
        ];
        if let Some(top) = b.top {
            faces.push((
                [p(u0, v0, r1), p(u1, v0, r1), p(u1, v1, r1), p(u0, v1, r1)],
                [0, 1, 3, 2],
                [bu, bv],
                top,
            ));
        }
        if b.bottom {
            faces.push((
                [p(u0, v0, r0), p(u0, v1, r0), p(u1, v1, r0), p(u1, v0, r0)],
                [0, 2, 3, 1],
                [bu, bv],
                b.top.unwrap_or(b.side),
            ));
        }
        for (corners, uv, size, tex) in faces {
            let corners = corners.map(|c| self.lattice(c));
            self.push_quad(false, corners, uv, tex, b.tint, size);
        }
    }

    fn push_quad(
        &mut self,
        water: bool,
        corners: [[i16; 3]; 4],
        uv: [u32; 4],
        tex: Tex,
        tint: u16,
        size: [u32; 2],
    ) {
        let size = size.map(|s| s.clamp(1, 32));
        let target = if water {
            &mut self.mesh.water
        } else {
            &mut self.mesh.opaque
        };
        for c in 0..4 {
            let mut data = BlockVertex::pack(tex as u32, uv[c], 3, 15, size);
            if tex == Tex::Lava {
                // It glows at any distance.
                data |= 15 << 28;
            }
            target.push(BlockVertex::new(corners[c], tint, data));
        }
    }

    /// Greedy-merged top faces: `get(i, j)` returns (layer, texture, tint) or `None`.
    fn tops(&mut self, get: &dyn Fn(i32, i32) -> Option<(i32, Tex, u16)>, water: bool) {
        let n = LOD_CELLS;
        let mut done = vec![false; (n * n) as usize];
        for j in 0..n {
            for i in 0..n {
                if done[(i + j * n) as usize] {
                    continue;
                }
                let Some(first) = get(i, j) else { continue };
                let same = |a: Option<(i32, Tex, u16)>| {
                    a.is_some_and(|a| a.0 == first.0 && a.1 == first.1)
                };
                let mut w = 1;
                while i + w < n
                    && w < self.max_merge
                    && !done[(i + w + j * n) as usize]
                    && same(get(i + w, j))
                {
                    w += 1;
                }
                let mut h = 1;
                'grow: while j + h < n && h < self.max_merge {
                    for x in i..i + w {
                        if done[(x + (j + h) * n) as usize] || !same(get(x, j + h)) {
                            break 'grow;
                        }
                    }
                    h += 1;
                }
                for y in j..j + h {
                    for x in i..i + w {
                        done[(x + y * n) as usize] = true;
                    }
                }
                let k = first.0;
                let corners = [
                    self.corner(i, j, k),
                    self.corner(i + w, j, k),
                    self.corner(i + w, j + h, k),
                    self.corner(i, j + h, k),
                ];
                let size = [(w * self.step) as u32, (h * self.step) as u32];
                self.push_quad(water, corners, [0, 1, 3, 2], first.1, first.2, size);
            }
        }
    }

    /// A vertical wall on side `dir` (+u, -u, +v, -v) of cell (i, j), from the top of layer
    /// `bottom` up to the top of layer `top`, facing outwards.
    #[allow(clippy::too_many_arguments)]
    fn wall(&mut self, i: i32, j: i32, dir: usize, bottom: i32, top: i32, tex: Tex, tint: u16) {
        let tint = if matches!(tex, Tex::OakLeaves | Tex::JungleLeaves | Tex::SpruceLeaves) {
            tint
        } else {
            BlockVertex::tint565(WHITE)
        };
        let (lo, hi) = (bottom, top);
        let size = [self.step as u32, (hi - lo).max(1) as u32];
        let (corners, uv) = match dir {
            0 => (
                [
                    self.corner(i + 1, j, lo),
                    self.corner(i + 1, j + 1, lo),
                    self.corner(i + 1, j + 1, hi),
                    self.corner(i + 1, j, hi),
                ],
                [2, 3, 1, 0],
            ),
            1 => (
                [
                    self.corner(i, j, lo),
                    self.corner(i, j, hi),
                    self.corner(i, j + 1, hi),
                    self.corner(i, j + 1, lo),
                ],
                [2, 0, 1, 3],
            ),
            2 => (
                [
                    self.corner(i, j + 1, lo),
                    self.corner(i, j + 1, hi),
                    self.corner(i + 1, j + 1, hi),
                    self.corner(i + 1, j + 1, lo),
                ],
                [2, 0, 1, 3],
            ),
            _ => (
                [
                    self.corner(i, j, lo),
                    self.corner(i + 1, j, lo),
                    self.corner(i + 1, j, hi),
                    self.corner(i, j, hi),
                ],
                [2, 3, 1, 0],
            ),
        };
        self.push_quad(false, corners, uv, tex, tint, size);
    }
}

/// Distance (m) from the camera to the nearest point of a tile, and whether the tile can be
/// above the horizon at all.
pub fn tile_distance(planet: &Planet, key: LodKey, camera_rel: DVec3) -> (f64, bool) {
    const LOWEST: f64 = -120.0;
    const HIGHEST: f64 = 420.0;
    let (centre, angular_radius) = tile_bounds(planet, key);
    let rc = camera_rel.length().max(1.0);
    let cam_dir = camera_rel / rc;
    let angle = cam_dir.dot(centre).clamp(-1.0, 1.0).acos();
    let nearest_angle = (angle - angular_radius).max(0.0);

    // Horizon: how far around the planet the camera can see terrain as high as `HIGHEST`.
    let r_low = planet.sea_radius + LOWEST;
    let r_high = planet.sea_radius + HIGHEST;
    let horizon = (r_low / rc.max(r_low)).acos() + (r_low / r_high).acos();
    let visible = nearest_angle <= horizon;

    // Distance to the nearest point of the tile at a typical terrain height.
    let rs = planet.sea_radius + (rc - planet.sea_radius).clamp(LOWEST, HIGHEST);
    let d2 = rc * rc + rs * rs - 2.0 * rc * rs * nearest_angle.cos();
    (d2.max(0.0).sqrt(), visible)
}

/// Selects the LOD tiles to show around the camera (quadtree leaves), with their distances.
pub fn select_tiles(planet: &Planet, camera_rel: DVec3, out: &mut Vec<(LodKey, f64)>) {
    let root = root_level(planet);
    for face in Face::ALL {
        let mut stack = vec![LodKey {
            face,
            level: root,
            tu: 0,
            tv: 0,
        }];
        while let Some(key) = stack.pop() {
            let (distance, visible) = tile_distance(planet, key, camera_rel);
            if !visible {
                continue;
            }
            let width = key.columns() as f64;
            if key.level > 1 && distance < LOD_SPLIT_DISTANCE * width {
                stack.extend(key.children());
            } else {
                out.push((key, distance));
            }
        }
    }
}

/// Real chunks are `CHUNK_SIZE` wide; LOD level 0 would equal a chunk column.
const _: () = assert!(LOD_CELLS == CHUNK_SIZE);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn selection_covers_the_view_without_overlaps() {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b);
        let up = DVec3::new(0.3, 0.8, 0.5).normalize();
        for altitude in [5.0, 400.0, 5_000.0, 90_000.0] {
            let mut tiles = Vec::new();
            select_tiles(&planet, up * (planet.sea_radius + altitude), &mut tiles);
            assert!(!tiles.is_empty());
            assert!(tiles.len() < 3000, "{} tiles at {altitude} m", tiles.len());
            // No tile may be an ancestor of another.
            let set: std::collections::HashSet<_> = tiles.iter().map(|t| t.0).collect();
            for &(t, _) in &tiles {
                let mut p = t;
                while p.level < root_level(&planet) {
                    p = p.parent();
                    assert!(!set.contains(&p), "{t:?} overlaps its ancestor {p:?}");
                }
            }
        }
    }

    #[test]
    fn tiles_mesh_within_vertex_range() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let terrain = TerrainGenerator::new(planet.clone());
        for level in 1..=root_level(&planet) {
            let key = LodKey {
                face: Face(2),
                level,
                tu: 0,
                tv: 0,
            };
            let tile = build_tile(&terrain, key);
            assert!(tile.mesh.is_some(), "level {level} produced no mesh");
        }

        // Tall mountains and dense jungle: every vertex must fit the i16 lattice range.
        for (lat, lon) in [(46.3f64, 35.5f64), (-43.0, 2.0)] {
            let (la, lo) = (lat.to_radians(), lon.to_radians());
            let dir = DVec3::new(la.cos() * lo.cos(), la.sin(), la.cos() * lo.sin());
            let (face, u, v, _) = planet.locate(dir * planet.sea_radius);
            for level in 1..=4u8 {
                let columns = (LOD_CELLS << level) as f64;
                let key = LodKey {
                    face,
                    level,
                    tu: (u / columns) as i32,
                    tv: (v / columns) as i32,
                };
                let (centre, _) = tile_bounds(&planet, key);
                let tile = build_tile(&terrain, key);
                let mesh = tile.mesh.expect("tile without mesh");
                let unit = mesh.scale as f64;
                let origin_units = tile.origin / unit;
                for vertex in mesh.opaque.iter().chain(&mesh.water) {
                    let [x, y, z, _] = vertex.position;
                    // Saturated coordinates would mean the vertex was clamped.
                    assert!(
                        [x, y, z].iter().all(|c| c.abs() < i16::MAX),
                        "level {level}: vertex out of range"
                    );
                    let p = (DVec3::new(x as f64, y as f64, z as f64) + origin_units) * unit;
                    assert!(
                        p.normalize().angle_between(centre) < 0.02,
                        "level {level}: vertex far outside its tile"
                    );
                }
            }
        }
    }
}
