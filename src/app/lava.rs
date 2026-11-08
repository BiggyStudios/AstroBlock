//! Lava on the move.
//!
//! When a volcano erupts, rivers of lava start at the rim and run downhill. Each follows a
//! path found on the terrain as it goes (steepest descent), with a front that advances along
//! it. Only chunks near the player exist as blocks, so the rivers live in two forms:
//! - Near the player the front pours real `Block::Lava` over the ground (burning the trees it
//!   meets, turning to basalt in water). Coming back later, what was missed is poured then.
//! - Further away, where only the far terrain exists, a stream of glowing GPU particles runs
//!   down the same path.
//!
//! After the eruption the rivers cool to basalt from the far end up, and stay as scars.
//! Any other lava with a way out runs too (`leaks`): dig next to a crater lake, or get hit
//! by a lava bomb, and it spreads like a fluid, falling first, then sideways for a few blocks.

use std::collections::VecDeque;

use glam::DVec3;

use super::Game;
use crate::{
    effects::{
        self,
        debris::{FlyingBlock, Landing},
    },
    world::{block::Block, chunk::BlockCell, light, terrain::TerrainGenerator},
};

/// Distance between points of a river's path, and how far ahead the terrain is probed (m).
const PATH_STEP: f64 = 3.0;
const PROBE: f64 = 9.0;
/// A river stops after this long, or this long on the flat (m).
const MAX_LENGTH: f64 = 2_400.0;
const MAX_FLAT: f64 = 70.0;
/// How often blocks change (s), and how many a tick at most.
const TICK: f32 = 0.1;
const POURS_PER_TICK: usize = 90;
const LEAKS_PER_TICK: usize = 24;
const COOLS_PER_TICK: usize = 60;
/// Beyond this from the camera chunks aren't loaded, and a river is particles (m).
const POUR_RANGE: f64 = 372.0;
const STREAM_RANGE: f64 = 14_000.0;
/// How many path points per second get a new glowing particle, at most per frame.
const STREAM_RATE: f32 = 620.0;
const STREAM_PER_FRAME: usize = 60;
/// How far a leak spreads sideways (blocks), and how long loose lava stays molten (game s).
const LEAK_REACH: u8 = 6;
const LOOSE_LIFE: f64 = 70.0;
/// How far a player can be and still see lava bombs (m).
const BOMB_RANGE: f64 = 4_000.0;

struct Point {
    dir: DVec3,
    height: f64,
    along: f64,
    /// Poured as real blocks already.
    poured: bool,
}

struct River {
    points: Vec<Point>,
    heading: DVec3,
    front: f64,
    speed: f64,
    width: f64,
    wiggle: f64,
    flat: f64,
    ended: bool,
}

struct Flow {
    volcano: usize,
    /// Still being fed by the eruption.
    active: bool,
    rivers: Vec<River>,
    /// The blocks poured so far, oldest first, then (after the eruption) when each cools.
    placed: Vec<BlockCell>,
    cooling: VecDeque<(f64, BlockCell)>,
    /// How bright the far stream still is, 0..1.
    glow: f32,
}

pub(super) struct LavaFlows {
    flows: Vec<Flow>,
    erupting: Vec<bool>,
    /// Molten blocks that got out on their own: when they cool.
    loose: VecDeque<(f64, BlockCell)>,
    /// Cells lava is about to run into, and how far it can still spread.
    leaks: VecDeque<(BlockCell, u8)>,
    tick: f32,
    bombs: f32,
    stream: f32,
    stream_cursor: usize,
    rng: u64,
}

impl Default for LavaFlows {
    fn default() -> Self {
        Self {
            flows: Vec::new(),
            erupting: Vec::new(),
            loose: VecDeque::new(),
            leaks: VecDeque::new(),
            tick: 0.0,
            bombs: 0.0,
            stream: 0.0,
            stream_cursor: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

impl LavaFlows {
    /// Everything still molten that will have to cool (for saving).
    pub(super) fn molten(&self) -> Vec<BlockCell> {
        let flows = self.flows.iter().flat_map(|f| {
            f.placed
                .iter()
                .copied()
                .chain(f.cooling.iter().map(|c| c.1))
        });
        flows.chain(self.loose.iter().map(|c| c.1)).collect()
    }

    /// After loading: what was molten cools down over the next few minutes. (A river that
    /// was running when the game was saved stops there.)
    pub(super) fn resume_cooling(&mut self, cells: Vec<BlockCell>, now: f64) {
        if cells.is_empty() {
            return;
        }
        let count = cells.len() as f64;
        let cooling = cells
            .into_iter()
            .enumerate()
            .map(|(i, cell)| (now + 30.0 + 220.0 * i as f64 / count, cell))
            .collect();
        self.flows.push(Flow {
            volcano: usize::MAX,
            active: false,
            rivers: Vec::new(),
            placed: Vec::new(),
            cooling,
            glow: 0.0,
        });
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn rolls(&mut self) -> [f32; 4] {
        std::array::from_fn(|_| self.random() as f32)
    }
}

impl River {
    fn start(dir: DVec3, height: f64, heading: DVec3, speed: f64, width: f64, wiggle: f64) -> Self {
        Self {
            points: vec![Point {
                dir,
                height,
                along: 0.0,
                poured: false,
            }],
            heading,
            front: 0.0,
            speed,
            width,
            wiggle,
            flat: 0.0,
            ended: false,
        }
    }

    /// One more point of the path: wherever the ground falls away fastest ahead.
    fn extend(&mut self, terrain: &TerrainGenerator, radius: f64) {
        let last = self.points.last().expect("a river has a start");
        let (dir, height, along) = (last.dir, last.height, last.along);
        if along >= MAX_LENGTH || height < 0.6 || self.flat > MAX_FLAT {
            self.ended = true;
            return;
        }
        let heading = (self.heading - dir * self.heading.dot(dir)).normalize_or(self.heading);
        let side = dir.cross(heading);
        let here = dir * radius;
        let drift = (along * 0.027 + self.wiggle).sin() * 0.3;
        let mut best = (f64::MAX, f64::MAX, heading);
        for i in -4i32..=4 {
            let angle = i as f64 * 0.34 + drift;
            let candidate = heading * angle.cos() + side * angle.sin();
            let probe = (here + candidate * PROBE).normalize();
            let h = terrain.sample_column(probe).height;
            // (Going straight on wins ties, so it doesn't jitter on the flat.)
            let score = h + i.abs() as f64 * 0.08;
            if score < best.0 {
                best = (score, h, candidate);
            }
        }
        // (Small dips it fills and runs out of: only a real basin stops it.)
        if best.1 > height + 6.0 {
            self.ended = true;
            return;
        }
        if best.1 > height - 0.05 {
            self.flat += PATH_STEP;
        } else {
            self.flat = 0.0;
        }
        self.heading = best.2;
        let next = (here + best.2 * PATH_STEP).normalize();
        self.points.push(Point {
            dir: next,
            height: terrain.sample_column(next).height,
            along: along + PATH_STEP,
            poured: false,
        });
    }

    /// Half the river's width (blocks) at a point: it swells and narrows.
    fn half_width(&self, along: f64) -> f64 {
        self.width * (0.8 + 0.35 * (along * 0.09 + self.wiggle).sin())
    }
}

impl Game {
    /// Advances all lava (home only). `dt` is real seconds.
    pub(super) fn update_lava(&mut self, dt: f32, rel: DVec3) {
        let game_dt = dt * self.effective_warp.min(30.0) as f32;
        let snapshot = self.weather.snapshot.clone();
        if self.lava.erupting.len() != snapshot.volcanoes.len() {
            self.lava.erupting = vec![false; snapshot.volcanoes.len()];
        }
        for (i, v) in snapshot.volcanoes.iter().enumerate() {
            let erupting = v.activity > 0.12;
            if erupting != self.lava.erupting[i] {
                self.lava.erupting[i] = erupting;
                if erupting {
                    // (Rivers are worked out by each player alone, so only one weather may
                    // make them, or the same blocks wouldn't turn to lava for everyone.)
                    if self.net.lava_shared() {
                        self.start_flow(i);
                    }
                    let top = v.dir.as_dvec3() * (self.planet.sea_radius + v.summit as f64);
                    self.eruption_begins(top, (top - rel).length());
                } else {
                    self.end_flow(i);
                }
            }
        }

        let radius = self.planet.sea_radius;
        for flow in &mut self.lava.flows {
            if flow.active {
                for river in &mut flow.rivers {
                    let last = river.points.last().map_or(0.0, |p| p.along);
                    if !river.ended || river.front < last {
                        river.front += river.speed * game_dt as f64;
                    }
                    for _ in 0..32 {
                        let last = river.points.last().map_or(0.0, |p| p.along);
                        if river.ended || last > river.front + PATH_STEP {
                            break;
                        }
                        river.extend(&self.terrain, radius);
                    }
                    let last = river.points.last().map_or(0.0, |p| p.along);
                    river.front = river.front.min(last);
                }
            } else {
                flow.glow = (flow.glow - game_dt / 45.0).max(0.0);
            }
        }

        self.lava_bombs(dt, rel);
        self.lava_stream(dt, rel);
        self.notice_openings();
        self.lava.tick -= dt;
        if self.lava.tick <= 0.0 {
            self.lava.tick = TICK;
            self.pour_rivers(rel);
            self.run_leaks();
            self.cool_lava();
        }
        // (Our own edits don't open anything up.)
        self.streamer.changes.clear();
    }

    fn start_flow(&mut self, volcano: usize) {
        let radius = self.planet.sea_radius;
        let Some(v) = self.terrain.volcanoes.get(volcano) else {
            return;
        };
        let (dir, crater) = (v.dir, v.crater);
        let east = DVec3::Y.cross(dir).try_normalize().unwrap_or(DVec3::X);
        let north = dir.cross(east);
        // Different rivers each time, but the same ones for every player.
        let started = self
            .weather
            .snapshot
            .volcanoes
            .get(volcano)
            .map_or(0, |v| v.started.to_bits());
        self.lava.rng = (started ^ (volcano as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;
        for _ in 0..8 {
            self.lava.random();
        }
        let base = self.lava.random() * std::f64::consts::TAU;
        let count = 3 + (self.lava.random() * 3.0) as usize;
        let mut rivers = Vec::with_capacity(count);
        for i in 0..count {
            let angle =
                base + std::f64::consts::TAU * (i as f64 + self.lava.random() * 0.6) / count as f64;
            let heading = east * angle.cos() + north * angle.sin();
            let start = (dir * radius + heading * crater * 1.03).normalize();
            let height = self.terrain.sample_column(start).height;
            let speed = 4.5 + self.lava.random() * 3.5;
            let width = 2.2 + self.lava.random() * 2.0;
            let wiggle = self.lava.random() * 100.0;
            rivers.push(River::start(start, height, heading, speed, width, wiggle));
        }
        self.lava.flows.push(Flow {
            volcano,
            active: true,
            rivers,
            placed: Vec::new(),
            cooling: VecDeque::new(),
            glow: 1.0,
        });
    }

    /// The eruption is over: rivers stop, and start cooling from their far ends.
    fn end_flow(&mut self, volcano: usize) {
        let now = self.time;
        for flow in &mut self.lava.flows {
            if flow.volcano != volcano || !flow.active {
                continue;
            }
            flow.active = false;
            let count = flow.placed.len().max(1) as f64;
            for (i, cell) in flow.placed.drain(..).rev().enumerate() {
                flow.cooling
                    .push_back((now + 30.0 + 220.0 * i as f64 / count, cell));
            }
        }
    }

    /// Turns what the rivers have reached into blocks, where chunks are loaded.
    fn pour_rivers(&mut self, rel: DVec3) {
        let mut flows = std::mem::take(&mut self.lava.flows);
        let radius = self.planet.sea_radius;
        let mut budget = POURS_PER_TICK;
        for flow in flows.iter_mut().filter(|f| f.active) {
            let placed = &mut flow.placed;
            for river in &mut flow.rivers {
                let widths: Vec<f64> = river
                    .points
                    .iter()
                    .map(|p| river.half_width(p.along))
                    .collect();
                for (point, half) in river.points.iter_mut().zip(widths) {
                    if point.along > river.front {
                        break;
                    }
                    if point.poured || budget == 0 {
                        continue;
                    }
                    let at = point.dir * (radius + point.height);
                    if (at - rel).length() > POUR_RANGE {
                        continue;
                    }
                    let (done, used) = self.pour_disc(at, half, placed);
                    budget = budget.saturating_sub(used);
                    point.poured = done;
                }
                // The advancing tip smokes and spits.
                let tip = river.points.iter().rfind(|p| p.along <= river.front);
                if !river.ended
                    && let Some(tip) = tip
                    && (tip.dir * radius - rel).length() < 1_400.0
                {
                    let at = tip.dir * (radius + tip.height + 1.5);
                    self.survival
                        .effects
                        .queue
                        .extend(effects::lava_front(at, tip.dir, 3));
                }
            }
        }
        flows.retain(|f| f.active || !f.cooling.is_empty() || f.glow > 0.0);
        self.lava.flows = flows;
    }

    /// Pours a round patch of lava over the ground. Returns whether all of it was loaded,
    /// and how many blocks it used.
    fn pour_disc(&mut self, at: DVec3, half: f64, placed: &mut Vec<BlockCell>) -> (bool, usize) {
        let Some(center) = self.planet.cell_at(at) else {
            return (true, 0);
        };
        let reach = half.ceil() as i32;
        let (mut used, mut complete) = (0, true);
        for dv in -reach..=reach {
            for du in -reach..=reach {
                if ((du * du + dv * dv) as f64) > half * half {
                    continue;
                }
                let Some(column) =
                    self.planet
                        .cell(center.face, center.u + du, center.v + dv, center.k)
                else {
                    continue;
                };
                match self.pour_column(column) {
                    Pour::NotLoaded => complete = false,
                    Pour::Nothing => {}
                    Pour::Placed(cell) => {
                        placed.push(cell);
                        used += 1;
                    }
                }
            }
        }
        (complete, used)
    }

    /// Finds the ground in the column around `cell` and puts lava on it. What burns, burns.
    fn pour_column(&mut self, cell: BlockCell) -> Pour {
        for k in (cell.k - 18..=cell.k + 16).rev() {
            let c = BlockCell { k, ..cell };
            let Some(block) = self.streamer.block(c) else {
                return Pour::NotLoaded;
            };
            match block {
                Block::Air | Block::Water => continue,
                Block::Lava => return Pour::Nothing,
                b if burns(b) => {
                    self.streamer.set_block(c, Block::Air);
                    continue;
                }
                _ => {}
            }
            let above = BlockCell { k: k + 1, ..cell };
            return match self.streamer.block(above) {
                Some(Block::Water) => {
                    self.streamer.set_block(above, Block::Basalt);
                    Pour::Nothing
                }
                Some(_) => {
                    self.streamer.set_block(above, Block::Lava);
                    Pour::Placed(above)
                }
                None => Pour::NotLoaded,
            };
        }
        Pour::Nothing
    }

    /// Lava beside a cell that has just opened up runs into it.
    fn notice_openings(&mut self) {
        if self.streamer.changes.is_empty() {
            return;
        }
        let changes = std::mem::take(&mut self.streamer.changes);
        for cell in changes {
            let beside_lava = light::neighbours(&self.planet, cell)
                .into_iter()
                .any(|n| n.k >= cell.k && self.streamer.block(n) == Some(Block::Lava));
            if beside_lava {
                self.lava.leaks.push_back((cell, LEAK_REACH));
            }
        }
    }

    /// One step of loose lava: down if it can, else out to the sides, towards any drop.
    fn run_leaks(&mut self) {
        let free = |game: &Game, c: BlockCell| {
            game.streamer
                .block(c)
                .is_some_and(|b| !b.is_solid() && b != Block::Water)
        };
        let below = |game: &Game, c: BlockCell| game.planet.cell(c.face, c.u, c.v, c.k - 1);
        for _ in 0..LEAKS_PER_TICK {
            let Some((cell, reach)) = self.lava.leaks.pop_front() else {
                return;
            };
            match self.streamer.block(cell) {
                Some(Block::Water) => {
                    self.streamer.set_block(cell, Block::Basalt);
                    continue;
                }
                Some(b) if !b.is_solid() => {}
                _ => continue,
            }
            self.streamer.set_block(cell, Block::Lava);
            let due = self.time + LOOSE_LIFE * (0.7 + 0.6 * self.lava.random());
            self.lava.loose.push_back((due, cell));
            if self.lava.leaks.len() > 3_000 {
                continue;
            }
            if let Some(down) = below(self, cell)
                && free(self, down)
            {
                self.lava.leaks.push_back((down, reach));
                continue;
            }
            if reach == 0 {
                continue;
            }
            let sides: Vec<BlockCell> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .into_iter()
                .filter_map(|(du, dv)| {
                    self.planet
                        .cell(cell.face, cell.u + du, cell.v + dv, cell.k)
                })
                .filter(|&c| free(self, c))
                .collect();
            let drops: Vec<BlockCell> = sides
                .iter()
                .copied()
                .filter(|&c| below(self, c).is_some_and(|d| free(self, d)))
                .collect();
            for c in if drops.is_empty() { sides } else { drops } {
                self.lava.leaks.push_back((c, reach - 1));
            }
        }
    }

    fn cool_lava(&mut self) {
        let now = self.time;
        let mut budget = COOLS_PER_TICK;
        let mut flows = std::mem::take(&mut self.lava.flows);
        for flow in &mut flows {
            while budget > 0 && flow.cooling.front().is_some_and(|c| c.0 <= now) {
                let (_, cell) = flow.cooling.pop_front().expect("just looked");
                self.harden(cell);
                budget -= 1;
            }
        }
        self.lava.flows = flows;
        while budget > 0 && self.lava.loose.front().is_some_and(|c| c.0 <= now) {
            let (_, cell) = self.lava.loose.pop_front().expect("just looked");
            self.harden(cell);
            budget -= 1;
        }
    }

    fn harden(&mut self, cell: BlockCell) {
        if self.streamer.block(cell).is_none_or(|b| b == Block::Lava) {
            self.streamer.set_block(cell, Block::Basalt);
        }
    }

    /// A lava bomb came down at `at` (planet-relative): it runs a little where it lands.
    pub(super) fn lava_splat(&mut self, at: DVec3) {
        let up = at.normalize_or(DVec3::Y);
        if let Some(cell) = self.planet.cell_at(at + up * 0.8) {
            self.lava.leaks.push_back((cell, 1));
        }
        self.survival
            .effects
            .queue
            .push(effects::lava_splash(at, up));
    }

    /// Glowing rocks thrown out of an erupting crater on high arcs.
    fn lava_bombs(&mut self, dt: f32, rel: DVec3) {
        let snapshot = self.weather.snapshot.clone();
        let radius = self.planet.sea_radius;
        let mut rate = 0.0;
        let mut source = None;
        for v in &snapshot.volcanoes {
            if v.activity < 0.15 {
                continue;
            }
            let dir = v.dir.as_dvec3();
            let at = dir * (radius + v.vent as f64);
            if (at - rel).length() < BOMB_RANGE {
                rate = 9.0 * v.activity;
                source = Some((at, dir));
                break;
            }
        }
        let Some((at, up)) = source else {
            return;
        };
        let east = DVec3::Y.cross(up).try_normalize().unwrap_or(DVec3::X);
        let north = up.cross(east);
        let n = rate * dt + self.lava.bombs;
        self.lava.bombs = n.fract();
        for _ in 0..n as u32 {
            let angle = self.lava.random() * std::f64::consts::TAU;
            let out = 12.0 + self.lava.random() * 62.0;
            let rise = 85.0 + self.lava.random() * 75.0;
            let velocity = (east * angle.cos() + north * angle.sin()) * out + up * rise;
            let spot = at + up * 8.0 + east * (self.lava.random() - 0.5) * 60.0;
            let size = if self.lava.random() < 0.35 { 2 } else { 1 };
            let rolls = self.lava.rolls();
            let block = FlyingBlock::new(Block::Lava, spot, velocity, size, Landing::Burn, rolls);
            self.debris.push(block);
        }
    }

    /// The rivers as glowing particles, where no blocks show them.
    fn lava_stream(&mut self, dt: f32, rel: DVec3) {
        let radius = self.planet.sea_radius;
        let n = STREAM_RATE * dt + self.lava.stream;
        self.lava.stream = n.fract();
        let wanted = (n as usize).min(STREAM_PER_FRAME);
        if wanted == 0 || self.lava.flows.is_empty() {
            return;
        }
        // Every fourth point of every river that the lava has reached, in turn.
        let total: usize = self
            .lava
            .flows
            .iter()
            .filter(|f| f.glow > 0.0)
            .flat_map(|f| &f.rivers)
            .map(|r| r.points.iter().take_while(|p| p.along <= r.front).count() / 4)
            .sum();
        if total == 0 {
            return;
        }
        let mut queue = Vec::with_capacity(wanted);
        let mut cursor = self.lava.stream_cursor;
        for _ in 0..wanted {
            cursor = (cursor + 1) % total;
            let mut left = cursor;
            'find: for flow in self.lava.flows.iter().filter(|f| f.glow > 0.0) {
                for river in &flow.rivers {
                    let reached = river
                        .points
                        .iter()
                        .take_while(|p| p.along <= river.front)
                        .count();
                    if left >= reached / 4 {
                        left -= reached / 4;
                        continue;
                    }
                    let point = &river.points[left * 4];
                    let surface = point.dir * (radius + point.height);
                    let distance = (surface - rel).length();
                    let inside = point.poured && distance < POUR_RANGE;
                    if !inside && distance < STREAM_RANGE {
                        let next = &river.points[(left * 4 + 4).min(reached - 1)];
                        let along = ((next.dir * (radius + next.height) - surface)
                            .try_normalize()
                            .unwrap_or(DVec3::ZERO))
                        .as_vec3();
                        let size = (distance as f32 * 0.0052).clamp(3.5, 40.0);
                        // Far terrain is coarse: stay clear of its steps.
                        let lift = (distance * 0.003).clamp(1.5, 26.0);
                        let at = surface + point.dir * lift;
                        queue.push(effects::lava_stream(at, along, size, flow.glow));
                    }
                    break 'find;
                }
            }
        }
        self.lava.stream_cursor = cursor;
        self.survival.effects.queue.extend(queue);
    }
}

enum Pour {
    NotLoaded,
    Nothing,
    Placed(BlockCell),
}

/// What lava burns away instead of flowing around.
fn burns(block: Block) -> bool {
    use Block::*;
    matches!(
        block,
        OakLeaves
            | SpruceLeaves
            | JungleLeaves
            | OakLog
            | SpruceLog
            | OakPlanks
            | SprucePlanks
            | Cactus
            | WhiteWool
            | Torch
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{planet::Planet, textures::Tex};
    use std::sync::Arc;

    /// `shaders/block.frag` must know which texture layer is lava.
    #[test]
    fn lava_layer_matches_the_shader() {
        let source = include_str!("../../shaders/block.frag");
        let wanted = format!("const uint LAVA_LAYER = {}u;", Tex::Lava as u32);
        assert!(
            source.contains(&wanted),
            "expected `{wanted}` in block.frag"
        );
    }

    /// A river from a volcano's rim runs a long way, and only ever downhill overall.
    #[test]
    fn rivers_run_down_the_mountain() {
        let planet = Arc::new(Planet::new(DVec3::ZERO, 2048, 128, 7));
        let terrain = TerrainGenerator::new(planet.clone());
        assert!(!terrain.volcanoes.is_empty());
        let radius = planet.sea_radius;
        for v in terrain.volcanoes.iter().take(6) {
            let east = DVec3::Y.cross(v.dir).normalize();
            let start = (v.dir * radius + east * v.crater * 1.03).normalize();
            let height = terrain.sample_column(start).height;
            let mut river = River::start(start, height, east, 5.0, 3.0, 1.0);
            while !river.ended {
                river.extend(&terrain, radius);
            }
            let end = river.points.last().unwrap();
            assert!(end.along > 300.0, "stopped after {} m", end.along);
            assert!(
                end.height < height - 80.0,
                "from {height:.0} m only down to {:.0} m",
                end.height
            );
        }
    }
}
