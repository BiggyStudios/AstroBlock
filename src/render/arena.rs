//! GPU memory for streamed meshes without per-mesh Vulkan allocations.
//!
//! * [`VertexArena`]: vertex data lives in a few big device-local buffers ("pages").
//!   Blocks are power-of-two sized (min 4 KiB) and recycled through per-size free lists, so
//!   allocating/freeing a chunk mesh is O(1) and never touches the Vulkan allocator.
//! * [`StagingRing`]: one persistently mapped upload buffer per frame in flight; uploads
//!   are bump-allocated and the whole buffer is reset once that frame's GPU work is done.

use anyhow::{Result, ensure};
use ash::vk;
use gpu_allocator::{MemoryLocation, vulkan::Allocator};

use super::buffer::Buffer;

const MIN_BLOCK_SHIFT: u32 = 12; // 4 KiB
const PAGE_SIZE: u64 = 64 << 20; // 64 MiB

#[derive(Clone, Copy, Debug)]
pub struct ArenaAlloc {
    pub page: u32,
    pub offset: vk::DeviceSize,
    class: u8,
    pub size: vk::DeviceSize,
}

pub struct VertexArena {
    pages: Vec<Buffer>,
    /// Bump pointer of the newest page.
    bump: vk::DeviceSize,
    free: Vec<Vec<(u32, vk::DeviceSize)>>,
    /// Bytes requested by live allocations.
    pub used_bytes: u64,
    /// Bytes of live blocks (including power-of-two rounding).
    pub block_bytes: u64,
}

impl VertexArena {
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            bump: PAGE_SIZE,
            free: vec![Vec::new(); 20],
            used_bytes: 0,
            block_bytes: 0,
        }
    }

    pub fn allocated_bytes(&self) -> u64 {
        self.pages.len() as u64 * PAGE_SIZE
    }

    pub fn alloc(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        size: vk::DeviceSize,
    ) -> Result<ArenaAlloc> {
        let block = size.max(1 << MIN_BLOCK_SHIFT).next_power_of_two();
        ensure!(
            block <= PAGE_SIZE,
            "mesh of {size} bytes is too large for the vertex arena"
        );
        let class = (block.trailing_zeros() - MIN_BLOCK_SHIFT) as u8;

        let (page, offset) = match self.free[class as usize].pop() {
            Some(slot) => slot,
            None => {
                if self.bump + block > PAGE_SIZE {
                    self.pages.push(Buffer::new(
                        device,
                        allocator,
                        "vertex arena page",
                        PAGE_SIZE,
                        vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                        MemoryLocation::GpuOnly,
                    )?);
                    self.bump = 0;
                    log::debug!("vertex arena: {} pages", self.pages.len());
                }
                let offset = self.bump;
                self.bump += block;
                ((self.pages.len() - 1) as u32, offset)
            }
        };
        self.used_bytes += size;
        self.block_bytes += block;
        Ok(ArenaAlloc {
            page,
            offset,
            class,
            size,
        })
    }

    /// Returns a block to the free list. Only call once the GPU no longer uses it.
    pub fn free(&mut self, alloc: ArenaAlloc) {
        self.used_bytes -= alloc.size;
        self.block_bytes -= 1 << (alloc.class as u32 + MIN_BLOCK_SHIFT);
        self.free[alloc.class as usize].push((alloc.page, alloc.offset));
    }

    pub fn buffer(&self, page: u32) -> vk::Buffer {
        self.pages[page as usize].handle
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        for mut page in self.pages.drain(..) {
            page.destroy(device, allocator);
        }
    }
}

pub struct StagingRing {
    buffer: Buffer,
    cursor: usize,
    capacity: usize,
}

impl StagingRing {
    pub fn new(device: &ash::Device, allocator: &mut Allocator, capacity: usize) -> Result<Self> {
        let buffer = Buffer::new(
            device,
            allocator,
            "staging ring",
            capacity as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        Ok(Self {
            buffer,
            cursor: 0,
            capacity,
        })
    }

    pub fn handle(&self) -> vk::Buffer {
        self.buffer.handle
    }

    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// Copies `parts` back to back into the ring. Returns the start offset, or `None` if
    /// they don't fit this frame.
    pub fn push(&mut self, parts: &[&[u8]]) -> Result<Option<vk::DeviceSize>> {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let start = self.cursor.next_multiple_of(16);
        if start + len > self.capacity {
            return Ok(None);
        }
        let mut at = start;
        for part in parts {
            self.buffer.write(at, part)?;
            at += part.len();
        }
        self.cursor = at;
        Ok(Some(start as vk::DeviceSize))
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        self.buffer.destroy(device, allocator);
    }
}
