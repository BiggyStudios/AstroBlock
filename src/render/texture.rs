//! Block texture array (one layer per block texture) with CPU-generated mipmaps.

use anyhow::{Result, ensure};
use ash::vk;
use gpu_allocator::{
    MemoryLocation,
    vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator},
};

use super::{atmosphere::Lut, buffer::Buffer, context::VulkanContext};

/// Square RGBA8 (sRGB) images, all the same size, one per array layer.
pub struct TextureArrayData {
    pub size: u32,
    pub layers: Vec<Vec<u8>>,
}

pub struct Texture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    allocation: Option<Allocation>,
}

impl Texture {
    pub fn new_array(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        upload_pool: vk::CommandPool,
        data: &TextureArrayData,
    ) -> Result<Self> {
        let size = data.size;
        let layer_count = data.layers.len() as u32;
        ensure!(
            size.is_power_of_two() && layer_count > 0,
            "bad texture array"
        );
        ensure!(
            data.layers
                .iter()
                .all(|l| l.len() == (size * size * 4) as usize)
        );

        let mip_levels = size.ilog2() + 1;
        let (staging_bytes, regions) = build_mip_chain(data, mip_levels);
        let device = &ctx.device;

        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .extent(vk::Extent3D {
                width: size,
                height: size,
                depth: 1,
            })
            .mip_levels(mip_levels)
            .array_layers(layer_count)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&info, None)? };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "block textures",
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

        let mut staging = Buffer::new(
            device,
            allocator,
            "texture staging",
            staging_bytes.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        staging.write(0, &staging_bytes)?;

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(mip_levels)
            .layer_count(layer_count);
        ctx.submit_and_wait(upload_pool, |cmd| unsafe {
            let to_transfer = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
            );
            device.cmd_copy_buffer_to_image(
                cmd,
                staging.handle,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
            let to_shader = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_shader),
            );
        })?;
        staging.destroy(device, allocator);

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .subresource_range(range),
                None,
            )?
        };

        // Crisp pixels up close (NEAREST mag), smooth mipmapped minification far away.
        let mut sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT)
            .max_lod(mip_levels as f32);
        if let Some(max) = ctx.max_anisotropy {
            sampler_info = sampler_info
                .anisotropy_enable(true)
                .max_anisotropy(max.min(8.0));
        }
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        Ok(Self {
            image,
            view,
            sampler,
            allocation: Some(allocation),
        })
    }

    /// A 2D RGBA16F lookup table with a linear, clamp-to-edge sampler.
    pub fn new_lut(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        upload_pool: vk::CommandPool,
        lut: &Lut,
        name: &str,
    ) -> Result<Self> {
        let texels = lut.to_rgba16f();
        Self::new_2d(
            ctx,
            allocator,
            upload_pool,
            (lut.width as u32, lut.height as u32),
            vk::Format::R16G16B16A16_SFLOAT,
            bytemuck::cast_slice(&texels),
            vk::Filter::LINEAR,
            name,
        )
    }

    /// A single-mip 2D texture with a clamp-to-edge sampler.
    #[allow(clippy::too_many_arguments)]
    pub fn new_2d(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        upload_pool: vk::CommandPool,
        (width, height): (u32, u32),
        format: vk::Format,
        bytes: &[u8],
        filter: vk::Filter,
        name: &str,
    ) -> Result<Self> {
        let device = &ctx.device;
        let extent = vk::Extent3D {
            width,
            height,
            depth: 1,
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&info, None)? };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

        let mut staging = Buffer::new(
            device,
            allocator,
            "lut staging",
            bytes.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        staging.write(0, bytes)?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        ctx.submit_and_wait(upload_pool, |cmd| unsafe {
            let to_transfer = [vk::ImageMemoryBarrier2::default()
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
            );
            let region = vk::BufferImageCopy {
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_extent: extent,
                ..Default::default()
            };
            device.cmd_copy_buffer_to_image(
                cmd,
                staging.handle,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            let to_shader = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(range)];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_shader),
            );
        })?;
        staging.destroy(device, allocator);

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(range),
                None,
            )?
        };
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(filter)
                    .min_filter(filter)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?
        };
        Ok(Self {
            image,
            view,
            sampler,
            allocation: Some(allocation),
        })
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.allocation.take() {
            let _ = allocator.free(a);
        }
    }
}

/// Builds all mip levels (box filter in linear space) and returns the staging bytes plus one
/// copy region per mip level (covering all layers).
fn build_mip_chain(
    data: &TextureArrayData,
    mip_levels: u32,
) -> (Vec<u8>, Vec<vk::BufferImageCopy>) {
    let to_linear: Vec<f32> = (0..256).map(|i| srgb_to_linear(i as f32 / 255.0)).collect();

    let mut levels: Vec<Vec<Vec<u8>>> = vec![data.layers.clone()];
    let mut size = data.size;
    for _ in 1..mip_levels {
        let half = size / 2;
        let prev = levels.last().unwrap();
        let next = prev
            .iter()
            .map(|src| {
                let mut dst = vec![0u8; (half * half * 4) as usize];
                for y in 0..half {
                    for x in 0..half {
                        let mut acc = [0.0f32; 4];
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let i = (((y * 2 + dy) * size + x * 2 + dx) * 4) as usize;
                            for c in 0..3 {
                                acc[c] += to_linear[src[i + c] as usize];
                            }
                            acc[3] += src[i + 3] as f32 / 255.0;
                        }
                        let o = ((y * half + x) * 4) as usize;
                        for c in 0..3 {
                            dst[o + c] = (linear_to_srgb(acc[c] / 4.0) * 255.0).round() as u8;
                        }
                        dst[o + 3] = (acc[3] / 4.0 * 255.0).round() as u8;
                    }
                }
                dst
            })
            .collect();
        levels.push(next);
        size = half;
    }

    let mut bytes = Vec::new();
    let mut regions = Vec::new();
    for (level, layers) in levels.iter().enumerate() {
        let level_size = (data.size >> level).max(1);
        regions.push(vk::BufferImageCopy {
            buffer_offset: bytes.len() as u64,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: level as u32,
                base_array_layer: 0,
                layer_count: layers.len() as u32,
            },
            image_offset: vk::Offset3D::default(),
            image_extent: vk::Extent3D {
                width: level_size,
                height: level_size,
                depth: 1,
            },
        });
        for layer in layers {
            bytes.extend_from_slice(layer);
        }
    }
    (bytes, regions)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
