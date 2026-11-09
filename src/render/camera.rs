//! Free-flying camera with an f64 world position and a planet-relative "up".

use glam::{DVec3, Mat4, Quat, Vec3, camera::rh::proj};

/// View-model depth mapping (see `Camera::view_model_projection`): depth = a + b / distance.
const VIEW_MODEL_DEPTH: (f32, f32) = (0.6, 0.032);
/// `Camera::ease_up`: how fast the view settles (1/s), and how close counts as there (radians).
const UP_EASE: f32 = 10.0;
const UP_ARRIVED: f32 = 0.002;

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

    /// `set_up`, but only part of the way (stepping on or off something tilted, where the
    /// new "up" isn't where the old one was). Returns whether it has got there.
    pub fn ease_up(&mut self, up: Vec3, dt: f32) -> bool {
        let Some(up) = up.try_normalize() else {
            return true;
        };
        let current = self.frame * Vec3::Y;
        if current.angle_between(up) < UP_ARRIVED {
            self.set_up(up);
            return true;
        }
        let fraction = 1.0 - (-UP_EASE * dt).exp();
        let turn = Quat::IDENTITY.slerp(Quat::from_rotation_arc(current, up), fraction);
        self.frame = (turn * self.frame).normalize();
        false
    }

    /// The level frame and view angles (what a saved game keeps).
    pub fn view_state(&self) -> (Quat, f32, f32) {
        (self.frame, self.yaw, self.pitch)
    }

    pub fn set_view_state(&mut self, (frame, yaw, pitch): (Quat, f32, f32)) {
        self.frame = frame.normalize();
        self.yaw = yaw;
        self.pitch = pitch;
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

    /// Turns the whole view by `q` (moving to another world's frame).
    pub fn rotate_frame(&mut self, q: Quat) {
        self.frame = (q * self.frame).normalize();
    }

    /// Points the camera along `forward` with `up` as the screen's up (chase and map
    /// cameras). Yaw and pitch become zero: the whole orientation is in the frame.
    pub fn look_at(&mut self, forward: Vec3, up: Vec3) {
        let Some(f) = forward.try_normalize() else {
            return;
        };
        let r = f
            .cross(up)
            .try_normalize()
            .unwrap_or_else(|| f.any_orthonormal_vector());
        let u = r.cross(f);
        self.frame = Quat::from_mat3(&glam::Mat3::from_cols(r, u, -f)).normalize();
        self.yaw = 0.0;
        self.pitch = 0.0;
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

    /// `view_projection` for the first-person view model: the same picture, but its depth
    /// is squeezed into 0.6..1 (depth = 0.6 + 0.032 / distance, so 1 at 8 cm). Everything
    /// in the world is further than 8 cm and so has depth below 0.6 (`near` / distance):
    /// the arm is always in front and never pokes into walls, yet still depth-sorts itself.
    pub fn view_model_projection(&self, aspect: f32) -> Mat4 {
        let mut proj = proj::vulkan::perspective_infinite_reverse(self.fov_y, aspect, self.near);
        // clip.z = 0.6 * w + 0.032, where w = -z_view is the distance along the view.
        proj.z_axis.z = -VIEW_MODEL_DEPTH.0;
        proj.w_axis.z = VIEW_MODEL_DEPTH.1;
        proj * Mat4::from_quat(self.orientation().inverse())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_eases_over_without_a_jump() {
        let mut camera = Camera::new(DVec3::ZERO, Vec3::Y);
        let tilted = Vec3::new(0.3, 1.0, 0.0).normalize();
        let total = Vec3::Y.angle_between(tilted);
        assert!(!camera.ease_up(tilted, 1.0 / 120.0));
        let first = Vec3::Y.angle_between(camera.up());
        assert!(first > 0.0 && first < total * 0.1, "a small step: {first}");
        let mut frames = 1;
        while !camera.ease_up(tilted, 1.0 / 120.0) {
            frames += 1;
            assert!(frames < 240, "it gets there");
        }
        assert!(camera.up().angle_between(tilted) < 1e-3);
        assert!(frames > 30, "over several frames: {frames}");
    }

    #[test]
    fn view_model_depth_stays_in_front() {
        let camera = Camera::new(DVec3::ZERO, Vec3::Y);
        let depth = |m: Mat4, d: f32| {
            let clip = m * (camera.forward() * d).extend(1.0);
            clip.z / clip.w
        };
        let world = camera.view_projection(1.5);
        let arm = camera.view_model_projection(1.5);
        assert!((depth(world, 0.5) - camera.near / 0.5).abs() < 1e-5);
        assert!((depth(arm, 0.3) - (0.6 + 0.032 / 0.3)).abs() < 1e-5);
        assert!(depth(arm, 0.081) <= 1.0, "visible from 8 cm");
        // Nearer than any wall can be (the player keeps 0.3 m from walls).
        assert!(depth(arm, 1.0) > depth(world, 0.083));
        // And the same picture on screen.
        let p = Vec3::new(0.2, -0.1, -0.4);
        let (a, b) = (world * p.extend(1.0), arm * p.extend(1.0));
        assert!((a.x / a.w - b.x / b.w).abs() < 1e-6 && (a.y / a.w - b.y / b.w).abs() < 1e-6);
    }
}
