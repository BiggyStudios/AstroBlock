//! Core Vulkan objects: instance, surface, physical/logical device and queue.
//!
//! We target Vulkan 1.3 so we can use dynamic rendering (no render pass /
//! framebuffer objects) and synchronization2 (saner barriers).

use std::ffi::{CStr, c_char, c_void};

use anyhow::{Context as _, Result, bail};
use ash::{
    ext::debug_utils,
    khr::{surface, swapchain},
    vk,
};
use winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle},
    window::Window,
};

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

pub struct VulkanContext {
    pub _entry: ash::Entry,
    pub instance: ash::Instance,
    debug: Option<(debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub surface_loader: surface::Instance,
    pub surface: vk::SurfaceKHR,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    /// Single queue family used for graphics, transfer and present.
    /// (Dedicated transfer/compute queues can be added later for async chunk uploads.)
    pub queue_family: u32,
    pub queue: vk::Queue,
    pub swapchain_loader: swapchain::Device,
    /// `Some(max)` if anisotropic texture filtering is supported (and enabled).
    pub max_anisotropy: Option<f32>,
}

impl VulkanContext {
    pub fn new(window: &Window) -> Result<Self> {
        let entry = unsafe { ash::Entry::load() }.context("failed to load the Vulkan loader")?;
        let display_handle = window.display_handle()?.as_raw();
        let window_handle = window.window_handle()?.as_raw();

        // ---- Instance -----------------------------------------------------------------
        let debug_build = cfg!(debug_assertions);

        let available_layers = unsafe { entry.enumerate_instance_layer_properties()? };
        let has_validation = available_layers
            .iter()
            .any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
        let available_exts = unsafe { entry.enumerate_instance_extension_properties(None)? };
        let has_debug_utils = available_exts
            .iter()
            .any(|e| e.extension_name_as_c_str() == Ok(debug_utils::NAME));

        let enable_validation = debug_build && has_validation;
        let enable_debug_utils = debug_build && has_debug_utils;
        if debug_build && !has_validation {
            log::warn!(
                "Vulkan validation layers not found - install the LunarG Vulkan SDK to enable them"
            );
        }

        let mut extensions = ash_window::enumerate_required_extensions(display_handle)?.to_vec();
        if enable_debug_utils {
            extensions.push(debug_utils::NAME.as_ptr());
        }
        let layers: Vec<*const c_char> = if enable_validation {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            vec![]
        };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Astro Block")
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(c"Astro Block")
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_3);

        let mut debug_info = debug_messenger_info();
        let mut instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if enable_debug_utils {
            // Also captures messages from vkCreateInstance / vkDestroyInstance themselves.
            instance_info = instance_info.push_next(&mut debug_info);
        }
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .context("failed to create Vulkan instance (is your driver Vulkan 1.3 capable?)")?;

        let debug = if enable_debug_utils {
            let loader = debug_utils::Instance::new(&entry, &instance);
            let messenger =
                unsafe { loader.create_debug_utils_messenger(&debug_messenger_info(), None)? };
            Some((loader, messenger))
        } else {
            None
        };

        // ---- Surface ------------------------------------------------------------------
        let surface_loader = surface::Instance::new(&entry, &instance);
        let surface = unsafe {
            ash_window::create_surface(&entry, &instance, display_handle, window_handle, None)?
        };

        // ---- Physical device ----------------------------------------------------------
        let (physical_device, queue_family) =
            pick_physical_device(&instance, &surface_loader, surface)?;
        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        log::info!(
            "GPU: {} (Vulkan {}.{}.{})",
            props
                .device_name_as_c_str()
                .unwrap_or(c"?")
                .to_string_lossy(),
            vk::api_version_major(props.api_version),
            vk::api_version_minor(props.api_version),
            vk::api_version_patch(props.api_version),
        );

        // ---- Logical device -----------------------------------------------------------
        let priorities = [1.0];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let device_extensions = [swapchain::NAME.as_ptr()];

        let supported = unsafe { instance.get_physical_device_features(physical_device) };
        let anisotropy = supported.sampler_anisotropy == vk::TRUE;
        let max_anisotropy = anisotropy.then_some(props.limits.max_sampler_anisotropy.min(16.0));

        let mut features13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .features(vk::PhysicalDeviceFeatures::default().sampler_anisotropy(anisotropy))
            .push_next(&mut features13);

        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_extensions)
            .push_next(&mut features2);
        let device = unsafe { instance.create_device(physical_device, &device_info, None)? };
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let swapchain_loader = swapchain::Device::new(&instance, &device);

        Ok(Self {
            _entry: entry,
            instance,
            debug,
            surface_loader,
            surface,
            physical_device,
            device,
            queue_family,
            queue,
            swapchain_loader,
            max_anisotropy,
        })
    }

    /// Records commands with `record`, submits them and blocks until the GPU is done.
    /// Only for one-off setup work (initial uploads); per-frame work goes through the
    /// frame command buffers.
    pub fn submit_and_wait(
        &self,
        pool: vk::CommandPool,
        record: impl FnOnce(vk::CommandBuffer),
    ) -> Result<()> {
        let device = &self.device;
        unsafe {
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            record(cmd);
            device.end_command_buffer(cmd)?;

            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let cmd_info = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            let submit = vk::SubmitInfo2::default().command_buffer_infos(&cmd_info);
            let result = device
                .queue_submit2(self.queue, &[submit], fence)
                .and_then(|_| device.wait_for_fences(&[fence], true, u64::MAX));
            device.destroy_fence(fence, None);
            device.free_command_buffers(pool, &[cmd]);
            result?;
        }
        Ok(())
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            if let Some((loader, messenger)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// Picks the best GPU that supports Vulkan 1.3, the swapchain extension, the features
/// we need, and has a queue family that can do both graphics and present.
fn pick_physical_device(
    instance: &ash::Instance,
    surface_loader: &surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32)> {
    let mut best: Option<(u32, vk::PhysicalDevice, u32)> = None;

    for pd in unsafe { instance.enumerate_physical_devices()? } {
        let props = unsafe { instance.get_physical_device_properties(pd) };
        if props.api_version < vk::API_VERSION_1_3 {
            continue;
        }

        let exts = unsafe { instance.enumerate_device_extension_properties(pd)? };
        if !exts
            .iter()
            .any(|e| e.extension_name_as_c_str() == Ok(swapchain::NAME))
        {
            continue;
        }

        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut f13);
        unsafe { instance.get_physical_device_features2(pd, &mut f2) };
        if f13.dynamic_rendering == vk::FALSE || f13.synchronization2 == vk::FALSE {
            continue;
        }

        let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
        let Some(family) = (0..families.len() as u32).find(|&i| {
            families[i as usize]
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS)
                && unsafe { surface_loader.get_physical_device_surface_support(pd, i, surface) }
                    .unwrap_or(false)
        }) else {
            continue;
        };

        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 100,
            _ => 10,
        };
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, pd, family));
        }
    }

    match best {
        Some((_, pd, family)) => Ok((pd, family)),
        None => {
            bail!("no GPU found with Vulkan 1.3 + dynamic rendering + synchronization2 support")
        }
    }
}

fn debug_messenger_info() -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback))
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    let message = unsafe {
        if data.is_null() || (*data).p_message.is_null() {
            "<no message>".into()
        } else {
            CStr::from_ptr((*data).p_message).to_string_lossy()
        }
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        log::error!("[vulkan {ty:?}] {message}");
    } else {
        log::warn!("[vulkan {ty:?}] {message}");
    }
    vk::FALSE
}
