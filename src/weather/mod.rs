//! Weather: the global simulation (`sim`), run on its own thread, and what the rest of the
//! game asks of it.
//!
//! The game calls `Weather::update` every frame. A few times a second that hands the worker a
//! step (it never waits for it), and picks up the newest finished `Snapshot`: the two weather
//! maps for the GPU, the wind, tornadoes and eruptions. `Snapshot::sample` says what the
//! weather is at any point, and every local effect (rain, thunder, snow, sandstorms...) comes
//! from that, so all of it agrees with the clouds seen in the sky.

pub mod sim;

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, bounded};
use glam::Vec3;

use crate::world::terrain::TerrainGenerator;
use sim::{CELLS, Force, Sim, cell_index, locate};

/// Bytes of one weather map (6 faces of RGBA8).
pub const MAP_BYTES: usize = CELLS * 4;
pub const MAP_SIZE: u32 = sim::N as u32;

/// Weather simulation steps per real second.
const STEPS_PER_SECOND: f64 = 4.0;
/// The longest step (game seconds): under heavy time warp the weather just runs slower.
const MAX_STEP: f64 = 30.0;
/// Steps run before the first snapshot, so the game starts with weather already developed.
const SPIN_UP_STEPS: usize = 110;
const SPIN_UP_DT: f32 = 32.0;

/// What the weather is doing, in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Condition {
    Clear,
    PartlyCloudy,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    HeavyRain,
    Thunderstorm,
    Snow,
    Blizzard,
    Sandstorm,
    AshFall,
}

impl Condition {
    pub fn name(self) -> &'static str {
        match self {
            Condition::Clear => "clear",
            Condition::PartlyCloudy => "partly cloudy",
            Condition::Overcast => "overcast",
            Condition::Fog => "fog",
            Condition::Drizzle => "drizzle",
            Condition::Rain => "rain",
            Condition::HeavyRain => "heavy rain",
            Condition::Thunderstorm => "thunderstorm",
            Condition::Snow => "snow",
            Condition::Blizzard => "blizzard",
            Condition::Sandstorm => "sandstorm",
            Condition::AshFall => "ash fall",
        }
    }
}

/// The weather at one point, 0..1 unless noted.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub cover: f32,
    pub top: f32,
    pub precip: f32,
    pub storm: f32,
    pub dust: f32,
    pub snow: f32,
    pub fog: f32,
    pub ash: f32,
    /// Wind at cloud level (m/s, tangent, body frame).
    pub wind: Vec3,
}

impl Sample {
    /// Wind near the ground (m/s).
    pub fn surface_wind(&self) -> Vec3 {
        self.wind * sim::SURFACE_WIND
    }

    pub fn condition(&self) -> Condition {
        let speed = self.surface_wind().length();
        if self.ash > 0.3 {
            Condition::AshFall
        } else if self.dust > 0.3 {
            Condition::Sandstorm
        } else if self.precip > 0.05 {
            if self.snow > 0.5 {
                if self.precip > 0.3 && speed > 9.0 {
                    Condition::Blizzard
                } else {
                    Condition::Snow
                }
            } else if self.storm > 0.5 && self.precip > 0.3 {
                Condition::Thunderstorm
            } else if self.precip > 0.5 {
                Condition::HeavyRain
            } else if self.precip > 0.17 {
                Condition::Rain
            } else {
                Condition::Drizzle
            }
        } else if self.fog > 0.4 {
            Condition::Fog
        } else if self.cover < 0.25 {
            Condition::Clear
        } else if self.cover < 0.72 {
            Condition::PartlyCloudy
        } else {
            Condition::Overcast
        }
    }
}

/// One finished step of the weather, shared with the renderer's upload and the game.
#[derive(Clone)]
pub struct Snapshot {
    /// Cover, cloud top, precipitation, storm (see `shaders/weather.glsl`).
    pub map_a: Vec<u8>,
    /// Dust, snow fraction, fog, ash.
    pub map_b: Vec<u8>,
    pub wind: Vec<Vec3>,
    pub tornadoes: Vec<sim::Tornado>,
    pub volcanoes: Vec<sim::VolcanoState>,
    /// The cyclones (not drawn, but they are what the wind and the clouds do next).
    pub systems: Vec<sim::System>,
    /// Game time of this step.
    pub time: f64,
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            map_a: vec![0; MAP_BYTES],
            map_b: vec![0; MAP_BYTES],
            wind: vec![Vec3::ZERO; CELLS],
            tornadoes: Vec::new(),
            volcanoes: Vec::new(),
            systems: Vec::new(),
            time: 0.0,
        }
    }

    /// The weather at direction `d` (unit, body frame), interpolated.
    pub fn sample(&self, d: Vec3) -> Sample {
        let n = sim::N as i32;
        let (face, s, t) = locate(d);
        let (x, y) = (s - 0.5, t - 0.5);
        let (i, j) = (x.floor(), y.floor());
        let (fx, fy) = (x - i, y - j);
        let at = |a: i32, b: i32| {
            cell_index(
                face,
                (i as i32 + a).clamp(0, n - 1) as usize,
                (j as i32 + b).clamp(0, n - 1) as usize,
            )
        };
        let corners = [
            (at(0, 0), (1.0 - fx) * (1.0 - fy)),
            (at(1, 0), fx * (1.0 - fy)),
            (at(0, 1), (1.0 - fx) * fy),
            (at(1, 1), fx * fy),
        ];
        let mut a = [0.0f32; 4];
        let mut b = [0.0f32; 4];
        let mut wind = Vec3::ZERO;
        for (c, w) in corners {
            for k in 0..4 {
                a[k] += self.map_a[c * 4 + k] as f32 * w / 255.0;
                b[k] += self.map_b[c * 4 + k] as f32 * w / 255.0;
            }
            wind += self.wind[c] * w;
        }
        Sample {
            cover: a[0],
            top: a[1],
            precip: a[2],
            storm: a[3],
            dust: b[0],
            snow: b[1],
            fog: b[2],
            ash: b[3],
            wind,
        }
    }
}

enum Request {
    Step {
        dt: f32,
        time: f64,
        sun: Vec3,
    },
    Force(Vec3, Force),
    Release,
    Erupt(Vec3, f64),
    /// Carry on from this weather, the sun where it is, at this game time.
    Import(Arc<Snapshot>, Vec3, f64),
}

/// The game's handle on the weather.
pub struct Weather {
    requests: Sender<Request>,
    results: Receiver<Arc<Snapshot>>,
    pub snapshot: Arc<Snapshot>,
    /// A new snapshot arrived and hasn't been uploaded yet.
    pub fresh: bool,
    /// Game time of the last step requested, and real seconds until the next one.
    last_step: f64,
    timer: f64,
    busy: bool,
    /// Simulated here (otherwise it only comes from the network).
    local: bool,
    /// The worker thread exists.
    worker: bool,
}

/// The worker thread's life: one request after the other.
fn serve(mut sim: Sim, jobs: Receiver<Request>, done: Sender<Arc<Snapshot>>) {
    while let Ok(request) = jobs.recv() {
        match request {
            Request::Step { dt, time, sun } => {
                sim.step(dt, time, sun);
                if done.send(snapshot(&sim, time)).is_err() {
                    return;
                }
            }
            Request::Force(dir, force) => sim.force(dir, force),
            Request::Release => sim.release(),
            Request::Erupt(dir, now) => {
                sim.erupt_nearest(dir, now);
            }
            Request::Import(from, sun, now) => {
                sim.import(&from.map_a, &from.map_b, &from.systems, sun, now);
            }
        }
    }
}

impl Weather {
    /// Starts the worker thread for the planet of `terrain`.
    pub fn new(terrain: Arc<TerrainGenerator>, start: f64, sun: Vec3) -> Self {
        let (requests, jobs) = bounded::<Request>(16);
        let (done, results) = bounded::<Arc<Snapshot>>(2);
        std::thread::Builder::new()
            .name("weather".into())
            .spawn(move || {
                let mut sim = Sim::new(&terrain);
                let mut clock = start - SPIN_UP_STEPS as f64 * SPIN_UP_DT as f64;
                for _ in 0..SPIN_UP_STEPS {
                    sim.step(SPIN_UP_DT, clock, sun);
                    clock += SPIN_UP_DT as f64;
                }
                sim.settle_volcanoes(clock);
                let _ = done.send(snapshot(&sim, clock));
                serve(sim, jobs, done);
            })
            .expect("weather thread");
        Self {
            requests,
            results,
            snapshot: Arc::new(Snapshot::empty()),
            fresh: false,
            last_step: start,
            timer: 0.0,
            busy: true,
            local: true,
            worker: true,
        }
    }

    /// Weather that only comes from the network: a client's, whose host runs the simulation.
    pub fn remote(start: f64) -> Self {
        let (requests, _) = bounded::<Request>(1);
        let (_, results) = bounded::<Arc<Snapshot>>(1);
        Self {
            requests,
            results,
            snapshot: Arc::new(Snapshot::empty()),
            fresh: false,
            last_step: start,
            timer: 0.0,
            busy: true,
            local: false,
            worker: false,
        }
    }

    /// Starts simulating here, carrying on from the weather as it looks right now (a player
    /// who has warped away from the one they were getting it from).
    pub fn go_local(&mut self, terrain: &Arc<TerrainGenerator>, time: f64, sun: Vec3) {
        if self.local {
            return;
        }
        if !self.worker {
            let (requests, jobs) = bounded::<Request>(16);
            let (done, results) = bounded::<Arc<Snapshot>>(2);
            let terrain = terrain.clone();
            std::thread::Builder::new()
                .name("weather".into())
                .spawn(move || serve(Sim::new(&terrain), jobs, done))
                .expect("weather thread");
            self.requests = requests;
            self.results = results;
            self.worker = true;
        }
        let _ = self
            .requests
            .try_send(Request::Import(self.snapshot.clone(), sun, time));
        self.local = true;
        self.last_step = time;
        self.timer = 0.0;
        self.busy = false;
    }

    /// From now on the weather comes from the network.
    pub fn stop_local(&mut self) {
        self.local = false;
    }

    /// The game clock has been set to `time` (it may have gone backwards).
    pub fn clock_jumped(&mut self, time: f64) {
        self.last_step = time;
    }

    /// New contents for one cube face of the maps (`FACE_BYTES` of `a`, then of `b`, then the
    /// wind at low resolution; see `pack_face`).
    pub fn set_face(&mut self, face: usize, packed: &[u8], time: f64) {
        if face >= 6 || packed.len() != PACKED_FACE {
            return;
        }
        let snapshot = Arc::make_mut(&mut self.snapshot);
        let (a, rest) = packed.split_at(FACE_BYTES);
        let (b, wind) = rest.split_at(FACE_BYTES);
        snapshot.map_a[face * FACE_BYTES..(face + 1) * FACE_BYTES].copy_from_slice(a);
        snapshot.map_b[face * FACE_BYTES..(face + 1) * FACE_BYTES].copy_from_slice(b);
        unpack_wind(&mut snapshot.wind, face, wind);
        snapshot.time = time.max(1.0);
        self.fresh = true;
    }

    /// What the network says about tornadoes and eruptions.
    pub fn set_live(
        &mut self,
        time: f64,
        tornadoes: Vec<sim::Tornado>,
        volcanoes: Vec<sim::VolcanoState>,
    ) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.time = time.max(1.0);
        snapshot.tornadoes = tornadoes;
        snapshot.volcanoes = volcanoes;
    }

    /// What the network says about the cyclones.
    pub fn set_systems(&mut self, systems: Vec<sim::System>) {
        Arc::make_mut(&mut self.snapshot).systems = systems;
    }

    /// The first weather (after the spin-up) has arrived.
    pub fn started(&self) -> bool {
        self.snapshot.time != 0.0
    }

    /// Picks up finished work and starts the next step when it is due. `dt` is real seconds,
    /// `time` game time, `sun` the sun's direction in the planet's body frame.
    pub fn update(&mut self, dt: f64, time: f64, sun: Vec3) {
        if !self.local {
            // (Anything still on its way from when this ran here is out of date.)
            while self.results.try_recv().is_ok() {
                self.busy = false;
            }
            return;
        }
        while let Ok(snapshot) = self.results.try_recv() {
            self.snapshot = snapshot;
            self.fresh = true;
            self.busy = false;
        }
        self.timer -= dt;
        if self.busy || self.timer > 0.0 {
            return;
        }
        let step = (time - self.last_step).clamp(0.0, MAX_STEP);
        if step <= 0.0 {
            return;
        }
        self.timer = 1.0 / STEPS_PER_SECOND;
        self.last_step = time;
        if self
            .requests
            .try_send(Request::Step {
                dt: step as f32,
                time,
                sun,
            })
            .is_ok()
        {
            self.busy = true;
        }
    }

    /// Holds `force` around `dir` until `release` (the `weather` console command).
    pub fn force(&self, dir: Vec3, force: Force) {
        let _ = self.requests.try_send(Request::Force(dir, force));
    }

    pub fn release(&self) {
        let _ = self.requests.try_send(Request::Release);
    }

    /// Makes the nearest volcano erupt now.
    pub fn erupt(&self, dir: Vec3, time: f64) {
        let _ = self.requests.try_send(Request::Erupt(dir, time));
    }
}

/// Bytes of one cube face of one weather map.
pub const FACE_BYTES: usize = sim::N * sim::N * 4;
/// The wind goes over the network at this many samples per face edge.
const WIND_GRID: usize = 32;
const PACKED_WIND: usize = WIND_GRID * WIND_GRID * 3;
/// A face as it goes over the network (`pack_face`).
pub const PACKED_FACE: usize = FACE_BYTES * 2 + PACKED_WIND;

/// One cube face of a snapshot as bytes to send: both maps and a coarse wind (m/s as i8).
pub fn pack_face(snapshot: &Snapshot, face: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(PACKED_FACE);
    out.extend_from_slice(&snapshot.map_a[face * FACE_BYTES..(face + 1) * FACE_BYTES]);
    out.extend_from_slice(&snapshot.map_b[face * FACE_BYTES..(face + 1) * FACE_BYTES]);
    let step = sim::N / WIND_GRID;
    for j in 0..WIND_GRID {
        for i in 0..WIND_GRID {
            let w = snapshot.wind[cell_index(face, i * step + step / 2, j * step + step / 2)];
            for c in w.to_array() {
                out.push(c.round().clamp(-127.0, 127.0) as i8 as u8);
            }
        }
    }
    out
}

/// Spreads the coarse wind of a face back over its cells.
fn unpack_wind(wind: &mut [Vec3], face: usize, packed: &[u8]) {
    let sample = |i: usize, j: usize| {
        let at = (j.min(WIND_GRID - 1) * WIND_GRID + i.min(WIND_GRID - 1)) * 3;
        Vec3::new(
            packed[at] as i8 as f32,
            packed[at + 1] as i8 as f32,
            packed[at + 2] as i8 as f32,
        )
    };
    let step = (sim::N / WIND_GRID) as f32;
    for j in 0..sim::N {
        for i in 0..sim::N {
            let (x, y) = (
                ((i as f32 + 0.5) / step - 0.5).max(0.0),
                ((j as f32 + 0.5) / step - 0.5).max(0.0),
            );
            let (ix, iy) = (x as usize, y as usize);
            let (fx, fy) = (x.fract(), y.fract());
            let bottom = sample(ix, iy).lerp(sample(ix + 1, iy), fx);
            let top = sample(ix, iy + 1).lerp(sample(ix + 1, iy + 1), fx);
            wind[cell_index(face, i, j)] = bottom.lerp(top, fy);
        }
    }
}

fn snapshot(sim: &Sim, time: f64) -> Arc<Snapshot> {
    let mut map_a = Vec::with_capacity(MAP_BYTES);
    let mut map_b = Vec::with_capacity(MAP_BYTES);
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for i in 0..CELLS {
        let c = sim.cell(i);
        map_a.extend_from_slice(&[byte(c.cover), byte(c.top), byte(c.precip), byte(c.storm)]);
        map_b.extend_from_slice(&[byte(c.dust), byte(c.snow), byte(c.fog), byte(c.ash)]);
    }
    Arc::new(Snapshot {
        map_a,
        map_b,
        wind: sim.wind.clone(),
        tornadoes: sim.tornadoes.clone(),
        volcanoes: sim.volcanoes.clone(),
        systems: sim.systems.clone(),
        time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{util::FastMap, world::planet::Planet};
    use glam::DVec3;
    use std::time::Instant;

    fn fresh_sim() -> Sim {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 1024, 0x5eed_a57b));
        Sim::new(&TerrainGenerator::new(planet))
    }

    fn sample_all(s: &Sim) -> Snapshot {
        let snap = snapshot(s, 0.0);
        Arc::try_unwrap(snap).ok().unwrap()
    }

    /// Weather that was only ever seen as maps carries on looking the same.
    #[test]
    fn imported_weather_carries_on_from_what_it_looked_like() {
        let mut source = fresh_sim();
        let sun = Vec3::X;
        for i in 0..40 {
            source.step(30.0, i as f64 * 30.0, sun);
        }
        let seen = sample_all(&source);
        let mut copy = fresh_sim();
        copy.import(&seen.map_a, &seen.map_b, &seen.systems, sun, 1_200.0);
        let again = sample_all(&copy);
        let worst = seen
            .map_a
            .chunks(4)
            .zip(again.map_a.chunks(4))
            .map(|(a, b)| (a[0] as i32 - b[0] as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 2, "cover off by {worst} of 255");
        assert_eq!(seen.map_b, again.map_b);
        // And it goes on from there: neither clearing away nor filling the sky.
        let mean =
            |m: &[u8]| m.chunks(4).map(|c| c[0] as u64).sum::<u64>() as f64 / (m.len() / 4) as f64;
        for i in 0..10 {
            let t = 1_230.0 + i as f64 * 30.0;
            source.step(30.0, t, sun);
            copy.step(30.0, t, sun);
        }
        let (real, carried) = (
            mean(&sample_all(&source).map_a),
            mean(&sample_all(&copy).map_a),
        );
        assert!(
            (real - carried).abs() < 20.0,
            "cover {carried}, really {real}"
        );
    }

    /// Runs the planet for a few game hours and checks that every kind of weather happens
    /// somewhere, without any of them taking over and without the numbers going wrong.
    /// `cargo test --release weather_makes -- --nocapture` prints the mix.
    #[test]
    fn weather_makes_every_condition() {
        let mut sim = fresh_sim();
        // Noon over the prime meridian at the equinox, moving round.
        let (mut seen, mut total, mut cover) = (FastMap::default(), 0usize, 0.0f64);
        let (mut storm_cells, mut tornadoes, mut systems) = (0usize, 0usize, 0usize);
        let start = Instant::now();
        let steps = 700;
        for step in 0..steps {
            let t = step as f64 * 20.0;
            let angle = (t / 1800.0 * std::f64::consts::TAU) as f32;
            let sun = Vec3::new(angle.cos(), 0.0, angle.sin());
            sim.step(20.0, t, sun);
            tornadoes += sim.tornadoes.len();
            systems += sim.system_count();
            // Skip the spin-up, then look every few steps.
            if step > 150 && step % 12 == 0 {
                let snap = sample_all(&sim);
                for i in (0..CELLS).step_by(7) {
                    let d = {
                        let n = sim::N;
                        let (face, j, k) = (i / (n * n), i / n % n, i % n);
                        sim::grid_dir(face, k as f32 + 0.5, j as f32 + 0.5)
                    };
                    let s = snap.sample(d);
                    *seen.entry(s.condition()).or_insert(0usize) += 1;
                    total += 1;
                    cover += s.cover as f64;
                    storm_cells += (s.storm > 0.5) as usize;
                    assert!(s.cover.is_finite() && s.wind.is_finite());
                }
            }
        }
        println!(
            "{} steps, {:.1} ms per step",
            steps,
            start.elapsed().as_secs_f64() * 1e3 / steps as f64
        );
        let mut mix: Vec<_> = seen.iter().map(|(c, n)| (*n, c.name())).collect();
        mix.sort();
        for (n, name) in mix.iter().rev() {
            println!("{name:>14}: {:5.1}%", *n as f64 * 100.0 / total as f64);
        }
        println!(
            "mean cover {:.2}, storm cells {:.2}%, tornado-steps {tornadoes}, mean systems {:.1}",
            cover / total as f64,
            storm_cells as f64 * 100.0 / total as f64,
            systems as f64 / steps as f64
        );
        for c in [
            Condition::Clear,
            Condition::PartlyCloudy,
            Condition::Overcast,
            Condition::Fog,
            Condition::Drizzle,
            Condition::Rain,
            Condition::HeavyRain,
            Condition::Thunderstorm,
            Condition::Snow,
            Condition::Blizzard,
            Condition::Sandstorm,
        ] {
            assert!(seen.get(&c).copied().unwrap_or(0) > 0, "never saw {c:?}");
        }
        assert!(tornadoes > 0, "no tornado in the whole run");
        let clear = seen.get(&Condition::Clear).copied().unwrap_or(0) as f64 / total as f64;
        assert!((0.1..0.75).contains(&clear), "clear sky fraction {clear}");
        let mean = cover / total as f64;
        assert!((0.25..0.75).contains(&mean), "mean cover {mean}");
    }

    #[test]
    fn a_face_sent_over_the_network_arrives_whole() {
        let mut from = Snapshot::empty();
        from.map_a[FACE_BYTES * 2 + 5] = 200;
        from.map_b[FACE_BYTES * 2 + 9] = 77;
        from.wind.fill(Vec3::new(20.0, -10.0, 3.0));
        let packed = pack_face(&from, 2);
        assert_eq!(packed.len(), PACKED_FACE);
        let mut to = Weather::remote(0.0);
        assert!(!to.started());
        to.set_face(2, &packed, 123.0);
        assert!(to.started() && to.fresh);
        assert_eq!(to.snapshot.map_a[FACE_BYTES * 2 + 5], 200);
        assert_eq!(to.snapshot.map_b[FACE_BYTES * 2 + 9], 77);
        assert_eq!(to.snapshot.map_a[5], 0, "another face");
        let wind = to.snapshot.wind[cell_index(2, 64, 64)];
        assert!(
            (wind - Vec3::new(20.0, -10.0, 3.0)).length() < 1e-3,
            "{wind}"
        );
        to.set_face(9, &packed, 1.0); // out of range: ignored
        to.set_face(0, &packed[1..], 1.0); // cut short: ignored
    }

    #[test]
    fn forcing_changes_the_weather_there() {
        let mut sim = fresh_sim();
        let d = Vec3::new(0.3, 0.2, 0.9).normalize();
        let sun = Vec3::X;
        for step in 0..3 {
            sim.step(10.0, step as f64 * 10.0, sun);
        }
        sim.force(d, Force::Storm);
        sim.step(1.0, 40.0, sun);
        let s = sample_all(&sim).sample(d);
        assert_eq!(s.condition(), Condition::Thunderstorm, "{s:?}");
        sim.force(d, Force::Clear);
        sim.step(1.0, 41.0, sun);
        assert_eq!(sample_all(&sim).sample(d).condition(), Condition::Clear);
        sim.force(d, Force::Blizzard);
        sim.step(1.0, 42.0, sun);
        assert_eq!(sample_all(&sim).sample(d).condition(), Condition::Blizzard);
        sim.force(d, Force::Sandstorm);
        sim.step(1.0, 43.0, sun);
        assert_eq!(sample_all(&sim).sample(d).condition(), Condition::Sandstorm);
        sim.force(d, Force::Tornado);
        sim.step(1.0, 44.0, sun);
        assert!(!sim.tornadoes.is_empty());
    }
}
