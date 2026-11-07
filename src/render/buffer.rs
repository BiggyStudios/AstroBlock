//! GPU buffers backed by gpu-allocator.

use anyhow::{Context as _, Result};
use ash::vk;
use gpu_allocator::{
    MemoryLocation,
    vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator},
};

pub struct Buffer {
    pub handle: vk::Buffer,
    allocation: Option<Allocation>,
}

impl Buffer {
    pub fn new(
        device: &ash::Device,
        allocator: &mut Allocator,
        name: &str,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
    ) -> Result<Self> {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let handle = unsafe { device.create_buffer(&info, None)? };
        let requirements = unsafe { device.get_buffer_memory_requirements(handle) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        unsafe { device.bind_buffer_memory(handle, allocation.memory(), allocation.offset())? };
        Ok(Self {
            handle,
            allocation: Some(allocation),
        })
    }

    /// Copies `data` into a host-visible buffer at byte `offset`.
    pub fn write(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        let slice = self
            .allocation
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
            .context("buffer is not host-visible")?;
        slice[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Contents of a host-visible buffer.
    pub fn read(&self) -> Option<&[u8]> {
        self.allocation.as_ref().and_then(|a| a.mapped_slice())
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe { device.destroy_buffer(self.handle, None) };
        if let Some(a) = self.allocation.take() {
            let _ = allocator.free(a);
        }
    }
}
