//! Free-flying camera with an f64 world position and a planet-relative "up".

use glam::{DVec3, Mat4, Quat, Vec3, camera::rh::proj};

pub struct Camera {
    /// Absolute world position in meters. f64 so it stays precise at interplanetary
    /// (and, with per-system reference frames later, interstellar) distances.
    pub position: DVec3,
    /// Local "level" frame: rotates +Y onto the current up direction. It is parallel-
    /// transported (minimal rotation) whenever `up` changes, so your heading stays stable
    /// while you travel around a planet.
    frame: Quat,
    /// Radians around local up; positive turns right.
    pub yaw: f32,
    /// Radians, clamped to just under +-90 degrees.
    pub pitch: f32,
    pub fov_y: f32,
    pub near: f32,
}

impl Camera {
    pub fn new(position: DVec3, up: Vec3) -> Self {
        Self {
            position,
            frame: Quat::from_rotation_arc(Vec3::Y, up.normalize()),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            near: 0.05,
        }
    }

    /// Re-aligns the camera's local up (e.g. to the planet's radial direction).
    pub fn set_up(&mut self, up: Vec3) {
        let Some(up) = up.try_normalize() else { return };
        let current = self.frame * Vec3::Y;
        if current.dot(up) > 0.5 {
            // Small change (normal movement): parallel-transport the frame.
            let align = Quat::from_rotation_arc(current, up);
            self.frame = (align * self.frame).normalize();
        } else {
            // Up flipped (e.g. flying through the planet's centre): keep looking in the same
            // world direction and re-derive yaw/pitch relative to the new up. Transporting
            // the frame by ~180 degrees would spin the view around instead.
            let forward = self.forward();
            self.frame = Quat::from_rotation_arc(Vec3::Y, up);
            let local = self.frame.inverse() * forward;
            let limit = 89.9f32.to_radians();
            self.pitch = local.y.clamp(-1.0, 1.0).asin().clamp(-limit, limit);
            self.yaw = local.x.atan2(-local.z).rem_euclid(std::f32::consts::TAU);
        }
    }

    pub fn up(&self) -> Vec3 {
        self.frame * Vec3::Y
    }

    pub fn orientation(&self) -> Quat {
        self.frame * Quat::from_rotation_y(-self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    pub fn forward(&self) -> Vec3 {
        self.orientation() * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.orientation() * Vec3::X
    }

    pub fn rotate(&mut self, d_yaw: f32, d_pitch: f32) {
        let limit = 89.9f32.to_radians();
        self.yaw = (self.yaw + d_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + d_pitch).clamp(-limit, limit);
    }

    /// Projection * rotation-only view. The camera sits at the origin of render space;
    /// translation is applied per draw, relative to the camera (see `render` module docs).
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        // Reverse-Z with an infinite far plane: depth 1.0 at `near`, 0.0 at infinity.
        // The `vulkan` variant also flips Y to match Vulkan's Y-down clip space.
        let proj = proj::vulkan::perspective_infinite_reverse(self.fov_y, aspect, self.near);
        proj * Mat4::from_quat(self.orientation().inverse())
    }
}
