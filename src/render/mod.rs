//! Vulkan renderer.
//!
//! # Large-world precision
//! All world positions are `f64` (`DVec3`). The GPU only ever works in f32 *relative to the
//! camera*: each frame, for every draw, we compute `origin - camera_position` in f64 on the CPU
//! and bake that (now small) offset into the MVP matrix. Geometry near the camera therefore
//! always has full precision, whether the camera is at the world origin or 10 light-years away.
//! The view matrix contains rotation only - the camera is always at the origin in render space.
//!
//! # Streaming
//! Meshes can be created and destroyed every frame without stalling: uploads are recorded at
//! the start of the next frame's command buffer, and freed buffers are kept alive until the
//! GPU has finished every frame that might still use them.

mod arena;
pub mod atmosphere;
mod buffer;
pub mod camera;
mod cloud_noise;
mod context;
pub mod font;
mod frustum;
mod mesh;
pub mod models;
mod particles;
mod pipeline;
mod starfield;
mod swapchain;
mod texture;

use std::{
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    slice,
    sync::Arc,
    time::Instant,
};

use anyhow::{Result, ensure};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat3, Mat4, Quat, Vec3, Vec4};
use gpu_allocator::{
    MemoryLocation,
    vulkan::{Allocator, AllocatorCreateDesc},
};
use winit::window::Window;

use crate::render::camera::Camera;
use arena::{ArenaAlloc, StagingRing, VertexArena};
use atmosphere::{Atmosphere, AtmosphereLuts};
use buffer::Buffer;
use context::VulkanContext;
use frustum::Frustum;
use mesh::{GpuMesh, PushConstants};
use particles::{
    CAPACITY as PARTICLE_CAPACITY, GROUP as PARTICLE_GROUP, ParticleEnvironment, ParticleFrame,
    ParticlePush, Particles,
};
use pipeline::{BINDINGS, PUSH_STAGES, Pipelines};
use swapchain::{
    ColorTarget, DepthBuffer, Swapchain, color_subresource_range, depth_subresource_range,
};
use texture::Texture;

pub use mesh::{BlockVertex, ChunkMesh, DrawItem, Fade, MeshHandle, UiVertex};
pub use particles::{Emitter as ParticleEmitter, ROOF_CELLS, RoofMap, flags as particle_flags};
pub use texture::TextureArrayData;

/// How many frames the CPU may record ahead of the GPU.
const FRAMES_IN_FLIGHT: usize = 2;
/// Every mesh is a list of quads, so a single shared index buffer
/// (0,1,2, 0,2,3, 4,5,6, 4,6,7, ...) serves all of them.
const MAX_QUADS_PER_DRAW: u32 = 1 << 17;
/// Clouds are rendered at 1 / this of the screen's resolution in each direction.
const CLOUD_DIVISOR: u32 = 2;
/// Overlay vertices per frame (4 per quad).
const MAX_UI_VERTICES: usize = 1 << 16;

/// Per-frame scene parameters. Directions are in the world (= planet body) frame.
pub struct FrameParams {
    /// Camera position relative to home's centre (for the atmosphere).
    pub camera_planet: DVec3,
    /// What particles fall towards (absolute): the centre of the world loaded now.
    pub gravity_center: DVec3,
    /// Direction towards the sun.
    pub sun_dir: Vec3,
    pub sun_angular_radius: f32,
    /// Sun illuminance above the atmosphere. All scene radiance is in these units; the
    /// exposure maps it to the display.
    pub sun_illuminance: Vec3,
    /// Planet body frame -> inertial frame (orients the star field).
    pub star_rotation: Mat3,
    pub exposure: f32,
    /// Extra star brightness (artistic: stars stand out more in space).
    pub star_boost: f32,
    /// Atmosphere and sky; off underground (dark background and cave fog instead).
    pub sky: bool,
    /// Underground fog: colour and distance.
    pub cave_fog: Vec4,
    /// Starlight / airglow illuminance, so nights aren't pitch black.
    pub night_ambient: f32,
    /// Seconds, for animated effects.
    pub time: f32,
    /// LOD fragments closer than this are discarded (real chunks cover that area), and chunk
    /// water fragments further away (LOD water covers that). Infinity when LOD is off.
    pub far_discard_radius: f32,
    /// The block the player is looking at (outlined).
    pub target: Option<BlockTarget>,
    /// Particle simulation: gravity at the surface (m/s^2), wind (m/s) and the time step (s).
    pub gravity: f32,
    pub wind: Vec3,
    pub particle_dt: f32,
    /// Clouds, tornadoes and the roof over weather particles.
    pub clouds: CloudParams,
    /// Tornadoes for the particles: where (absolute) and their core radius (0 = none).
    pub vortices: [(DVec3, f32); 3],
    pub roof: Option<RoofMap>,
}

/// What the cloud pass needs each frame (see `shaders/atmosphere.glsl`).
pub struct CloudParams {
    /// Active frame axes -> home's body frame.
    pub rotation: Mat3,
    /// Cloud base above sea level, and the thickness of the shell (m).
    pub base: f32,
    pub thickness: f32,
    /// How far the cloud noise has been carried by the wind (m).
    pub offset: Vec3,
    /// Lightning: camera-relative position (m) and intensity.
    pub flashes: [Vec4; 2],
    /// Funnels: foot, camera-relative (m), and core radius (0 = none).
    pub tornadoes: [Vec4; 3],
    /// Erupting volcanoes: vent, camera-relative (m), and how hard (0 = none); the wind
    /// there (m/s) and how long ago it began (s).
    pub plumes: [Vec4; 2],
    pub plume_wind: [Vec4; 2],
    /// Lightning's light on the ground, and the northern lights' strength.
    pub flash_light: f32,
    pub aurora: f32,
}

/// The targeted block, for the outline drawn by `block.frag`.
pub struct BlockTarget {
    /// Maps a camera-relative position into the block's local box, [-1, 1]^3 inside.
    pub local_from_camera: Mat4,
    /// Survival mining progress, 0..1 (darkens the block).
    pub progress: f32,
}

/// Uniform buffer contents (std140); must match `Frame` in `shaders/atmosphere.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniforms {
    inv_view_proj: Mat4,
    star_rotation: Mat4,
    camera: Vec4,
    sun: Vec4,
    sun_color: Vec4,
    radii: Vec4,
    rayleigh: Vec4,
    mie: Vec4,
    ozone: Vec4,
    misc: Vec4,
    cave: Vec4,
    target: Mat4,
    target_info: Vec4,
    weather_rotation: Mat4,
    prev_view_proj: Mat4,
    prev_offset: Vec4,
    clouds: Vec4,
    cloud_offset: Vec4,
    flashes: [Vec4; 2],
    tornadoes: [Vec4; 3],
    plumes: [Vec4; 2],
    plume_wind: [Vec4; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub chunks_drawn: usize,
    pub chunks_culled: usize,
    pub far_drawn: usize,
    /// Time the CPU spent waiting for the GPU to finish an older frame (high = GPU-bound).
    pub gpu_wait_ms: f32,
}

#[derive(Clone, Copy)]
struct Frame {
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
}

struct PendingUpload {
    src_offset: vk::DeviceSize,
    dst: ArenaAlloc,
}

/// Staging space per frame in flight; caps how much mesh data is uploaded per frame.
const STAGING_BYTES_PER_FRAME: usize = 24 << 20;

pub struct Renderer {
    ctx: VulkanContext,
    allocator: ManuallyDrop<Allocator>,
    swapchain: Swapchain,
    depth: DepthBuffer,
    /// The scene is rendered here (HDR), then composited into the swapchain.
    hdr: ColorTarget,
    pipelines: Pipelines,
    texture: Texture,
    /// Transmittance, multiple scattering and sky irradiance LUTs, and the galaxy map.
    luts: [Texture; 4],
    atmosphere: Atmosphere,
    /// One uniform buffer and descriptor set per frame in flight.
    frame_uniforms: Vec<Buffer>,
    descriptor_pool: vk::DescriptorPool,
    descriptor_sets: Vec<vk::DescriptorSet>,
    quad_indices: Buffer,
    /// Font atlas for the overlay, and the overlay's vertices (one buffer per frame).
    font: Texture,
    ui_buffers: Vec<Buffer>,
    /// GPU particles (see `particles`).
    particles: Particles,
    /// The two weather maps (cube maps, see `shaders/weather.glsl`) and the cloud noise.
    weather_maps: [Texture; 2],
    cloud_noise: [Texture; 2],
    /// Half-resolution cloud targets: this frame's raw march, and its accumulated result.
    /// The accumulated ones alternate: frame slot `i` writes `clouds_history[i]`.
    clouds_raw: ColorTarget,
    clouds_history: [ColorTarget; FRAMES_IN_FLIGHT],
    /// The accumulated targets don't hold anything yet (first frame, after a resize).
    clouds_fresh: bool,
    frame_number: u32,
    prev_view_proj: Mat4,
    prev_camera: DVec3,
    /// Staging offset of a weather map update waiting to be copied.
    pending_weather: Option<vk::DeviceSize>,

    frames: Vec<Frame>,
    /// Vertex blocks to free once the matching frame slot's fence has signalled.
    garbage: Vec<Vec<ArenaAlloc>>,
    frame_index: usize,
    upload_pool: vk::CommandPool,

    arena: VertexArena,
    staging: Vec<StagingRing>,
    /// Whether `staging[frame_index]` has been made safe to write this frame.
    staging_ready: bool,
    meshes: Vec<Option<GpuMesh>>,
    free_mesh_slots: Vec<usize>,
    pending_uploads: Vec<PendingUpload>,
    needs_resize: bool,
    vsync: bool,
    capture_request: Option<PathBuf>,

    // Declared last so it's dropped after the Vulkan surface is destroyed.
    window: Arc<Window>,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        textures: &TextureArrayData,
        atmosphere: &Atmosphere,
        atmosphere_luts: &AtmosphereLuts,
        vsync: bool,
    ) -> Result<Self> {
        let ctx = VulkanContext::new(&window)?;
        let device = &ctx.device;

        let mut allocator = Allocator::new(&AllocatorCreateDesc {
            instance: ctx.instance.clone(),
            device: device.clone(),
            physical_device: ctx.physical_device,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;

        let swapchain = Swapchain::new(&ctx, window.inner_size(), vk::SwapchainKHR::null(), vsync)?;
        let depth = DepthBuffer::new(device, &mut allocator, swapchain.extent)?;
        let hdr = ColorTarget::new(device, &mut allocator, swapchain.extent)?;
        let pipelines = Pipelines::new(device, ColorTarget::FORMAT, swapchain.format)?;

        let upload_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                    .queue_family_index(ctx.queue_family),
                None,
            )?
        };

        let texture = Texture::new_array(&ctx, &mut allocator, upload_pool, textures)?;
        let lut = |allocator: &mut Allocator, lut, name| {
            Texture::new_lut(&ctx, allocator, upload_pool, lut, name)
        };
        let luts = [
            lut(
                &mut allocator,
                &atmosphere_luts.transmittance,
                "transmittance lut",
            )?,
            lut(
                &mut allocator,
                &atmosphere_luts.multiscattering,
                "multiple scattering lut",
            )?,
            lut(
                &mut allocator,
                &atmosphere_luts.irradiance,
                "irradiance lut",
            )?,
            lut(&mut allocator, &starfield::galaxy_map(), "galaxy map")?,
        ];
        let frame_uniforms = (0..FRAMES_IN_FLIGHT)
            .map(|_| {
                Buffer::new(
                    device,
                    &mut allocator,
                    "frame uniforms",
                    size_of::<FrameUniforms>() as u64,
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                    MemoryLocation::CpuToGpu,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let font = Texture::new_2d(
            &ctx,
            &mut allocator,
            upload_pool,
            font::ATLAS_SIZE,
            vk::Format::R8G8B8A8_UNORM,
            &font::atlas(),
            vk::Filter::NEAREST,
            "font",
        )?;
        let ui_buffers = (0..FRAMES_IN_FLIGHT)
            .map(|_| {
                Buffer::new(
                    device,
                    &mut allocator,
                    "ui vertices",
                    (MAX_UI_VERTICES * size_of::<UiVertex>()) as u64,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    MemoryLocation::CpuToGpu,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let particles = Particles::new(device, &mut allocator, FRAMES_IN_FLIGHT)?;
        let cube = |allocator: &mut Allocator, name| {
            Texture::new_cube(&ctx, allocator, upload_pool, crate::weather::MAP_SIZE, name)
        };
        let weather_maps = [
            cube(&mut allocator, "weather map a")?,
            cube(&mut allocator, "weather map b")?,
        ];
        let volume = |allocator: &mut Allocator, v: cloud_noise::Volume, name| {
            Texture::new_3d(&ctx, allocator, upload_pool, v.size, &v.levels, name)
        };
        let cloud_noise = [
            volume(&mut allocator, cloud_noise::shape(), "cloud shape")?,
            volume(&mut allocator, cloud_noise::detail(), "cloud detail")?,
        ];
        let clouds_size = cloud_extent(swapchain.extent);
        let clouds_raw = ColorTarget::new(device, &mut allocator, clouds_size)?;
        let clouds_history = [
            ColorTarget::new(device, &mut allocator, clouds_size)?,
            ColorTarget::new(device, &mut allocator, clouds_size)?,
        ];
        let (descriptor_pool, descriptor_sets) = create_descriptors(device, &pipelines)?;
        let quad_indices = create_quad_index_buffer(&ctx, &mut allocator, upload_pool)?;

        let frames = (0..FRAMES_IN_FLIGHT)
            .map(|_| -> Result<Frame> {
                unsafe {
                    let pool_info = vk::CommandPoolCreateInfo::default()
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                        .queue_family_index(ctx.queue_family);
                    let command_pool = device.create_command_pool(&pool_info, None)?;
                    let alloc_info = vk::CommandBufferAllocateInfo::default()
                        .command_pool(command_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1);
                    let command_buffer = device.allocate_command_buffers(&alloc_info)?[0];
                    let image_available =
                        device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
                    let in_flight = device.create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )?;
                    Ok(Frame {
                        command_pool,
                        command_buffer,
                        image_available,
                        in_flight,
                    })
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let staging = (0..FRAMES_IN_FLIGHT)
            .map(|_| StagingRing::new(device, &mut allocator, STAGING_BYTES_PER_FRAME))
            .collect::<Result<Vec<_>>>()?;

        let renderer = Self {
            ctx,
            allocator: ManuallyDrop::new(allocator),
            swapchain,
            depth,
            hdr,
            pipelines,
            texture,
            luts,
            atmosphere: *atmosphere,
            frame_uniforms,
            descriptor_pool,
            descriptor_sets,
            quad_indices,
            font,
            ui_buffers,
            particles,
            weather_maps,
            cloud_noise,
            clouds_raw,
            clouds_history,
            clouds_fresh: true,
            frame_number: 0,
            prev_view_proj: Mat4::IDENTITY,
            prev_camera: DVec3::ZERO,
            pending_weather: None,
            frames,
            garbage: (0..FRAMES_IN_FLIGHT).map(|_| Vec::new()).collect(),
            frame_index: 0,
            upload_pool,
            arena: VertexArena::new(),
            staging,
            staging_ready: false,
            meshes: Vec::new(),
            free_mesh_slots: Vec::new(),
            pending_uploads: Vec::new(),
            needs_resize: false,
            vsync,
            capture_request: None,
            window,
        };
        renderer.write_descriptors();
        Ok(renderer)
    }

    /// Points every frame's descriptor set at the current resources (again after a resize,
    /// which recreates the HDR target and the depth buffer).
    fn write_descriptors(&self) {
        let image = |view, layout| {
            [vk::DescriptorImageInfo {
                sampler: vk::Sampler::null(),
                image_view: view,
                image_layout: layout,
            }]
        };
        let sampler = |sampler| {
            [vk::DescriptorImageInfo {
                sampler,
                image_view: vk::ImageView::null(),
                image_layout: vk::ImageLayout::UNDEFINED,
            }]
        };
        let read_only = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let images = [
            (0, image(self.texture.view, read_only)),
            (1, sampler(self.texture.sampler)),
            (2, sampler(self.luts[0].sampler)),
            (3, image(self.luts[0].view, read_only)),
            (4, image(self.luts[1].view, read_only)),
            (5, image(self.luts[2].view, read_only)),
            (7, image(self.hdr.view, read_only)),
            (9, image(self.luts[3].view, read_only)),
            (10, image(self.font.view, read_only)),
            (13, image(self.weather_maps[0].view, read_only)),
            (14, image(self.weather_maps[1].view, read_only)),
            (15, image(self.cloud_noise[0].view, read_only)),
            (16, image(self.cloud_noise[1].view, read_only)),
            (17, image(self.clouds_raw.view, read_only)),
            (20, sampler(self.cloud_noise[0].sampler)),
            (
                8,
                image(self.depth.view, vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL),
            ),
        ];
        let particles = [vk::DescriptorBufferInfo {
            buffer: self.particles.buffer.handle,
            offset: 0,
            range: vk::WHOLE_SIZE,
        }];
        for (slot, ((set, uniforms), batch)) in self
            .descriptor_sets
            .iter()
            .zip(&self.frame_uniforms)
            .zip(&self.particles.batches)
            .enumerate()
        {
            // Frame slot `i` writes `clouds_history[i]` and reads the other one.
            let clouds_prev = image(
                self.clouds_history[(slot + 1) % FRAMES_IN_FLIGHT].view,
                read_only,
            );
            let clouds_now = image(self.clouds_history[slot].view, read_only);
            let buffer = [vk::DescriptorBufferInfo {
                buffer: uniforms.handle,
                offset: 0,
                range: size_of::<FrameUniforms>() as u64,
            }];
            let batch = [vk::DescriptorBufferInfo {
                buffer: batch.handle,
                offset: 0,
                range: Particles::batch_bytes(),
            }];
            let mut writes: Vec<_> = images
                .iter()
                .map(|(binding, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(*set)
                        .dst_binding(*binding)
                        .descriptor_type(BINDINGS[*binding as usize].0)
                        .image_info(info)
                })
                .collect();
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(18)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&clouds_prev),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(19)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&clouds_now),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(6)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(11)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&particles),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(12)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&batch),
            );
            unsafe { self.ctx.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    /// Call when the window was resized; the swapchain is rebuilt before the next frame.
    pub fn resize(&mut self) {
        self.needs_resize = true;
    }

    /// Turns vsync on or off (the swapchain is rebuilt before the next frame).
    pub fn set_vsync(&mut self, vsync: bool) {
        if vsync != self.vsync {
            self.vsync = vsync;
            self.needs_resize = true;
        }
    }

    /// Another world is about to be loaded: every mesh goes, particles vanish and the clouds
    /// start over. Mesh handles anyone still holds become invalid.
    pub fn forget_world(&mut self) {
        for i in 0..self.meshes.len() {
            self.destroy_mesh(MeshHandle(i));
        }
        self.particles.forget();
        self.clouds_fresh = true;
    }

    /// Swapchain size in pixels.
    pub fn screen_size(&self) -> (f32, f32) {
        let e = self.swapchain.extent;
        (e.width as f32, e.height as f32)
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.swapchain.extent.width as f32 / self.swapchain.extent.height.max(1) as f32
    }

    /// Creates a GPU mesh. Returns immediately; the data is copied to VRAM at the start of
    /// the next rendered frame (before any draws), so the mesh is usable right away.
    ///
    /// Returns `Ok(None)` if this frame's upload budget is used up - try again next frame.
    pub fn create_mesh(&mut self, mesh: &ChunkMesh) -> Result<Option<MeshHandle>> {
        let opaque_quads = (mesh.opaque.len() / 4) as u32;
        let water_quads = (mesh.water.len() / 4) as u32;
        let cutout_quads = (mesh.cutout.len() / 4) as u32;
        ensure!(
            opaque_quads + water_quads + cutout_quads > 0,
            "cannot create an empty mesh"
        );
        ensure!(
            opaque_quads.max(water_quads).max(cutout_quads) <= MAX_QUADS_PER_DRAW,
            "mesh too large"
        );

        let opaque_bytes: &[u8] = bytemuck::cast_slice(&mesh.opaque);
        let water_bytes: &[u8] = bytemuck::cast_slice(&mesh.water);
        let cutout_bytes: &[u8] = bytemuck::cast_slice(&mesh.cutout);
        let size = (opaque_bytes.len() + water_bytes.len() + cutout_bytes.len()) as vk::DeviceSize;

        self.prepare_staging()?;
        let Some(src_offset) =
            self.staging[self.frame_index].push(&[opaque_bytes, water_bytes, cutout_bytes])?
        else {
            return Ok(None);
        };
        let alloc = self
            .arena
            .alloc(&self.ctx.device, &mut self.allocator, size)?;
        self.pending_uploads.push(PendingUpload {
            src_offset,
            dst: alloc,
        });

        let gpu = GpuMesh {
            alloc,
            opaque_quads,
            water_quads,
            cutout_quads,
            scale: mesh.scale,
        };
        let slot = match self.free_mesh_slots.pop() {
            Some(slot) => {
                self.meshes[slot] = Some(gpu);
                slot
            }
            None => {
                self.meshes.push(Some(gpu));
                self.meshes.len() - 1
            }
        };
        Ok(Some(MeshHandle(slot)))
    }

    /// Destroys a mesh. Its memory is released once in-flight frames no longer use it.
    pub fn destroy_mesh(&mut self, handle: MeshHandle) {
        if let Some(mesh) = self.meshes.get_mut(handle.0).and_then(Option::take) {
            self.garbage[self.frame_index].push(mesh.alloc);
            self.free_mesh_slots.push(handle.0);
        }
    }

    /// (live mesh bytes, VRAM reserved for meshes) - for stats.
    pub fn mesh_memory(&self) -> (u64, u64) {
        (self.arena.used_bytes, self.arena.allocated_bytes())
    }

    /// Makes this frame slot's staging ring writable: the GPU must have finished the frame
    /// that used it last.
    fn prepare_staging(&mut self) -> Result<()> {
        if !self.staging_ready {
            let fence = self.frames[self.frame_index].in_flight;
            unsafe { self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)? };
            self.staging[self.frame_index].reset();
            self.staging_ready = true;
        }
        Ok(())
    }

    /// Renders one frame: chunk meshes (frustum culled), far terrain, models and dropped items
    /// (`objects`), translucent water, and the first-person `view_model` in front of it all.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        camera: &Camera,
        params: &FrameParams,
        chunks: &[DrawItem],
        far: &[DrawItem],
        objects: &[DrawItem],
        view_model: Option<DrawItem>,
        ui: &[UiVertex],
    ) -> Result<RenderStats> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(RenderStats::default()); // minimized
        }
        if self.needs_resize {
            self.recreate_swapchain()?;
        }

        let frame = self.frames[self.frame_index];
        let wait_start = Instant::now();
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[frame.in_flight], true, u64::MAX)?
        };
        let gpu_wait_ms = wait_start.elapsed().as_secs_f32() * 1000.0;

        // The GPU is done with everything submitted in this slot two frames ago.
        for alloc in self.garbage[self.frame_index].drain(..) {
            self.arena.free(alloc);
        }
        // Particles: what to draw, simulate and spawn this frame (writes this slot's batch).
        let particle_frame = self.particles.prepare(
            self.frame_index,
            &ParticleEnvironment {
                camera: camera.position,
                planet_center: params.gravity_center,
                gravity: params.gravity,
                wind: params.wind,
                dt: params.particle_dt,
                view_proj: camera.view_projection(self.aspect_ratio()),
                near: camera.near,
                vortices: params.vortices,
                roof: params.roof.clone(),
            },
        )?;
        let uniforms = self.frame_uniforms(camera, params);
        self.frame_uniforms[self.frame_index].write(0, bytemuck::bytes_of(&uniforms))?;
        self.prev_view_proj = camera.view_projection(self.aspect_ratio());
        self.prev_camera = camera.position;
        self.frame_number = self.frame_number.wrapping_add(1);
        let ui_len = ui.len().min(MAX_UI_VERTICES) / 4 * 4;
        self.ui_buffers[self.frame_index].write(0, bytemuck::cast_slice(&ui[..ui_len]))?;

        let image_index = match unsafe {
            self.ctx.swapchain_loader.acquire_next_image(
                self.swapchain.handle,
                u64::MAX,
                frame.image_available,
                vk::Fence::null(),
            )
        } {
            Ok((index, suboptimal)) => {
                self.needs_resize |= suboptimal;
                index as usize
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.needs_resize = true;
                return Ok(RenderStats::default());
            }
            Err(e) => return Err(e.into()),
        };

        let cmd = frame.command_buffer;
        unsafe {
            let device = &self.ctx.device;
            // Only reset the fence once we know we'll submit work that signals it.
            device.reset_fences(&[frame.in_flight])?;
            device.reset_command_pool(frame.command_pool, vk::CommandPoolResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        self.record_uploads(cmd);
        self.record_weather_upload(cmd);
        let capture = match self.capture_request.take() {
            Some(path) => {
                let extent = self.swapchain.extent;
                let buffer = Buffer::new(
                    &self.ctx.device,
                    &mut self.allocator,
                    "screenshot",
                    extent.width as u64 * extent.height as u64 * 4,
                    vk::BufferUsageFlags::TRANSFER_DST,
                    MemoryLocation::GpuToCpu,
                )?;
                Some((path, buffer))
            }
            None => None,
        };
        let mut stats = self.record_scene(
            cmd,
            image_index,
            camera,
            params,
            chunks,
            far,
            objects,
            view_model,
            particle_frame,
            ui_len as u32 / 4,
            capture.as_ref().map(|c| c.1.handle),
        );
        stats.gpu_wait_ms = gpu_wait_ms;
        self.clouds_fresh = false;
        unsafe { self.ctx.device.end_command_buffer(cmd)? };

        let render_finished = self.swapchain.render_finished[image_index];
        unsafe {
            let wait = [vk::SemaphoreSubmitInfo::default()
                .semaphore(frame.image_available)
                .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
            let signal = [vk::SemaphoreSubmitInfo::default()
                .semaphore(render_finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let cmd_info = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&wait)
                .command_buffer_infos(&cmd_info)
                .signal_semaphore_infos(&signal);
            self.ctx
                .device
                .queue_submit2(self.ctx.queue, &[submit], frame.in_flight)?;
        }

        if let Some((path, mut buffer)) = capture {
            unsafe {
                self.ctx
                    .device
                    .wait_for_fences(&[frame.in_flight], true, u64::MAX)?
            };
            let extent = self.swapchain.extent;
            let rgba_order = matches!(
                self.swapchain.format,
                vk::Format::R8G8B8A8_SRGB | vk::Format::R8G8B8A8_UNORM
            );
            let result = match buffer.read() {
                Some(pixels) => write_bmp(&path, extent.width, extent.height, pixels, rgba_order),
                None => Err(anyhow::anyhow!("screenshot buffer is not mapped")),
            };
            buffer.destroy(&self.ctx.device, &mut self.allocator);
            match result {
                Ok(()) => log::info!("saved screenshot to {}", path.display()),
                Err(e) => log::error!("screenshot failed: {e:#}"),
            }
        }

        let wait_semaphores = [render_finished];
        let swapchains = [self.swapchain.handle];
        let image_indices = [image_index as u32];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        match unsafe {
            self.ctx
                .swapchain_loader
                .queue_present(self.ctx.queue, &present_info)
        } {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.needs_resize = true,
            Err(e) => return Err(e.into()),
        }

        self.frame_index = (self.frame_index + 1) % FRAMES_IN_FLIGHT;
        self.staging_ready = false;
        Ok(stats)
    }

    /// Queues new weather maps (`a` and `b`, 6 faces each) for the next frame.
    pub fn upload_weather(&mut self, a: &[u8], b: &[u8]) -> Result<()> {
        self.prepare_staging()?;
        if let Some(offset) = self.staging[self.frame_index].push(&[a, b])? {
            self.pending_weather = Some(offset);
        }
        Ok(())
    }

    /// Copies a queued weather map update into the cube maps.
    fn record_weather_upload(&mut self, cmd: vk::CommandBuffer) {
        use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as P};
        let Some(offset) = self.pending_weather.take() else {
            return;
        };
        let device = &self.ctx.device;
        let size = crate::weather::MAP_SIZE;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(6);
        let staging = self.staging[self.frame_index].handle();
        for (i, map) in self.weather_maps.iter().enumerate() {
            let to_copy = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(P::FRAGMENT_SHADER)
                .src_access_mask(A::NONE)
                .dst_stage_mask(P::COPY)
                .dst_access_mask(A::TRANSFER_WRITE)
                .old_layout(L::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(L::TRANSFER_DST_OPTIMAL)
                .image(map.image)
                .subresource_range(range)];
            let region = vk::BufferImageCopy {
                buffer_offset: offset + (i * crate::weather::MAP_BYTES) as u64,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 6,
                },
                image_extent: vk::Extent3D {
                    width: size,
                    height: size,
                    depth: 1,
                },
                ..Default::default()
            };
            let to_read = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(P::COPY)
                .src_access_mask(A::TRANSFER_WRITE)
                .dst_stage_mask(P::FRAGMENT_SHADER)
                .dst_access_mask(A::SHADER_SAMPLED_READ)
                .old_layout(L::TRANSFER_DST_OPTIMAL)
                .new_layout(L::SHADER_READ_ONLY_OPTIMAL)
                .image(map.image)
                .subresource_range(range)];
            unsafe {
                device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
                );
                device.cmd_copy_buffer_to_image(
                    cmd,
                    staging,
                    map.image,
                    L::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
                device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_read),
                );
            }
        }
    }

    /// Copies pending mesh data into VRAM; the barrier makes it visible to vertex input.
    fn record_uploads(&mut self, cmd: vk::CommandBuffer) {
        if self.pending_uploads.is_empty() {
            return;
        }
        let device = &self.ctx.device;
        let staging = self.staging[self.frame_index].handle();
        for upload in self.pending_uploads.drain(..) {
            let region = vk::BufferCopy {
                src_offset: upload.src_offset,
                dst_offset: upload.dst.offset,
                size: upload.dst.size,
            };
            unsafe {
                device.cmd_copy_buffer(cmd, staging, self.arena.buffer(upload.dst.page), &[region])
            };
        }
        let barrier = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VERTEX_ATTRIBUTE_INPUT)
            .dst_access_mask(vk::AccessFlags2::VERTEX_ATTRIBUTE_READ)];
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().memory_barriers(&barrier),
            );
        }
    }

    fn frame_uniforms(&self, camera: &Camera, params: &FrameParams) -> FrameUniforms {
        let a = &self.atmosphere;
        let view_proj = camera.view_projection(self.aspect_ratio());
        let pixel_angle = camera.fov_y / self.swapchain.extent.height.max(1) as f32;
        FrameUniforms {
            inv_view_proj: view_proj.inverse(),
            star_rotation: Mat4::from_mat3(params.star_rotation),
            camera: params.camera_planet.as_vec3().extend(params.exposure),
            sun: params.sun_dir.extend(params.sun_angular_radius.cos()),
            sun_color: params
                .sun_illuminance
                .extend(if params.sky { 1.0 } else { 0.0 }),
            radii: Vec4::new(a.bottom_radius, a.top_radius, params.time, pixel_angle),
            rayleigh: a.rayleigh_scattering.extend(a.rayleigh_scale_height),
            mie: Vec4::new(
                a.mie_scattering,
                a.mie_extinction,
                a.mie_scale_height,
                a.mie_g,
            ),
            ozone: a.ozone_absorption.extend(a.ozone_center),
            misc: Vec4::new(
                a.ozone_half_width,
                a.ground_albedo,
                params.night_ambient,
                params.star_boost,
            ),
            cave: params.cave_fog,
            target: params
                .target
                .as_ref()
                .map_or(Mat4::ZERO, |t| t.local_from_camera),
            target_info: params
                .target
                .as_ref()
                .map_or(Vec4::ZERO, |t| Vec4::new(1.0, t.progress, 0.0, 0.0)),
            weather_rotation: Mat4::from_mat3(params.clouds.rotation),
            prev_view_proj: self.prev_view_proj,
            prev_offset: (camera.position - self.prev_camera)
                .as_vec3()
                .extend(if self.clouds_fresh { 0.0 } else { 1.0 }),
            clouds: Vec4::new(
                params.clouds.base,
                params.clouds.thickness,
                (self.frame_number % 4096) as f32,
                params.clouds.flash_light,
            ),
            cloud_offset: params.clouds.offset.extend(params.clouds.aurora),
            flashes: params.clouds.flashes,
            tornadoes: params.clouds.tornadoes,
            plumes: params.clouds.plumes,
            plume_wind: params.clouds.plume_wind,
        }
    }

    /// The weather in the air: the raw half-resolution march, then the accumulated result
    /// (which the sky pass reads). The caller has made depth readable.
    fn record_clouds(&self, cmd: vk::CommandBuffer, extent: vk::Extent2D) {
        use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as P};
        let device = &self.ctx.device;
        let size = cloud_extent(extent);
        let current = &self.clouds_history[self.frame_index];
        let previous = &self.clouds_history[(self.frame_index + 1) % FRAMES_IN_FLIGHT];
        let to_attachment = |image| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(P::FRAGMENT_SHADER)
                .src_access_mask(A::NONE)
                .dst_stage_mask(P::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(A::COLOR_ATTACHMENT_WRITE)
                .old_layout(L::UNDEFINED)
                .new_layout(L::COLOR_ATTACHMENT_OPTIMAL)
                .image(image)
                .subresource_range(color_subresource_range())
        };
        let to_read = |image| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(P::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(A::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(P::FRAGMENT_SHADER)
                .dst_access_mask(A::SHADER_SAMPLED_READ)
                .old_layout(L::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(L::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(color_subresource_range())
        };
        let mut before = vec![
            to_attachment(self.clouds_raw.image),
            to_attachment(current.image),
        ];
        if self.clouds_fresh {
            // Nothing has been written to it yet; the resolve won't use it (`prev_offset.w`).
            before.push(
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(P::TOP_OF_PIPE)
                    .src_access_mask(A::NONE)
                    .dst_stage_mask(P::FRAGMENT_SHADER)
                    .dst_access_mask(A::SHADER_SAMPLED_READ)
                    .old_layout(L::UNDEFINED)
                    .new_layout(L::SHADER_READ_ONLY_OPTIMAL)
                    .image(previous.image)
                    .subresource_range(color_subresource_range()),
            );
        }
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: size,
        };
        let pass = |view, pipeline| unsafe {
            let target = vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(L::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::DONT_CARE)
                .store_op(vk::AttachmentStoreOp::STORE);
            device.cmd_begin_rendering(
                cmd,
                &vk::RenderingInfo::default()
                    .render_area(area)
                    .layer_count(1)
                    .color_attachments(slice::from_ref(&target)),
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_rendering(cmd);
        };
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&before),
            );
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: size.width as f32,
                    height: size.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(cmd, 0, &[area]);
        }
        pass(self.clouds_raw.view, self.pipelines.clouds);
        let raw_ready = [to_read(self.clouds_raw.image)];
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&raw_ready),
            );
        }
        pass(current.view, self.pipelines.clouds_resolve);
        let done = [to_read(current.image)];
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&done),
            );
            // Back to the full screen for the sky and the overlay.
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: extent.width as f32,
                    height: extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent,
                }],
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_scene(
        &self,
        cmd: vk::CommandBuffer,
        image_index: usize,
        camera: &Camera,
        params: &FrameParams,
        chunks: &[DrawItem],
        far: &[DrawItem],
        objects: &[DrawItem],
        view_model: Option<DrawItem>,
        particle_frame: ParticleFrame,
        ui_quads: u32,
        capture: Option<vk::Buffer>,
    ) -> RenderStats {
        let device = &self.ctx.device;
        let extent = self.swapchain.extent;
        let image = self.swapchain.images[image_index];
        let view_proj = camera.view_projection(self.aspect_ratio());
        let frustum = Frustum::from_view_projection(view_proj);
        let mut stats = RenderStats::default();

        // ---- Culling ------------------------------------------------------------------
        // (distance, mesh slot, camera-relative origin, fade). Subtracting in f64 *then*
        // converting to f32 is what keeps huge worlds precise.
        let mut visible_chunks = Vec::with_capacity(chunks.len());
        for draw in chunks {
            let rel = (draw.origin - camera.position).as_vec3();
            if self.meshes.get(draw.mesh.0).is_some_and(Option::is_some)
                && frustum.sphere_visible(rel, draw.radius)
            {
                visible_chunks.push((rel.length(), draw.mesh.0, rel, draw.fade));
            } else {
                stats.chunks_culled += 1;
            }
        }
        // Front-to-back so early depth testing rejects hidden fragments.
        visible_chunks.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        stats.chunks_drawn = visible_chunks.len();

        // Models and dropped items (only those whose mesh is uploaded).
        let visible_objects: Vec<(usize, Vec3, Quat)> = objects
            .iter()
            .filter_map(|d| {
                let rel = (d.origin - camera.position).as_vec3();
                (self.meshes.get(d.mesh.0).is_some_and(Option::is_some)
                    && frustum.sphere_visible(rel, d.radius))
                .then_some((d.mesh.0, rel, d.rotation))
            })
            .collect();

        // (Far terrain can be another world's, turned into place: it keeps its rotation.)
        let mut visible_far: Vec<(f32, usize, Vec3, Fade, Quat)> = far
            .iter()
            .filter_map(|d| {
                let rel = (d.origin - camera.position).as_vec3();
                frustum.sphere_visible(rel, d.radius).then_some((
                    rel.length(),
                    d.mesh.0,
                    rel,
                    d.fade,
                    d.rotation,
                ))
            })
            .collect();
        visible_far.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        stats.far_drawn = visible_far.len();

        let push_for = |rel: Vec3, scale: f32, clip: [f32; 2], fade: Fade, rotation: Quat| {
            let (progress, direction) = fade.shader();
            PushConstants {
                mvp: view_proj * Mat4::from_translation(rel),
                offset: rel.extend(clip[0]),
                params: Vec4::new(scale, clip[1], progress, direction),
                rotation: Vec4::from(rotation),
            }
        };

        unsafe {
            if particle_frame.clear {
                device.cmd_fill_buffer(cmd, self.particles.buffer.handle, 0, vk::WHOLE_SIZE, 0);
                self.particle_barrier(
                    cmd,
                    (
                        vk::PipelineStageFlags2::CLEAR,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags2::VERTEX_SHADER
                            | vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_READ
                            | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                );
            }
            // Transition the HDR target + depth buffer into attachment layouts. Both are
            // shared between frames in flight, so the barriers also order us after the
            // previous frame's writes and its atmosphere pass reading them.
            let to_attachment = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(
                        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    )
                    .src_access_mask(vk::AccessFlags2::NONE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .image(self.hdr.image)
                    .subresource_range(color_subresource_range()),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(
                        vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS
                            | vk::PipelineStageFlags2::FRAGMENT_SHADER
                            | vk::PipelineStageFlags2::COMPUTE_SHADER,
                    )
                    .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_stage_mask(
                        vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    )
                    .dst_access_mask(
                        vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                            | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                    )
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                    .image(self.depth.image)
                    .subresource_range(depth_subresource_range()),
            ];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_attachment),
            );

            let color_attachment = vk::RenderingAttachmentInfo::default()
                .image_view(self.hdr.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    // Alpha 0: the sky pass reads how much particles cover the empty sky.
                    color: vk::ClearColorValue {
                        float32: [0.0, 0.0, 0.0, 0.0],
                    },
                });
            // Depth is kept: the atmosphere pass needs the distance to every pixel.
            let depth_attachment = vk::RenderingAttachmentInfo::default()
                .image_view(self.depth.view)
                .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    // Reverse-Z: 0.0 is infinitely far away.
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 0.0,
                        stencil: 0,
                    },
                });
            let rendering_info = vk::RenderingInfo::default()
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent,
                })
                .layer_count(1)
                .color_attachments(slice::from_ref(&color_attachment))
                .depth_attachment(&depth_attachment);
            device.cmd_begin_rendering(cmd, &rendering_info);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent,
                }],
            );

            let layout = self.pipelines.layout;
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                &[self.descriptor_sets[self.frame_index]],
                &[],
            );
            device.cmd_bind_index_buffer(cmd, self.quad_indices.handle, 0, vk::IndexType::UINT32);

            // `clip`: [inner, outer] discard radii (clip pipelines only).
            let draw_rotated = |slot: usize,
                                rel: Vec3,
                                section: Section,
                                clip: [f32; 2],
                                fade: Fade,
                                rotation: Quat| {
                let Some(Some(mesh)) = self.meshes.get(slot) else {
                    return;
                };
                let (quads, first_vertex) = match section {
                    Section::Opaque => (mesh.opaque_quads, 0),
                    Section::Water => (mesh.water_quads, mesh.opaque_quads * 4),
                    Section::Cutout => (
                        mesh.cutout_quads,
                        (mesh.opaque_quads + mesh.water_quads) * 4,
                    ),
                };
                if quads == 0 {
                    return;
                }
                device.cmd_push_constants(
                    cmd,
                    layout,
                    PUSH_STAGES,
                    0,
                    bytemuck::bytes_of(&push_for(rel, mesh.scale, clip, fade, rotation)),
                );
                device.cmd_bind_vertex_buffers(
                    cmd,
                    0,
                    &[self.arena.buffer(mesh.alloc.page)],
                    &[mesh.alloc.offset],
                );
                device.cmd_draw_indexed(cmd, quads * 6, 1, 0, first_vertex as i32, 0);
            };
            let draw = |slot: usize, rel: Vec3, section: Section, clip: [f32; 2], fade: Fade| {
                draw_rotated(slot, rel, section, clip, fade, Quat::IDENTITY)
            };
            // LOD terrain is hidden inside `lod_radius`, where the real chunks are loaded; chunk
            // water is hidden outside it (LOD water takes over there).
            let lod_radius = params.far_discard_radius;
            const NONE: f32 = f32::INFINITY;

            // 1. Opaque chunks, front to back.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.opaque);
            for &(_, slot, rel, fade) in &visible_chunks {
                if fade == Fade::Visible {
                    draw(slot, rel, Section::Opaque, [0.0, NONE], fade);
                }
            }

            // 2. Chunks fading in or out, and the LOD terrain (everything beyond the loaded
            //    chunks, out to the horizon). Both need the clip pipeline.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.far);
            for &(_, slot, rel, fade) in &visible_chunks {
                if fade != Fade::Visible {
                    draw(slot, rel, Section::Opaque, [0.0, NONE], fade);
                }
            }
            for &(_, slot, rel, fade, turn) in &visible_far {
                draw_rotated(slot, rel, Section::Opaque, [lod_radius, NONE], fade, turn);
            }
            // Glass and other cutout blocks in chunks.
            for &(_, slot, rel, fade) in &visible_chunks {
                draw(slot, rel, Section::Cutout, [0.0, NONE], fade);
            }
            // Models and dropped items: the clip pipeline also does their cutout sprites.
            for &(slot, rel, rotation) in &visible_objects {
                draw_rotated(
                    slot,
                    rel,
                    Section::Opaque,
                    [0.0, NONE],
                    Fade::Visible,
                    rotation,
                );
            }

            // Particles: one quad per ring slot of the live range (dead ones collapse).
            let draw_particles = |pipeline: vk::Pipeline, blended: bool| {
                let Some((first, count)) = particle_frame.draw else {
                    return;
                };
                let orientation = camera.orientation();
                let push = ParticlePush {
                    view_proj,
                    anchor: (particle_frame.draw_anchor - camera.position)
                        .as_vec3()
                        .extend(if blended { 1.0 } else { 0.0 }),
                    right: (orientation * Vec3::X).extend(first as f32),
                    up: (orientation * Vec3::Y).extend(PARTICLE_CAPACITY as f32),
                };
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                device.cmd_push_constants(cmd, layout, PUSH_STAGES, 0, bytemuck::bytes_of(&push));
                device.cmd_draw(cmd, count * 6, 1, 0, 0);
            };
            // 3. Particles: simulate and spawn against the depth of the world drawn so far, i.e.
            //    not the particles themselves, the water or the arm. (Debris colliding with its
            //    own quad climbed towards the camera.) Rendering pauses for the compute work.
            if particle_frame.simulate.is_some() || particle_frame.spawn > 0 {
                device.cmd_end_rendering(cmd);
                self.particle_compute(cmd, &particle_frame);
                let color = color_attachment.load_op(vk::AttachmentLoadOp::LOAD);
                let depth = depth_attachment.load_op(vk::AttachmentLoadOp::LOAD);
                let resume = vk::RenderingInfo::default()
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent,
                    })
                    .layer_count(1)
                    .color_attachments(slice::from_ref(&color))
                    .depth_attachment(&depth);
                device.cmd_begin_rendering(cmd, &resume);
            }
            // Debris and other cut-out particles.
            draw_particles(self.pipelines.particles, false);

            // 4. Translucent water, back to front.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.water);
            for &(_, slot, rel, fade, turn) in visible_far.iter().rev() {
                draw_rotated(slot, rel, Section::Water, [lod_radius, NONE], fade, turn);
            }
            for &(_, slot, rel, fade) in visible_chunks.iter().rev() {
                draw(slot, rel, Section::Water, [0.0, lod_radius], fade);
            }

            // Translucent particles (rain, snow, dust) over everything but the view model.
            draw_particles(self.pipelines.particles_blend, true);

            // 5. The first-person view model, last and with its own depth range (always in
            //    front, see `Camera::view_model_projection`). params.z < 0 tells the shader
            //    it isn't part of the world (no target outline).
            if let Some(vm) = view_model
                && let Some(Some(mesh)) = self.meshes.get(vm.mesh.0)
                && mesh.opaque_quads > 0
            {
                let rel = (vm.origin - camera.position).as_vec3();
                let push = PushConstants {
                    mvp: camera.view_model_projection(self.aspect_ratio())
                        * Mat4::from_translation(rel),
                    offset: rel.extend(0.0),
                    params: Vec4::new(mesh.scale, NONE, -1.0, 0.0),
                    rotation: Vec4::from(Quat::IDENTITY),
                };
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.far);
                device.cmd_push_constants(cmd, layout, PUSH_STAGES, 0, bytemuck::bytes_of(&push));
                device.cmd_bind_vertex_buffers(
                    cmd,
                    0,
                    &[self.arena.buffer(mesh.alloc.page)],
                    &[mesh.alloc.offset],
                );
                device.cmd_draw_indexed(cmd, mesh.opaque_quads * 6, 1, 0, 0, 0);
            }

            device.cmd_end_rendering(cmd);

            // 6. Atmosphere, sky and tone mapping: reads the HDR scene + depth, writes the
            //    swapchain image.
            let to_composite = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(self.hdr.image)
                    .subresource_range(color_subresource_range()),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(
                        vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    )
                    .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_stage_mask(
                        vk::PipelineStageFlags2::FRAGMENT_SHADER
                            | vk::PipelineStageFlags2::COMPUTE_SHADER,
                    )
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .old_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL)
                    .image(self.depth.image)
                    .subresource_range(depth_subresource_range()),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags2::NONE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .image(image)
                    .subresource_range(color_subresource_range()),
            ];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_composite),
            );
            self.record_clouds(cmd, extent);
            let target = vk::RenderingAttachmentInfo::default()
                .image_view(self.swapchain.views[image_index])
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::DONT_CARE)
                .store_op(vk::AttachmentStoreOp::STORE);
            device.cmd_begin_rendering(
                cmd,
                &vk::RenderingInfo::default()
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent,
                    })
                    .layer_count(1)
                    .color_attachments(slice::from_ref(&target)),
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.sky);
            device.cmd_draw(cmd, 3, 1, 0, 0);

            // 7. The overlay (console, HUD).
            if ui_quads > 0 {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipelines.ui);
                let screen = [extent.width as f32, extent.height as f32, 0.0, 0.0];
                device.cmd_push_constants(cmd, layout, PUSH_STAGES, 0, bytemuck::bytes_of(&screen));
                device.cmd_bind_vertex_buffers(
                    cmd,
                    0,
                    &[self.ui_buffers[self.frame_index].handle],
                    &[0],
                );
                device.cmd_draw_indexed(cmd, ui_quads * 6, 1, 0, 0, 0);
            }
            device.cmd_end_rendering(cmd);

            let mut last_layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
            let mut last_stage = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
            let mut last_access = vk::AccessFlags2::COLOR_ATTACHMENT_WRITE;
            if let Some(capture) = capture {
                let to_transfer = [vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(last_stage)
                    .src_access_mask(last_access)
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(last_layout)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(color_subresource_range())];
                device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
                );
                let region = vk::BufferImageCopy {
                    buffer_offset: 0,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    image_offset: vk::Offset3D::default(),
                    image_extent: vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    },
                };
                device.cmd_copy_image_to_buffer(
                    cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    capture,
                    &[region],
                );
                last_layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
                last_stage = vk::PipelineStageFlags2::COPY;
                last_access = vk::AccessFlags2::NONE;
            }

            let to_present = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(last_stage)
                .src_access_mask(last_access)
                .dst_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_access_mask(vk::AccessFlags2::NONE)
                .old_layout(last_layout)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(color_subresource_range())];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_present),
            );
        }
        stats
    }

    /// Simulates the live particles and spawns this frame's new ones, between the world and
    /// everything drawn after it: the depth buffer becomes readable for the compute shaders,
    /// then an attachment again (contents kept); the HDR target keeps its contents.
    fn particle_compute(&self, cmd: vk::CommandBuffer, frame: &ParticleFrame) {
        use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as P};
        let device = &self.ctx.device;
        let tests = P::EARLY_FRAGMENT_TESTS | P::LATE_FRAGMENT_TESTS;
        let depth = |(src_stage, src_access, old): (P, A, L),
                     (dst_stage, dst_access, new): (P, A, L)| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(src_stage)
                .src_access_mask(src_access)
                .dst_stage_mask(dst_stage)
                .dst_access_mask(dst_access)
                .old_layout(old)
                .new_layout(new)
                .image(self.depth.image)
                .subresource_range(depth_subresource_range())
        };
        let to_compute = [depth(
            (
                tests,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_ATTACHMENT_OPTIMAL,
            ),
            (
                P::COMPUTE_SHADER,
                A::SHADER_SAMPLED_READ,
                L::DEPTH_READ_ONLY_OPTIMAL,
            ),
        )];
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_compute),
            );
        }

        let read_write = A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE;
        // Last frame's particle draws read the buffer we now write.
        self.particle_barrier(
            cmd,
            (P::VERTEX_SHADER, A::SHADER_STORAGE_READ),
            (P::COMPUTE_SHADER, read_write),
        );
        unsafe {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipelines.layout,
                0,
                &[self.descriptor_sets[self.frame_index]],
                &[],
            );
            if let Some((_, count)) = frame.simulate {
                let sim = self.pipelines.particle_sim;
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, sim);
                device.cmd_dispatch(cmd, count.div_ceil(PARTICLE_GROUP), 1, 1);
            }
        }
        if frame.simulate.is_some() {
            self.particle_barrier(
                cmd,
                (P::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE),
                (P::COMPUTE_SHADER, read_write),
            );
        }
        if frame.spawn > 0 {
            let emit = self.pipelines.particle_emit;
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, emit);
                device.cmd_dispatch(cmd, frame.spawn.div_ceil(PARTICLE_GROUP), 1, 1);
            }
        }
        // This frame's particle draws read the result.
        self.particle_barrier(
            cmd,
            (P::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE),
            (P::VERTEX_SHADER | P::COMPUTE_SHADER, read_write),
        );

        // Back to rendering: depth an attachment again, colour writes ordered after the
        // first half's.
        let back = [depth(
            (P::COMPUTE_SHADER, A::NONE, L::DEPTH_READ_ONLY_OPTIMAL),
            (
                tests,
                A::DEPTH_STENCIL_ATTACHMENT_READ | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_ATTACHMENT_OPTIMAL,
            ),
        )];
        let colour = [vk::MemoryBarrier2::default()
            .src_stage_mask(P::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(A::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(P::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE)];
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&colour)
                    .image_memory_barriers(&back),
            );
        }
    }

    /// A memory barrier on the whole particle buffer, from `(stages, accesses)` to `(stages,
    /// accesses)`.
    fn particle_barrier(
        &self,
        cmd: vk::CommandBuffer,
        (src_stage, src_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
        (dst_stage, dst_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    ) {
        let barrier = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(dst_stage)
            .dst_access_mask(dst_access)
            .buffer(self.particles.buffer.handle)
            .offset(0)
            .size(vk::WHOLE_SIZE)];
        let info = vk::DependencyInfo::default().buffer_memory_barriers(&barrier);
        unsafe { self.ctx.device.cmd_pipeline_barrier2(cmd, &info) };
    }

    /// Queues particles to spawn at the end of this frame (see `particles::Emitter`).
    pub fn emit_particles(&mut self, emitters: &[ParticleEmitter]) {
        self.particles.emit(emitters);
    }

    /// Saves the next rendered frame as a BMP file.
    pub fn request_capture(&mut self, path: PathBuf) {
        self.capture_request = Some(path);
    }

    fn recreate_swapchain(&mut self) -> Result<()> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let device = &self.ctx.device;
        unsafe { device.device_wait_idle()? };

        let new = Swapchain::new(&self.ctx, size, self.swapchain.handle, self.vsync)?;
        let mut old = std::mem::replace(&mut self.swapchain, new);
        old.destroy(&self.ctx);

        self.depth.destroy(device, &mut self.allocator);
        self.depth = DepthBuffer::new(device, &mut self.allocator, self.swapchain.extent)?;
        self.hdr.destroy(device, &mut self.allocator);
        self.hdr = ColorTarget::new(device, &mut self.allocator, self.swapchain.extent)?;
        let size = cloud_extent(self.swapchain.extent);
        self.clouds_raw.destroy(device, &mut self.allocator);
        self.clouds_raw = ColorTarget::new(device, &mut self.allocator, size)?;
        for target in &mut self.clouds_history {
            target.destroy(device, &mut self.allocator);
            *target = ColorTarget::new(device, &mut self.allocator, size)?;
        }
        self.clouds_fresh = true;
        self.write_descriptors();

        self.needs_resize = false;
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.ctx.device;
        let allocator = &mut *self.allocator;
        unsafe {
            let _ = device.device_wait_idle();

            // Mesh memory all lives in the arena pages.
            self.arena.destroy(device, allocator);
            for staging in &mut self.staging {
                staging.destroy(device, allocator);
            }
            self.quad_indices.destroy(device, allocator);
            self.texture.destroy(device, allocator);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.depth.destroy(device, allocator);
            self.hdr.destroy(device, allocator);
            self.clouds_raw.destroy(device, allocator);
            for target in &mut self.clouds_history {
                target.destroy(device, allocator);
            }
            for texture in self.weather_maps.iter_mut().chain(&mut self.cloud_noise) {
                texture.destroy(device, allocator);
            }
            for lut in &mut self.luts {
                lut.destroy(device, allocator);
            }
            for uniforms in &mut self.frame_uniforms {
                uniforms.destroy(device, allocator);
            }
            self.font.destroy(device, allocator);
            for buffer in &mut self.ui_buffers {
                buffer.destroy(device, allocator);
            }
            self.particles.destroy(device, allocator);
            self.swapchain.destroy(&self.ctx);
            self.pipelines.destroy(device);
            for frame in &self.frames {
                device.destroy_command_pool(frame.command_pool, None);
                device.destroy_semaphore(frame.image_available, None);
                device.destroy_fence(frame.in_flight, None);
            }
            device.destroy_command_pool(self.upload_pool, None);

            // The allocator must be gone before the device is destroyed (in ctx's Drop).
            ManuallyDrop::drop(&mut self.allocator);
        }
    }
}

/// The size of the cloud targets for a screen of `extent`.
fn cloud_extent(extent: vk::Extent2D) -> vk::Extent2D {
    vk::Extent2D {
        width: (extent.width / CLOUD_DIVISOR).max(1),
        height: (extent.height / CLOUD_DIVISOR).max(1),
    }
}

/// One descriptor set per frame in flight (they differ only in the uniform buffer).
fn create_descriptors(
    device: &ash::Device,
    pipelines: &Pipelines,
) -> Result<(vk::DescriptorPool, Vec<vk::DescriptorSet>)> {
    let frames = FRAMES_IN_FLIGHT as u32;
    let count = |ty| BINDINGS.iter().filter(|b| b.0 == ty).count() as u32 * frames;
    let sizes = [
        vk::DescriptorType::SAMPLED_IMAGE,
        vk::DescriptorType::SAMPLER,
        vk::DescriptorType::UNIFORM_BUFFER,
        vk::DescriptorType::STORAGE_BUFFER,
    ]
    .map(|ty| vk::DescriptorPoolSize {
        ty,
        descriptor_count: count(ty),
    });
    unsafe {
        let pool = device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(frames)
                .pool_sizes(&sizes),
            None,
        )?;
        let layouts = vec![pipelines.set_layout; FRAMES_IN_FLIGHT];
        let sets = device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&layouts),
        )?;
        Ok((pool, sets))
    }
}

/// Writes a 32-bit top-down BMP from BGRA (or RGBA if `rgba_order`) pixels.
fn write_bmp(path: &Path, width: u32, height: u32, pixels: &[u8], rgba_order: bool) -> Result<()> {
    let data_len = width * height * 4;
    let mut out = Vec::with_capacity(54 + data_len as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_len).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(-(height as i32)).to_le_bytes()); // negative = top-down
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]); // BI_RGB, no compression, defaults
    for px in pixels[..data_len as usize].as_chunks::<4>().0 {
        if rgba_order {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        } else {
            out.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn create_quad_index_buffer(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    upload_pool: vk::CommandPool,
) -> Result<Buffer> {
    let indices: Vec<u32> = (0..MAX_QUADS_PER_DRAW)
        .flat_map(|q| {
            let b = q * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let bytes: &[u8] = bytemuck::cast_slice(&indices);
    let size = bytes.len() as vk::DeviceSize;
    let device = &ctx.device;

    let mut staging = Buffer::new(
        device,
        allocator,
        "index staging",
        size,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write(0, bytes)?;
    let buffer = Buffer::new(
        device,
        allocator,
        "quad indices",
        size,
        vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
    )?;
    ctx.submit_and_wait(upload_pool, |cmd| unsafe {
        device.cmd_copy_buffer(
            cmd,
            staging.handle,
            buffer.handle,
            &[vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size,
            }],
        );
    })?;
    staging.destroy(device, allocator);
    Ok(buffer)
}

/// Which part of a mesh to draw.
#[derive(Clone, Copy)]
enum Section {
    Opaque,
    Water,
    /// Cutout blocks (glass): alpha-tested, drawn with the clip pipeline.
    Cutout,
}
