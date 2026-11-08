//! Multithreaded chunk streaming around the camera.
//!
//! * The main thread decides *which* chunks should exist (a sphere around the camera,
//!   found by flood-filling chunk neighbours - which naturally crosses cube-face edges) and
//!   sorts them nearest-first.
//! * A pool of worker threads generates + meshes chunks. Only a bounded number of jobs is
//!   queued at a time, so when the camera moves, newly-near chunks jump the queue quickly.
//! * Finished meshes are uploaded within a per-frame budget to avoid hitches, and chunks
//!   that fall out of range are unloaded (with some hysteresis).

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use glam::DVec3;

use super::{
    block::Block,
    chunk::{BlockCell, ChunkBlocks, ChunkKey},
    edits::WorldEdits,
    light::Lighting,
    lod::{self, LodKey, LodResult},
    planet::{CHUNK_SIZE, Face, Planet},
    terrain::{ChunkResult, TerrainGenerator},
};
use crate::{
    render::{DrawItem, Fade, MeshHandle, Renderer},
    util::{FastMap, FastSet},
};

/// Conservative bounding radius of a chunk (half-diagonal of a 32^3 block plus distortion).
const CHUNK_BOUND: f64 = 30.0;
/// Chunks are only unloaded once this much further out than the load radius.
const UNLOAD_MARGIN: f64 = 48.0;
/// Max chunk meshes uploaded to the GPU per frame.
const UPLOADS_PER_FRAME: usize = 48;
/// Extra effective distance per meter a chunk lies below the local terrain floor.
const UNDERGROUND_PENALTY: f64 = 2.0;
/// Load radius while the camera is deep underground.
pub const UNDERGROUND_RADIUS: f64 = 192.0;
/// How far below the local surface the camera must be to count as underground.
const UNDERGROUND_DEPTH: f64 = 24.0;

/// Max LOD tile meshes uploaded per frame.
const LOD_UPLOADS_PER_FRAME: usize = 24;
/// The LOD quadtree is re-selected after the camera moves this far.
const LOD_RESELECT_DISTANCE: f64 = 16.0;
/// Dithered cross-fade durations: chunks appearing over / disappearing into the LOD
/// terrain, and LOD tiles swapping detail levels.
const CHUNK_FADE: f32 = 0.35;
const LOD_FADE: f32 = 0.5;

struct LoadedChunk {
    blocks: ChunkBlocks,
    mesh: Option<MeshHandle>,
    origin: DVec3,
    radius: f32,
    /// When the mesh was uploaded (it fades in from then).
    shown: Instant,
}

/// An unloaded chunk mesh that is still fading out.
struct RetiringMesh {
    mesh: MeshHandle,
    origin: DVec3,
    radius: f32,
    since: Instant,
}

/// Seconds since `t`, as f32.
fn secs_since(now: Instant, t: Instant) -> f32 {
    now.saturating_duration_since(t).as_secs_f32()
}

/// `now` moved back by `secs` (clamped to `now` if the clock can't go that far back).
fn secs_before(now: Instant, secs: f32) -> Instant {
    now.checked_sub(Duration::from_secs_f32(secs.max(0.0)))
        .unwrap_or(now)
}

struct LodTile {
    mesh: Option<MeshHandle>,
    origin: DVec3,
    radius: f32,
}

enum Job {
    /// Generate a chunk; the number is its edit version (see `WorldStreamer::versions`).
    Chunk(ChunkKey, u32),
    Lod(LodKey),
}

enum JobResult {
    Chunk(ChunkResult, u32),
    Lod(LodResult),
}

pub struct StreamStats {
    pub loaded: usize,
    pub desired: usize,
    pub in_flight: usize,
    pub workers: usize,
    pub lod_desired: usize,
}

pub struct WorldStreamer {
    /// Cells emptied by edits since the app last looked (lava may run into them).
    pub changes: Vec<BlockCell>,
    planet: Arc<Planet>,
    terrain: Arc<TerrainGenerator>,
    load_radius: f64,

    job_tx: Option<Sender<Job>>,
    /// Jobs that jump the queue: remeshing chunks after block edits.
    priority_tx: Option<Sender<Job>>,
    result_rx: Receiver<JobResult>,
    shutdown: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    max_in_flight: usize,

    chunks: FastMap<ChunkKey, LoadedChunk>,
    /// Chunks being generated (or generated and waiting for upload), with their distance.
    in_flight: FastMap<ChunkKey, f64>,
    ready: VecDeque<(ChunkResult, u32)>,
    /// Desired chunks not yet loaded or in flight, nearest first.
    queue: VecDeque<(ChunkKey, f64)>,
    /// Chunks that should be loaded, nearest first, with their distance to the camera.
    desired: Vec<(ChunkKey, f64)>,
    desired_set: FastSet<ChunkKey>,
    centre: Option<ChunkKey>,
    last_maintenance: Instant,
    /// Recently uploaded chunks (distance, upload time): until they have faded in, they
    /// don't count as loaded for `loaded_radius`.
    fading_in: Vec<(f64, Instant)>,
    retiring: Vec<RetiringMesh>,

    // ---- Block edits ----
    edits: Arc<WorldEdits>,
    /// Bumped on every edit that affects a chunk; older generation results are discarded.
    versions: FastMap<ChunkKey, u32>,
    /// Chunks being remeshed after an edit (their results are uploaded first).
    remesh: FastSet<ChunkKey>,
    /// Block light (torches), shared with the workers for meshing.
    lighting: Lighting,

    // ---- LOD tiles (same idea as chunks, but a quadtree of surface tiles) ----
    max_lod_in_flight: usize,
    lod_tiles: FastMap<LodKey, LodTile>,
    lod_in_flight: FastMap<LodKey, f64>,
    lod_ready: VecDeque<LodResult>,
    lod_queue: VecDeque<(LodKey, f64)>,
    /// Current quadtree leaves, nearest first.
    lod_desired: Vec<(LodKey, f64)>,
    lod_set: FastSet<LodKey>,
    /// Every strict ancestor of a desired leaf.
    lod_internal: FastSet<LodKey>,
    /// Tiles being drawn, with the start of their current fade and whether it is a fade out.
    lod_visible: FastMap<LodKey, (Instant, bool)>,
    /// Set when the desired tiles or the loaded tiles changed.
    lod_dirty: bool,
    lod_camera: Option<DVec3>,
}

impl WorldStreamer {
    pub fn new(planet: Arc<Planet>, terrain: Arc<TerrainGenerator>, load_radius: f64) -> Self {
        // Leave a core for the main/render thread and one for the OS + driver.
        let worker_count =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).max(1));
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (priority_tx, priority_rx) = crossbeam_channel::unbounded::<Job>();
        let edits = Arc::new(WorldEdits::default());
        let lighting = Lighting::default();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));

        let workers = (0..worker_count)
            .map(|i| {
                let (job_rx, priority_rx) = (job_rx.clone(), priority_rx.clone());
                let result_tx = result_tx.clone();
                let (terrain, shutdown, edits) = (terrain.clone(), shutdown.clone(), edits.clone());
                let light = lighting.map.clone();
                thread::Builder::new()
                    .name(format!("worldgen-{i}"))
                    .spawn(move || {
                        loop {
                            // Priority jobs (edits) first.
                            let job = match priority_rx.try_recv() {
                                Ok(job) => Ok(job),
                                Err(_) => crossbeam_channel::select! {
                                    recv(priority_rx) -> job => job,
                                    recv(job_rx) -> job => job,
                                },
                            };
                            let Ok(job) = job else { break };
                            if shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            let result = match job {
                                Job::Chunk(key, version) => JobResult::Chunk(
                                    terrain.generate_chunk(key, &edits, &light),
                                    version,
                                ),
                                Job::Lod(key) => JobResult::Lod(lod::build_tile(&terrain, key)),
                            };
                            if result_tx.send(result).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("failed to spawn worldgen thread")
            })
            .collect();

        Self {
            changes: Vec::new(),
            planet,
            terrain,
            load_radius,
            job_tx: Some(job_tx),
            priority_tx: Some(priority_tx),
            result_rx,
            shutdown,
            workers,
            max_in_flight: worker_count * 4,
            chunks: FastMap::default(),
            in_flight: FastMap::default(),
            ready: VecDeque::new(),
            queue: VecDeque::new(),
            desired: Vec::new(),
            desired_set: FastSet::default(),
            centre: None,
            last_maintenance: Instant::now(),
            fading_in: Vec::new(),
            retiring: Vec::new(),
            edits,
            versions: FastMap::default(),
            remesh: FastSet::default(),
            lighting,
            max_lod_in_flight: worker_count * 2,
            lod_tiles: FastMap::default(),
            lod_in_flight: FastMap::default(),
            lod_ready: VecDeque::new(),
            lod_queue: VecDeque::new(),
            lod_desired: Vec::new(),
            lod_set: FastSet::default(),
            lod_internal: FastSet::default(),
            lod_visible: FastMap::default(),
            lod_dirty: true,
            lod_camera: None,
        }
    }

    /// Call once per frame. Chunks load around `focus` (the player or their craft); the LOD
    /// terrain is chosen for `lod_camera` (the same, except when looking from afar: the map).
    pub fn update(
        &mut self,
        focus: DVec3,
        lod_camera: DVec3,
        renderer: &mut Renderer,
    ) -> Result<()> {
        let rel = focus - self.planet.center;
        let lod_rel = lod_camera - self.planet.center;

        let centre = self.planet.chunk_at(rel);
        if self.centre != Some(centre) {
            self.centre = Some(centre);
            let t = Instant::now();
            self.recompute_desired(rel, centre);
            let t_recompute = t.elapsed();
            self.unload_distant(rel, renderer);
            log::debug!(
                "recompute {:?}, unload {:?}",
                t_recompute,
                t.elapsed() - t_recompute
            );
        }

        if self
            .lod_camera
            .is_none_or(|c| (c - lod_rel).length() > LOD_RESELECT_DISTANCE)
        {
            self.lod_camera = Some(lod_rel);
            self.reselect_lod(lod_rel);
        }

        // Collect finished work and upload a bounded number of meshes.
        for result in self.result_rx.try_iter() {
            match result {
                JobResult::Chunk(r, version) => {
                    if self.remesh.remove(&r.key) {
                        self.ready.push_front((r, version));
                    } else {
                        self.ready.push_back((r, version));
                    }
                }
                JobResult::Lod(r) => self.lod_ready.push_back(r),
            }
        }
        self.upload_lod(renderer)?;
        let mut uploads = 0;
        let upload_start = Instant::now();
        while uploads < UPLOADS_PER_FRAME {
            let Some((result, version)) = self.ready.pop_front() else {
                break;
            };
            if !self.desired_set.contains(&result.key) {
                self.in_flight.remove(&result.key);
                continue; // camera moved on while it was generating
            }
            if version != self.versions.get(&result.key).copied().unwrap_or(0) {
                continue; // generated before an edit; the newer job is on its way
            }
            let mesh = match &result.mesh {
                Some(m) => match renderer.create_mesh(m)? {
                    Some(handle) => {
                        uploads += 1;
                        Some(handle)
                    }
                    None => {
                        // This frame's upload budget is full; retry next frame.
                        self.ready.push_front((result, version));
                        break;
                    }
                },
                None => None,
            };
            let distance = self.in_flight.remove(&result.key).unwrap_or(0.0);
            // A remeshed chunk (after an edit) stays visible: no fade.
            let existing = self.chunks.get(&result.key).map(|c| c.shown);
            let shown = existing.unwrap_or_else(Instant::now);
            if mesh.is_some() && existing.is_none() {
                self.fading_in.push((distance, shown));
            }
            let chunk = LoadedChunk {
                blocks: result.blocks,
                mesh,
                origin: result.origin,
                radius: result.radius,
                shown,
            };
            if let Some(old) = self.chunks.insert(result.key, chunk).and_then(|c| c.mesh) {
                renderer.destroy_mesh(old);
            }
        }

        if uploads > 0 {
            log::debug!("{uploads} uploads {:?}", upload_start.elapsed());
        }

        // Finish fades.
        let now = Instant::now();
        self.fading_in
            .retain(|&(_, shown)| secs_since(now, shown) < CHUNK_FADE);
        self.retiring.retain(|r| {
            let done = secs_since(now, r.since) >= CHUNK_FADE;
            if done {
                renderer.destroy_mesh(r.mesh);
            }
            !done
        });

        // Queue more work, nearest chunks first.
        if let Some(tx) = &self.job_tx {
            while self.in_flight.len() < self.max_in_flight {
                let Some((key, d)) = self.queue.pop_front() else {
                    break;
                };
                if !self.chunks.contains_key(&key) && !self.in_flight.contains_key(&key) {
                    self.in_flight.insert(key, d);
                    let version = self.versions.get(&key).copied().unwrap_or(0);
                    let _ = tx.send(Job::Chunk(key, version));
                }
            }
            while self.lod_in_flight.len() < self.max_lod_in_flight {
                let Some((key, d)) = self.lod_queue.pop_front() else {
                    break;
                };
                if !self.lod_tiles.contains_key(&key) && !self.lod_in_flight.contains_key(&key) {
                    self.lod_in_flight.insert(key, d);
                    let _ = tx.send(Job::Lod(key));
                }
            }
        }
        self.update_lod_draw(renderer);

        if self.last_maintenance.elapsed() > Duration::from_secs(2) {
            self.last_maintenance = Instant::now();
            self.terrain.maintain_cache();
        }
        Ok(())
    }

    /// Flood-fills outward from the camera's chunk, collecting every chunk whose bounding
    /// sphere is within the load radius.
    fn recompute_desired(&mut self, rel: DVec3, centre: ChunkKey) {
        let reach = self.load_radius + CHUNK_BOUND;
        let floor = self.local_floor_radius(rel);
        // Chunks below the local terrain floor count as further away, so we load ~130 m of
        // caves under the camera instead of the full radius. Underground chunks are irregular
        // cave geometry, by far the most expensive to mesh and store.
        // Deep underground (inside the planet, away from the surface) only caves are around
        // and you can't see far: load a smaller sphere instead.
        let underground = self.camera_underground(rel);
        let distance = |key: ChunkKey| {
            let centre = self.planet.chunk_center(key);
            if underground {
                return (centre - rel).length() * self.load_radius / UNDERGROUND_RADIUS;
            }
            let below_floor = (floor - (centre.length() + CHUNK_BOUND)).max(0.0);
            (centre - rel).length() + below_floor * UNDERGROUND_PENALTY
        };

        self.desired.clear();
        self.desired_set.clear();
        let mut neighbours = Vec::with_capacity(8);
        let mut visited = FastSet::default();
        visited.insert(centre);
        let mut queue = VecDeque::new();
        let d = distance(centre);
        if d <= reach {
            queue.push_back((centre, d));
        }
        while let Some((key, d)) = queue.pop_front() {
            self.desired.push((key, d));
            self.desired_set.insert(key);
            neighbours.clear();
            self.planet.chunk_neighbors(key, &mut neighbours);
            for &n in &neighbours {
                if visited.insert(n) {
                    let dn = distance(n);
                    if dn <= reach {
                        queue.push_back((n, dn));
                    }
                }
            }
        }
        self.desired.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));

        // Rebuild the work queue and refresh distances of chunks already being generated.
        for d in self.in_flight.values_mut() {
            *d = f64::INFINITY;
        }
        self.queue.clear();
        for &(key, d) in &self.desired {
            if let Some(in_flight) = self.in_flight.get_mut(&key) {
                *in_flight = d;
            } else if !self.chunks.contains_key(&key) {
                self.queue.push_back((key, d));
            }
        }
    }

    /// Picks the LOD quadtree leaves for the camera position and queues missing tiles.
    fn reselect_lod(&mut self, rel: DVec3) {
        self.lod_desired.clear();
        lod::select_tiles(&self.planet, rel, &mut self.lod_desired);
        self.lod_desired
            .sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
        self.lod_set = self.lod_desired.iter().map(|t| t.0).collect();
        self.lod_internal.clear();
        let root = lod::root_level(&self.planet);
        for &(key, _) in &self.lod_desired {
            let mut k = key;
            while k.level < root {
                k = k.parent();
                if !self.lod_internal.insert(k) {
                    break;
                }
            }
        }
        self.lod_dirty = true;

        for d in self.lod_in_flight.values_mut() {
            *d = f64::INFINITY;
        }
        self.lod_queue.clear();
        for &(key, d) in &self.lod_desired {
            if let Some(in_flight) = self.lod_in_flight.get_mut(&key) {
                *in_flight = d;
            } else if !self.lod_tiles.contains_key(&key) {
                self.lod_queue.push_back((key, d));
            }
        }
    }

    fn upload_lod(&mut self, renderer: &mut Renderer) -> Result<()> {
        let mut uploads = 0;
        while uploads < LOD_UPLOADS_PER_FRAME {
            let Some(result) = self.lod_ready.pop_front() else {
                break;
            };
            if !self.lod_set.contains(&result.key) {
                self.lod_in_flight.remove(&result.key);
                continue;
            }
            let mesh = match &result.mesh {
                Some(m) => match renderer.create_mesh(m)? {
                    Some(handle) => {
                        uploads += 1;
                        Some(handle)
                    }
                    None => {
                        self.lod_ready.push_front(result);
                        break;
                    }
                },
                None => None,
            };
            self.lod_in_flight.remove(&result.key);
            self.lod_dirty = true;
            let tile = LodTile {
                mesh,
                origin: result.origin,
                radius: result.radius,
            };
            if let Some(old) = self.lod_tiles.insert(result.key, tile).and_then(|t| t.mesh) {
                renderer.destroy_mesh(old);
            }
        }
        Ok(())
    }

    /// Decides which tiles to draw and cross-fades between detail levels.
    ///
    /// A tile is only replaced by its 4 children once all of them are generated (until then
    /// the parent keeps standing in for them), and a desired tile that isn't generated yet is
    /// stood in for by loaded finer tiles. Tiles that change visibility fade in / out with a
    /// dither; the old and new tiles of a swap fade in lockstep, so they exactly complement
    /// each other. Tiles that are neither desired nor visible are unloaded.
    fn update_lod_draw(&mut self, renderer: &mut Renderer) {
        let now = Instant::now();
        let dirty = self.lod_dirty;
        if dirty {
            self.lod_dirty = false;
            let mut draw = Vec::new();
            let root = lod::root_level(&self.planet);
            for face in Face::ALL {
                self.resolve_lod(
                    LodKey {
                        face,
                        level: root,
                        tu: 0,
                        tv: 0,
                    },
                    &mut draw,
                );
            }
            let draw: FastSet<LodKey> = draw.into_iter().collect();
            for &key in &draw {
                match self.lod_visible.get_mut(&key) {
                    None => {
                        self.lod_visible.insert(key, (now, false));
                    }
                    Some(state) if state.1 => {
                        // Was fading out: fade back in from what is still shown.
                        let hidden = (secs_since(now, state.0) / LOD_FADE).min(1.0);
                        *state = (secs_before(now, (1.0 - hidden) * LOD_FADE), false);
                    }
                    Some(_) => {}
                }
            }
            for (key, state) in self.lod_visible.iter_mut() {
                if !state.1 && !draw.contains(key) {
                    let shown = (secs_since(now, state.0) / LOD_FADE).min(1.0);
                    *state = (secs_before(now, (1.0 - shown) * LOD_FADE), true);
                }
            }
        }

        let visible = self.lod_visible.len();
        self.lod_visible
            .retain(|_, s| !(s.1 && secs_since(now, s.0) >= LOD_FADE));
        if dirty || self.lod_visible.len() != visible {
            let unload: Vec<LodKey> = self
                .lod_tiles
                .keys()
                .filter(|k| !self.lod_visible.contains_key(k) && !self.lod_set.contains(k))
                .copied()
                .collect();
            for key in unload {
                if let Some(mesh) = self.lod_tiles.remove(&key).and_then(|t| t.mesh) {
                    renderer.destroy_mesh(mesh);
                }
            }
        }
    }

    /// Collects the tiles that cover `key`'s area; returns false if they don't cover all of
    /// it (some tile isn't generated yet and nothing loaded can stand in).
    fn resolve_lod(&self, key: LodKey, out: &mut Vec<LodKey>) -> bool {
        let loaded = self.lod_tiles.contains_key(&key);
        if self.lod_set.contains(&key) {
            if loaded {
                out.push(key);
                return true;
            }
            return self.resolve_finer(key, 0, out);
        }
        if !self.lod_internal.contains(&key) {
            return true; // nothing wanted here (beyond the horizon)
        }
        let mark = out.len();
        let mut complete = true;
        for child in key.children() {
            complete &= self.resolve_lod(child, out);
        }
        if !complete && loaded {
            // Keep showing this tile until all of its children are ready.
            out.truncate(mark);
            out.push(key);
            return true;
        }
        complete
    }

    /// Loaded tiles finer than `key` (left over from when the camera was closer) that can
    /// stand in for it while it generates.
    fn resolve_finer(&self, key: LodKey, depth: u32, out: &mut Vec<LodKey>) -> bool {
        if depth > 0 && self.lod_tiles.contains_key(&key) {
            out.push(key);
            return true;
        }
        if depth >= 4 || key.level <= 1 {
            return false;
        }
        let mut complete = true;
        for child in key.children() {
            complete &= self.resolve_finer(child, depth + 1, out);
        }
        complete
    }

    /// LOD tiles to draw this frame.
    pub fn lod_draw_items(&self) -> Vec<DrawItem> {
        let now = Instant::now();
        self.lod_visible
            .iter()
            .filter_map(|(key, &(start, out))| {
                let tile = self.lod_tiles.get(key)?;
                Some(DrawItem {
                    rotation: glam::Quat::IDENTITY,
                    mesh: tile.mesh?,
                    origin: self.planet.center + tile.origin,
                    radius: tile.radius,
                    fade: Fade::new(secs_since(now, start), LOD_FADE, out),
                })
            })
            .collect()
    }

    /// True when the camera is well below the terrain surface above it.
    pub fn camera_underground(&self, rel: DVec3) -> bool {
        let up = rel.try_normalize().unwrap_or(DVec3::Y);
        let surface = self.planet.sea_radius + self.terrain.sample_column(up).height;
        rel.length() < surface - UNDERGROUND_DEPTH
    }

    /// Radius (from the planet centre) of the lowest terrain surface around the camera -
    /// or the camera itself if it is lower (exploring caves).
    fn local_floor_radius(&self, rel: DVec3) -> f64 {
        let up = rel.try_normalize().unwrap_or(DVec3::Y);
        let east = up.any_orthonormal_vector();
        let north = up.cross(east);
        let mut lowest = self.terrain.sample_column(up).height;
        for ring in [0.5, 1.0] {
            for i in 0..8 {
                let a = i as f64 * std::f64::consts::TAU / 8.0;
                let offset = (east * a.cos() + north * a.sin()) * ring * self.load_radius;
                let dir = (up * self.planet.sea_radius + offset).normalize();
                lowest = lowest.min(self.terrain.sample_column(dir).height);
            }
        }
        (self.planet.sea_radius + lowest - 16.0).min(rel.length())
    }

    fn unload_distant(&mut self, rel: DVec3, renderer: &mut Renderer) {
        let keep_radius = self.load_radius + CHUNK_BOUND + UNLOAD_MARGIN;
        let remove: Vec<ChunkKey> = self
            .chunks
            .iter()
            .filter(|(key, c)| {
                !self.desired_set.contains(key) && (c.origin - rel).length() > keep_radius
            })
            .map(|(&key, _)| key)
            .collect();
        let since = Instant::now();
        for key in remove {
            let Some(chunk) = self.chunks.remove(&key) else {
                continue;
            };
            if let Some(mesh) = chunk.mesh {
                if self.retiring.len() < 2048 {
                    // Fade out over the LOD terrain instead of vanishing.
                    self.retiring.push(RetiringMesh {
                        mesh,
                        origin: chunk.origin,
                        radius: chunk.radius,
                        since,
                    });
                } else {
                    renderer.destroy_mesh(mesh);
                }
            }
        }
    }

    /// Everything within this distance of the camera is fully loaded, so the far terrain can
    /// be hidden inside it.
    pub fn loaded_radius(&self) -> f32 {
        let nearest_missing = self
            .in_flight
            .values()
            .copied()
            .chain(self.queue.front().map(|q| q.1))
            .chain(self.fading_in.iter().map(|f| f.0))
            .fold(f64::INFINITY, f64::min);
        (nearest_missing - CHUNK_BOUND).clamp(0.0, self.load_radius) as f32
    }

    /// The block at a planet-relative position, if its chunk is loaded.
    pub fn block_at(&self, rel: DVec3) -> Option<Block> {
        match self.planet.cell_at(rel) {
            Some(cell) => self.block(cell),
            // Above the build limit (or below the centre, which can't happen).
            None => Some(Block::Air),
        }
    }

    /// The player's edits to this world, as a file.
    pub fn edits_bytes(&self) -> Vec<u8> {
        self.edits.to_bytes()
    }

    /// Replaces the edits with a saved set (before any chunk of this world is loaded).
    pub fn load_edits(&self, data: &[u8]) -> Result<()> {
        self.edits.read_bytes(data)
    }

    /// No far terrain is still being generated.
    pub fn lod_settled(&self) -> bool {
        self.lod_queue.is_empty() && self.lod_in_flight.is_empty() && self.lod_ready.is_empty()
    }

    /// The block in a cell, if its chunk is loaded.
    pub fn block(&self, cell: BlockCell) -> Option<Block> {
        let (key, [u, v, k]) = self.planet.cell_chunk(cell);
        Some(self.chunks.get(&key)?.blocks.get(u, v, k))
    }

    /// Changes a block. Collision sees it at once; the chunk (and every neighbour whose
    /// padding includes the cell) is remeshed ahead of all other work, and the edit is kept
    /// so it survives the chunk being unloaded and generated again.
    pub fn set_block(&mut self, cell: BlockCell, block: Block) {
        let planet = self.planet.clone();
        self.edits.set(&planet, cell, block);
        if !block.is_solid() {
            self.changes.push(cell);
        }
        let (key, [u, v, k]) = planet.cell_chunk(cell);
        if let Some(chunk) = self.chunks.get_mut(&key) {
            chunk.blocks.set(u, v, k, block);
        }
        // Light may now flow into (or be cut off from) this cell.
        let chunks = &self.chunks;
        let passes = |c: BlockCell| light_passes(&planet, chunks, c);
        let relit = self.lighting.block_changed(&planet, cell, &passes);

        let mut affected = vec![key];
        let n = planet.res_at(cell.k);
        for i in 0..27 {
            let (du, dv, dk) = (i % 3 - 1, (i / 3) % 3 - 1, i / 9 - 1);
            // Padding doesn't carry edits across a shell boundary (see `WorldEdits`).
            if planet.res_at((cell.k + dk).clamp(0, planet.layers - 1)) != n {
                continue;
            }
            if let Some(near) = planet.cell(cell.face, cell.u + du, cell.v + dv, cell.k + dk) {
                let (near_key, _) = planet.cell_chunk(near);
                if !affected.contains(&near_key) {
                    affected.push(near_key);
                }
            }
        }
        let mut affected: FastSet<ChunkKey> = affected.into_iter().collect();
        affected.extend(self.chunks_showing(&relit));
        self.remesh_chunks(affected);
    }

    /// Adds (level > 0) or removes (0) a light source, and remeshes what it lights.
    pub fn set_light(&mut self, cell: BlockCell, level: u8) {
        let planet = self.planet.clone();
        let chunks = &self.chunks;
        let passes = |c: BlockCell| light_passes(&planet, chunks, c);
        let relit = self.lighting.set_source(&planet, cell, level, &passes);
        let affected = self.chunks_showing(&relit);
        self.remesh_chunks(affected);
    }

    /// Block light in a cell.
    pub fn light(&self, cell: BlockCell) -> u8 {
        self.lighting.map.get(cell)
    }

    /// The chunks whose meshes show these cells (their own, and neighbours whose padding
    /// holds them).
    fn chunks_showing(&self, cells: &[BlockCell]) -> FastSet<ChunkKey> {
        let planet = &self.planet;
        let mut keys = FastSet::default();
        for &cell in cells {
            let (key, local) = planet.cell_chunk(cell);
            keys.insert(key);
            if local.iter().all(|&l| (1..CHUNK_SIZE - 1).contains(&l)) {
                continue;
            }
            for i in 0..27 {
                let (du, dv, dk) = (i % 3 - 1, (i / 3) % 3 - 1, i / 9 - 1);
                if let Some(near) = planet.cell(cell.face, cell.u + du, cell.v + dv, cell.k + dk) {
                    keys.insert(planet.cell_chunk(near).0);
                }
            }
        }
        keys
    }

    /// Regenerates chunks (priority jobs); results from before are discarded by version.
    fn remesh_chunks(&mut self, keys: FastSet<ChunkKey>) {
        for key in keys {
            let version = self.versions.entry(key).or_insert(0);
            *version += 1;
            let version = *version;
            if (self.chunks.contains_key(&key) || self.in_flight.contains_key(&key))
                && let Some(tx) = &self.priority_tx
            {
                self.remesh.insert(key);
                let _ = tx.send(Job::Chunk(key, version));
            }
        }
    }

    pub fn draw_items(&self) -> Vec<DrawItem> {
        let now = Instant::now();
        let loaded = self.chunks.values().filter_map(|c| {
            c.mesh.map(|mesh| DrawItem {
                rotation: glam::Quat::IDENTITY,
                mesh,
                origin: self.planet.center + c.origin,
                radius: c.radius,
                fade: Fade::new(secs_since(now, c.shown), CHUNK_FADE, false),
            })
        });
        let retiring = self.retiring.iter().map(|r| DrawItem {
            rotation: glam::Quat::IDENTITY,
            mesh: r.mesh,
            origin: self.planet.center + r.origin,
            radius: r.radius,
            fade: Fade::new(secs_since(now, r.since), CHUNK_FADE, true),
        });
        loaded.chain(retiring).collect()
    }

    pub fn stats(&self) -> StreamStats {
        StreamStats {
            loaded: self.chunks.len(),
            desired: self.desired.len(),
            in_flight: self.in_flight.len(),
            workers: self.workers.len(),
            lod_desired: self.lod_desired.len(),
        }
    }
}

impl Drop for WorldStreamer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Disconnect the channels so idle workers wake up and exit.
        self.job_tx = None;
        self.priority_tx = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Whether light passes through a cell (unloaded cells block it).
fn light_passes(planet: &Planet, chunks: &FastMap<ChunkKey, LoadedChunk>, cell: BlockCell) -> bool {
    let (key, [u, v, k]) = planet.cell_chunk(cell);
    chunks
        .get(&key)
        .is_some_and(|chunk| !chunk.blocks.get(u, v, k).is_opaque())
}
