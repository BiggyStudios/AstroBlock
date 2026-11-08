//! The game side of the weather: everything that happens where the player is, read from the
//! planet-wide simulation (`crate::weather`), so what you feel always matches the clouds you
//! see: rain, snow and sand as GPU particles, lightning and thunder, tornadoes throwing
//! things around, volcanoes erupting, the wind and rain you hear.
//!
//! Weather belongs to home: on the moon none of it applies (home's clouds still show in the
//! sky, and from orbit).

use anyhow::Result;
use glam::{DVec3, Mat3, Vec3, Vec4};

use super::Game;
use crate::{
    audio::{Ambience, Sound},
    effects::{
        self,
        debris::{FlyingBlock, Landing, Vortex},
    },
    player::GameMode,
    render::{CloudParams, DrawItem, Fade, ROOF_CELLS, RoofMap},
    space::bodies::Body,
    ui::anim::approach,
    weather::{Condition, Sample, sim::Force},
    world::{block::Block, terrain::Biome},
};

/// Cloud base above sea level and the thickness of the shell clouds live in (m).
const CLOUD_BASE: f32 = 900.0;
const CLOUD_THICKNESS: f32 = 4200.0;
/// The noise tile clouds are carved from (`SHAPE_TILE` in `shaders/weather.glsl`).
const NOISE_TILE: f32 = 4800.0;
/// Size of a roof map cell (m), and how far the camera moves before it is re-centred.
const ROOF_CELL: f32 = 1.5;
const ROOF_RECENTER: f64 = 10.0;
/// Roof map rows scanned per frame.
const ROOF_ROWS_PER_FRAME: usize = 6;
/// Layers above and below the camera searched for the highest block.
const ROOF_UP: i32 = 30;
const ROOF_DOWN: i32 = 6;

/// Flashes show for this long (s), and thunder arrives at the speed of sound.
const FLASH_TIME: f32 = 0.45;
const SPEED_OF_SOUND: f64 = 340.0;
/// Tornadoes are drawn out to this far, and act on things within these distances (m).
const TORNADO_VIEW: f64 = 40_000.0;
const TORNADO_SOUND: f64 = 1_500.0;
/// Blocks a funnel tears up per second: real ones near enough for chunks to be loaded, and
/// ones made up for show out where they aren't.
const RIP_RATE: f32 = 46.0;
const RIP_RANGE: f64 = 372.0;
const SHOW_RATE: f32 = 26.0;
const SHOW_RANGE: f64 = 1_300.0;
/// Eruption columns are drawn out to this far (m).
const PLUME_VIEW: f64 = 60_000.0;
/// Gravity flying blocks fall with (m/s^2), and the time step they take at most (s).
const BLOCK_GRAVITY: f64 = 9.81;
const BLOCK_MAX_STEP: f32 = 0.05;
/// Flying blocks are simulated and drawn out to here (m).
const BLOCK_VIEW: f64 = 3_000.0;
/// A block going faster than this relative to the player (m/s) hurts, within this distance (m).
const BLOCK_HIT_SPEED: f64 = 9.0;
const BLOCK_HIT_RANGE: f64 = 1.5;

#[derive(Clone, Copy, Default)]
struct Flash {
    /// Planet-relative.
    at: DVec3,
    age: f32,
    strength: f32,
}

pub(super) struct WeatherFx {
    /// The weather at the camera, eased so nothing switches abruptly.
    pub local: Sample,
    /// Far the wind has carried the cloud noise (wraps at the tile).
    offset: Vec3,
    /// Fractions of particles left over between frames.
    carry: [f32; 8],
    flashes: [Flash; 2],
    next_flash: f32,
    /// Thunder on its way: seconds to go, where it came from, loudness.
    thunder: Vec<(f32, DVec3, f32)>,
    roof: RoofScan,
    rng: u64,
    /// Fractions of blocks left over between frames, and seconds until the next lava burn.
    rip: f32,
    show: f32,
    burn: f32,
    /// Seconds until flying lava next leaves its smoke.
    trail: f32,
    forced: Option<Force>,
}

impl Default for WeatherFx {
    fn default() -> Self {
        Self {
            local: Sample::default(),
            offset: Vec3::ZERO,
            carry: [0.0; 8],
            flashes: [Flash::default(); 2],
            next_flash: 2.0,
            thunder: Vec::new(),
            roof: RoofScan::default(),
            rng: 0x2545_f491_4f6c_dd1d,
            rip: 0.0,
            show: 0.0,
            burn: 0.0,
            trail: 0.0,
            forced: None,
        }
    }
}

impl WeatherFx {
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// `rate` particles per second over `dt`, keeping the fraction for later.
    fn count(&mut self, slot: usize, rate: f32, dt: f32) -> u32 {
        let n = rate.max(0.0) * dt + self.carry[slot];
        self.carry[slot] = n.fract();
        n as u32
    }
}

/// The highest block around the camera, scanned a few rows per frame.
struct RoofScan {
    center: DVec3,
    up: DVec3,
    u: DVec3,
    v: DVec3,
    heights: Vec<f32>,
    row: usize,
    /// One whole pass has finished since the last re-centring.
    ready: bool,
    active: bool,
}

impl Default for RoofScan {
    fn default() -> Self {
        Self {
            center: DVec3::ZERO,
            up: DVec3::Y,
            u: DVec3::X,
            v: DVec3::Z,
            heights: vec![-1e6; ROOF_CELLS * ROOF_CELLS],
            row: 0,
            ready: false,
            active: false,
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A tornado as seen right now.
struct LiveTornado {
    /// Planet-relative foot of the funnel, and its local up.
    foot: DVec3,
    up: DVec3,
    radius: f32,
    distance: f64,
}

impl Game {
    /// Advances the weather and everything it does around the player.
    pub(super) fn update_weather(&mut self, dt: f64) -> Result<()> {
        let sun = self
            .system
            .star_in_body_frame(self.time)
            .normalize()
            .as_vec3();
        self.weather.update(dt, self.time, sun);
        if self.weather.fresh {
            self.weather.fresh = false;
            let snapshot = self.weather.snapshot.clone();
            self.renderer
                .upload_weather(&snapshot.map_a, &snapshot.map_b)?;
        }

        let dt = dt as f32;
        let rel = self.camera.position - self.planet.center;
        let at_home = self.active == Body::Planet;
        let dir = rel.try_normalize().unwrap_or(DVec3::Y);

        // The weather here, eased; the wind carrying the cloud noise is the local wind.
        let target = if at_home {
            self.weather.snapshot.sample(dir.as_vec3())
        } else {
            Sample::default()
        };
        let fx = &mut self.weather_fx;
        let ease = |a: f32, b: f32| approach(a, b, 0.9, dt);
        let l = &mut fx.local;
        l.cover = ease(l.cover, target.cover);
        l.top = ease(l.top, target.top);
        l.precip = ease(l.precip, target.precip);
        l.storm = ease(l.storm, target.storm);
        l.dust = ease(l.dust, target.dust);
        l.snow = ease(l.snow, target.snow);
        l.fog = ease(l.fog, target.fog);
        l.ash = ease(l.ash, target.ash);
        l.wind += (target.wind - l.wind) * (1.0 - (-dt * 0.9).exp());
        let game_dt = dt * self.effective_warp.min(30.0) as f32;
        let carried = fx.offset - l.wind * game_dt;
        fx.offset = Vec3::new(
            carried.x.rem_euclid(NOISE_TILE),
            carried.y.rem_euclid(NOISE_TILE),
            carried.z.rem_euclid(NOISE_TILE),
        );

        let altitude = (rel.length() - self.planet.sea_radius) as f32;
        let underground = self.streamer.camera_underground(rel);
        // The effects happen below the clouds, outside.
        let here = if at_home && !underground {
            smoothstep(CLOUD_BASE + 200.0, CLOUD_BASE - 100.0, altitude)
        } else {
            0.0
        };
        self.update_flashes(dt, rel, here);
        let tornadoes = self.live_tornadoes(rel);
        if here > 0.0 {
            self.weather_particles(dt, rel, dir, here);
            self.scan_roof(rel, dir);
        } else {
            self.weather_fx.roof.active = false;
        }
        self.tornado_effects(dt, rel, &tornadoes);
        if at_home {
            self.volcano_effects(dt, rel);
            self.update_lava(dt, rel);
        } else {
            self.streamer.changes.clear();
        }
        self.update_debris(dt, rel, &tornadoes);
        self.lava_burns(dt);
        self.weather_sounds(dt, rel, here, underground, &tornadoes);
        Ok(())
    }

    /// Starts new lightning near the camera and ages what's in the air.
    fn update_flashes(&mut self, dt: f32, rel: DVec3, here: f32) {
        for flash in &mut self.weather_fx.flashes {
            flash.age += dt;
        }
        let storm = self.weather_fx.local.storm;
        self.weather_fx.next_flash -= dt;
        if self.weather_fx.next_flash > 0.0 {
            return;
        }
        // More strikes the harder it storms, and some in the distance from a clear spot.
        let rate = 0.03 + 1.1 * storm * storm;
        let wait = -(1.0 - self.weather_fx.random()).max(1e-3).ln() / rate;
        self.weather_fx.next_flash = wait.min(12.0);
        if here <= 0.0 {
            return;
        }
        let up = rel.normalize();
        let east = DVec3::Y.cross(up).try_normalize().unwrap_or(DVec3::X);
        let north = up.cross(east);
        let snapshot = self.weather.snapshot.clone();
        // A storm cell somewhere around.
        for _ in 0..8 {
            let angle = self.weather_fx.random() as f64 * std::f64::consts::TAU;
            let distance = 500.0 + (self.weather_fx.random() as f64).powi(2) * 8_000.0;
            let at = (rel + (east * angle.cos() + north * angle.sin()) * distance).normalize();
            if snapshot.sample(at.as_vec3()).storm < 0.32 {
                continue;
            }
            self.strike(at, distance);
            break;
        }
    }

    /// A lightning bolt at direction `at`, `distance` metres from the camera.
    fn strike(&mut self, at: DVec3, distance: f64) {
        let ground = self.terrain.sample_column(at).height.max(0.0);
        let top = at * (self.planet.sea_radius + CLOUD_BASE as f64 + 150.0);
        let bottom = at * (self.planet.sea_radius + ground);
        let seed = self.weather_fx.rng;
        self.weather_fx.random();
        let bolts = effects::lightning(top, bottom, seed);
        self.survival.effects.queue.extend(bolts);
        let oldest = usize::from(self.weather_fx.flashes[1].age > self.weather_fx.flashes[0].age);
        let strength = 0.8 + 0.6 * self.weather_fx.random();
        self.weather_fx.flashes[oldest] = Flash {
            at: (top + bottom) * 0.5,
            age: 0.0,
            strength,
        };
        let loud = (1.0 - distance / 9_500.0).clamp(0.08, 1.0) as f32;
        self.weather_fx
            .thunder
            .push(((distance / SPEED_OF_SOUND) as f32, bottom, loud));
    }

    /// Tornadoes near enough to matter, nearest first.
    fn live_tornadoes(&self, rel: DVec3) -> Vec<LiveTornado> {
        if self.active != Body::Planet {
            return Vec::new();
        }
        let snapshot = &self.weather.snapshot;
        let since = (self.time - snapshot.time).clamp(0.0, 2.0) as f32;
        let mut out: Vec<LiveTornado> = snapshot
            .tornadoes
            .iter()
            .filter_map(|t| {
                let dir = (t.dir + t.velocity * since / self.planet.sea_radius as f32)
                    .normalize()
                    .as_dvec3();
                let height = self.terrain.sample_column(dir).height.max(0.0);
                let foot = dir * (self.planet.sea_radius + height);
                let distance = (foot - rel).length();
                (distance < TORNADO_VIEW).then_some(LiveTornado {
                    foot,
                    up: dir,
                    radius: t.radius * t.strength,
                    distance,
                })
            })
            .collect();
        out.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        out.truncate(3);
        out
    }

    /// Rain, snow, sand and ash around the camera.
    fn weather_particles(&mut self, dt: f32, rel: DVec3, up: DVec3, here: f32) {
        let l = self.weather_fx.local;
        let wind = l.surface_wind();
        let speed = wind.length();
        let mut queue = Vec::new();
        let (rain, snow) = (l.precip * (1.0 - l.snow) * here, l.precip * l.snow * here);
        let n = self.weather_fx.count(0, 70_000.0 * rain.powf(1.2), dt);
        if n > 0 {
            queue.push(effects::rain_fall(rel, up, wind, n, rain));
        }
        // Flakes in calm air, streaks when it blows.
        let blizzard = smoothstep(7.0, 14.0, speed);
        let n = self
            .weather_fx
            .count(1, 14_000.0 * snow.powf(0.8) * (1.0 - blizzard * 0.5), dt);
        if n > 0 {
            queue.push(effects::snow_fall(rel, up, wind, n));
        }
        let n = self.weather_fx.count(2, 60_000.0 * snow * blizzard, dt);
        if n > 0 {
            queue.push(effects::snow_streaks(rel, up, wind, n));
        }
        let dust = smoothstep(0.08, 0.55, l.dust) * here;
        let n = self.weather_fx.count(3, 50_000.0 * dust, dt);
        if n > 0 {
            queue.push(effects::sand_streaks(rel, up, wind, n));
        }
        let n = self.weather_fx.count(4, 240.0 * dust, dt);
        if n > 0 {
            queue.push(effects::dust_puffs(rel, up, wind, n));
        }
        let ash = smoothstep(0.1, 0.5, l.ash) * here;
        let n = self.weather_fx.count(5, 5_000.0 * ash, dt);
        if n > 0 {
            queue.push(effects::ash_fall(rel, up, wind, n));
        }
        self.survival.effects.queue.extend(queue);
    }

    /// Scans a few more rows of the map of the highest block around the camera.
    fn scan_roof(&mut self, rel: DVec3, up: DVec3) {
        let l = self.weather_fx.local;
        if l.precip + l.dust + l.ash < 0.01 {
            self.weather_fx.roof.active = false;
            return;
        }
        let scan = &mut self.weather_fx.roof;
        scan.active = true;
        if (rel - scan.center).length() > ROOF_RECENTER {
            scan.center = rel;
            scan.up = up;
            scan.u = up.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
            scan.v = scan.u.cross(up);
            scan.row = 0;
            scan.ready = false;
            scan.heights.fill(-1e6);
        }
        let (center, u, v) = (scan.center, scan.u, scan.v);
        let (mut row, center_radius) = (scan.row, center.length());
        let mut heights = std::mem::take(&mut scan.heights);
        for _ in 0..ROOF_ROWS_PER_FRAME {
            for x in 0..ROOF_CELLS {
                let offset =
                    |i: usize| (i as f64 + 0.5 - ROOF_CELLS as f64 * 0.5) * ROOF_CELL as f64;
                let p = center + u * offset(x) + v * offset(row);
                let mut top = -1e6f32;
                if let Some(cell) = self.planet.cell_at(p) {
                    for k in (cell.k - ROOF_DOWN..=cell.k + ROOF_UP).rev() {
                        let c = crate::world::chunk::BlockCell { k, ..cell };
                        if self.streamer.block(c).is_some_and(|b| b.is_solid()) {
                            top = (self.planet.layer_radius((k + 1) as f64) - center_radius) as f32;
                            break;
                        }
                    }
                }
                heights[row * ROOF_CELLS + x] = top;
            }
            row += 1;
            if row == ROOF_CELLS {
                row = 0;
                self.weather_fx.roof.ready = true;
            }
        }
        let scan = &mut self.weather_fx.roof;
        scan.heights = heights;
        scan.row = row;
    }

    /// Dust, debris and the push of tornadoes near the camera; the blocks they tear up.
    fn tornado_effects(&mut self, dt: f32, rel: DVec3, tornadoes: &[LiveTornado]) {
        for t in tornadoes {
            if t.radius < 2.0 || t.distance > 2_500.0 {
                continue;
            }
            let radius = t.radius.max(8.0);
            let n = self.weather_fx.count(6, 120.0, dt);
            if n > 0 {
                self.survival
                    .effects
                    .queue
                    .push(effects::tornado_dust(t.foot, t.up, n, radius));
            }
            if t.distance < 900.0 {
                let n = self.weather_fx.count(7, 90.0, dt);
                if n > 0 {
                    self.survival
                        .effects
                        .queue
                        .push(effects::tornado_debris(t.foot, t.up, n, radius));
                }
            }
            self.rip_blocks(dt, t);
            self.show_blocks(dt, t);
            self.tornado_push(dt, rel, t);
        }
    }

    /// Pulls and lifts whoever is close enough, animals included. Near the core it easily
    /// beats gravity (including the extra pull a walking player gets near the ground).
    fn tornado_push(&mut self, dt: f32, rel: DVec3, t: &LiveTornado) {
        let radius = (t.radius as f64).max(8.0);
        let reach = radius * 6.0 + 30.0;
        let push = |pos: DVec3| -> Option<DVec3> {
            let d = pos - t.foot;
            let h = d.dot(t.up);
            let radial = d - t.up * h;
            let r = radial.length();
            if r > reach || r < 1e-3 || h > 260.0 {
                return None;
            }
            let inward = -radial / r;
            let around = t.up.cross(inward);
            let strength = (1.0 - r / reach).powf(1.5);
            // Round and round, in, and up the middle.
            let lift = if r < radius * 2.6 { 84.0 } else { 6.0 };
            Some((around * 50.0 + inward * 36.0 + t.up * lift) * strength)
        };
        let dt64 = dt as f64;
        if self.player.mode != GameMode::Spectator
            && self.flight.boarded.is_none()
            && self.flight.eva.is_none()
            && let Some(a) = push(rel)
        {
            let frame = self.active_frame(self.time);
            self.player.velocity += frame.to_inertial(a) * dt64;
            if a.dot(t.up) > 2.0 {
                self.player.on_ground = false;
            }
        }
        for mob in &mut self.survival.mobs {
            if let Some(a) = push(mob.position) {
                mob.velocity += a * dt64;
            }
        }
    }

    /// A random spot on the ground under the funnel, within `reach` of its centre.
    fn funnel_spot(&mut self, t: &LiveTornado, reach: f64) -> DVec3 {
        let east = DVec3::Y.cross(t.up).try_normalize().unwrap_or(DVec3::X);
        let north = t.up.cross(east);
        let angle = self.weather_fx.random() as f64 * std::f64::consts::TAU;
        let r = reach * (self.weather_fx.random() as f64).sqrt();
        t.foot + (east * angle.cos() + north * angle.sin()) * r
    }

    /// Sends a block up into the funnel from `at`.
    fn throw_block(&mut self, block: Block, at: DVec3, t: &LiveTornado) {
        let d = at - t.foot;
        let radial = d - t.up * d.dot(t.up);
        let inward = -radial.try_normalize().unwrap_or(DVec3::X);
        let around = t.up.cross(inward);
        let fx = &mut self.weather_fx;
        let (a, b) = (fx.random() as f64, fx.random() as f64);
        let velocity = around * (16.0 + a * 24.0) + t.up * (14.0 + b * 22.0);
        let rolls = [fx.random(), fx.random(), fx.random(), fx.random()];
        self.debris.push(FlyingBlock::new(
            block,
            at + t.up * 0.8,
            velocity,
            1,
            Landing::Shatter,
            rolls,
        ));
    }

    /// Tears blocks out of the ground under the funnel and sends them flying, near the player
    /// only (only loaded chunks can change). Trees go whole, ground only a layer or two deep.
    fn rip_blocks(&mut self, dt: f32, t: &LiveTornado) {
        if t.distance > RIP_RANGE || t.radius < 5.0 {
            return;
        }
        let n = RIP_RATE * dt.min(0.1) + self.weather_fx.rip;
        self.weather_fx.rip = n.fract();
        for _ in 0..n as u32 {
            let p = self.funnel_spot(t, t.radius as f64 * 2.2 + 6.0);
            let Some(cell) = self.planet.cell_at(p) else {
                continue;
            };
            let dir = p.normalize();
            let surface = self.terrain.sample_column(dir).height;
            let floor = (self.planet.sea_layer as f64 + surface).floor() as i32 - 1;
            let mut taken = 0;
            for k in (floor..=cell.k + 40).rev() {
                let c = crate::world::chunk::BlockCell { k, ..cell };
                let Some(block) = self.streamer.block(c) else {
                    break;
                };
                if !block.is_solid() {
                    continue;
                }
                if !can_be_ripped(block) {
                    break;
                }
                self.streamer.set_block(c, Block::Air);
                self.survival.announce_broken(c);
                let center = crate::player::interact::cell_center(&self.planet, c);
                self.survival
                    .effects
                    .queue
                    .push(effects::block_break(block, center, t.up));
                self.throw_block(block, center, t);
                taken += 1;
                // Treetops come away in armfuls.
                if taken >= 3 || !is_plant(block) {
                    break;
                }
            }
        }
    }

    /// Blocks thrown about further out, where the ground isn't loaded as real blocks: they
    /// are only for show, made of what the ground there is made of.
    fn show_blocks(&mut self, dt: f32, t: &LiveTornado) {
        if t.distance < RIP_RANGE * 0.8 || t.distance > SHOW_RANGE || t.radius < 5.0 {
            return;
        }
        let n = SHOW_RATE * dt.min(0.1) + self.weather_fx.show;
        self.weather_fx.show = n.fract();
        for _ in 0..n as u32 {
            let p = self.funnel_spot(t, t.radius as f64 * 2.2 + 6.0);
            let column = self.terrain.sample_column(p.normalize());
            let block = loose_ground(column.biome, self.weather_fx.random());
            let at = p.normalize() * (self.planet.sea_radius + column.height.max(0.0));
            self.throw_block(block, at, t);
        }
    }

    /// Everything in the air: flies, lands, hits the player.
    fn update_debris(&mut self, dt: f32, rel: DVec3, tornadoes: &[LiveTornado]) {
        if self.active != Body::Planet {
            self.debris.blocks.clear();
            return;
        }
        if self.debris.blocks.is_empty() {
            return;
        }
        let game_dt = dt * self.effective_warp.min(10.0) as f32;
        let steps = (game_dt / BLOCK_MAX_STEP).ceil().max(1.0);
        let step = (game_dt / steps) as f64;
        let vortices: Vec<Vortex> = tornadoes
            .iter()
            .filter(|t| t.radius > 1.0)
            .map(|t| Vortex {
                foot: t.foot,
                up: t.up,
                radius: t.radius as f64,
            })
            .collect();
        self.weather_fx.trail -= dt;
        let leave_trails = self.weather_fx.trail <= 0.0;
        if leave_trails {
            self.weather_fx.trail = 0.09;
        }
        let up = rel.normalize_or(DVec3::Y);
        let body = rel - up * 0.7;
        let can_be_hit = self.player.mode == GameMode::Survival
            && self.flight.boarded.is_none()
            && self.flight.eva.is_none();
        let frame = self.active_frame(self.time);
        let player_velocity = self.player.relative_velocity(&frame);
        let radius = self.planet.sea_radius;
        let mut blocks = std::mem::take(&mut self.debris.blocks);
        let mut landed = Vec::new();
        let mut hurt = false;
        blocks.retain_mut(|b| {
            let mut alive = true;
            for _ in 0..steps as u32 {
                b.step(step, BLOCK_GRAVITY, &vortices);
                // (Not at once: it starts out touching where it came from.)
                if b.age < 0.5 {
                    continue;
                }
                let hit = match self.planet.cell_at(b.position) {
                    None => false,
                    Some(cell) => match self.streamer.block(cell) {
                        Some(found) => found != Block::Air && found != Block::Torch,
                        None => {
                            b.ground_in -= step as f32;
                            if b.ground_in <= 0.0 {
                                b.ground_in = 0.2;
                                let h = self
                                    .terrain
                                    .sample_column(b.position.normalize())
                                    .height
                                    .max(0.0);
                                b.ground = radius + h;
                            }
                            b.position.length() < b.ground
                        }
                    },
                };
                if hit {
                    landed.push(*b);
                    alive = false;
                    break;
                }
            }
            if !alive || b.age > 40.0 || (b.position - rel).length() > BLOCK_VIEW {
                return false;
            }
            if can_be_hit
                && (b.position - body).length() < BLOCK_HIT_RANGE
                && (b.velocity - player_velocity).length() > BLOCK_HIT_SPEED
            {
                hurt = true;
                landed.push(*b);
                return false;
            }
            true
        });
        if leave_trails {
            for b in blocks.iter().filter(|b| b.landing == Landing::Burn) {
                if (b.position - rel).length() < 1_600.0 {
                    self.survival
                        .effects
                        .queue
                        .push(effects::bomb_trail(b.position, b.velocity.as_vec3()));
                }
            }
        }
        self.debris.blocks = blocks;
        for b in landed {
            let up = b.position.normalize_or(DVec3::Y);
            match b.landing {
                Landing::Shatter => {
                    self.survival
                        .effects
                        .queue
                        .push(effects::block_break(b.block, b.position, up));
                    self.survival
                        .sounds
                        .push((Sound::Break(b.block.material()), b.position));
                }
                Landing::Burn => self.lava_splat(b.position - b.velocity.normalize_or_zero()),
            }
        }
        if hurt {
            let at = self.camera.position - self.planet.center;
            if self.survival.hurt(3.0, false, at) {
                self.respawn();
            }
        }
    }

    /// The flying blocks' draw calls: one small cached cube each, turned and put in place.
    pub(super) fn debris_draw_items(&mut self, objects: &mut Vec<DrawItem>) -> Result<()> {
        let rel = self.camera.position - self.planet.center;
        for b in &self.debris.blocks {
            if (b.position - rel).length() > BLOCK_VIEW {
                continue;
            }
            let key = (b.block, b.size);
            let handle = match self.debris.meshes.get(&key) {
                Some(&handle) => handle,
                None => match self
                    .renderer
                    .create_mesh(&crate::effects::debris::mesh(b.block, b.size))?
                {
                    Some(handle) => {
                        self.debris.meshes.insert(key, handle);
                        handle
                    }
                    // This frame's upload budget is used up: next frame.
                    None => continue,
                },
            };
            objects.push(DrawItem {
                rotation: b.rotation(),
                mesh: handle,
                origin: self.planet.center + b.position,
                radius: b.size as f32,
                fade: Fade::Visible,
            });
        }
        Ok(())
    }

    /// Fire and ash close to a volcano in eruption. (The eruption cloud itself is a volume in
    /// `clouds.frag`, and the rivers of lava are in `app/lava.rs`.)
    fn volcano_effects(&mut self, dt: f32, rel: DVec3) {
        let snapshot = self.weather.snapshot.clone();
        for v in &snapshot.volcanoes {
            if v.activity < 0.02 {
                continue;
            }
            let dir = v.dir.as_dvec3();
            let vent = dir * (self.planet.sea_radius + v.vent as f64);
            let distance = (vent - rel).length();
            if distance > 16_000.0 {
                continue;
            }
            if distance < 6_000.0 {
                let top = dir * (self.planet.sea_radius + v.summit as f64);
                let wind = snapshot.sample(v.dir).surface_wind();
                let n = self.weather_fx.count(4, 30.0 * v.activity, dt);
                if n > 0 {
                    self.survival
                        .effects
                        .queue
                        .push(effects::ash_plume(top, dir, wind, n));
                }
            }
            let n = self.weather_fx.count(5, 300.0 * v.activity, dt);
            if n > 0 {
                self.survival.effects.queue.push(effects::lava_fountain(
                    vent,
                    dir,
                    n,
                    (distance as f32 * 0.0014).clamp(1.0, 6.0),
                ));
            }
        }
    }

    /// The blast at the start of an eruption, `distance` metres away: heard when it gets here.
    pub(super) fn eruption_begins(&mut self, top: DVec3, distance: f64) {
        let loud = (1.0 - distance / 16_000.0).clamp(0.0, 1.0) as f32;
        if loud > 0.0 {
            self.weather_fx
                .thunder
                .push(((distance / SPEED_OF_SOUND) as f32, top, loud));
        }
    }

    /// The nearest eruption: distance (m), compass direction and how hard it is.
    fn nearest_eruption(&self, rel: DVec3) -> Option<(f64, &'static str)> {
        let snapshot = &self.weather.snapshot;
        let up = rel.try_normalize()?;
        let east = DVec3::Y.cross(up).try_normalize().unwrap_or(DVec3::X);
        let north = up.cross(east);
        let radius = self.planet.sea_radius;
        snapshot
            .volcanoes
            .iter()
            .filter(|v| v.activity > 0.1)
            .map(|v| v.dir.as_dvec3() * (radius + v.summit as f64) - rel)
            .min_by(|a, b| a.length().total_cmp(&b.length()))
            .map(|to| {
                let angle = to
                    .dot(east)
                    .atan2(to.dot(north))
                    .to_degrees()
                    .rem_euclid(360.0);
                let names = ["n", "ne", "e", "se", "s", "sw", "w", "nw"];
                (to.length(), names[((angle + 22.5) / 45.0) as usize % 8])
            })
    }

    /// Standing on lava hurts.
    fn lava_burns(&mut self, dt: f32) {
        self.weather_fx.burn -= dt;
        if self.player.mode != GameMode::Survival
            || !self.player.on_ground
            || self.weather_fx.burn > 0.0
        {
            return;
        }
        let frame = self.active_frame(self.time);
        let feet = self.player.body_position(&frame);
        let up = feet.normalize_or(DVec3::Y);
        let on_lava = self
            .planet
            .cell_at(feet - up * 0.2)
            .and_then(|c| self.streamer.block(c))
            == Some(Block::Lava);
        if on_lava {
            self.weather_fx.burn = 0.5;
            let at = self.camera.position - self.planet.center;
            if self.survival.hurt(2.0, true, at) {
                self.respawn();
            }
        }
    }

    /// Rain, wind and the roar of tornadoes as loops; thunder when it arrives.
    fn weather_sounds(
        &mut self,
        dt: f32,
        rel: DVec3,
        here: f32,
        underground: bool,
        tornadoes: &[LiveTornado],
    ) {
        let l = self.weather_fx.local;
        // Caves and the upper air are quiet.
        let open = if underground { 0.08 } else { 1.0 };
        let air = smoothstep(
            9_000.0,
            3_000.0,
            (rel.length() - self.planet.sea_radius) as f32,
        );
        let sheltered = self.camera_sheltered(rel);
        let muffle = open * if sheltered { 0.45 } else { 1.0 };
        let rain = (l.precip * (1.0 - l.snow) * here).powf(0.7);
        self.audio.set_ambience(Ambience::Rain, rain * muffle);
        let speed = l.surface_wind().length();
        let wind = smoothstep(5.0, 18.0, speed + l.dust * 10.0) * air;
        self.audio.set_ambience(Ambience::Wind, wind * muffle);
        let near = tornadoes.first().map_or(0.0, |t| {
            ((1.0 - t.distance / TORNADO_SOUND).clamp(0.0, 1.0) as f32).powi(2)
                * (t.radius / 16.0).min(1.0)
        });
        // An eruption rumbles for kilometres around.
        let radius = self.planet.sea_radius;
        let rumble = self
            .weather
            .snapshot
            .volcanoes
            .iter()
            .map(|v| {
                let at = v.dir.as_dvec3() * (radius + v.summit as f64);
                ((1.0 - (at - rel).length() / 7_000.0).clamp(0.0, 1.0) as f32).powi(2) * v.activity
            })
            .fold(0.0, f32::max);
        self.audio
            .set_ambience(Ambience::Roar, near.max(rumble) * open);

        let right = self.camera.right().as_dvec3();
        let mut arrived = Vec::new();
        self.weather_fx.thunder.retain_mut(|(delay, at, gain)| {
            *delay -= dt;
            if *delay <= 0.0 {
                arrived.push((*at, *gain));
                false
            } else {
                true
            }
        });
        for (at, gain) in arrived {
            self.audio
                .play_gain(Sound::Thunder, at - rel, right, gain * open);
        }
    }

    /// Whether there is a block overhead (rain is muffled inside).
    fn camera_sheltered(&self, rel: DVec3) -> bool {
        let scan = &self.weather_fx.roof;
        if !scan.active || !scan.ready {
            return false;
        }
        let i = ROOF_CELLS as f64 * 0.5;
        let d = rel - scan.center;
        let cell = ROOF_CELL as f64;
        let (x, y) = (
            (d.dot(scan.u) / cell + i) as usize,
            (d.dot(scan.v) / cell + i) as usize,
        );
        x < ROOF_CELLS
            && y < ROOF_CELLS
            && scan.heights[y * ROOF_CELLS + x] > d.dot(scan.up) as f32 + 0.5
    }

    /// The cloud pass's input for this frame, and the tornadoes for the particles.
    #[allow(clippy::type_complexity)]
    pub(super) fn cloud_params(&self) -> (CloudParams, [(DVec3, f32); 3], Option<RoofMap>) {
        let rel = self.camera.position - self.planet.center;
        let at_home = self.active == Body::Planet;
        let rotation = Mat3::from_quat(
            self.bodies
                .view_rotation(Body::Planet, self.active, self.time)
                .as_quat(),
        );
        let fx = &self.weather_fx;
        let mut flash_light = 0.0;
        let mut flashes = [Vec4::ZERO; 2];
        if at_home {
            for (out, flash) in flashes.iter_mut().zip(&fx.flashes) {
                if flash.age < FLASH_TIME && flash.strength > 0.0 {
                    // A stroke, a dimmer return stroke or two.
                    let t = flash.age;
                    let pulse = (-t * 16.0).exp()
                        + 0.6 * (-(t - 0.09).abs() * 40.0).exp()
                        + 0.4 * (-(t - 0.2).abs() * 40.0).exp();
                    let intensity = flash.strength * pulse;
                    *out = (flash.at - rel).as_vec3().extend(intensity * 2.2);
                    let distance = (flash.at - rel).length() as f32;
                    flash_light += intensity * 0.05 / (1.0 + (distance / 2200.0).powi(2));
                }
            }
        }
        let tornadoes_live = self.live_tornadoes(rel);
        let mut tornadoes = [Vec4::ZERO; 3];
        let mut vortices = [(DVec3::ZERO, 0.0f32); 3];
        for (i, t) in tornadoes_live.iter().enumerate() {
            tornadoes[i] = (t.foot - rel).as_vec3().extend(t.radius);
            if t.distance < 3_000.0 {
                vortices[i] = (self.planet.center + t.foot, t.radius.max(1.0));
            }
        }
        // Eruptions, nearest first.
        let mut plumes = [Vec4::ZERO; 2];
        let mut plume_wind = [Vec4::ZERO; 2];
        if at_home {
            let snapshot = &self.weather.snapshot;
            let radius = self.planet.sea_radius;
            let mut active: Vec<_> = snapshot
                .volcanoes
                .iter()
                .filter(|v| v.activity > 0.01)
                .map(|v| (v, v.dir.as_dvec3() * (radius + v.vent as f64) - rel))
                .filter(|(_, to)| to.length() < PLUME_VIEW)
                .collect();
            active.sort_by(|a, b| a.1.length().total_cmp(&b.1.length()));
            for (i, (v, to)) in active.iter().take(2).enumerate() {
                plumes[i] = to.as_vec3().extend(v.activity);
                let age = (self.time - v.started).max(0.0) as f32;
                let wind = snapshot.sample(v.dir).wind;
                plume_wind[i] = wind.extend(age);
            }
        }
        // The lights only show from places they can be seen.
        let altitude = (rel.length() - self.planet.sea_radius) as f32;
        let visible = !at_home || rel.normalize().y.abs() > 0.5 || altitude > 8_000.0;
        let pulse = 0.72 + 0.28 * ((self.time * 0.0071).sin() * (self.time * 0.0173).cos()) as f32;
        let params = CloudParams {
            rotation,
            base: CLOUD_BASE,
            thickness: CLOUD_THICKNESS,
            offset: fx.offset,
            flashes,
            tornadoes,
            plumes,
            plume_wind,
            flash_light,
            aurora: if visible { pulse } else { 0.0 },
        };
        let roof = (fx.roof.active && self.active == Body::Planet).then(|| RoofMap {
            center: self.planet.center + fx.roof.center,
            u: fx.roof.u.as_vec3(),
            v: fx.roof.v.as_vec3(),
            cell: ROOF_CELL,
            heights: fx.roof.heights.clone(),
        });
        (params, vortices, roof)
    }

    /// `weather [condition|auto]`, `erupt`.
    pub(super) fn weather_command(&mut self, args: &[&str]) {
        let rel = self.camera.position - self.planet.center;
        let dir = rel.try_normalize().unwrap_or(DVec3::Y).as_vec3();
        let force = match args.first().copied() {
            None => {
                let l = self.weather_fx.local;
                self.console.print(format!(
                    "{}: cover {:.2} rain {:.2} storm {:.2} dust {:.2} snow {:.2} fog {:.2} wind {:.1} m/s ({})",
                    l.condition().name(),
                    l.cover,
                    l.precip,
                    l.storm,
                    l.dust,
                    l.snow,
                    l.fog,
                    l.surface_wind().length(),
                    match self.weather_fx.forced {
                        Some(f) => format!("held: {f:?}"),
                        None => "natural".into(),
                    }
                ));
                return;
            }
            Some("auto") => {
                self.weather.release();
                self.weather_fx.forced = None;
                self.console.print("Weather back to natural");
                return;
            }
            Some("approach") => {
                // Puts the player `m` metres from the nearest tornado (for testing).
                let distance = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(140.0);
                let tornadoes = self.live_tornadoes(rel);
                let Some(t) = tornadoes.first() else {
                    self.console.print("No tornado near here");
                    return;
                };
                let d = rel - t.foot;
                let side = (d - t.up * d.dot(t.up)).try_normalize().unwrap_or(DVec3::X);
                let at = (t.foot + side * distance).normalize();
                let ground = Self::ground_position(&self.planet, &self.terrain, at);
                let frame = self.active_frame(self.time);
                self.player.teleport(ground + at * 1.2, &frame);
                return;
            }
            Some("strike") => {
                // A bolt straight ahead (for testing).
                let forward = self.camera.forward().as_dvec3();
                let at = (rel + forward * 1_400.0).normalize();
                self.strike(at, 1_400.0);
                return;
            }
            Some("clear") => Force::Clear,
            Some("fair") => Force::Fair,
            Some("cloudy") => Force::Cloudy,
            Some("fog") => Force::Fog,
            Some("drizzle") => Force::Drizzle,
            Some("rain") => Force::Rain,
            Some("storm") | Some("thunder") => Force::Storm,
            Some("snow") => Force::Snow,
            Some("blizzard") => Force::Blizzard,
            Some("sandstorm") | Some("sand") => Force::Sandstorm,
            Some("tornado") => Force::Tornado,
            Some(_) => {
                self.console.print(
                    "Usage: weather [auto|clear|cloudy|fog|drizzle|rain|storm|snow|blizzard|sandstorm|tornado]",
                );
                return;
            }
        };
        self.weather.force(dir, force);
        self.weather_fx.forced = Some(force);
        self.console.print(format!(
            "Holding {force:?} here (`weather auto` to release)"
        ));
    }

    pub(super) fn erupt_command(&mut self) {
        let rel = self.camera.position - self.planet.center;
        let dir = rel.try_normalize().unwrap_or(DVec3::Y).as_vec3();
        self.weather.erupt(dir, self.time);
        let snapshot = self.weather.snapshot.clone();
        let nearest = snapshot
            .volcanoes
            .iter()
            .max_by(|a, b| a.dir.dot(dir).total_cmp(&b.dir.dot(dir)));
        match nearest {
            Some(v) => {
                let km =
                    v.dir.dot(dir).clamp(-1.0, 1.0).acos() * self.planet.sea_radius as f32 / 1000.0;
                let latitude = v.dir.y.asin().to_degrees();
                let longitude = v.dir.z.atan2(v.dir.x).to_degrees();
                self.console.print(format!(
                    "The nearest volcano erupts, {km:.1} km away (ASTRO_SPAWN={latitude:.2},{longitude:.2})"
                ));
            }
            None => self.console.print("No volcanoes on this world"),
        }
    }

    /// Where the nearest tornado's funnel or volcano's summit is (planet-relative).
    pub(super) fn weather_target(&self, what: &str) -> Option<DVec3> {
        let rel = self.camera.position - self.planet.center;
        if what == "tornado" {
            return self
                .live_tornadoes(rel)
                .first()
                .map(|t| t.foot + t.up * 90.0);
        }
        let dir = rel.try_normalize()?.as_vec3();
        let snapshot = &self.weather.snapshot;
        let v = snapshot
            .volcanoes
            .iter()
            .max_by(|a, b| a.dir.dot(dir).total_cmp(&b.dir.dot(dir)))?;
        Some(v.dir.as_dvec3() * (self.planet.sea_radius + v.summit as f64))
    }

    /// A line for the HUD: what the sky is doing here.
    pub(super) fn weather_line(&self) -> Option<String> {
        if self.active != Body::Planet {
            return None;
        }
        let l = self.weather_fx.local;
        let condition = if self.weather_fx.local.condition() == Condition::Clear && l.cover < 0.05 {
            "clear".to_string()
        } else {
            l.condition().name().to_string()
        };
        let rel = self.camera.position - self.planet.center;
        let eruption = self
            .nearest_eruption(rel)
            .filter(|(d, _)| *d < 40_000.0)
            .map_or(String::new(), |(d, way)| {
                format!("   eruption {:.1} km {way}", d / 1000.0)
            });
        Some(format!(
            "{condition}   wind {:.0} m/s{eruption}",
            l.surface_wind().length()
        ))
    }
}

/// Trees and such: a funnel takes several at a time.
fn is_plant(block: Block) -> bool {
    use Block::*;
    matches!(
        block,
        OakLeaves | SpruceLeaves | JungleLeaves | OakLog | SpruceLog | Cactus
    )
}

/// What a tornado can lift: loose and light things (not stone, not models with contents).
fn can_be_ripped(block: Block) -> bool {
    use Block::*;
    is_plant(block)
        || matches!(
            block,
            OakPlanks
                | SprucePlanks
                | Cobblestone
                | Grass
                | SnowyGrass
                | Dirt
                | Sand
                | RedSand
                | Snow
                | Gravel
                | Ash
                | Glass
                | WhiteWool
        )
}

/// A block of the loose ground of `biome`, for blocks only thrown for show.
fn loose_ground(biome: Biome, roll: f32) -> Block {
    match biome {
        Biome::Beach | Biome::Desert => Block::Sand,
        Biome::Badlands => Block::RedSand,
        Biome::Tundra | Biome::IceSheet | Biome::Taiga => Block::SnowyGrass,
        Biome::Volcanic => Block::Ash,
        _ if roll < 0.2 => Block::OakLog,
        _ if roll < 0.4 => Block::OakLeaves,
        _ if roll < 0.6 => Block::Dirt,
        _ => Block::Grass,
    }
}
