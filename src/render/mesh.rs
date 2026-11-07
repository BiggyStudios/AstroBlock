//! Vertex formats, CPU-side mesh data and GPU mesh handles.

use std::mem::{offset_of, size_of};

use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Quat, Vec4};

use super::arena::ArenaAlloc;

/// Vertex for block geometry (12 bytes).
///
/// Positions are integers on a global lattice of `ChunkMesh::scale` meters (2 mm for
/// chunks), stored relative to the mesh origin. Because every mesh snaps to the *same*
/// lattice, vertices shared by neighbouring chunks are bit-identical - no cracks.
/// Normals aren't stored: the fragment shader derives flat face normals from screen-space
/// derivatives, which also handles the slightly curved blocks of a spherical planet.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BlockVertex {
    /// xyz: position in lattice units relative to the mesh origin. w: tint colour as
    /// RGB565 of the square root of the linear colour (more precision in the darks).
    pub position: [i16; 4],
    /// See [`BlockVertex::pack`].
    pub data: u32,
}

impl BlockVertex {
    /// `layer`: texture array layer (0-255), `corner`: uv corner bits (u | v << 1),
    /// `ao`: ambient occlusion 0 (dark) - 3 (open), `light`: sky light 0-15,
    /// `size`: quad size in blocks along the texture u/v axes (1-32), so textures repeat
    /// once per block on merged quads.
    pub const fn pack(layer: u32, corner: u32, ao: u32, light: u32, size: [u32; 2]) -> u32 {
        layer
            | (corner << 8)
            | (ao << 10)
            | (light << 12)
            | ((size[0] - 1) << 16)
            | ((size[1] - 1) << 21)
    }

    pub fn new(position: [i16; 3], tint: u16, data: u32) -> Self {
        Self {
            position: [position[0], position[1], position[2], tint as i16],
            data,
        }
    }

    /// Packs a linear RGBA8 tint (as produced by `terrain::pack_rgb`) into RGB565.
    pub fn tint565(rgba: u32) -> u16 {
        let channel = |shift: u32, bits: u32| {
            let linear = ((rgba >> shift) & 0xff) as f32 / 255.0;
            (linear.sqrt() * ((1 << bits) - 1) as f32).round() as u16
        };
        (channel(0, 5) << 11) | (channel(8, 6) << 5) | channel(16, 5)
    }

    pub fn binding_descriptions() -> [vk::VertexInputBindingDescription; 1] {
        [vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<Self>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        }]
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 2] {
        let attr = |location, format, offset: usize| vk::VertexInputAttributeDescription {
            location,
            binding: 0,
            format,
            offset: offset as u32,
        };
        [
            attr(0, vk::Format::R16G16B16A16_SINT, offset_of!(Self, position)),
            attr(1, vk::Format::R32_UINT, offset_of!(Self, data)),
        ]
    }
}

/// A vertex of the 2D overlay (`shaders/ui.*`): pixel position, uv, sRGB colour + alpha,
/// and the block texture layer (or `u32::MAX` for the font atlas).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub layer: u32,
}

impl UiVertex {
    pub fn binding_descriptions() -> [vk::VertexInputBindingDescription; 1] {
        [vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<Self>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        }]
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 4] {
        let attr = |location, format, offset: usize| vk::VertexInputAttributeDescription {
            location,
            binding: 0,
            format,
            offset: offset as u32,
        };
        [
            attr(0, vk::Format::R32G32_SFLOAT, offset_of!(Self, pos)),
            attr(1, vk::Format::R32G32_SFLOAT, offset_of!(Self, uv)),
            attr(2, vk::Format::R8G8B8A8_UNORM, offset_of!(Self, color)),
            attr(3, vk::Format::R32_UINT, offset_of!(Self, layer)),
        ]
    }
}

/// Push constants shared by all block pipelines. Must match `shaders/block.*`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PushConstants {
    pub mvp: Mat4,
    /// xyz: mesh origin relative to camera, w: inner discard radius (fragments closer than
    /// this are dropped; clip pipelines only).
    pub offset: Vec4,
    /// x: meters per position unit, y: outer discard radius, z: fade progress, w: fade
    /// direction (1 in, -1 out, 0 none). y/z/w are only used by the clip pipelines.
    pub params: Vec4,
    /// The mesh's rotation (a quaternion, xyzw) about its origin, applied before `mvp`.
    pub rotation: Vec4,
}

/// CPU-side mesh made of quads (4 vertices each, drawn with the shared quad index buffer).
pub struct ChunkMesh {
    pub opaque: Vec<BlockVertex>,
    pub water: Vec<BlockVertex>,
    /// Alpha-tested quads (glass), drawn with the clip pipeline so the main opaque pipeline
    /// keeps early depth testing.
    pub cutout: Vec<BlockVertex>,
    /// Meters per position unit.
    pub scale: f32,
}

impl ChunkMesh {
    pub fn new(scale: f32) -> Self {
        Self {
            opaque: Vec::new(),
            water: Vec::new(),
            cutout: Vec::new(),
            scale,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.water.is_empty() && self.cutout.is_empty()
    }
}

/// GPU copy of a [`ChunkMesh`]: one vertex buffer with the opaque, water and cutout quads.
pub struct GpuMesh {
    pub alloc: ArenaAlloc,
    pub opaque_quads: u32,
    pub water_quads: u32,
    pub cutout_quads: u32,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshHandle(pub(super) usize);

/// One thing to draw this frame: a mesh at an absolute f64 world position.
#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    pub mesh: MeshHandle,
    pub origin: DVec3,
    /// Rotation of the mesh about `origin` (world axes). Identity for almost everything;
    /// the moon turns.
    pub rotation: Quat,
    /// Bounding sphere radius around `origin`, for frustum culling.
    pub radius: f32,
    pub fade: Fade,
}

/// Dithered cross-fade state of a draw (meshes appearing or disappearing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fade {
    Visible,
    /// Fading in; the fraction (0..1) already shown.
    In(f32),
    /// Fading out; the fraction (0..1) already hidden. A mesh fading out with progress p is
    /// drawn on exactly the pixels where a mesh fading in with progress p is not.
    Out(f32),
}

impl Fade {
    /// The state of a fade that started `elapsed` seconds ago and lasts `duration`.
    pub fn new(elapsed: f32, duration: f32, out: bool) -> Fade {
        let p = (elapsed / duration).max(0.0);
        match (out, p >= 1.0) {
            (false, true) => Fade::Visible,
            (false, false) => Fade::In(p),
            (true, _) => Fade::Out(p.min(1.0)),
        }
    }

    /// (progress, direction) as passed to the shader.
    pub fn shader(self) -> (f32, f32) {
        match self {
            Fade::Visible => (1.0, 0.0),
            Fade::In(p) => (p, 1.0),
            Fade::Out(p) => (p, -1.0),
        }
    }
}
