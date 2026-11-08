//! Graphics pipelines. All pipelines share one layout (set 0, see [`BINDINGS`], plus the
//! block push constants), so a single descriptor set per frame serves every pass.

use std::io::Cursor;

use anyhow::Result;
use ash::vk;

use super::{
    mesh::{BlockVertex, PushConstants, UiVertex},
    swapchain::DepthBuffer,
};

const BLOCK_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/block.vert.spv"));
const BLOCK_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/block.frag.spv"));
const WATER_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/block_water.frag.spv"));
const FAR_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/block_far.frag.spv"));
const FULLSCREEN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fullscreen.vert.spv"));
const SKY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.frag.spv"));
const CLOUDS_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/clouds.frag.spv"));
const CLOUDS_RESOLVE_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/clouds_resolve.frag.spv"));
const UI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.vert.spv"));
const UI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.frag.spv"));
const PARTICLES_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/particles.vert.spv"));
const PARTICLES_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/particles.frag.spv"));
const PARTICLES_BLEND_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/particles_blend.frag.spv"));
const PARTICLES_SIM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/particles_sim.comp.spv"));
const PARTICLES_EMIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/particles_emit.comp.spv"));

pub const PUSH_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw(),
);
/// Push constant bytes available to the graphics pipelines (blocks use 96, particles 112).
const PUSH_BYTES: u32 = 128;
const _: () = assert!(size_of::<PushConstants>() as u32 <= PUSH_BYTES);

/// Descriptor set 0 (must match the shaders): 0 block textures, 1 block sampler,
/// 2 LUT sampler, 3 transmittance LUT, 4 multiple scattering LUT, 5 sky irradiance LUT,
/// 6 frame uniforms, 7 HDR scene, 8 depth, 9 galaxy map, 10 font atlas, 11 particles,
/// 12 particle batch (this frame's simulation parameters and emitters), 13 and 14 weather
/// maps, 15 and 16 cloud noise, 17 raw clouds, 18 last frame's clouds, 19 this frame's clouds,
/// 20 repeating sampler.
pub const BINDINGS: [(vk::DescriptorType, vk::ShaderStageFlags); 21] = {
    use vk::{DescriptorType as T, ShaderStageFlags as S};
    const FRAGMENT_COMPUTE: S = S::from_raw(S::FRAGMENT.as_raw() | S::COMPUTE.as_raw());
    const VERTEX_COMPUTE: S = S::from_raw(S::VERTEX.as_raw() | S::COMPUTE.as_raw());
    [
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 0 block textures
        (T::SAMPLER, S::FRAGMENT),            // 1 block sampler
        (T::SAMPLER, FRAGMENT_COMPUTE),       // 2 LUT sampler (also reads depth in compute)
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 3 transmittance
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 4 multiple scattering
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 5 sky irradiance
        (T::UNIFORM_BUFFER, PUSH_STAGES),     // 6 frame uniforms
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 7 HDR scene
        (T::SAMPLED_IMAGE, FRAGMENT_COMPUTE), // 8 depth
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 9 galaxy map
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 10 font atlas
        (T::STORAGE_BUFFER, VERTEX_COMPUTE),  // 11 particles
        (T::STORAGE_BUFFER, S::COMPUTE),      // 12 particle batch
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 13 weather map A
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 14 weather map B
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 15 cloud shape noise
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 16 cloud detail noise
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 17 raw clouds
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 18 last frame's clouds
        (T::SAMPLED_IMAGE, S::FRAGMENT),      // 19 this frame's clouds
        (T::SAMPLER, S::FRAGMENT),            // 20 repeating sampler
    ]
};

pub struct Pipelines {
    pub set_layout: vk::DescriptorSetLayout,
    pub layout: vk::PipelineLayout,
    /// Block pipelines render HDR radiance into the offscreen target.
    pub opaque: vk::Pipeline,
    pub water: vk::Pipeline,
    pub far: vk::Pipeline,
    /// Full-screen atmosphere + tone mapping pass into the swapchain.
    pub sky: vk::Pipeline,
    /// Half-resolution weather in the air, and its temporal resolve.
    pub clouds: vk::Pipeline,
    pub clouds_resolve: vk::Pipeline,
    /// 2D overlay (console, HUD), alpha blended over the swapchain image.
    pub ui: vk::Pipeline,
    /// Particles: cut-out (with the opaque scene, writes depth) and blended (after water).
    pub particles: vk::Pipeline,
    pub particles_blend: vk::Pipeline,
    /// Compute: simulate the live particles, then spawn this frame's new ones.
    pub particle_sim: vk::Pipeline,
    pub particle_emit: vk::Pipeline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Opaque,
    Water,
    Far,
    Sky,
    Clouds,
    Ui,
    Particles,
    ParticlesBlend,
}

impl Pipelines {
    pub fn new(
        device: &ash::Device,
        hdr_format: vk::Format,
        swapchain_format: vk::Format,
    ) -> Result<Self> {
        let bindings: Vec<_> = BINDINGS
            .iter()
            .enumerate()
            .map(|(i, &(ty, stages))| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(i as u32)
                    .descriptor_type(ty)
                    .descriptor_count(1)
                    .stage_flags(stages)
            })
            .collect();
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?
        };

        let push_ranges = [vk::PushConstantRange {
            stage_flags: PUSH_STAGES,
            offset: 0,
            size: PUSH_BYTES,
        }];
        let set_layouts = [set_layout];
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&push_ranges),
                None,
            )?
        };

        let make = |vert: &[u8], frag: &[u8], kind| -> Result<vk::Pipeline> {
            let vert = shader_module(device, vert)?;
            let frag = shader_module(device, frag)?;
            let format = if matches!(kind, Kind::Sky | Kind::Ui) {
                swapchain_format
            } else {
                hdr_format
            };
            let pipeline = create(device, layout, format, vert, frag, kind);
            unsafe {
                device.destroy_shader_module(vert, None);
                device.destroy_shader_module(frag, None);
            }
            pipeline
        };
        let opaque = make(BLOCK_VERT, BLOCK_FRAG, Kind::Opaque);
        let water = make(BLOCK_VERT, WATER_FRAG, Kind::Water);
        let far = make(BLOCK_VERT, FAR_FRAG, Kind::Far);
        let sky = make(FULLSCREEN_VERT, SKY_FRAG, Kind::Sky);
        let clouds = make(FULLSCREEN_VERT, CLOUDS_FRAG, Kind::Clouds);
        let clouds_resolve = make(FULLSCREEN_VERT, CLOUDS_RESOLVE_FRAG, Kind::Clouds);
        let ui = make(UI_VERT, UI_FRAG, Kind::Ui);
        let particles = make(PARTICLES_VERT, PARTICLES_FRAG, Kind::Particles);
        let particles_blend = make(PARTICLES_VERT, PARTICLES_BLEND_FRAG, Kind::ParticlesBlend);
        let particle_sim = compute(device, layout, PARTICLES_SIM);
        let particle_emit = compute(device, layout, PARTICLES_EMIT);

        Ok(Self {
            set_layout,
            layout,
            opaque: opaque?,
            water: water?,
            far: far?,
            sky: sky?,
            clouds: clouds?,
            clouds_resolve: clouds_resolve?,
            ui: ui?,
            particles: particles?,
            particles_blend: particles_blend?,
            particle_sim: particle_sim?,
            particle_emit: particle_emit?,
        })
    }

    pub fn destroy(&self, device: &ash::Device) {
        unsafe {
            for p in [
                self.opaque,
                self.water,
                self.far,
                self.sky,
                self.clouds,
                self.clouds_resolve,
                self.ui,
                self.particles,
                self.particles_blend,
                self.particle_sim,
                self.particle_emit,
            ] {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

/// A compute pipeline (it shares the graphics layout: set 0; no push constants).
fn compute(device: &ash::Device, layout: vk::PipelineLayout, spv: &[u8]) -> Result<vk::Pipeline> {
    let module = shader_module(device, spv)?;
    let info = vk::ComputePipelineCreateInfo::default()
        .stage(
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main"),
        )
        .layout(layout);
    let pipelines =
        unsafe { device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None) };
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipelines.map_err(|(_, e)| e)?[0])
}

fn shader_module(device: &ash::Device, spv: &[u8]) -> Result<vk::ShaderModule> {
    let words = ash::util::read_spv(&mut Cursor::new(spv))?;
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    Ok(unsafe { device.create_shader_module(&info, None)? })
}

fn create(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    vert: vk::ShaderModule,
    frag: vk::ShaderModule,
    kind: Kind,
) -> Result<vk::Pipeline> {
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag)
            .name(c"main"),
    ];

    let bindings = BlockVertex::binding_descriptions();
    let attributes = BlockVertex::attribute_descriptions();
    let ui_bindings = UiVertex::binding_descriptions();
    let ui_attributes = UiVertex::attribute_descriptions();
    // The full-screen pass generates its triangle from the vertex index.
    let vertex_input = match kind {
        Kind::Sky | Kind::Clouds | Kind::Particles | Kind::ParticlesBlend => {
            vk::PipelineVertexInputStateCreateInfo::default()
        }
        Kind::Ui => vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&ui_bindings)
            .vertex_attribute_descriptions(&ui_attributes),
        _ => vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes),
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    // Viewport and scissor are dynamic so window resizes don't need a pipeline rebuild.
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    // Meshes use counter-clockwise front faces. Water is two-sided so its surface is
    // visible from below.
    let cull = if matches!(
        kind,
        Kind::Water | Kind::Sky | Kind::Clouds | Kind::Ui | Kind::Particles | Kind::ParticlesBlend
    ) {
        vk::CullModeFlags::NONE
    } else {
        vk::CullModeFlags::BACK
    };
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(cull)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);

    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    // Reverse-Z: closer = larger depth. Translucent water tests depth but doesn't write it.
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(!matches!(kind, Kind::Sky | Kind::Clouds | Kind::Ui))
        .depth_write_enable(matches!(kind, Kind::Opaque | Kind::Far | Kind::Particles))
        .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);

    let blend = if matches!(kind, Kind::Water | Kind::Ui | Kind::ParticlesBlend) {
        vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)
    } else {
        vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
    };
    let blend_attachments = [blend];
    let color_blend =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

    let color_formats = [color_format];
    let depth_format = if matches!(kind, Kind::Sky | Kind::Clouds | Kind::Ui) {
        vk::Format::UNDEFINED
    } else {
        DepthBuffer::FORMAT
    };
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(depth_format);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);

    let pipelines =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }
            .map_err(|(_, e)| e)?;
    Ok(pipelines[0])
}
