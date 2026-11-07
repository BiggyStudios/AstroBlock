//! View-frustum culling in camera-relative space.

use glam::{Mat4, Vec3, Vec4};

pub struct Frustum {
    /// Plane equations (xyz = inward normal, w = distance); a point p is inside if
    /// `dot(n, p) + w >= 0` for every plane.
    planes: [Vec4; 5],
}

impl Frustum {
    /// Extracts planes from a (camera-relative) view-projection matrix.
    ///
    /// Vulkan clip space: -w <= x,y <= w and 0 <= z <= w. With reverse-Z the near plane is
    /// z = w; the far plane is at infinity, so it's skipped.
    pub fn from_view_projection(m: Mat4) -> Self {
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let planes =
            [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2].map(|p| p / p.truncate().length());
        Self { planes }
    }

    pub fn sphere_visible(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }
}
