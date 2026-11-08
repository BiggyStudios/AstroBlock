//! Swapchain (the images we present to the window) and the depth buffer.

use anyhow::Result;
use ash::vk;
use gpu_allocator::{
    MemoryLocation,
    vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator},
};
use winit::dpi::PhysicalSize;

use super::context::VulkanContext;

pub struct Swapchain {
    pub handle: vk::SwapchainKHR,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    /// One per swapchain image: signaled when rendering to that image is done, waited on
    /// by present. Must be per-image (not per-frame-in-flight) because we can't know when
    /// the presentation engine is finished with a semaphore.
    pub render_finished: Vec<vk::Semaphore>,
}

impl Swapchain {
    pub fn new(
        ctx: &VulkanContext,
        window_size: PhysicalSize<u32>,
        old: vk::SwapchainKHR,
        vsync: bool,
    ) -> Result<Self> {
        let (pd, surface) = (ctx.physical_device, ctx.surface);
        let caps = unsafe {
            ctx.surface_loader
                .get_physical_device_surface_capabilities(pd, surface)?
        };
        let formats = unsafe {
            ctx.surface_loader
                .get_physical_device_surface_formats(pd, surface)?
        };

        // Prefer an sRGB format so the hardware does gamma correction for us
        // (shaders output linear color).
        let surface_format = formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f.format,
                    vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB
                ) && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(formats[0]);

        // FIFO = vsync, guaranteed to be supported everywhere. Without vsync: IMMEDIATE, else
        // MAILBOX.
        let modes = unsafe {
            ctx.surface_loader
                .get_physical_device_surface_present_modes(pd, surface)?
        };
        let present_mode = [vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::MAILBOX]
            .into_iter()
            .find(|m| !vsync && modes.contains(m))
            .unwrap_or(vk::PresentModeKHR::FIFO);

        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: window_size
                    .width
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: window_size
                    .height
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };

        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count > 0 {
            image_count = image_count.min(caps.max_image_count);
        }

        let composite_alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::INHERIT,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        ]
        .into_iter()
        .find(|&f| caps.supported_composite_alpha.contains(f))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);

        // TRANSFER_SRC lets us copy frames out for screenshots.
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | (caps.supported_usage_flags & vk::ImageUsageFlags::TRANSFER_SRC);

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(composite_alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old);

        let device = &ctx.device;
        let handle = unsafe { ctx.swapchain_loader.create_swapchain(&info, None)? };
        let images = unsafe { ctx.swapchain_loader.get_swapchain_images(handle)? };

        let views = images
            .iter()
            .map(|&image| {
                let info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(surface_format.format)
                    .subresource_range(color_subresource_range());
                unsafe { device.create_image_view(&info, None) }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let render_finished = images
            .iter()
            .map(|_| unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) })
            .collect::<Result<Vec<_>, _>>()?;

        log::info!(
            "swapchain: {}x{}, {} images, {:?}, {:?} (available: {:?})",
            extent.width,
            extent.height,
            images.len(),
            surface_format.format,
            present_mode,
            modes
        );

        Ok(Self {
            handle,
            format: surface_format.format,
            extent,
            images,
            views,
            render_finished,
        })
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            for &s in &self.render_finished {
                ctx.device.destroy_semaphore(s, None);
            }
            for &v in &self.views {
                ctx.device.destroy_image_view(v, None);
            }
            ctx.swapchain_loader.destroy_swapchain(self.handle, None);
        }
    }
}

pub fn color_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

pub fn depth_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::DEPTH)
        .level_count(1)
        .layer_count(1)
}

/// 32-bit float depth buffer. Used with reverse-Z (near = 1.0, infinity = 0.0), which
/// gives near-uniform precision across huge view distances - essential for planets.
pub struct DepthBuffer {
    pub image: vk::Image,
    pub view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl DepthBuffer {
    pub const FORMAT: vk::Format = vk::Format::D32_SFLOAT;

    pub fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(Self::FORMAT)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            // Sampled by the atmosphere pass (distance to the terrain).
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&info, None)? };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "depth buffer",
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(Self::FORMAT)
            .subresource_range(depth_subresource_range());
        let view = unsafe { device.create_image_view(&view_info, None)? };

        Ok(Self {
            image,
            view,
            allocation: Some(allocation),
        })
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.allocation.take() {
            let _ = allocator.free(a);
        }
    }
}

/// Offscreen HDR colour target: the scene is rendered here, then composited (atmosphere,
/// tone mapping) into the swapchain.
pub struct ColorTarget {
    pub image: vk::Image,
    pub view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl ColorTarget {
    pub const FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

    pub fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(Self::FORMAT)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&info, None)? };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "hdr target",
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };
        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(Self::FORMAT)
                    .subresource_range(color_subresource_range()),
                None,
            )?
        };
        Ok(Self {
            image,
            view,
            allocation: Some(allocation),
        })
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.allocation.take() {
            let _ = allocator.free(a);
        }
    }
}
