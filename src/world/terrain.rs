//! Earth-like procedural terrain.
//!
//! Generation happens in two stages:
//!
//! 1. **Columns** ([`TerrainGenerator::sample_column`]): for every (face, u, v) column we
//!    evaluate 2D-on-the-sphere fields: continents, mountain ranges, hills, climate
//!    (temperature from latitude + altitude, humidity), and biome-specific shapes (dunes,
//!    mesas, terraced cliffs). Column data is cached per chunk-column and shared by every
//!    chunk stacked in it.
//! 2. **Blocks** ([`TerrainGenerator::generate_chunk`]): fills a padded 34^3 block array from
//!    the column data, carves 3D caves, plants trees and places ore veins, then meshes it.
//!
//! Everything is a pure function of (seed, position), so any thread can generate any chunk
//! in any order and neighbouring chunks always agree at their borders.

use std::{
    f64::consts::FRAC_PI_2,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};

use glam::DVec3;

use super::{
    block::Block,
    chunk::{ChunkBlocks, ChunkKey, PaddedBlocks},
    edits::WorldEdits,
    light::LightMap,
    mesher::{self, Columns},
    noise::{Noise, hash3, lerp, mix64, smoothstep, unit},
    planet::{CHUNK_SIZE, Face, Planet},
};
use crate::{render::ChunkMesh, util::FastMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    DeepOcean,
    Ocean,
    FrozenOcean,
    Beach,
    Plains,
    Forest,
    Taiga,
    Tundra,
    IceSheet,
    Desert,
    Badlands,
    Savanna,
    Jungle,
    Mountains,
    /// A volcano's slopes: ash over basalt.
    Volcanic,
    /// The lava lake in a volcano's crater.
    Caldera,
    /// The moon's pale, cratered uplands.
    Highlands,
    /// The moon's dark basalt plains.
    Mare,
}

impl Biome {
    pub fn name(self) -> &'static str {
        match self {
            Biome::DeepOcean => "Deep Ocean",
            Biome::Ocean => "Ocean",
            Biome::FrozenOcean => "Frozen Ocean",
            Biome::Beach => "Beach",
            Biome::Plains => "Plains",
            Biome::Forest => "Forest",
            Biome::Taiga => "Taiga",
            Biome::Tundra => "Tundra",
            Biome::IceSheet => "Ice Sheet",
            Biome::Desert => "Desert",
            Biome::Badlands => "Badlands",
            Biome::Savanna => "Savanna",
            Biome::Jungle => "Jungle",
            Biome::Mountains => "Mountains",
            Biome::Volcanic => "Volcano",
            Biome::Caldera => "Caldera",
            Biome::Highlands => "Highlands",
            Biome::Mare => "Mare",
        }
    }
}

/// A volcano (see `TerrainGenerator::find_volcanoes`). Generation is still a pure function of
/// the seed: the list is the same every time.
#[derive(Clone, Copy, Debug)]
pub struct Volcano {
    /// Direction of its summit (unit, planet body frame).
    pub dir: DVec3,
    /// Radius of the cone's foot and of the crater's rim (m).
    pub radius: f64,
    pub crater: f64,
    /// Height of the rim above the ground around it, and of that ground (m).
    pub height: f64,
    pub base: f64,
}

/// Everything the generator knows about one column, from noise alone.
#[derive(Clone, Copy, Debug)]
pub struct ColumnSample {
    /// Surface height in meters relative to sea level.
    pub height: f64,
    pub biome: Biome,
    /// Roughly -0.3 (polar) .. 1.1 (equatorial lowlands), includes altitude cooling.
    pub temperature: f64,
    /// 0 (arid) .. 1 (wet).
    pub humidity: f64,
    /// Heavy-mineral sand deposit (only meaningful on beaches).
    pub ilmenite: bool,
    /// Small per-column offset that makes rock strata wavy.
    pub strata: i32,
}

#[derive(Clone, Copy)]
pub struct Column {
    /// Layer index of the topmost terrain block.
    pub height: i32,
    pub biome: Biome,
    pub steep: bool,
    pub temperature: f32,
    pub ilmenite: bool,
    pub strata: i32,
    pub grass_tint: u32,
    pub foliage_tint: u32,
    pub water_tint: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Oak,
    Acacia,
    Spruce,
    Jungle,
    Cactus,
}

/// A tree as seen by the LOD system (see [`TerrainGenerator::tree_at`]).
#[derive(Clone, Copy, Debug)]
pub struct TreeInfo {
    pub kind: TreeKind,
    /// Layer of the ground block the tree stands on.
    pub base: i32,
    /// Trunk height in blocks; the crown is centred on layer `base + height`.
    pub height: i32,
    pub foliage_tint: u32,
}

/// Highest tree density of any biome: columns whose roll is above this never hold a tree.
const MAX_TREE_DENSITY: f64 = 0.05;

struct Tree {
    /// Chunk-column-local position of the trunk.
    u: i32,
    v: i32,
    /// Layer of the ground block the tree stands on.
    base: i32,
    kind: TreeKind,
    height: i32,
    seed: u64,
}

/// Column data for one chunk column, padded by one column on each side, plus all trees
/// whose canopy can reach into that padded area.
pub struct ColumnData {
    columns: Vec<Column>,
    trees: Vec<Tree>,
    /// Highest terrain layer in the padded area.
    max_terrain: i32,
    /// Highest layer of anything (terrain or trees).
    max_top: i32,
}

const PAD_COLS: i32 = CHUNK_SIZE + 2;
/// Columns sampled around a chunk column: 1 for the mesher's padding, +3 for tree canopies,
/// +1 so every tree candidate can compute its slope from real neighbours.
const COL_MARGIN: i32 = 5;
const COL_GRID: i32 = CHUNK_SIZE + 2 * COL_MARGIN;
/// Trees never grow this close to a cube-face edge, so tree stamping never has to cross
/// between the differently oriented grids of two faces.
const TREE_EDGE_MARGIN: i32 = 6;

impl ColumnData {
    /// Column at chunk-local (u, v) in -1..=32.
    #[inline]
    pub fn get(&self, u: i32, v: i32) -> &Column {
        &self.columns[((u + 1) + (v + 1) * PAD_COLS) as usize]
    }
}

struct CacheEntry {
    data: OnceLock<ColumnData>,
    last_used: AtomicU32,
}

pub struct ChunkResult {
    pub key: ChunkKey,
    pub blocks: ChunkBlocks,
    pub mesh: Option<ChunkMesh>,
    /// Mesh origin relative to the planet centre.
    pub origin: DVec3,
    pub radius: f32,
}

pub struct TerrainGenerator {
    pub planet: Arc<Planet>,
    continent: Noise,
    mountain_mask: Noise,
    ridges: Noise,
    hills: Noise,
    detail: Noise,
    temperature: Noise,
    humidity: Noise,
    badlands: Noise,
    dunes: Noise,
    mesas: Noise,
    cliffs: Noise,
    placer: Noise,
    strata: Noise,
    cave_a: Noise,
    cave_b: Noise,
    cavern: Noise,
    provinces: [Noise; ORE_COUNT],
    /// The moon: craters and maria instead of continents and climate; no water or trees.
    pub lunar: bool,
    /// Home's volcanoes (none on the moon).
    pub volcanoes: Vec<Volcano>,
    cache: Mutex<FastMap<(Face, i32, i32), Arc<CacheEntry>>>,
    epoch: AtomicU32,
}

const CACHE_SOFT_LIMIT: usize = 6000;
const MAX_VOLCANOES: usize = 14;
/// Depth (m below sea level) of the surface zone: terrain, ores and trees live above it.
/// Caves exist at every depth.
const SURFACE_DEPTH: i32 = 1000;
/// Thickness of the rocky crust; below it is the mantle.
const CRUST_DEPTH: f64 = 3000.0;

impl TerrainGenerator {
    pub fn new(planet: Arc<Planet>) -> Self {
        let seed = planet.seed;
        let n = |i: u64| Noise::new(seed ^ mix64(i));
        let mut generator = Self {
            continent: n(1),
            mountain_mask: n(2),
            ridges: n(3),
            hills: n(4),
            detail: n(5),
            temperature: n(6),
            humidity: n(7),
            badlands: n(8),
            dunes: n(9),
            mesas: n(10),
            cliffs: n(11),
            placer: n(12),
            strata: n(13),
            cave_a: n(14),
            cave_b: n(15),
            cavern: n(16),
            provinces: std::array::from_fn(|i| n(100 + i as u64)),
            lunar: false,
            volcanoes: Vec::new(),
            cache: Mutex::new(FastMap::default()),
            epoch: AtomicU32::new(0),
            planet,
        };
        generator.volcanoes = generator.find_volcanoes();
        generator
    }

    /// Picks the volcanoes: on land or shallow shelves, well away from each other, the poles
    /// and the distorted cube corners. Runs before any exist, so it sees the bare terrain.
    fn find_volcanoes(&self) -> Vec<Volcano> {
        let seed = self.planet.seed ^ 0x0701_ca40;
        let radius = self.planet.sea_radius;
        let mut found: Vec<Volcano> = Vec::new();
        for (i, dir) in fibonacci_sphere(1_200).enumerate() {
            let hash = hash3(seed, i as i64, 0, 0);
            if unit(hash) > 0.25 || dir.y.abs() > 0.8 || self.corner_sea(dir) > 0.0 {
                continue;
            }
            if found
                .iter()
                .any(|v| (v.dir - dir).length() * radius < 11_000.0)
            {
                continue;
            }
            let ground = self.sample_column(dir);
            if !(-30.0..60.0).contains(&ground.height) {
                continue;
            }
            let t = unit(mix64(hash ^ 1));
            found.push(Volcano {
                dir,
                radius: 900.0 + 700.0 * t,
                crater: 105.0 + 70.0 * t,
                height: 340.0 + 220.0 * unit(mix64(hash ^ 2)),
                base: ground.height,
            });
            if found.len() == MAX_VOLCANOES {
                break;
            }
        }
        found
    }

    /// The volcano at `dir`, if any: the height it makes there (replacing `h`) and its biome
    /// (`None` on the outer slopes, where the land keeps its own).
    fn volcano_at(&self, dir: DVec3, p: DVec3, h: f64) -> Option<(f64, Option<Biome>)> {
        for v in &self.volcanoes {
            let d = (dir - v.dir).length() * self.planet.sea_radius;
            if d >= v.radius {
                continue;
            }
            if d < v.crater {
                // A bowl around a flat lake of lava.
                let floor = v.base + v.height * 0.62;
                let rim = h + v.height;
                let t = d / v.crater;
                return Some(if t < 0.72 {
                    (floor, Some(Biome::Caldera))
                } else {
                    (
                        lerp(floor, rim, smoothstep(0.72, 1.0, t)),
                        Some(Biome::Volcanic),
                    )
                });
            }
            // A concave cone, cut by gullies running down its sides.
            let x = (d - v.crater) / (v.radius - v.crater);
            let gullies = 1.0 - 0.16 * self.ridges.ridged(p / 170.0, 3) * smoothstep(0.0, 0.2, x);
            let cone = v.height * (1.0 - x).powf(1.8) * gullies;
            // The ash thins out raggedly into the land around it.
            let ash_edge = 0.62 + 0.3 * self.hills.fbm(p / 240.0, 2);
            return Some((h + cone, (x < ash_edge).then_some(Biome::Volcanic)));
        }
        None
    }

    /// A generator for a moon: grey, cratered, airless (see `lunar_column`).
    pub fn moon(planet: Arc<Planet>) -> Self {
        Self {
            lunar: true,
            volcanoes: Vec::new(),
            ..Self::new(planet)
        }
    }

    // ------------------------------------------------------------------------------------
    // Columns: shape + climate + biome
    // ------------------------------------------------------------------------------------

    pub fn sample_column(&self, dir: DVec3) -> ColumnSample {
        if self.lunar {
            return self.lunar_column(dir);
        }
        // Noise coordinates in meters on the sea-level sphere.
        let p = dir * self.planet.sea_radius;

        // Continents and ocean basins. The 8 cube corners lie under round seas (see
        // `corner_sea`).
        let c = self.continent.fbm(p / 12_000.0, 6) * 1.7 - 0.08;
        let c = lerp(c, -0.7, self.corner_sea(dir));
        let land = smoothstep(-0.05, 0.1, c);
        let mut h = spline(
            c,
            &[
                (-1.0, -85.0),
                (-0.4, -60.0),
                (-0.18, -32.0),
                (-0.05, -8.0),
                (0.0, 1.0),
                (0.06, 4.0),
                (0.2, 12.0),
                (0.45, 28.0),
                (1.0, 50.0),
            ],
        );

        // Mountain ranges: ridged noise, only on land, masked into long belts.
        let mountains = smoothstep(0.1, 0.45, self.mountain_mask.fbm(p / 7_000.0, 3) * 1.6)
            * smoothstep(0.03, 0.2, c);
        if mountains > 0.0 {
            h += mountains * (35.0 + 260.0 * self.ridges.ridged(p / 1_600.0, 5));
        }

        // Rolling hills and small detail.
        h += self.hills.fbm(p / 450.0, 4) * 16.0 * (1.0 - 0.7 * mountains) * (0.3 + 0.7 * land);
        h += self.detail.fbm(p / 70.0, 3) * 2.5;

        // Climate. Latitude: 0 at the equator, 1 at the poles (+Y is the spin axis).
        let latitude = dir.y.clamp(-1.0, 1.0).asin().abs() / FRAC_PI_2;
        let base_temp =
            1.05 - 1.35 * latitude.powf(1.4) + self.temperature.fbm(p / 9_000.0, 3) * 0.35;
        let humidity = (self.humidity.fbm(p / 7_500.0, 4) * 1.4 + 0.5).clamp(0.0, 1.0);

        // Hot + dry land becomes desert; some deserts are badlands with mesas.
        let desert = smoothstep(0.62, 0.78, base_temp) * smoothstep(0.42, 0.28, humidity) * land;
        let badlands = if desert > 0.0 {
            desert * smoothstep(0.0, 0.2, self.badlands.fbm(p / 6_000.0, 3))
        } else {
            0.0
        };

        if desert > 0.0 {
            // Dunes: ridged noise stretched along one axis.
            let q = DVec3::new(p.x / 38.0, p.y / 90.0, p.z / 38.0);
            let dune = self.dunes.ridged(q, 2);
            h += dune * dune * 11.0 * desert * (1.0 - badlands) * (1.0 - mountains);
        }
        if badlands > 0.0 {
            // Flat-topped mesas with stepped, terraced cliff sides.
            let plateau = smoothstep(0.08, 0.16, self.mesas.fbm(p / 650.0, 3));
            h += badlands * plateau * 40.0;
            h = lerp(h, terrace(h, 7.0, 0.8), badlands);
        }

        // Terraced cliff country (temperate regions).
        let cliff = smoothstep(0.22, 0.38, self.cliffs.fbm(p / 2_500.0, 3)) * land * (1.0 - desert);
        if cliff > 0.0 {
            h = lerp(h, terrace(h, 10.0, 0.85), cliff);
        }

        // Volcanoes rise out of whatever is around them.
        let volcano = self.volcano_at(dir, p, h);
        let volcano_biome = volcano.and_then(|(_, b)| b);
        if let Some((height, _)) = volcano {
            h = height;
        }

        // It gets colder the higher you go: snow-capped peaks even near the equator.
        let temperature = base_temp - h.max(0.0) / 300.0;

        let biome = if let Some(b) = volcano_biome {
            b
        } else if h < 0.0 {
            if temperature < 0.0 {
                Biome::FrozenOcean
            } else if h < -38.0 {
                Biome::DeepOcean
            } else {
                Biome::Ocean
            }
        } else if h < 3.0 && mountains < 0.25 && cliff < 0.5 && badlands < 0.3 {
            Biome::Beach
        } else if mountains > 0.3 && h > 55.0 {
            Biome::Mountains
        } else if temperature < -0.05 {
            Biome::IceSheet
        } else if temperature < 0.18 {
            Biome::Tundra
        } else if temperature < 0.4 {
            if humidity > 0.42 {
                Biome::Taiga
            } else {
                Biome::Plains
            }
        } else if temperature > 0.7 {
            if badlands > 0.35 {
                Biome::Badlands
            } else if desert > 0.35 {
                Biome::Desert
            } else if humidity < 0.55 {
                Biome::Savanna
            } else {
                Biome::Jungle
            }
        } else if humidity < 0.4 {
            Biome::Plains
        } else {
            Biome::Forest
        };

        let ilmenite = biome == Biome::Beach && self.placer.fbm(p / 250.0, 2) > 0.18;
        let strata = (self.strata.fbm(p / 120.0, 2) * 5.0) as i32;

        ColumnSample {
            height: h,
            biome,
            temperature,
            humidity,
            ilmenite,
            strata,
        }
    }

    /// A column of the moon: broad dark maria (low basalt plains) between pale highlands,
    /// and craters at four scales, from basins kilometres across down to dimples a few
    /// metres wide. Each crater is a bowl, a raised rim and ejecta fading outside, hashed
    /// per cell of a 3D grid and never reaching beyond the cells next to its own, so nearby
    /// columns always agree.
    fn lunar_column(&self, dir: DVec3) -> ColumnSample {
        let p = dir * self.planet.sea_radius;
        let mare = smoothstep(0.05, 0.3, self.continent.fbm(p / 9_000.0, 3));
        let mut h = -120.0 * mare + self.hills.fbm(p / 1_500.0, 4) * 45.0 * (1.0 - 0.6 * mare);
        h += self.detail.fbm(p / 90.0, 2) * 1.5;
        for (cell, density, salt) in [
            (3_200.0, 0.5, 1),
            (1_100.0, 0.55, 2),
            (360.0, 0.5, 3),
            (110.0, 0.45, 4),
        ] {
            h += self.crater_field(p, cell, density, salt);
        }
        // Titanium-rich patches in the maria.
        let ilmenite = mare > 0.5 && self.placer.fbm(p / 400.0, 2) > 0.22;
        ColumnSample {
            height: h,
            biome: if mare > 0.5 {
                Biome::Mare
            } else {
                Biome::Highlands
            },
            temperature: 0.0,
            humidity: 0.0,
            ilmenite,
            strata: (self.strata.fbm(p / 120.0, 2) * 5.0) as i32,
        }
    }

    /// Height (m) the craters of one grid of `cell`-metre cells add at `p`.
    fn crater_field(&self, p: DVec3, cell: f64, density: f64, salt: u64) -> f64 {
        let base = (p / cell).floor();
        let mut h = 0.0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let c = base + DVec3::new(dx as f64, dy as f64, dz as f64);
                    let hash = hash3(self.planet.seed ^ salt, c.x as i64, c.y as i64, c.z as i64);
                    if unit(hash) > density {
                        continue;
                    }
                    let jitter = DVec3::new(
                        unit(mix64(hash ^ 1)),
                        unit(mix64(hash ^ 2)),
                        unit(mix64(hash ^ 3)),
                    );
                    // At most a third of a cell across times 3 (its ejecta): the 3x3x3 cells
                    // around `p` hold every crater that reaches it.
                    let radius = cell * (0.1 + 0.22 * unit(mix64(hash ^ 4)));
                    let d = (p - (c + jitter) * cell).length() / radius;
                    if d > 3.0 {
                        continue;
                    }
                    let profile = if d < 1.0 {
                        d * d - 0.75
                    } else {
                        0.25 * (-(d - 1.0) * 3.0).exp() * smoothstep(3.0, 2.0, d)
                    };
                    h += profile * radius * 0.2;
                }
            }
        }
        h
    }

    /// Column data for a chunk column, from the cache or freshly generated. Concurrent
    /// requests for the same column wait for a single computation.
    fn columns(&self, face: Face, cu: i32, cv: i32) -> Arc<CacheEntry> {
        let entry = {
            let mut cache = self.cache.lock().unwrap();
            cache
                .entry((face, cu, cv))
                .or_insert_with(|| {
                    Arc::new(CacheEntry {
                        data: OnceLock::new(),
                        last_used: AtomicU32::new(0),
                    })
                })
                .clone()
        };
        entry
            .last_used
            .store(self.epoch.load(Ordering::Relaxed), Ordering::Relaxed);
        entry.data.get_or_init(|| self.build_columns(face, cu, cv));
        entry
    }

    /// Evicts column data that hasn't been used for a while. Call periodically.
    pub fn maintain_cache(&self) {
        let epoch = self.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        let mut cache = self.cache.lock().unwrap();
        if cache.len() > CACHE_SOFT_LIMIT {
            cache.retain(|_, e| e.last_used.load(Ordering::Relaxed) + 3 >= epoch);
        }
    }

    fn build_columns(&self, face: Face, cu: i32, cv: i32) -> ColumnData {
        let planet = &*self.planet;
        let n = planet.face_blocks;
        let sea = planet.sea_layer;
        let (u0, v0) = (cu * CHUNK_SIZE, cv * CHUNK_SIZE);
        let g = COL_GRID as usize;
        let idx = |i: i32, j: i32| (i + j * COL_GRID) as usize;

        let samples: Vec<ColumnSample> = (0..COL_GRID * COL_GRID)
            .map(|i| {
                let (gu, gv) = (
                    u0 + i % COL_GRID - COL_MARGIN,
                    v0 + i / COL_GRID - COL_MARGIN,
                );
                let (f, wu, wv) = planet.wrap_column(face, gu, gv, n);
                self.sample_column(planet.column_dir(f, wu, wv))
            })
            .collect();
        let heights: Vec<i32> = samples
            .iter()
            .map(|s| sea + s.height.floor() as i32)
            .collect();

        let mut grid: Vec<Column> = Vec::with_capacity(g * g);
        for j in 0..COL_GRID {
            for i in 0..COL_GRID {
                let s = &samples[idx(i, j)];
                let h = heights[idx(i, j)];
                let steep = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(di, dj)| {
                    let (ni, nj) = (
                        (i + di).clamp(0, COL_GRID - 1),
                        (j + dj).clamp(0, COL_GRID - 1),
                    );
                    (heights[idx(ni, nj)] - h).abs() >= 3
                });
                grid.push(Column {
                    height: h,
                    biome: s.biome,
                    steep,
                    temperature: s.temperature as f32,
                    ilmenite: s.ilmenite,
                    strata: s.strata,
                    grass_tint: pack_rgb(grass_color(s.temperature, s.humidity)),
                    foliage_tint: pack_rgb(
                        grass_color(s.temperature, s.humidity).map(|c| c * 0.82),
                    ),
                    water_tint: pack_rgb(water_color(s.temperature)),
                });
            }
        }

        // Trees: deterministic per column, so every chunk that can see a tree agrees on it.
        let mut trees = Vec::new();
        for j in 1..COL_GRID - 1 {
            for i in 1..COL_GRID - 1 {
                let (gu, gv) = (u0 + i - COL_MARGIN, v0 + j - COL_MARGIN);
                if gu < TREE_EDGE_MARGIN
                    || gv < TREE_EDGE_MARGIN
                    || gu >= n - TREE_EDGE_MARGIN
                    || gv >= n - TREE_EDGE_MARGIN
                {
                    continue;
                }
                let col = &grid[idx(i, j)];
                let Some((kind, density)) = tree_for(col, sea) else {
                    continue;
                };
                let h = tree_roll(planet.seed, face, gu, gv);
                if unit(h) >= density {
                    continue;
                }
                let r = mix64(h);
                let height = tree_height(kind, r);
                trees.push(Tree {
                    u: i - COL_MARGIN,
                    v: j - COL_MARGIN,
                    base: col.height,
                    kind,
                    height,
                    seed: r,
                });
            }
        }

        let mut columns = Vec::with_capacity((PAD_COLS * PAD_COLS) as usize);
        for v in -1..=CHUNK_SIZE {
            for u in -1..=CHUNK_SIZE {
                columns.push(grid[idx(u + COL_MARGIN, v + COL_MARGIN)]);
            }
        }
        let max_terrain = columns.iter().map(|c| c.height).max().unwrap();
        let max_top = trees
            .iter()
            .map(|t| t.base + t.height + 2)
            .fold(max_terrain, i32::max);

        ColumnData {
            columns,
            trees,
            max_terrain,
            max_top,
        }
    }

    /// The tree growing at surface column (gu, gv) of `face`, if any - exactly the tree the
    /// chunk generator plants there. Cheap for most columns (one hash); only candidates are
    /// sampled. Used by the LOD tiles so distant trees match the real ones.
    pub fn tree_at(&self, face: Face, gu: i32, gv: i32) -> Option<TreeInfo> {
        let planet = &*self.planet;
        let n = planet.face_blocks;
        if gu < TREE_EDGE_MARGIN
            || gv < TREE_EDGE_MARGIN
            || gu >= n - TREE_EDGE_MARGIN
            || gv >= n - TREE_EDGE_MARGIN
        {
            return None;
        }
        let h = tree_roll(planet.seed, face, gu, gv);
        let roll = unit(h);
        if roll >= MAX_TREE_DENSITY {
            return None;
        }
        let sea = planet.sea_layer;
        let height_at = |u: i32, v: i32| -> (ColumnSample, i32) {
            let s = self.sample_column(planet.column_dir(face, u, v));
            (s, sea + s.height.floor() as i32)
        };
        let (s, base) = height_at(gu, gv);
        let (kind, density) = tree_kind(s.biome, s.temperature as f32, base, sea)?;
        if roll >= density {
            return None;
        }
        let steep = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|&(du, dv)| (height_at(gu + du, gv + dv).1 - base).abs() >= 3);
        if steep {
            return None;
        }
        Some(TreeInfo {
            kind,
            base,
            height: tree_height(kind, mix64(h)),
            foliage_tint: pack_rgb(grass_color(s.temperature, s.humidity).map(|c| c * 0.82)),
        })
    }

    // ------------------------------------------------------------------------------------
    // Blocks
    // ------------------------------------------------------------------------------------

    /// Generates and meshes a chunk, with the player's `edits` applied on top.
    pub fn generate_chunk(
        &self,
        key: ChunkKey,
        edits: &WorldEdits,
        light: &LightMap,
    ) -> ChunkResult {
        let k0 = key.ck * CHUNK_SIZE;

        // The deep interior (below the surface zone, including all inner shells) has no
        // terrain surface, just radial rock layers and caves: no column data needed.
        if k0 + CHUNK_SIZE < self.surface_floor() - 1 {
            return self.deep_chunk(key, edits, light);
        }

        let entry = self.columns(key.face, key.cu, key.cv);
        let cols = entry.data.get().expect("initialized by columns()");

        // Open sky: nothing to generate, nothing to draw (unless blocks were placed).
        let sky_from = if self.lunar {
            cols.max_top
        } else {
            cols.max_top.max(self.planet.sea_layer)
        };
        if k0 - 1 > sky_from && !edits.touches(&self.planet, key) {
            return ChunkResult {
                key,
                blocks: ChunkBlocks::Uniform(Block::Air),
                mesh: None,
                origin: self.planet.chunk_center(key),
                radius: 0.0,
            };
        }

        let mut blocks = PaddedBlocks::new();
        self.fill_terrain(key, cols, &mut blocks);
        self.place_trees(key, cols, &mut blocks);
        self.place_ores(key, cols, &mut blocks);
        edits.apply(&self.planet, key, &mut blocks);

        let (mesh, origin, radius) = mesher::mesh_chunk(
            &self.planet,
            key,
            &blocks,
            Columns::Surface(cols),
            &self.light_lookup(key, light),
        );
        ChunkResult {
            key,
            blocks: ChunkBlocks::from_padded(&blocks),
            mesh: (!mesh.is_empty()).then_some(mesh),
            origin,
            radius,
        }
    }

    /// Block light at chunk-local (padded) coordinates, for the mesher. Chunks no light
    /// source can reach skip the lookups.
    fn light_lookup<'a>(
        &'a self,
        key: ChunkKey,
        light: &'a LightMap,
    ) -> impl Fn([i32; 3]) -> u8 + 'a {
        let lit = light.reaches(self.planet.chunk_center(key), 40.0);
        let (u0, v0, k0) = (
            key.cu * CHUNK_SIZE,
            key.cv * CHUNK_SIZE,
            key.ck * CHUNK_SIZE,
        );
        move |[u, v, k]: [i32; 3]| {
            if !lit {
                return 0;
            }
            self.planet
                .cell(key.face, u0 + u, v0 + v, k0 + k)
                .map_or(0, |c| light.get(c))
        }
    }

    fn fill_terrain(&self, key: ChunkKey, cols: &ColumnData, out: &mut PaddedBlocks) {
        let planet = &*self.planet;
        let n = planet.face_blocks;
        let sea = planet.sea_layer;
        let (u0, v0, k0) = (
            key.cu * CHUNK_SIZE,
            key.cv * CHUNK_SIZE,
            key.ck * CHUNK_SIZE,
        );
        let caves =
            (k0 - 1 <= cols.max_terrain).then(|| CaveLattice::new(self, key.face, u0, v0, k0, n));

        for v in -1..=CHUNK_SIZE {
            for u in -1..=CHUNK_SIZE {
                let col = cols.get(u, v);
                let (gu, gv) = (u0 + u, v0 + v);
                // Padding columns beyond the face edge belong to another face.
                let wrapped = (!(0..n).contains(&gu) || !(0..n).contains(&gv))
                    .then(|| planet.wrap_column(key.face, gu, gv, n));
                let ocean_column = !self.lunar && col.height < sea + 3;

                for kl in -1..=CHUNK_SIZE {
                    let k = k0 + kl;
                    let block = if k > col.height {
                        if !self.lunar && k <= sea && col.height < sea {
                            if k == sea && col.temperature < 0.0 {
                                Block::Ice
                            } else {
                                Block::Water
                            }
                        } else {
                            Block::Air
                        }
                    } else {
                        let solid = self.terrain_block(col, k);
                        let depth = col.height - k;
                        let carve = match &caves {
                            Some(lattice) if !(ocean_column && depth < 12) => {
                                let vals = match wrapped {
                                    None => lattice.sample(gu, gv, k),
                                    Some((f, wu, wv)) => self.cave_values_slow(f, wu, wv, k, n),
                                };
                                self.is_cave(vals, depth, k)
                            }
                            _ => false,
                        };
                        if carve { Block::Air } else { solid }
                    };
                    out.set(u, v, kl, block);
                }
            }
        }
    }

    /// Lowest layer of the surface zone (terrain, ores, trees...). Below it, chunks are
    /// generated without column data.
    fn surface_floor(&self) -> i32 {
        self.planet.sea_layer - SURFACE_DEPTH
    }

    /// The planet's interior below the crust, by radius (fraction of the sea-level radius),
    /// loosely following Earth: rocky mantle, molten outer core, solid iron-nickel inner core.
    fn deep_block(&self, k: i32) -> Block {
        let r = self.planet.layer_radius(k as f64 + 0.5);
        let surface = self.planet.sea_radius;
        if r > surface - CRUST_DEPTH {
            Block::Deepstone
        } else if r > surface * 0.55 {
            Block::Mantle
        } else if r > surface * 0.2 {
            Block::OuterCore
        } else {
            Block::InnerCore
        }
    }

    fn deep_chunk(&self, key: ChunkKey, edits: &WorldEdits, light: &LightMap) -> ChunkResult {
        let mut blocks = PaddedBlocks::new();
        self.fill_deep(key, &mut blocks);
        edits.apply(&self.planet, key, &mut blocks);
        let (mesh, origin, radius) = if blocks.all_opaque() {
            // Solid rock all around: no faces can be exposed.
            (None, self.planet.chunk_center(key), 0.0)
        } else {
            let columns = Columns::Deep(self.planet.sea_layer);
            let lookup = self.light_lookup(key, light);
            let (mesh, origin, radius) =
                mesher::mesh_chunk(&self.planet, key, &blocks, columns, &lookup);
            ((!mesh.is_empty()).then_some(mesh), origin, radius)
        };
        ChunkResult {
            key,
            blocks: ChunkBlocks::from_padded(&blocks),
            mesh,
            origin,
            radius,
        }
    }

    /// Fills a deep chunk: radial rock layers with caves carved out. Handles padding across
    /// cube-face seams and across shell boundaries (where the grid resolution changes).
    fn fill_deep(&self, key: ChunkKey, out: &mut PaddedBlocks) {
        let planet = &*self.planet;
        let sea = planet.sea_layer;
        let (u0, v0, k0) = (
            key.cu * CHUNK_SIZE,
            key.cv * CHUNK_SIZE,
            key.ck * CHUNK_SIZE,
        );
        let n = planet.res_at(k0);
        let lattice = CaveLattice::new(self, key.face, u0, v0, k0, n);

        for v in -1..=CHUNK_SIZE {
            for u in -1..=CHUNK_SIZE {
                let (gu, gv) = (u0 + u, v0 + v);
                let (face, wu, wv) = planet.wrap_column(key.face, gu, gv, n);
                let wrapped = face != key.face || wu != gu || wv != gv;
                for kl in -1..=CHUNK_SIZE {
                    let k = k0 + kl;
                    let block = if k < 0 {
                        Block::InnerCore // "below" the centre: treat as solid
                    } else if k >= planet.layers {
                        Block::Air
                    } else {
                        let kn = planet.res_at(k);
                        let cave = |f: Face, cu: i32, cv: i32, res: i32| {
                            self.is_cave(self.cave_values_slow(f, cu, cv, k, res), sea - k, k)
                        };
                        let open = if kn == n {
                            let vals = if wrapped {
                                self.cave_values_slow(face, wu, wv, k, n)
                            } else {
                                lattice.sample(gu, gv, k)
                            };
                            self.is_cave(vals, sea - k, k)
                        } else if kn < n {
                            // Coarser shell below: one cell covers 2x2 of our columns.
                            cave(face, wu / 2, wv / 2, kn)
                        } else {
                            // Finer shell above: open if any of the 4 cells over us is open, so
                            // our top face is drawn wherever something could look at it.
                            (0..4).any(|i| cave(face, wu * 2 + i % 2, wv * 2 + i / 2, kn))
                        };
                        if open { Block::Air } else { self.deep_block(k) }
                    };
                    out.set(u, v, kl, block);
                }
            }
        }
    }

    /// The block at layer `k` (at or below the column's surface), before caves and ores.
    fn terrain_block(&self, col: &Column, k: i32) -> Block {
        let sea = self.planet.sea_layer;
        let d = col.height - k;
        let rock = if k < sea - 64 + col.strata {
            Block::Deepstone
        } else if self.lunar {
            Block::MoonRock
        } else {
            Block::Stone
        };
        match col.biome {
            Biome::Highlands => {
                if d < 3 && !(col.steep && d == 0) {
                    Block::Regolith
                } else {
                    rock
                }
            }
            Biome::Mare => {
                if d == 0 && col.ilmenite && !col.steep {
                    Block::IlmeniteSand
                } else if d < 2 && !col.steep {
                    Block::Regolith
                } else if d < 40 {
                    Block::Basalt
                } else {
                    rock
                }
            }
            Biome::Caldera => {
                if d < 4 {
                    Block::Lava
                } else {
                    Block::Basalt
                }
            }
            Biome::Volcanic => {
                if d < 2 && !col.steep {
                    Block::Ash
                } else if d < 60 {
                    Block::Basalt
                } else {
                    rock
                }
            }
            Biome::Ocean | Biome::DeepOcean | Biome::FrozenOcean => {
                if d < 3 {
                    if col.biome == Biome::Ocean && col.temperature > 0.25 {
                        Block::Sand
                    } else {
                        Block::Gravel
                    }
                } else if d < 5 && col.biome == Biome::Ocean {
                    Block::Sandstone
                } else {
                    rock
                }
            }
            Biome::Beach => {
                if d < 4 {
                    if col.temperature < 0.15 {
                        Block::Gravel
                    } else if col.ilmenite && d < 2 {
                        Block::IlmeniteSand
                    } else {
                        Block::Sand
                    }
                } else if d < 7 {
                    Block::Sandstone
                } else {
                    rock
                }
            }
            Biome::Desert => {
                if col.steep && d == 0 {
                    Block::Sandstone
                } else if d < 5 {
                    Block::Sand
                } else if d < 14 {
                    Block::Sandstone
                } else {
                    rock
                }
            }
            Biome::Badlands => {
                if d == 0 && !col.steep {
                    Block::RedSand
                } else if d < 70 {
                    terracotta_band(k + col.strata)
                } else {
                    rock
                }
            }
            Biome::Mountains => {
                if col.steep {
                    rock
                } else if d == 0 {
                    if col.temperature < 0.1 {
                        Block::Snow
                    } else {
                        Block::Grass
                    }
                } else if d < 3 {
                    Block::Dirt
                } else {
                    rock
                }
            }
            Biome::IceSheet => {
                if d < 3 {
                    Block::Snow
                } else if d < 12 {
                    Block::Ice
                } else {
                    rock
                }
            }
            Biome::Tundra | Biome::Taiga => {
                if col.steep {
                    rock
                } else if d == 0 {
                    if col.temperature < 0.28 {
                        Block::SnowyGrass
                    } else {
                        Block::Grass
                    }
                } else if d < 4 {
                    Block::Dirt
                } else {
                    rock
                }
            }
            Biome::Plains | Biome::Forest | Biome::Savanna | Biome::Jungle => {
                if col.steep {
                    rock
                } else if d == 0 {
                    Block::Grass
                } else if d < 4 {
                    Block::Dirt
                } else {
                    rock
                }
            }
        }
    }

    /// Cave noise values (tunnel a, tunnel b, cavern) at a lattice point of a grid with `n`
    /// columns per face edge.
    ///
    /// Caves change character with depth: near the surface positions are stretched radially
    /// so tunnels tend to run horizontally; deeper down the noise is scaled up so tunnels get
    /// wider and caverns enormous. All parameters vary smoothly with radius, so there are no
    /// seams between depth zones.
    fn cave_noise(&self, face: Face, u: i32, v: i32, k: i32, n: i32) -> [f32; 3] {
        let planet = &*self.planet;
        let dir = planet.dir(face, u as f64, v as f64, n);
        let r = planet.layer_radius(k as f64);
        let depth = planet.sea_radius - r;
        let deep = smoothstep(0.03, 0.35, depth / planet.sea_radius);
        let flatten = 1.0 - smoothstep(2_000.0, 6_000.0, depth);
        let p = dir * (r + (r - planet.sea_radius) * 0.8 * flatten);
        let tunnel_scale = 45.0 * (1.0 + 1.5 * deep);
        let cavern_scale = 80.0 * (1.0 + 2.5 * deep);
        [
            self.cave_a.noise(p / tunnel_scale) as f32,
            self.cave_b.noise(p / tunnel_scale) as f32,
            self.cavern.fbm(p / cavern_scale, 2) as f32,
        ]
    }

    /// Spaghetti tunnels where two independent noise fields are both near zero, plus big
    /// "cheese" caverns. `depth` is blocks below the local surface; `k` is the layer.
    /// Caverns are most common in the mantle and get rarer towards the core.
    fn is_cave(&self, [a, b, cavern]: [f32; 3], depth: i32, k: i32) -> bool {
        let planet = &*self.planet;
        let f = ((planet.sea_radius - planet.layer_radius(k as f64)) / planet.sea_radius)
            .clamp(0.0, 1.0);
        let tunnel_limit = spline(
            f,
            &[(0.0, 0.011), (0.45, 0.011), (0.8, 0.006), (1.0, 0.005)],
        );
        let cavern_limit = spline(
            f,
            &[
                (0.0, 0.5),
                (0.03, 0.5),
                (0.1, 0.42),
                (0.4, 0.44),
                (0.6, 0.5),
                (0.85, 0.56),
                (1.0, 0.58),
            ],
        );
        let tunnel = a * a + b * b < tunnel_limit as f32;
        let chamber = depth > 14 && cavern > cavern_limit as f32;
        tunnel || chamber
    }

    /// Cave values for a single block without a prepared lattice (padding across face edges
    /// and shell boundaries).
    fn cave_values_slow(&self, face: Face, u: i32, v: i32, k: i32, n: i32) -> [f32; 3] {
        let s = CAVE_STEP;
        let (lu, lv, lk) = (
            u.div_euclid(s) * s,
            v.div_euclid(s) * s,
            k.div_euclid(s) * s,
        );
        let (fu, fv, fk) = (
            (u - lu) as f32 / s as f32,
            (v - lv) as f32 / s as f32,
            (k - lk) as f32 / s as f32,
        );
        let c = |du, dv, dk| self.cave_noise(face, lu + du * s, lv + dv * s, lk + dk * s, n);
        trilerp(
            [
                c(0, 0, 0),
                c(1, 0, 0),
                c(0, 1, 0),
                c(1, 1, 0),
                c(0, 0, 1),
                c(1, 0, 1),
                c(0, 1, 1),
                c(1, 1, 1),
            ],
            fu,
            fv,
            fk,
        )
    }

    fn place_trees(&self, key: ChunkKey, cols: &ColumnData, out: &mut PaddedBlocks) {
        let k0 = key.ck * CHUNK_SIZE;
        let mut put = |u: i32, v: i32, k: i32, block: Block, replace_leaves: bool| {
            let kl = k - k0;
            if PaddedBlocks::in_bounds(u, v, kl) {
                let existing = out.get(u, v, kl);
                if existing == Block::Air
                    || (replace_leaves
                        && matches!(
                            existing,
                            Block::OakLeaves | Block::SpruceLeaves | Block::JungleLeaves
                        ))
                {
                    out.set(u, v, kl, block);
                }
            }
        };

        // Leaves first, then trunks, so trunks always win.
        for tree in &cols.trees {
            let top = tree.base + tree.height;
            let bit = |i: u64| (mix64(tree.seed ^ i) & 1) == 1;
            match tree.kind {
                TreeKind::Oak => {
                    for dk in -2..=1 {
                        let r: i32 = if dk <= -1 { 2 } else { 1 };
                        for dv in -r..=r {
                            for du in -r..=r {
                                let corner = du.abs() == r && dv.abs() == r;
                                if corner && (dk == 1 || bit((du * 7 + dv * 13 + dk * 31) as u64)) {
                                    continue;
                                }
                                put(tree.u + du, tree.v + dv, top + dk, Block::OakLeaves, false);
                            }
                        }
                    }
                }
                TreeKind::Acacia => {
                    for dk in 0..=1 {
                        let r = if dk == 0 { 3 } else { 1 };
                        for dv in -r..=r {
                            for du in -r..=r {
                                if du * du + dv * dv <= r * r + 1 {
                                    put(
                                        tree.u + du,
                                        tree.v + dv,
                                        top + dk,
                                        Block::OakLeaves,
                                        false,
                                    );
                                }
                            }
                        }
                    }
                }
                TreeKind::Spruce => {
                    for dk in (3 - tree.height)..=1 {
                        let dist = 1 - dk;
                        let r = if dist <= 1 { dist.min(1) } else { 1 + dist % 2 };
                        for dv in -r..=r {
                            for du in -r..=r {
                                if r == 2 && du.abs() == 2 && dv.abs() == 2 {
                                    continue;
                                }
                                put(
                                    tree.u + du,
                                    tree.v + dv,
                                    top + dk,
                                    Block::SpruceLeaves,
                                    false,
                                );
                            }
                        }
                    }
                }
                TreeKind::Jungle => {
                    for dk in -2..=1 {
                        let r = if dk == -2 || dk == 1 { 2 } else { 3 };
                        for dv in -r..=r {
                            for du in -r..=r {
                                if du * du + dv * dv <= r * r + 1 {
                                    put(
                                        tree.u + du,
                                        tree.v + dv,
                                        top + dk,
                                        Block::JungleLeaves,
                                        false,
                                    );
                                }
                            }
                        }
                    }
                }
                TreeKind::Cactus => {}
            }
        }
        for tree in &cols.trees {
            let trunk = match tree.kind {
                TreeKind::Spruce => Block::SpruceLog,
                TreeKind::Cactus => Block::Cactus,
                _ => Block::OakLog,
            };
            for dk in 1..=tree.height {
                put(tree.u, tree.v, tree.base + dk, trunk, true);
            }
        }
    }

    /// Ore veins. Space is divided into 8^3 cells; each cell may hold one vein, whose type
    /// depends on depth, biome and large-scale "ore provinces". Veins stay inside their cell,
    /// so each chunk can place its own ores without looking at neighbours.
    fn place_ores(&self, key: ChunkKey, cols: &ColumnData, out: &mut PaddedBlocks) {
        let planet = &*self.planet;
        let k0 = key.ck * CHUNK_SIZE;
        const CELL: i32 = 8;
        for ck in 0..CHUNK_SIZE / CELL {
            for cv in 0..CHUNK_SIZE / CELL {
                for cu in 0..CHUNK_SIZE / CELL {
                    let (lu, lv, lk) = (cu * CELL, cv * CELL, ck * CELL);
                    let col = cols.get(lu + CELL / 2, lv + CELL / 2);
                    let depth = col.height - (k0 + lk + CELL / 2);
                    if depth < 2 {
                        continue;
                    }
                    let (gu, gv, gk) =
                        (key.cu * CHUNK_SIZE + lu, key.cv * CHUNK_SIZE + lv, k0 + lk);
                    let mut h = hash3(
                        planet.seed ^ 0x0e5_0e5,
                        key.face.0 as i64 * 1_000_003 + gu as i64,
                        gv as i64,
                        gk as i64,
                    );
                    let dir = planet.column_dir(key.face, gu + CELL / 2, gv + CELL / 2);
                    let province = |ore: Ore| {
                        smoothstep(
                            0.1,
                            0.4,
                            self.provinces[ore as usize].fbm(dir * planet.sea_radius / 4_000.0, 2),
                        )
                    };
                    let Some(ore) = choose_ore(unit(h), depth, col.biome, province) else {
                        continue;
                    };

                    let radius = ore.vein_radius(unit(mix64(h ^ 1)));
                    h = mix64(h ^ 2);
                    let mut centre = [0.0f32; 3];
                    for c in &mut centre {
                        let span = (CELL as f32 / 2.0 - radius - 0.5).max(0.0);
                        *c = CELL as f32 / 2.0 + (unit(h) as f32 * 2.0 - 1.0) * span;
                        h = mix64(h);
                    }
                    for dk in 0..CELL {
                        for dv in 0..CELL {
                            for du in 0..CELL {
                                let (u, v, k) = (lu + du, lv + dv, lk + dk);
                                let d2 = (du as f32 + 0.5 - centre[0]).powi(2)
                                    + (dv as f32 + 0.5 - centre[1]).powi(2)
                                    + (dk as f32 + 0.5 - centre[2]).powi(2);
                                let jitter =
                                    0.6 + 0.6 * unit(hash3(h, u as i64, v as i64, k as i64)) as f32;
                                if d2 <= radius * radius * jitter
                                    && ore.can_replace(out.get(u, v, k))
                                {
                                    out.set(u, v, k, ore.block());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// How much of a corner sea covers `dir` (0..1). At the 8 cube corners only 3 block columns
    /// meet, and the grid has to bend round them: blocks there are skewed and smaller (see
    /// `planet::MAP_Q`). Each corner lies under a round, deep sea with a ragged coast, so the
    /// distorted blocks are seabed nobody builds on.
    pub fn corner_sea(&self, dir: DVec3) -> f64 {
        // Angle to the nearest corner, (+-1, +-1, +-1) / sqrt(3).
        let cos = (dir.x.abs() + dir.y.abs() + dir.z.abs()) / 3f64.sqrt();
        let angle = cos.min(1.0).acos().to_degrees();
        let coast = self.hills.fbm(dir * self.planet.sea_radius / 2_500.0, 3) * 2.0;
        smoothstep(CORNER_SEA_OUTER, CORNER_SEA_INNER, angle + coast)
    }

    /// A pleasant spawn point: temperate land where the blocks are as close to 1 m cubes as the
    /// cube-sphere allows (see `planet::MAP_Q`). Blocks are square almost everywhere; their
    /// width is 1 m on a ring around each face centre (wider inside it, narrower outside).
    pub fn find_spawn(&self) -> DVec3 {
        // About 0.5 km apart.
        let mut candidates: Vec<(f64, DVec3)> = fibonacci_sphere(100_000)
            .map(|dir| {
                let (wu, wv, angle) = self.planet.block_shape(dir);
                let skew = angle.to_radians().cos().abs();
                ((wu - 1.0).abs().max((wv - 1.0).abs()).max(skew), dir)
            })
            .filter(|&(deviation, _)| deviation < 0.08)
            .collect();
        // Stable sort, so this is deterministic.
        candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
        candidates
            .iter()
            .map(|&(_, dir)| dir)
            .find(|&dir| self.good_spawn(dir))
            .unwrap_or_else(|| self.find_spawn_near(DVec3::new(0.55, 0.45, 0.1).normalize()))
    }

    fn good_spawn(&self, dir: DVec3) -> bool {
        let s = self.sample_column(dir);
        if !(4.0..40.0).contains(&s.height)
            || !matches!(s.biome, Biome::Plains | Biome::Forest | Biome::Savanna)
        {
            return false;
        }
        // Not in or under a tree (crowns reach 3 columns out).
        let (face, u, v, _) = self.planet.locate(dir * self.planet.sea_radius);
        let (u, v) = (u.floor() as i32, v.floor() as i32);
        (-3..=3).all(|du| (-3..=3).all(|dv| self.tree_at(face, u + du, v + dv).is_none()))
    }

    /// Fallback: temperate land anywhere near `preferred` (a unit direction).
    fn find_spawn_near(&self, preferred: DVec3) -> DVec3 {
        let mut candidates: Vec<DVec3> = fibonacci_sphere(20_000).collect();
        candidates.sort_by(|a, b| b.dot(preferred).total_cmp(&a.dot(preferred)));
        candidates
            .into_iter()
            .take(4_000)
            .find(|&dir| self.good_spawn(dir))
            .unwrap_or(preferred)
    }
}

/// `count` evenly spread unit directions.
/// Corner seas (see `TerrainGenerator::corner_sea`): full sea within this many degrees of a
/// cube corner, the coast by the outer radius (+- a couple of degrees of ragged coastline).
const CORNER_SEA_INNER: f64 = 11.0;
const CORNER_SEA_OUTER: f64 = 15.0;

fn fibonacci_sphere(count: usize) -> impl Iterator<Item = DVec3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..count).map(move |i| {
        let y = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
        let r = (1.0 - y * y).sqrt();
        let a = golden * i as f64;
        DVec3::new(r * a.cos(), y, r * a.sin())
    })
}

// ----------------------------------------------------------------------------------------
// Caves
// ----------------------------------------------------------------------------------------

/// Cave noise is expensive in 3D, so (like Minecraft) it's evaluated on a coarse lattice
/// every `CAVE_STEP` blocks and trilinearly interpolated. The lattice is aligned to global
/// block coordinates, so every chunk computes identical values for shared blocks.
const CAVE_STEP: i32 = 4;
const LATTICE: i32 = (CHUNK_SIZE + 8) / CAVE_STEP + 1;

struct CaveLattice {
    u0: i32,
    v0: i32,
    k0: i32,
    values: Vec<[f32; 3]>,
}

impl CaveLattice {
    fn new(generator: &TerrainGenerator, face: Face, u0: i32, v0: i32, k0: i32, n: i32) -> Self {
        let (u0, v0, k0) = (u0 - CAVE_STEP, v0 - CAVE_STEP, k0 - CAVE_STEP);
        let mut values = Vec::with_capacity((LATTICE * LATTICE * LATTICE) as usize);
        for k in 0..LATTICE {
            for v in 0..LATTICE {
                for u in 0..LATTICE {
                    values.push(generator.cave_noise(
                        face,
                        u0 + u * CAVE_STEP,
                        v0 + v * CAVE_STEP,
                        k0 + k * CAVE_STEP,
                        n,
                    ));
                }
            }
        }
        Self { u0, v0, k0, values }
    }

    fn sample(&self, u: i32, v: i32, k: i32) -> [f32; 3] {
        let s = CAVE_STEP;
        let (ru, rv, rk) = (u - self.u0, v - self.v0, k - self.k0);
        let (iu, iv, ik) = (ru / s, rv / s, rk / s);
        let (fu, fv, fk) = (
            (ru % s) as f32 / s as f32,
            (rv % s) as f32 / s as f32,
            (rk % s) as f32 / s as f32,
        );
        let at = |du: i32, dv: i32, dk: i32| {
            self.values[((iu + du) + (iv + dv) * LATTICE + (ik + dk) * LATTICE * LATTICE) as usize]
        };
        trilerp(
            [
                at(0, 0, 0),
                at(1, 0, 0),
                at(0, 1, 0),
                at(1, 1, 0),
                at(0, 0, 1),
                at(1, 0, 1),
                at(0, 1, 1),
                at(1, 1, 1),
            ],
            fu,
            fv,
            fk,
        )
    }
}

fn trilerp(c: [[f32; 3]; 8], fu: f32, fv: f32, fk: f32) -> [f32; 3] {
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    std::array::from_fn(|i| {
        let x00 = l(c[0][i], c[1][i], fu);
        let x10 = l(c[2][i], c[3][i], fu);
        let x01 = l(c[4][i], c[5][i], fu);
        let x11 = l(c[6][i], c[7][i], fu);
        l(l(x00, x10, fv), l(x01, x11, fv), fk)
    })
}

// ----------------------------------------------------------------------------------------
// Ores
// ----------------------------------------------------------------------------------------

const ORE_COUNT: usize = 8;

#[derive(Clone, Copy, Debug)]
enum Ore {
    Coal,
    Iron,
    Copper,
    Gold,
    Diamond,
    Bauxite,
    Uranium,
    /// Oil shale: rolled for only after all the others, so it changes none of theirs.
    Oil,
}

impl Ore {
    fn block(self) -> Block {
        match self {
            Ore::Coal => Block::CoalOre,
            Ore::Iron => Block::IronOre,
            Ore::Copper => Block::CopperOre,
            Ore::Gold => Block::GoldOre,
            Ore::Diamond => Block::DiamondOre,
            Ore::Bauxite => Block::BauxiteOre,
            Ore::Uranium => Block::UraniumOre,
            Ore::Oil => Block::OilShale,
        }
    }

    fn vein_radius(self, t: f64) -> f32 {
        let (min, max) = match self {
            Ore::Coal => (2.0, 3.0),
            Ore::Iron => (1.6, 2.5),
            Ore::Copper => (1.7, 2.6),
            Ore::Gold => (1.2, 1.8),
            Ore::Diamond => (1.0, 1.5),
            Ore::Bauxite => (2.4, 3.2),
            Ore::Uranium => (1.2, 1.8),
            Ore::Oil => (2.6, 3.4),
        };
        (min + (max - min) * t) as f32
    }

    fn can_replace(self, block: Block) -> bool {
        match self {
            Ore::Bauxite => matches!(block, Block::Stone | Block::Dirt),
            Ore::Oil => matches!(block, Block::Stone | Block::Sandstone | Block::Dirt),
            _ => matches!(block, Block::Stone | Block::Deepstone),
        }
    }
}

/// Picks the ore (if any) for one 8^3 cell. Loosely based on real geology:
/// coal under old swamps/forests, copper in arid mountain belts (porphyry deposits), gold
/// and uranium in mountains, diamonds deep under cold ancient cratons, bauxite in tropical
/// soils (laterite), iron everywhere but richest in banded-iron provinces.
fn choose_ore(roll: f64, depth: i32, biome: Biome, province: impl Fn(Ore) -> f64) -> Option<Ore> {
    use Biome::*;
    let mountains = matches!(biome, Mountains);
    let arid = matches!(biome, Desert | Badlands);
    let in_range = |min: i32, max: i32| (min..=max).contains(&depth);

    let mut chances: [(Ore, f64); ORE_COUNT - 1] = [
        (
            Ore::Coal,
            if in_range(4, 120) {
                0.22 * if matches!(biome, Forest | Plains | Taiga | Jungle) {
                    1.6
                } else {
                    1.0
                }
            } else {
                0.0
            },
        ),
        (
            Ore::Iron,
            if in_range(6, 320) {
                0.14 * (1.0 + 2.0 * province(Ore::Iron))
            } else {
                0.0
            },
        ),
        (
            Ore::Copper,
            if in_range(4, 160) {
                0.05 * (1.0 + 3.0 * (mountains || arid) as u8 as f64 + 2.0 * province(Ore::Copper))
            } else {
                0.0
            },
        ),
        (
            Ore::Gold,
            if in_range(40, 500) {
                0.025 * (1.0 + 3.0 * (mountains || biome == Badlands) as u8 as f64)
            } else {
                0.0
            },
        ),
        (
            Ore::Diamond,
            if depth > 350 {
                0.02 * (1.0 + 3.0 * matches!(biome, Tundra | Taiga | IceSheet) as u8 as f64)
            } else {
                0.0
            },
        ),
        (
            Ore::Bauxite,
            if in_range(2, 24) && matches!(biome, Jungle | Savanna) {
                0.3
            } else {
                0.0
            },
        ),
        (
            Ore::Uranium,
            if in_range(60, 600) {
                0.01 * (1.0 + 4.0 * mountains as u8 as f64 + 3.0 * province(Ore::Uranium))
            } else {
                0.0
            },
        ),
    ];
    chances.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut roll = roll;
    for (ore, chance) in chances {
        if roll < chance {
            return Some(ore);
        }
        roll -= chance;
    }
    // Oil shale in the old seabeds under dry plains, deserts and the cold flats, in what is
    // left of the roll (so every cell that held another ore still does).
    let basin = matches!(biome, Desert | Badlands | Plains | Savanna | Tundra);
    let oil = if basin && in_range(6, 90) {
        0.12 * (1.0 + 2.0 * province(Ore::Oil))
    } else {
        0.0
    };
    (roll < oil).then_some(Ore::Oil)
}

// ----------------------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------------------

/// The per-column random roll that decides whether (and how tall) a tree grows there.
fn tree_roll(seed: u64, face: Face, gu: i32, gv: i32) -> u64 {
    hash3(
        seed ^ 0x7ee5_7ee5,
        face.0 as i64 * 1_000_003 + gu as i64,
        gv as i64,
        7,
    )
}

fn tree_height(kind: TreeKind, r: u64) -> i32 {
    match kind {
        TreeKind::Oak => 4 + (r % 3) as i32,
        TreeKind::Acacia => 4 + (r % 2) as i32,
        TreeKind::Spruce => 6 + (r % 4) as i32,
        TreeKind::Jungle => 9 + (r % 5) as i32,
        TreeKind::Cactus => 2 + (r % 3) as i32,
    }
}

fn tree_for(col: &Column, sea: i32) -> Option<(TreeKind, f64)> {
    if col.steep {
        return None;
    }
    tree_kind(col.biome, col.temperature, col.height, sea)
}

fn tree_kind(biome: Biome, temperature: f32, height: i32, sea: i32) -> Option<(TreeKind, f64)> {
    if height <= sea {
        return None;
    }
    let warm_enough = temperature >= 0.1;
    Some(match biome {
        Biome::Forest => (TreeKind::Oak, 0.045),
        Biome::Plains => (TreeKind::Oak, 0.003),
        Biome::Taiga => (TreeKind::Spruce, 0.035),
        Biome::Tundra => (TreeKind::Spruce, 0.002),
        Biome::Jungle => (TreeKind::Jungle, 0.05),
        Biome::Savanna => (TreeKind::Acacia, 0.006),
        Biome::Desert => (TreeKind::Cactus, 0.004),
        Biome::Mountains if warm_enough => (TreeKind::Spruce, 0.01),
        _ => return None,
    })
}

fn terracotta_band(k: i32) -> Block {
    use Block::*;
    const BANDS: [Block; 16] = [
        TerracottaOrange,
        TerracottaOrange,
        TerracottaRed,
        TerracottaYellow,
        TerracottaOrange,
        TerracottaWhite,
        TerracottaBrown,
        TerracottaOrange,
        TerracottaRed,
        TerracottaRed,
        TerracottaYellow,
        TerracottaOrange,
        TerracottaWhite,
        TerracottaOrange,
        TerracottaBrown,
        TerracottaRed,
    ];
    BANDS[(k.div_euclid(2)).rem_euclid(16) as usize]
}

/// Piecewise-linear curve through `points` (sorted by x).
fn spline(x: f64, points: &[(f64, f64)]) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return lerp(y0, y1, (x - x0) / (x1 - x0));
        }
    }
    points[points.len() - 1].1
}

/// Stair-steps a height: mostly flat treads with steep risers (cliffs).
/// `sharpness` in 0..1 - higher means steeper risers.
fn terrace(h: f64, step: f64, sharpness: f64) -> f64 {
    let x = h / step;
    let k = x.floor();
    let f = x - k;
    let rise = ((f - sharpness) / (1.0 - sharpness)).clamp(0.0, 1.0);
    (k + rise * rise * (3.0 - 2.0 * rise)) * step
}

/// Biome grass colour (linear RGB) from temperature and humidity.
pub fn grass_color(temperature: f64, humidity: f64) -> [f64; 3] {
    let t = ((temperature - 0.1) / 0.8).clamp(0.0, 1.0);
    let h = humidity.clamp(0.0, 1.0);
    let mix = |a: [f64; 3], b: [f64; 3], t: f64| std::array::from_fn(|i| lerp(a[i], b[i], t));
    let cold = mix([0.20, 0.42, 0.16], [0.10, 0.36, 0.14], h);
    let hot = mix([0.50, 0.52, 0.12], [0.06, 0.52, 0.04], h);
    mix(cold, hot, t)
}

/// Ocean colour (linear RGB): turquoise in the tropics, dark blue near the poles.
pub fn water_color(temperature: f64) -> [f64; 3] {
    let t = temperature.clamp(0.0, 1.0);
    let cold = [0.05, 0.12, 0.32];
    let warm = [0.06, 0.38, 0.55];
    std::array::from_fn(|i| lerp(cold[i], warm[i], t))
}

pub fn pack_rgb(c: [f64; 3]) -> u32 {
    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (255 << 24)
}

pub const WHITE: u32 = 0xffff_ffff;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The player spawns where blocks are closest to 1 m cubes.
    #[test]
    fn spawn_blocks_are_square() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let generator = TerrainGenerator::new(planet.clone());
        let spawn = generator.find_spawn();
        let s = generator.sample_column(spawn);
        assert!((4.0..40.0).contains(&s.height), "spawn height {}", s.height);
        let (wu, wv, angle) = planet.block_shape(spawn);
        println!("spawn {spawn:?}: {wu:.3} x {wv:.3} m, {angle:.2} deg");
        let (face, u, v, _) = planet.locate(spawn * planet.sea_radius);
        assert!(
            generator.tree_at(face, u as i32, v as i32).is_none(),
            "spawned in a tree"
        );
        assert!((wu - 1.0).abs() < 0.05 && (wv - 1.0).abs() < 0.05);
        assert!((angle - 90.0).abs() < 2.0);
    }

    /// Every surface block that is visibly off (skewed more than 5 degrees, or more than 20%
    /// narrower or wider than 1 m) lies under a corner sea.
    #[test]
    fn distorted_blocks_are_under_the_corner_seas() {
        use crate::world::planet::BLOCK_WIDTH;
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let generator = TerrainGenerator::new(planet.clone());
        let (mut bad, mut uncovered, mut worst) = (0, 0, 0f64);
        let mut sea = 0.0;
        let count = 200_000;
        for dir in fibonacci_sphere(count) {
            let cover = generator.corner_sea(dir);
            sea += cover;
            let (wu, wv, angle) = planet.block_shape(dir);
            let off = (angle - 90.0).abs() > 5.0
                || [wu, wv]
                    .iter()
                    .any(|w| !(0.8..1.2).contains(&(w / BLOCK_WIDTH)));
            if off {
                bad += 1;
                let corner = ((dir.x.abs() + dir.y.abs() + dir.z.abs()) / 3f64.sqrt())
                    .min(1.0)
                    .acos()
                    .to_degrees();
                worst = worst.max(corner);
                if cover < 0.5 {
                    uncovered += 1;
                }
            }
        }
        println!(
            "distorted blocks: {:.1}% of the surface, up to {worst:.1} deg from a corner; corner seas cover {:.1}%",
            bad as f64 / count as f64 * 100.0,
            sea / count as f64 * 100.0
        );
        assert_eq!(uncovered, 0, "{uncovered} distorted blocks on land");
    }

    /// Volcanoes: a handful, each a real cone with a lava lake in its crater, deterministic.
    #[test]
    fn volcanoes_have_cones_and_lava() {
        let make = || {
            let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
            TerrainGenerator::new(planet)
        };
        let (generator, again) = (make(), make());
        let n = generator.volcanoes.len();
        assert!((4..=MAX_VOLCANOES).contains(&n), "{n} volcanoes");
        assert_eq!(n, again.volcanoes.len());
        for (v, w) in generator.volcanoes.iter().zip(&again.volcanoes) {
            assert_eq!(v.dir, w.dir);
            let summit = generator.sample_column(v.dir);
            assert_eq!(summit.biome, Biome::Caldera, "{v:?}");
            // Beside the summit: the crater's rim, well above the land around the foot.
            let side = v.dir.cross(DVec3::Y).normalize();
            let at =
                |metres: f64| (v.dir + side * (metres / generator.planet.sea_radius)).normalize();
            let rim = generator.sample_column(at(v.crater + 5.0));
            let foot = generator.sample_column(at(v.radius + 50.0));
            assert!(
                rim.height > foot.height + 150.0,
                "{} vs {}",
                rim.height,
                foot.height
            );
            assert!(rim.height > summit.height, "the lake lies below the rim");
        }
        let lava = generator.terrain_block(
            &Column {
                height: 100,
                biome: Biome::Caldera,
                steep: false,
                temperature: 0.5,
                ilmenite: false,
                strata: 0,
                grass_tint: 0,
                foliage_tint: 0,
                water_tint: 0,
            },
            100,
        );
        assert_eq!(lava, Block::Lava);
    }

    /// The moon: only lunar biomes, real craters (a spread of heights), no water and no
    /// trees, regolith over rock.
    #[test]
    fn moon_is_grey_dry_and_cratered() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 512, 512, 7));
        let moon = TerrainGenerator::moon(planet.clone());
        let (mut lo, mut hi, mut mare) = (f64::MAX, f64::MIN, 0);
        let mut low_dir = DVec3::X;
        for dir in fibonacci_sphere(3000) {
            let s = moon.sample_column(dir);
            assert!(matches!(s.biome, Biome::Highlands | Biome::Mare));
            mare += (s.biome == Biome::Mare) as i32;
            if s.height < lo {
                (lo, low_dir) = (s.height, dir);
            }
            hi = hi.max(s.height);
        }
        assert!(hi - lo > 150.0, "heights {lo}..{hi}");
        assert!((300..2700).contains(&mare), "{mare} mare columns of 3000");
        // The lowest spot is well below the datum: still no water there.
        let (face, u, v, _) = planet.locate(low_dir * planet.sea_radius);
        let (cu, cv) = (u as i32 / CHUNK_SIZE, v as i32 / CHUNK_SIZE);
        let cols = moon.build_columns(face, cu, cv);
        assert!(cols.trees.is_empty());
        let col = *cols.get(u as i32 % CHUNK_SIZE, v as i32 % CHUNK_SIZE);
        let key = ChunkKey {
            face,
            cu,
            cv,
            ck: col.height.div_euclid(CHUNK_SIZE),
        };
        let mut blocks = PaddedBlocks::new();
        moon.fill_terrain(key, &cols, &mut blocks);
        let mut seen = Vec::new();
        for k in 0..CHUNK_SIZE {
            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    let b = blocks.get(u, v, k);
                    assert!(!matches!(
                        b,
                        Block::Water | Block::Ice | Block::Grass | Block::Dirt
                    ));
                    if !seen.contains(&b) {
                        seen.push(b);
                    }
                }
            }
        }
        assert!(seen.contains(&Block::Regolith), "{seen:?}");
    }

    #[test]
    fn oil_shale_lies_under_dry_plains_and_changes_no_other_ore() {
        let rolls = (0..1000).map(|i| i as f64 / 1000.0);
        let picks = |biome: Biome, depth: i32| -> Vec<Option<Block>> {
            rolls
                .clone()
                .map(|r| choose_ore(r, depth, biome, |_| 0.0).map(Ore::block))
                .collect()
        };
        let oil = |p: &[Option<Block>]| p.iter().filter(|b| **b == Some(Block::OilShale)).count();
        let desert = picks(Biome::Desert, 30);
        assert!(oil(&desert) > 50, "{} cells of 1000", oil(&desert));
        assert_eq!(oil(&picks(Biome::Forest, 30)), 0);
        assert_eq!(oil(&picks(Biome::Desert, 300)), 0);
        // Oil only takes rolls above all the others' (ones that used to give nothing), so every
        // cell that held another ore still does.
        let last_other = desert
            .iter()
            .rposition(|b| b.is_some_and(|b| b != Block::OilShale));
        let first_oil = desert.iter().position(|b| *b == Some(Block::OilShale));
        assert!(last_other < first_oil, "{last_other:?} {first_oil:?}");
    }

    /// The LOD system draws trees from `tree_at`; they must be exactly the chunk's trees.
    #[test]
    fn tree_at_matches_chunk_trees() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let generator = TerrainGenerator::new(planet.clone());
        let mut total = 0;
        // Forest and jungle (see `locate_biomes`).
        for (lat, lon) in [(-60.0f64, 7.0f64), (-45.0, 122.0)] {
            let (la, lo) = (lat.to_radians(), lon.to_radians());
            let dir = DVec3::new(la.cos() * lo.cos(), la.sin(), la.cos() * lo.sin());
            let (face, u, v, _) = planet.locate(dir * planet.sea_radius);
            for dc in 0..4 {
                let (cu, cv) = (
                    u as i32 / CHUNK_SIZE + dc % 2,
                    v as i32 / CHUNK_SIZE + dc / 2,
                );
                let cols = generator.build_columns(face, cu, cv);
                let chunk_trees: Vec<_> = cols
                    .trees
                    .iter()
                    .filter(|t| (0..CHUNK_SIZE).contains(&t.u) && (0..CHUNK_SIZE).contains(&t.v))
                    .map(|t| (t.u, t.v, t.base, t.height, t.kind))
                    .collect();
                let mut lod_trees = Vec::new();
                for v in 0..CHUNK_SIZE {
                    for u in 0..CHUNK_SIZE {
                        let (gu, gv) = (cu * CHUNK_SIZE + u, cv * CHUNK_SIZE + v);
                        if let Some(t) = generator.tree_at(face, gu, gv) {
                            lod_trees.push((u, v, t.base, t.height, t.kind));
                        }
                    }
                }
                let mut chunk_trees = chunk_trees;
                chunk_trees.sort_by_key(|t| (t.1, t.0));
                lod_trees.sort_by_key(|t| (t.1, t.0));
                assert_eq!(chunk_trees, lod_trees, "chunk column {cu},{cv}");
                total += lod_trees.len();
            }
        }
        assert!(total > 20, "only {total} trees checked");
    }

    /// Prints the biome mix over the whole planet - handy when tuning.
    /// Run with `cargo test biome_distribution -- --nocapture`.
    #[test]
    fn biome_distribution() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed));
        let generator = TerrainGenerator::new(planet);
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let n = 20_000;
        for i in 0..n {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - y * y).sqrt();
            let a = 2.399963 * i as f64;
            let s = generator.sample_column(DVec3::new(r * a.cos(), y, r * a.sin()));
            *counts.entry(s.biome.name()).or_default() += 1;
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        for (name, c) in counts {
            println!("{name:>14}: {:5.1}%", c as f64 * 100.0 / n as f64);
        }
    }
}

#[cfg(test)]
mod locate_biomes {
    use super::*;
    use std::collections::HashMap;

    /// Prints a location (lat, lon, height) for each biome - useful with ASTRO_SPAWN.
    /// Run with `cargo test --release locate_biomes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn locate_biomes() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        let generator = TerrainGenerator::new(planet);
        // name -> (lat, lon, height, score)
        let mut found: HashMap<&str, (f64, f64, f64, f64)> = HashMap::new();
        for lat in (-85..=85).step_by(1) {
            for lon in (-180..180).step_by(1) {
                let (la, lo) = ((lat as f64).to_radians(), (lon as f64).to_radians());
                let dir = DVec3::new(la.cos() * lo.cos(), la.sin(), la.cos() * lo.sin());
                let s = generator.sample_column(dir);
                // Prefer the most "typical" sample: the tallest mountain, the warmest ocean...
                let score = match s.biome {
                    Biome::Mountains => s.height,
                    Biome::Ocean | Biome::DeepOcean => s.temperature,
                    _ => 0.0,
                };
                let e = found
                    .entry(s.biome.name())
                    .or_insert((lat as f64, lon as f64, s.height, score));
                if score > e.3 {
                    *e = (lat as f64, lon as f64, s.height, score);
                }
            }
        }
        let mut found: Vec<_> = found.into_iter().collect();
        found.sort_by_key(|f| f.0);
        for (name, (lat, lon, h, _)) in found {
            println!("{name:>14}: ASTRO_SPAWN={lat},{lon}   (height {h:.0} m)");
        }
    }
}
