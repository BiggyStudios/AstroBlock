//! The planet-wide weather simulation: a few fields on a cube-sphere grid that the wind
//! carries around. It isn't meteorology, it is a small set of rules chosen to give the right
//! *behaviour* cheaply:
//!
//! - **Wind**: bands by latitude (trades, westerlies, polar easterlies), slowly changing
//!   eddies (the curl of a noise field), and drifting cyclones with spiral arms.
//! - **Moisture**: water vapour evaporates off the sea and wet land. Air that rises (into a
//!   cyclone, up a mountain, at the equator's convergence, in the eddies) condenses it into
//!   cloud; sinking air clears it. Warm air holds more, so deserts and poles differ.
//! - **Rain or snow** falls out of thick cloud, depending on the temperature. Warm moist air
//!   under thick cloud builds **storms** (tall towers, lightning, sometimes a tornado).
//! - **Dust** is lifted off deserts by strong wind; volcanoes put out **ash**.
//!
//! Advection is semi-Lagrangian with cubic interpolation (bilinear smears storms away).
//! Everything here runs on the weather worker thread (see `weather`), in game time.

use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{DVec3, Vec3};

use crate::world::{
    noise::Noise,
    terrain::{Biome, TerrainGenerator},
};

/// Cells per cube face edge (about 540 m each at the face centres).
pub const N: usize = 128;
pub const CELLS: usize = 6 * N * N;

/// Cyclones alive at once, tornadoes planet-wide.
const MAX_SYSTEMS: usize = 16;
const MAX_TORNADOES: usize = 4;
/// Near-surface wind relative to the wind at cloud level (friction).
pub const SURFACE_WIND: f32 = 0.38;
/// Peak speed of the wind bands (m/s, at cloud level).
const BAND_WIND: f32 = 26.0;

pub fn cell_index(face: usize, i: usize, j: usize) -> usize {
    (face * N + j) * N + i
}

/// The direction of a point on a cube face (`s`, `t` in grid units, texel centres at +0.5),
/// following Vulkan's cube map layout, so a cell of the grid is a texel of the weather maps.
pub fn grid_dir(face: usize, s: f32, t: f32) -> Vec3 {
    let (a, b) = (2.0 * s / N as f32 - 1.0, 2.0 * t / N as f32 - 1.0);
    match face {
        0 => Vec3::new(1.0, -b, -a),
        1 => Vec3::new(-1.0, -b, a),
        2 => Vec3::new(a, 1.0, b),
        3 => Vec3::new(a, -1.0, -b),
        4 => Vec3::new(a, -b, 1.0),
        _ => Vec3::new(-a, -b, -1.0),
    }
    .normalize()
}

/// The face and grid coordinates (see `grid_dir`) of a direction.
pub fn locate(d: Vec3) -> (usize, f32, f32) {
    let a = d.abs();
    let (face, sc, tc, ma) = if a.x >= a.y && a.x >= a.z {
        if d.x > 0.0 {
            (0, -d.z, -d.y, a.x)
        } else {
            (1, d.z, -d.y, a.x)
        }
    } else if a.y >= a.z {
        if d.y > 0.0 {
            (2, d.x, d.z, a.y)
        } else {
            (3, d.x, -d.z, a.y)
        }
    } else if d.z > 0.0 {
        (4, d.x, -d.y, a.z)
    } else {
        (5, -d.x, -d.y, a.z)
    };
    let n = N as f32;
    (face, (sc / ma + 1.0) * 0.5 * n, (tc / ma + 1.0) * 0.5 * n)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Undoes `smoothstep(0, 1, x)`.
fn unsmooth(y: f32) -> f32 {
    0.5 - ((1.0 - 2.0 * y.clamp(0.0, 1.0)).asin() / 3.0).sin()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// What never changes about a cell.
#[derive(Clone, Copy)]
struct Site {
    dir: Vec3,
    east: Vec3,
    north: Vec3,
    /// Sine-free latitude (radians, signed).
    latitude: f32,
    land: bool,
    height: f32,
    /// Climate temperature (see `ColumnSample::temperature`) and how wet the land is.
    temperature: f32,
    moisture: f32,
    /// 1 over desert sand.
    desert: f32,
    /// Terrain slope (rise per metre, tangent, world axes).
    slope: Vec3,
}

/// Builds the grid's climate from the terrain (the columns are expensive, so several threads).
fn build_sites(terrain: &TerrainGenerator) -> Vec<Site> {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 8);
    let radius = terrain.planet.sea_radius as f32;
    let rows = 6 * N;
    let mut sites = Vec::with_capacity(CELLS);
    let parts: Vec<Vec<(f32, Biome, f32, f32)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|part| {
                scope.spawn(move || {
                    let (from, to) = (rows * part / threads, rows * (part + 1) / threads);
                    let mut out = Vec::with_capacity((to - from) * N);
                    for row in from..to {
                        let (face, j) = (row / N, row % N);
                        for i in 0..N {
                            let d = grid_dir(face, i as f32 + 0.5, j as f32 + 0.5).as_dvec3();
                            let s = terrain.sample_column(d);
                            out.push((
                                s.height as f32,
                                s.biome,
                                s.temperature as f32,
                                s.humidity as f32,
                            ));
                        }
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (index, (height, biome, temperature, moisture)) in parts.into_iter().flatten().enumerate() {
        let (face, j, i) = (index / (N * N), index / N % N, index % N);
        let dir = grid_dir(face, i as f32 + 0.5, j as f32 + 0.5);
        let east = Vec3::Y.cross(dir).try_normalize().unwrap_or(Vec3::X);
        sites.push(Site {
            dir,
            east,
            north: dir.cross(east),
            latitude: dir.y.clamp(-1.0, 1.0).asin(),
            land: height > 0.0,
            height: height.max(0.0),
            temperature,
            moisture,
            desert: matches!(biome, Biome::Desert | Biome::Badlands) as u8 as f32,
            slope: Vec3::ZERO,
        });
    }
    // Slopes, by differences to the neighbours on the same face.
    let cell = radius * TAU / 4.0 / N as f32;
    let heights: Vec<f32> = sites.iter().map(|s| s.height).collect();
    for face in 0..6 {
        for j in 0..N {
            for i in 0..N {
                let at = |i: usize, j: usize| heights[cell_index(face, i, j)];
                let (i0, i1) = (i.saturating_sub(1), (i + 1).min(N - 1));
                let (j0, j1) = (j.saturating_sub(1), (j + 1).min(N - 1));
                let dh_s = (at(i1, j) - at(i0, j)) / (cell * (i1 - i0) as f32);
                let dh_t = (at(i, j1) - at(i, j0)) / (cell * (j1 - j0) as f32);
                let d = sites[cell_index(face, i, j)].dir;
                let along_s = (grid_dir(face, i as f32 + 1.5, j as f32 + 0.5) - d).normalize();
                let along_t = (grid_dir(face, i as f32 + 0.5, j as f32 + 1.5) - d).normalize();
                sites[cell_index(face, i, j)].slope = along_s * dh_s + along_t * dh_t;
            }
        }
    }
    sites
}

/// A large rotating low-pressure system.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct System {
    dir: Vec3,
    /// Radius of its eye wall region (m) and how strong it is now (0..1).
    radius: f32,
    strength: f32,
    age: f32,
    life: f32,
}

/// A tornado on the ground.
#[derive(Clone, Copy, Debug)]
pub struct Tornado {
    /// Unit direction (body frame) and tangent velocity (m/s).
    pub dir: Vec3,
    pub velocity: Vec3,
    /// Core radius (m) and how formed it is (0..1, grows and fades).
    pub radius: f32,
    pub strength: f32,
    age: f32,
    life: f32,
}

impl Tornado {
    /// A tornado as a client hears of it (only the host's simulation ages them).
    pub fn seen(dir: Vec3, velocity: Vec3, radius: f32, strength: f32) -> Self {
        Self {
            dir,
            velocity,
            radius,
            strength,
            age: 0.0,
            life: 1.0,
        }
    }
}

/// One volcano's eruption schedule.
#[derive(Clone, Copy, Debug)]
pub struct VolcanoState {
    pub dir: Vec3,
    /// The rim's height above sea level, and the crater floor's (m).
    pub summit: f32,
    pub vent: f32,
    /// When this eruption began.
    pub started: f64,
    /// Game time the next eruption starts and the current one ends.
    next: f64,
    until: f64,
    /// 0..1 how hard it's erupting now.
    pub activity: f32,
}

impl VolcanoState {
    /// A volcano as a client hears of it.
    pub fn seen(dir: Vec3, summit: f32, vent: f32, started: f64, activity: f32) -> Self {
        Self {
            dir,
            summit,
            vent,
            started,
            next: 0.0,
            until: 0.0,
            activity,
        }
    }
}

/// What can be imposed around a point (the `weather` console command).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Force {
    Clear,
    Fair,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Storm,
    Snow,
    Blizzard,
    Sandstorm,
    Tornado,
}

/// Everything the renderer and the game need from one cell, 0..1.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub cover: f32,
    pub top: f32,
    pub precip: f32,
    pub storm: f32,
    pub dust: f32,
    pub snow: f32,
    pub fog: f32,
    pub ash: f32,
}

pub struct Sim {
    sites: Vec<Site>,
    radius: f32,
    noise: Noise,
    rng: u64,
    steps: u64,
    q: Vec<f32>,
    cloud: Vec<f32>,
    storm: Vec<f32>,
    dust: Vec<f32>,
    ash: Vec<f32>,
    /// Advection's write buffers.
    scratch: [Vec<f32>; 5],
    pub precip: Vec<f32>,
    fog: Vec<f32>,
    snow: Vec<f32>,
    pub wind: Vec<Vec3>,
    lift: Vec<f32>,
    eddy: Vec<Vec3>,
    eddy_lift: Vec<f32>,
    pub systems: Vec<System>,
    pub tornadoes: Vec<Tornado>,
    pub volcanoes: Vec<VolcanoState>,
    forced: Option<(Vec3, Force)>,
}

/// Radius (m) of what `Force` paints.
const FORCE_RADIUS: f32 = 9_000.0;

impl Sim {
    pub fn new(terrain: &TerrainGenerator) -> Self {
        let sites = build_sites(terrain);
        let seed = terrain.planet.seed ^ 0x57_0a4d;
        let noise = Noise::new(seed);
        let q = sites
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = noise.fbm((s.dir * 5.0).as_dvec3() + DVec3::splat(i as f64 * 1e-7), 2);
                saturation(s.temperature) * (0.85 + 0.25 * n as f32)
            })
            .collect();
        let zero = || vec![0.0f32; CELLS];
        let mut volcano_rng = seed ^ 0xa5;
        let volcanoes = terrain
            .volcanoes
            .iter()
            .map(|v| {
                volcano_rng = mix(volcano_rng);
                VolcanoState {
                    dir: v.dir.as_vec3(),
                    summit: (v.base + v.height) as f32,
                    vent: (v.base + v.height * 0.62) as f32,
                    started: 0.0,
                    next: unit_f64(volcano_rng) * 1_200.0 + 90.0,
                    until: 0.0,
                    activity: 0.0,
                }
            })
            .collect();
        Self {
            sites,
            radius: terrain.planet.sea_radius as f32,
            noise,
            rng: seed | 1,
            steps: 0,
            q,
            cloud: zero(),
            storm: zero(),
            dust: zero(),
            ash: zero(),
            scratch: std::array::from_fn(|_| zero()),
            precip: zero(),
            fog: zero(),
            snow: zero(),
            wind: vec![Vec3::ZERO; CELLS],
            lift: zero(),
            eddy: vec![Vec3::ZERO; CELLS],
            eddy_lift: zero(),
            systems: Vec::new(),
            tornadoes: Vec::new(),
            volcanoes,
            forced: None,
        }
    }

    fn random(&mut self) -> f32 {
        self.rng = mix(self.rng);
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Imposes `force` around `dir` until `release`.
    pub fn force(&mut self, dir: Vec3, force: Force) {
        self.forced = Some((dir, force));
        if force == Force::Tornado {
            // Close, and coming this way.
            let side = Vec3::Y.cross(dir).try_normalize().unwrap_or(Vec3::X);
            let at = (dir + side * (420.0 / self.radius)).normalize();
            self.tornadoes.truncate(MAX_TORNADOES - 1);
            self.tornadoes.push(Tornado {
                dir: at,
                velocity: (dir - at).normalize() * 7.0,
                radius: 24.0,
                strength: 0.2,
                age: 0.0,
                life: 240.0,
            });
        }
    }

    pub fn release(&mut self) {
        self.forced = None;
    }

    /// Starts the volcano nearest to `dir` erupting; returns how far away it is (radians).
    pub fn erupt_nearest(&mut self, dir: Vec3, now: f64) -> Option<f32> {
        let v = self
            .volcanoes
            .iter_mut()
            .max_by(|a, b| a.dir.dot(dir).total_cmp(&b.dir.dot(dir)))?;
        v.next = now;
        v.until = 0.0;
        Some(v.dir.dot(dir).clamp(-1.0, 1.0).acos())
    }

    /// The world is about to start: nothing is erupting, and the first eruptions come soon
    /// (the spin-up mustn't use them up, or leave ash over everything).
    pub fn settle_volcanoes(&mut self, now: f64) {
        for i in 0..self.volcanoes.len() {
            let roll = self.random() as f64;
            let v = &mut self.volcanoes[i];
            v.next = now + 90.0 + 1_200.0 * roll;
            v.until = 0.0;
            v.activity = 0.0;
        }
        self.ash.fill(0.0);
    }

    /// Carries on from weather known only by its two maps (`Cell`s as bytes): the cells' own
    /// values are worked back from what they looked like. Tornadoes start over.
    pub fn import(&mut self, a: &[u8], b: &[u8], systems: &[System], sun: Vec3, now: f64) {
        self.settle_volcanoes(now);
        self.tornadoes.clear();
        self.systems = systems
            .iter()
            .filter(|s| s.dir.is_finite() && s.radius.is_finite() && s.radius > 1.0)
            .filter(|s| s.age.is_finite() && s.life.is_finite() && s.strength.is_finite())
            .take(MAX_SYSTEMS)
            .copied()
            .collect();
        self.update_eddies(now);
        self.update_wind();
        for (i, s) in self.sites.iter().enumerate() {
            let cover = a[i * 4] as f32 / 255.0;
            self.cloud[i] = 0.12 + 0.83 * unsmooth(cover);
            // Vapour isn't on the maps. Enough for what is there to last: the clouds shift a
            // little at first, which goes unnoticed in a sky that is being warped through.
            let qs = saturation(temperature(s, sun));
            let threshold = condensing_at(qs, self.lift[i]);
            self.q[i] = if cover > 0.02 {
                threshold
            } else {
                threshold * 0.9
            };
            self.precip[i] = a[i * 4 + 2] as f32 / 255.0;
            self.storm[i] = a[i * 4 + 3] as f32 / 255.0;
            self.dust[i] = b[i * 4] as f32 / 255.0;
            self.snow[i] = b[i * 4 + 1] as f32 / 255.0;
            self.fog[i] = b[i * 4 + 2] as f32 / 255.0;
            self.ash[i] = b[i * 4 + 3] as f32 / 255.0;
        }
    }

    /// Advances the weather by `dt` game seconds at time `now`; `sun` is the direction of the
    /// sun in the body frame.
    pub fn step(&mut self, dt: f32, now: f64, sun: Vec3) {
        if self.steps.is_multiple_of(16) {
            self.update_eddies(now);
        }
        self.steps += 1;
        self.update_systems(dt, now);
        self.update_wind();
        self.advect(dt);
        self.physics(dt, sun);
        self.update_tornadoes(dt);
        self.update_volcanoes(dt, now);
        if let Some((dir, force)) = self.forced {
            self.paint(dir, force);
        }
    }

    /// Slowly changing eddies: the curl of a noise field, plus patches of rising air.
    fn update_eddies(&mut self, now: f64) {
        let drift = DVec3::new(0.0, now / 2_400.0, now / 3_100.0);
        let noise = &self.noise;
        for (i, s) in self.sites.iter().enumerate() {
            let p = s.dir.as_dvec3() * 4.5 + drift;
            let e = 0.02;
            let psi = |p: DVec3| noise.fbm(p, 2) as f32;
            let c = psi(p);
            let de = (psi(p + s.east.as_dvec3() * e) - c) / e as f32;
            let dn = (psi(p + s.north.as_dvec3() * e) - c) / e as f32;
            self.eddy[i] = (s.east * dn - s.north * de) * 9.0;
            let q = s.dir.as_dvec3() * 7.5 - drift * 1.7 + DVec3::splat(31.7);
            self.eddy_lift[i] = noise.fbm(q, 3) as f32 * 1.5;
        }
    }

    fn update_systems(&mut self, dt: f32, now: f64) {
        // New ones form over warm water (or anywhere, less often).
        if self.systems.len() < MAX_SYSTEMS && self.random() < 1.0 - (-dt / 45.0).exp() {
            for _ in 0..24 {
                let i = (self.random() * CELLS as f32) as usize % CELLS;
                let s = self.sites[i];
                if s.land && self.random() > 0.25 {
                    continue;
                }
                let radius = 3_500.0 + 6_500.0 * self.random();
                let life = 700.0 + 1_500.0 * self.random();
                self.systems.push(System {
                    dir: s.dir,
                    radius,
                    strength: 0.0,
                    age: 0.0,
                    life,
                });
                break;
            }
        }
        let radius = self.radius;
        for sys in &mut self.systems {
            sys.age += dt;
            sys.strength = smoothstep(0.0, 0.2, sys.age / sys.life)
                * smoothstep(1.0, 0.75, sys.age / sys.life);
            // Steered by the bands, curving polewards.
            let east = Vec3::Y.cross(sys.dir).try_normalize().unwrap_or(Vec3::X);
            let north = sys.dir.cross(east);
            let latitude = sys.dir.y.clamp(-1.0, 1.0).asin();
            let velocity = east * band_u(latitude) * 0.7
                + north * (latitude.signum() * 3.0 + band_v(latitude));
            sys.dir = (sys.dir + velocity * dt / radius).normalize();
        }
        self.systems.retain(|s| s.age < s.life);
        let _ = now;
    }

    /// The wind at cloud level and how much the air rises, per cell.
    fn update_wind(&mut self) {
        let radius = self.radius;
        for (i, s) in self.sites.iter().enumerate() {
            let mut wind =
                s.east * band_u(s.latitude) + s.north * band_v(s.latitude) + self.eddy[i];
            // The equator and 60 degrees converge; 30 degrees (the deserts) sinks.
            let mut lift = 0.2 * (6.0 * s.latitude).cos() + self.eddy_lift[i];
            for sys in &self.systems {
                let c = s.dir.dot(sys.dir);
                let reach = sys.radius * 3.0 / radius;
                if c < 1.0 - reach * reach * 0.5 {
                    continue;
                }
                let to_center = sys.dir - s.dir * c;
                let distance = to_center.length() * radius;
                let d = distance / sys.radius;
                if d < 1e-3 {
                    continue;
                }
                let inward = to_center.normalize();
                // Spins with the planet's rotation sense in each hemisphere.
                let tangent = s.dir.cross(inward) * sys.dir.y.signum();
                let swirl = d * (1.0 - d).exp() * 38.0 * sys.strength;
                wind += tangent * swirl + inward * swirl * 0.28;
                // Spiral rain bands around a clear eye.
                let e1 = sys.dir.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
                let e2 = sys.dir.cross(e1);
                let theta = inward.dot(e2).atan2(inward.dot(e1));
                let arms = 0.55 + 0.45 * (2.0 * theta + d.max(0.2).ln() * 3.2).cos();
                let eye = smoothstep(0.1, 0.32, d);
                lift +=
                    (1.05 * (-d * d * 0.4).exp() * arms * eye - 0.35 * (1.0 - eye)) * sys.strength;
            }
            // Up the windward side of mountains.
            let up_slope = (wind * SURFACE_WIND).dot(s.slope);
            lift += (up_slope * 1.6).clamp(0.0, 0.6);
            self.wind[i] = wind;
            self.lift[i] = lift;
        }
    }

    /// Carries the fields along the wind.
    fn advect(&mut self, dt: f32) {
        let k = dt / self.radius;
        let [out_q, out_cloud, out_storm, out_dust, out_ash] = &mut self.scratch;
        let fields: [(&[f32], &mut Vec<f32>); 5] = [
            (&self.q, out_q),
            (&self.cloud, out_cloud),
            (&self.storm, out_storm),
            (&self.dust, out_dust),
            (&self.ash, out_ash),
        ];
        // One field per thread: they don't depend on each other.
        let (sites, wind) = (&self.sites, &self.wind);
        std::thread::scope(|scope| {
            for (from, to) in fields {
                scope.spawn(move || {
                    for (i, s) in sites.iter().enumerate() {
                        let back = (s.dir - wind[i] * k).normalize();
                        to[i] = sample_cubic(from, back);
                    }
                });
            }
        });
        std::mem::swap(&mut self.q, &mut self.scratch[0]);
        std::mem::swap(&mut self.cloud, &mut self.scratch[1]);
        std::mem::swap(&mut self.storm, &mut self.scratch[2]);
        std::mem::swap(&mut self.dust, &mut self.scratch[3]);
        std::mem::swap(&mut self.ash, &mut self.scratch[4]);
    }

    fn physics(&mut self, dt: f32, sun: Vec3) {
        let rate = |tau: f32| 1.0 - (-dt / tau).exp();
        let (evaporate_sea, evaporate_land) = (rate(240.0), rate(520.0));
        let (condense, dissolve) = (rate(70.0), rate(130.0));
        let (rain_out, storm_up, storm_down) = (rate(380.0), rate(90.0), rate(170.0));
        let (dust_up, dust_down, wash) = (rate(70.0), rate(260.0), rate(22.0));
        let ash_down = rate(700.0);
        for (i, s) in self.sites.iter().enumerate() {
            let t = temperature(s, sun);
            let qs = saturation(t);
            let speed = (self.wind[i].length() * SURFACE_WIND).max(0.0);

            let (mut q, mut c, mut st) = (self.q[i], self.cloud[i], self.storm[i]);
            // Evaporation towards what the surface can supply.
            if s.land {
                q += (qs * (0.42 + 0.55 * s.moisture * (1.0 - s.desert * 0.8)) - q).max(0.0)
                    * evaporate_land;
            } else {
                q += (qs * 1.02 - q).max(0.0) * evaporate_sea;
            }
            // Rising air condenses, sinking air clears.
            let threshold = condensing_at(qs, self.lift[i]);
            let excess = q - threshold;
            if excess > 0.0 {
                let amount = excess * condense;
                q -= amount * 0.6;
                c += amount * 2.5;
            } else {
                let amount = c * dissolve * (-excess / qs.max(0.05) * 4.0).clamp(0.0, 1.0);
                c -= amount;
                q += amount * 0.15;
            }
            // Thick cloud rains out; more so when it's a storm.
            let precip = rainfall(c, st);
            c = (c - precip * rain_out).clamp(0.0, 1.25);

            // Warm, moist, rising air under thick cloud builds storms; sun on land helps.
            let solar = if s.land {
                sun.dot(s.dir).max(0.0) * 0.2
            } else {
                0.0
            };
            let unstable = (q / 0.9) * (t - 0.28).max(0.0) * 2.2 + solar;
            let target = smoothstep(0.82, 1.12, c) * smoothstep(0.5, 0.9, unstable);
            st += (target - st) * if target > st { storm_up } else { storm_down };

            // Dust off the deserts, in strong wind.
            let mut d = self.dust[i];
            d += s.desert * smoothstep(8.5, 13.5, speed) * dust_up * (1.0 - d);
            d -= d * dust_down + d * precip * wash;

            self.q[i] = q;
            self.cloud[i] = c;
            self.storm[i] = st;
            self.dust[i] = d;
            self.ash[i] -= self.ash[i] * (ash_down + precip * wash);
            self.precip[i] = precip;
            self.snow[i] = smoothstep(0.16, 0.05, t);
            // Fog: damp, still air near the ground, mostly at night.
            let night = smoothstep(0.25, -0.1, sun.dot(s.dir));
            self.fog[i] = smoothstep(0.82, 1.02, q / qs)
                * smoothstep(5.5, 2.0, speed)
                * (0.25 + 0.75 * night)
                * smoothstep(0.0, 0.1, t);
        }
    }

    fn update_tornadoes(&mut self, dt: f32) {
        // They come down out of strong storms over warm land.
        if self.tornadoes.len() < MAX_TORNADOES {
            for _ in 0..30 {
                let i = (self.random() * CELLS as f32) as usize % CELLS;
                let s = self.sites[i];
                if !s.land || self.storm[i] < 0.72 || s.temperature < 0.3 || s.height > 150.0 {
                    continue;
                }
                if self.random() < 1.0 - (-dt / 30.0).exp() {
                    let life = 90.0 + 130.0 * self.random();
                    let radius = 16.0 + 26.0 * self.random();
                    self.tornadoes.push(Tornado {
                        dir: s.dir,
                        velocity: Vec3::ZERO,
                        radius,
                        strength: 0.0,
                        age: 0.0,
                        life,
                    });
                }
                break;
            }
        }
        let radius = self.radius;
        let mut wander = [0.0f32; MAX_TORNADOES];
        for w in wander.iter_mut().take(self.tornadoes.len()) {
            *w = self.random() - 0.5;
        }
        for (t, w) in self.tornadoes.iter_mut().zip(wander) {
            t.age += dt;
            t.strength = smoothstep(0.0, 12.0, t.age) * smoothstep(t.life, t.life - 15.0, t.age);
            let (face, s, u) = locate(t.dir);
            let cell = cell_index(face, (s as usize).min(N - 1), (u as usize).min(N - 1));
            // Pushed along by the storm's wind, wandering a little.
            let steer = self.wind[cell] * 0.42;
            let side = t.dir.cross(steer).try_normalize().unwrap_or(Vec3::ZERO);
            let target = steer + side * w * 14.0;
            t.velocity += (target - t.velocity) * (1.0 - (-dt / 8.0).exp());
            t.velocity -= t.dir * t.velocity.dot(t.dir);
            t.dir = (t.dir + t.velocity * dt / radius).normalize();
        }
        self.tornadoes.retain(|t| t.age < t.life);
    }

    fn update_volcanoes(&mut self, dt: f32, now: f64) {
        let mut rolls = [0.0f32; 32];
        for r in rolls.iter_mut().take(self.volcanoes.len()) {
            *r = self.random();
        }
        for (v, roll) in self.volcanoes.iter_mut().zip(rolls) {
            if now >= v.next && now >= v.until {
                v.until = now + 200.0 + 240.0 * roll as f64;
                v.next = v.until + 900.0 + 2_100.0 * roll as f64;
                v.started = now;
            }
            let target = (now < v.until) as u8 as f32;
            // A sudden start, and a long dying down.
            let ease = if target > v.activity { 2.5 } else { 25.0 };
            v.activity += (target - v.activity) * (1.0 - (-dt / ease).exp());
            if v.activity > 0.02 {
                let (face, s, t) = locate(v.dir);
                for dj in -1i32..=1 {
                    for di in -1i32..=1 {
                        let i = (s as i32 + di).clamp(0, N as i32 - 1) as usize;
                        let j = (t as i32 + dj).clamp(0, N as i32 - 1) as usize;
                        let c = cell_index(face, i, j);
                        self.ash[c] = (self.ash[c] + v.activity * dt / 45.0).min(0.9);
                    }
                }
            }
        }
    }

    /// Imposes forced weather around a point: it moves the fields quickly towards the
    /// condition, so wind and physics can take over smoothly once released.
    fn paint(&mut self, dir: Vec3, force: Force) {
        use Force::*;
        // (cloud, storm, dust, q scale, fog, cold, wind)
        let (cloud, storm, dust, fog, cold, wind): (f32, f32, f32, f32, f32, f32) = match force {
            Clear => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            Fair => (0.5, 0.0, 0.0, 0.0, 0.0, 0.0),
            Cloudy => (0.75, 0.0, 0.0, 0.0, 0.0, 0.0),
            Fog => (0.15, 0.0, 0.0, 1.0, 0.0, 0.0),
            Drizzle => (0.85, 0.0, 0.0, 0.0, 0.0, 0.0),
            Rain => (0.98, 0.25, 0.0, 0.0, 0.0, 0.0),
            Storm | Tornado => (1.2, 1.0, 0.0, 0.0, 0.0, 12.0),
            Snow => (0.92, 0.1, 0.0, 0.0, 1.0, 0.0),
            Blizzard => (1.2, 0.5, 0.0, 0.0, 1.0, 62.0),
            Sandstorm => (0.15, 0.0, 1.0, 0.0, 0.0, 60.0),
        };
        for (i, s) in self.sites.iter().enumerate() {
            let distance = (s.dir - dir).length() * self.radius;
            if distance > FORCE_RADIUS {
                continue;
            }
            let w = smoothstep(FORCE_RADIUS, FORCE_RADIUS * 0.5, distance);
            self.cloud[i] = lerp(self.cloud[i], cloud, w);
            self.storm[i] = lerp(self.storm[i], storm, w);
            self.dust[i] = lerp(self.dust[i], dust, w);
            self.fog[i] = lerp(self.fog[i], fog, w);
            self.precip[i] = lerp(self.precip[i], rainfall(cloud, storm), w);
            self.snow[i] = lerp(
                self.snow[i],
                cold.max(self.snow[i] * (cold == 0.0) as u8 as f32),
                w,
            );
            if wind > 0.0 {
                let w2 = self.wind[i].length().max(1.0);
                self.wind[i] = self.wind[i] / w2 * lerp(w2, wind, w);
            }
        }
    }

    /// What the renderer and game see of a cell.
    pub fn cell(&self, i: usize) -> Cell {
        let c = self.cloud[i];
        let st = self.storm[i];
        Cell {
            cover: smoothstep(0.12, 0.95, c).max(st),
            top: (0.1 + 0.18 * c.min(1.0) + 0.72 * st * st).min(1.0),
            precip: self.precip[i],
            storm: st,
            dust: self.dust[i].clamp(0.0, 1.0),
            snow: self.snow[i],
            fog: self.fog[i],
            ash: self.ash[i].clamp(0.0, 1.0),
        }
    }

    /// How many cyclones are alive (for tests and the console).
    #[cfg(test)]
    pub fn system_count(&self) -> usize {
        self.systems.len()
    }
}

/// How hard it rains or snows out of cloud `c` with storm strength `st`.
fn rainfall(c: f32, st: f32) -> f32 {
    smoothstep(0.7, 1.05, c) * (0.3 + 0.7 * st)
}

/// East-west wind at a latitude (m/s): easterly trades, mid-latitude westerlies, polar
/// easterlies.
fn band_u(latitude: f32) -> f32 {
    -BAND_WIND * (3.0 * latitude).cos() * (1.0 - (latitude.abs() / FRAC_PI_2).powi(6))
}

/// North-south wind: towards the equator in the tropics, polewards around 45 degrees.
fn band_v(latitude: f32) -> f32 {
    -4.0 * (6.0 * latitude).sin()
}

/// How much vapour it takes to form cloud, for air holding `qs` at most and rising by `lift`.
fn condensing_at(qs: f32, lift: f32) -> f32 {
    qs * (1.0 - 0.5 * (lift - 0.24).clamp(-0.5, 1.0))
}

/// Water vapour air of temperature `t` holds at saturation.
fn saturation(t: f32) -> f32 {
    (0.24 + 0.82 * t.max(0.0)).clamp(0.2, 1.1)
}

/// A cell's temperature now: climate, season and the time of day.
fn temperature(s: &Site, sun: Vec3) -> f32 {
    let season = sun.y * s.dir.y * 0.45;
    let daily = sun.dot(s.dir).clamp(-0.3, 1.0) * if s.land { 0.12 } else { 0.03 };
    s.temperature + season + daily
}

fn mix(x: u64) -> u64 {
    let x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn unit_f64(x: u64) -> f64 {
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// A field at a direction: Catmull-Rom inside a face (clamped to the local range, so it can't
/// overshoot), bilinear next to face edges.
fn sample_cubic(field: &[f32], d: Vec3) -> f32 {
    let (face, s, t) = locate(d);
    let (x, y) = (s - 0.5, t - 0.5);
    let (i, j) = (x.floor(), y.floor());
    let (fx, fy) = (x - i, y - j);
    let (i, j) = (i as i32, j as i32);
    let n = N as i32;
    if i >= 1 && j >= 1 && i + 2 < n && j + 2 < n {
        let wx = catmull(fx);
        let wy = catmull(fy);
        let mut value = 0.0;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for b in 0..4 {
            let row = cell_index(face, 0, (j + b - 1) as usize) + (i - 1) as usize;
            for a in 0..4 {
                let v = field[row + a as usize];
                value += v * wx[a as usize] * wy[b as usize];
                if (1..=2).contains(&a) && (1..=2).contains(&b) {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
        value.clamp(lo, hi)
    } else {
        let at = |a: i32, b: i32| {
            field[cell_index(face, a.clamp(0, n - 1) as usize, b.clamp(0, n - 1) as usize)]
        };
        lerp(
            lerp(at(i, j), at(i + 1, j), fx),
            lerp(at(i, j + 1), at(i + 1, j + 1), fx),
            fy,
        )
    }
}

fn catmull(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_directions_round_trip() {
        for face in 0..6 {
            for &(s, t) in &[(0.5, 0.5), (64.0, 32.5), (127.5, 127.5), (10.25, 99.75)] {
                let d = grid_dir(face, s, t);
                let (f, s2, t2) = locate(d);
                assert_eq!(f, face);
                assert!(
                    (s - s2).abs() < 1e-3 && (t - t2).abs() < 1e-3,
                    "{s} {t} {s2} {t2}"
                );
            }
        }
    }
}
