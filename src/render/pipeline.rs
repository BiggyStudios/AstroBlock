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
const UI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.vert.spv"));
const UI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.frag.spv"));

pub const PUSH_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw(),
);

/// Descriptor set 0 (must match the shaders): 0 block textures, 1 block sampler,
/// 2 LUT sampler, 3 transmittance LUT, 4 multiple scattering LUT, 5 sky irradiance LUT,
/// 6 frame uniforms, 7 HDR scene, 8 depth, 9 galaxy map, 10 font atlas.
pub const BINDINGS: [(vk::DescriptorType, vk::ShaderStageFlags); 11] = {
    use vk::{DescriptorType as T, ShaderStageFlags as S};
    [
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 0 block textures
        (T::SAMPLER, S::FRAGMENT),        // 1 block sampler
        (T::SAMPLER, S::FRAGMENT),        // 2 LUT sampler
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 3 transmittance
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 4 multiple scattering
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 5 sky irradiance
        (T::UNIFORM_BUFFER, PUSH_STAGES), // 6 frame uniforms
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 7 HDR scene
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 8 depth
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 9 galaxy map
        (T::SAMPLED_IMAGE, S::FRAGMENT),  // 10 font atlas
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
    /// 2D overlay (console, HUD), alpha blended over the swapchain image.
    pub ui: vk::Pipeline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Opaque,
    Water,
    Far,
    Sky,
    Ui,
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
            size: size_of::<PushConstants>() as u32,
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
        let ui = make(UI_VERT, UI_FRAG, Kind::Ui);

        Ok(Self {
            set_layout,
            layout,
            opaque: opaque?,
            water: water?,
            far: far?,
            sky: sky?,
            ui: ui?,
        })
    }

    pub fn destroy(&self, device: &ash::Device) {
        unsafe {
            for p in [self.opaque, self.water, self.far, self.sky, self.ui] {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
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
        Kind::Sky => vk::PipelineVertexInputStateCreateInfo::default(),
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
    let cull = if matches!(kind, Kind::Water | Kind::Sky | Kind::Ui) {
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
        .depth_test_enable(!matches!(kind, Kind::Sky | Kind::Ui))
        .depth_write_enable(matches!(kind, Kind::Opaque | Kind::Far))
        .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);

    let blend = if matches!(kind, Kind::Water | Kind::Ui) {
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
    let depth_format = if matches!(kind, Kind::Sky | Kind::Ui) {
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
