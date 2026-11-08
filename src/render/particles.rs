//! GPU particles: a ring buffer of particles that compute shaders spawn and simulate, drawn
//! as instanced quads.
//!
//! - **Spawning.** The CPU only sends small `Emitter` requests ("40 bits of this block, here,
//!   flying outwards"), so even continuous rain or snow costs it nothing per particle.
//!   `particles_emit.comp` writes the new particles after the newest ones in the ring.
//! - **Simulation** (`particles_sim.comp`): radial gravity towards the planet's centre, drag
//!   towards a wind velocity, and collision with this frame's depth buffer (bounce and settle,
//!   or die on impact). Particles off screen don't collide.
//! - **Drawing** (`particles.vert/.frag`): camera-facing quads or velocity streaks, lit like
//!   blocks. Cut-out particles (debris) are drawn with the opaque scene and write depth;
//!   blended ones (rain, snow, dust) after the water.
//!
//! The compute work runs in the middle of the scene pass: after the world (chunks, LOD, models,
//! mobs, drops) is drawn and before particles, water and the first-person arm. So particles
//! collide with the world only, never with their own quads (debris doing that climbed towards
//! the camera), and are drawn the same frame they were simulated.
//!
//! Positions are f32 relative to an *anchor* near the camera (f64). When the camera moves
//! more than `REBASE_DISTANCE` from it, the anchor moves to the camera and the next
//! simulation step shifts every particle by the difference.

use std::collections::VecDeque;

use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Vec3, Vec4};
use gpu_allocator::{MemoryLocation, vulkan::Allocator};

use super::buffer::Buffer;

/// Particles in the ring buffer (oldest are overwritten when it's full).
pub const CAPACITY: u32 = 1 << 18;
/// Emitters per frame, and new particles per frame.
const MAX_EMITTERS: usize = 256;
const MAX_NEW: u32 = 1 << 16;
const REBASE_DISTANCE: f64 = 256.0;
/// Compute workgroup size (`local_size_x` in the particle compute shaders).
pub const GROUP: u32 = 64;

/// Particle behaviour and look flags (must match `particle.glsl`).
pub mod flags {
    /// Collides with what the camera sees, bounces and settles.
    pub const BOUNCE: u32 = 1;
    /// Collides and disappears (rain).
    pub const DIE_ON_HIT: u32 = 2;
    /// Drawn as a streak along its velocity.
    pub const STRETCH: u32 = 4;
    /// Translucent: drawn blended, after the water, without writing depth.
    pub const BLEND: u32 = 8;
    /// A soft disc in the tint colour instead of a texture.
    pub const ROUND: u32 = 16;
    /// A block texture: its alpha is the biome tint mask (grass), not transparency.
    pub const TINT_MASK: u32 = 32;
    /// Glows by itself (flames, plasma) instead of being lit.
    pub const GLOW: u32 = 64;
    /// Weather: stops at the highest block above it, so it never falls indoors.
    pub const ROOFED: u32 = 256;
}

/// A request to spawn `count` particles this frame.
#[derive(Clone, Copy, Debug)]
pub struct Emitter {
    pub count: u32,
    /// Centre of the spawn box (absolute world position) and its half size.
    pub origin: DVec3,
    pub extent: Vec3,
    /// Base velocity, plus `burst` (m/s) outwards from the centre in proportion to how far
    /// out the particle starts, plus up to `spread` (m/s) in a random direction.
    pub velocity: Vec3,
    pub burst: f32,
    pub spread: f32,
    /// Lifetime (s) and size (m) ranges.
    pub life: (f32, f32),
    pub size: (f32, f32),
    /// Gravity scale (1 = falls normally), drag towards the wind (1/s), and bounciness.
    pub gravity: f32,
    pub drag: f32,
    pub bounce: f32,
    /// Texture array layer and the rect of it to show (texels: u0, v0, u1, v1); with
    /// `piece` > 0 each particle shows a random `piece` x `piece` texel part of the rect.
    pub layer: u32,
    pub rect: [u8; 4],
    pub piece: u8,
    /// Linear RGBA multiplied in (or the biome tint, with `TINT_MASK`).
    pub tint: [u8; 4],
    pub flags: u32,
    /// Block light and sky light (0-15) to light the particles with.
    pub light: (u32, u32),
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            count: 0,
            origin: DVec3::ZERO,
            extent: Vec3::ZERO,
            velocity: Vec3::ZERO,
            burst: 0.0,
            spread: 0.0,
            life: (1.0, 1.0),
            size: (0.1, 0.1),
            gravity: 1.0,
            drag: 0.0,
            bounce: 0.0,
            layer: 0,
            rect: [0, 0, 16, 16],
            piece: 0,
            tint: [255; 4],
            flags: 0,
            light: (0, 15),
        }
    }
}

/// Mirrors `Particle` in particle.glsl (only its size matters on the CPU).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuParticle {
    pos_age: Vec4,
    vel_life: Vec4,
    phys: Vec4,
    look: [u32; 4],
}

/// Mirrors the header of `Batch` in particle_batch.glsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BatchHeader {
    view_proj: Mat4,
    inv_view_proj: Mat4,
    anchor: Vec4,
    planet: Vec4,
    wind: Vec4,
    shift: Vec4,
    info: [u32; 4],
    sim: [u32; 4],
    vortex: [Vec4; 3],
    roof_center: Vec4,
    roof_u: Vec4,
    roof_v: Vec4,
}

/// The heights of the highest block around the camera, for weather (see `flags::ROOFED`).
/// Heights are along "up" from `center`, one per cell of a `ROOF_CELLS` square grid.
#[derive(Clone)]
pub struct RoofMap {
    pub center: DVec3,
    pub u: Vec3,
    pub v: Vec3,
    /// Cell size (m).
    pub cell: f32,
    /// Row by row; below -1e5 where no block was found.
    pub heights: Vec<f32>,
}

pub const ROOF_CELLS: usize = 48;
const ROOF_BYTES: usize = ROOF_CELLS * ROOF_CELLS * 4;

/// Mirrors `Emitter` in particle_batch.glsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuEmitter {
    origin: Vec4,
    extent: Vec4,
    velocity: Vec4,
    ranges: Vec4,
    phys: Vec4,
    look: [u32; 4],
    count: [u32; 4],
}

const BATCH_BYTES: usize =
    size_of::<BatchHeader>() + ROOF_BYTES + MAX_EMITTERS * size_of::<GpuEmitter>();

/// This frame's particle work, decided before recording.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParticleFrame {
    /// The particle buffer must be zeroed first (the very first frame).
    pub clear: bool,
    /// Drawn this frame, after the compute work: the anchor the positions are relative to,
    /// and the ring range that may hold live particles (first, count).
    pub draw_anchor: DVec3,
    pub draw: Option<(u32, u32)>,
    /// Simulated and spawned mid-frame, before the particles are drawn.
    pub simulate: Option<(u32, u32)>,
    pub spawn: u32,
}

/// The simulation's environment this frame.
pub struct ParticleEnvironment {
    pub camera: DVec3,
    pub planet_center: DVec3,
    /// Gravity at the surface (m/s^2), wind (m/s), the time step (s) and the camera.
    pub gravity: f32,
    pub wind: Vec3,
    pub dt: f32,
    pub view_proj: Mat4,
    pub near: f32,
    /// Tornadoes: where (absolute) and their core radius (0 = none).
    pub vortices: [(DVec3, f32); 3],
    pub roof: Option<RoofMap>,
}

pub struct Particles {
    pub buffer: Buffer,
    /// One batch (header + emitters) per frame in flight.
    pub batches: Vec<Buffer>,
    anchor: DVec3,
    /// Next ring index to write.
    head: u32,
    /// Spawned ranges still possibly alive: (first, count, expires at `clock`).
    live: VecDeque<(u32, u32, f64)>,
    clock: f64,
    pending: Vec<Emitter>,
    cleared: bool,
    seed: u32,
}

impl Particles {
    pub fn new(device: &ash::Device, allocator: &mut Allocator, frames: usize) -> Result<Self> {
        let buffer = Buffer::new(
            device,
            allocator,
            "particles",
            CAPACITY as u64 * size_of::<GpuParticle>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )?;
        let batches = (0..frames)
            .map(|_| {
                Buffer::new(
                    device,
                    allocator,
                    "particle batch",
                    BATCH_BYTES as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    MemoryLocation::CpuToGpu,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            buffer,
            batches,
            anchor: DVec3::ZERO,
            head: 0,
            live: VecDeque::new(),
            clock: 0.0,
            pending: Vec::new(),
            cleared: false,
            seed: 0x9e37_79b9,
        })
    }

    /// Drops every particle (moving to another world).
    pub fn forget(&mut self) {
        self.live.clear();
        self.pending.clear();
        self.head = 0;
    }

    pub fn emit(&mut self, emitters: &[Emitter]) {
        self.pending
            .extend(emitters.iter().filter(|e| e.count > 0).copied());
    }

    pub fn batch_bytes() -> u64 {
        BATCH_BYTES as u64
    }

    /// The ring range that may hold live particles.
    /// Spawned ranges are contiguous in the ring, so it runs from the oldest live range to
    /// the head, or is the whole ring once they add up to its capacity.
    fn span(&self) -> Option<(u32, u32)> {
        let &(first, _, _) = self.live.front()?;
        let total: u64 = self.live.iter().map(|l| l.1 as u64).sum();
        if total >= CAPACITY as u64 {
            Some((self.head, CAPACITY))
        } else {
            Some((first, total as u32))
        }
    }

    /// Plans this frame and writes the compute shaders' batch for frame slot `frame`.
    pub fn prepare(&mut self, frame: usize, env: &ParticleEnvironment) -> Result<ParticleFrame> {
        let mut out = ParticleFrame {
            clear: !self.cleared,
            ..Default::default()
        };
        self.cleared = true;

        // Forget ranges that have certainly died (oldest first, conservatively).
        self.clock += env.dt as f64;
        while self.live.front().is_some_and(|l| l.2 < self.clock) {
            self.live.pop_front();
        }
        out.simulate = self.span();

        // Re-base when the camera has wandered off.
        let old = self.anchor;
        if (env.camera - self.anchor).length() > REBASE_DISTANCE || self.live.is_empty() {
            self.anchor = env.camera;
        }
        let shift = (self.anchor - old).as_vec3();

        // This frame's emitters, in the order of their first particle.
        let mut gpu = Vec::new();
        let mut spawn = 0u32;
        for e in self.pending.drain(..).take(MAX_EMITTERS) {
            let count = e.count.min(MAX_NEW - spawn);
            if count == 0 {
                break;
            }
            let rect = u32::from_le_bytes(e.rect);
            let look = (e.layer & 255)
                | ((e.flags & 255) << 8)
                | ((e.light.0.min(15)) << 16)
                | ((e.light.1.min(15)) << 20)
                | (((e.flags >> 8) & 255) << 24);
            gpu.push(GpuEmitter {
                origin: (e.origin - self.anchor).as_vec3().extend(e.burst),
                extent: e.extent.extend(e.spread),
                velocity: e.velocity.extend(0.0),
                ranges: Vec4::new(e.life.0, e.life.1, e.size.0, e.size.1),
                phys: Vec4::new(e.gravity, e.drag, e.bounce, e.piece as f32),
                look: [look, rect, u32::from_le_bytes(e.tint), spawn],
                count: [count, 0, 0, 0],
            });
            spawn += count;
            let longest = e.life.0.max(e.life.1) as f64;
            self.live.push_back((
                (self.head + spawn - count) % CAPACITY,
                count,
                self.clock + longest + 0.1,
            ));
        }
        let first_new = self.head;
        self.head = (self.head + spawn) % CAPACITY;
        out.spawn = spawn;
        // Drawn after the simulation and spawning, in the new anchor.
        out.draw_anchor = self.anchor;
        out.draw = self.span();

        self.seed = self
            .seed
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453);
        let (sim_first, sim_count) = out.simulate.unwrap_or((0, 0));
        let header = BatchHeader {
            view_proj: env.view_proj,
            inv_view_proj: env.view_proj.inverse(),
            anchor: (self.anchor - env.camera).as_vec3().extend(env.dt),
            planet: (env.planet_center - self.anchor)
                .as_vec3()
                .extend(env.gravity),
            wind: env.wind.extend(env.near),
            shift: shift.extend(0.0),
            info: [spawn, gpu.len() as u32, first_new, CAPACITY],
            sim: [sim_first, sim_count, self.seed, 0],
            vortex: env
                .vortices
                .map(|(at, radius)| (at - self.anchor).as_vec3().extend(radius)),
            roof_center: env.roof.as_ref().map_or(Vec4::ZERO, |r| {
                (r.center - self.anchor).as_vec3().extend(1.0)
            }),
            roof_u: env.roof.as_ref().map_or(Vec4::ZERO, |r| r.u.extend(r.cell)),
            roof_v: env.roof.as_ref().map_or(Vec4::ZERO, |r| r.v.extend(0.0)),
        };
        let batch = &mut self.batches[frame];
        batch.write(0, bytemuck::bytes_of(&header))?;
        if let Some(roof) = &env.roof {
            batch.write(
                size_of::<BatchHeader>(),
                bytemuck::cast_slice(&roof.heights),
            )?;
        }
        if !gpu.is_empty() {
            batch.write(
                size_of::<BatchHeader>() + ROOF_BYTES,
                bytemuck::cast_slice(&gpu),
            )?;
        }
        Ok(out)
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        self.buffer.destroy(device, allocator);
        for b in &mut self.batches {
            b.destroy(device, allocator);
        }
    }
}

/// Push constants of the particle draw (`particles.vert`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ParticlePush {
    pub view_proj: Mat4,
    pub anchor: Vec4,
    pub right: Vec4,
    pub up: Vec4,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_shaders() {
        assert_eq!(size_of::<GpuParticle>(), 64);
        assert_eq!(size_of::<GpuEmitter>(), 112);
        assert_eq!(size_of::<BatchHeader>(), 320);
        assert_eq!((size_of::<BatchHeader>() + ROOF_BYTES) % 16, 0);
        assert!(size_of::<ParticlePush>() <= 128);
        assert_eq!(CAPACITY % GROUP, 0);
    }
}
