//! Items lying in the world: they fall, land, slide to a stop, get picked up when the
//! player comes close, and vanish after 5 minutes, as in Minecraft.
//!
//! Drops are simulated in the planet's body (rotating) frame with plain local gravity: they
//! only ever move a few meters, so the frame's Coriolis effects don't matter.

use glam::DVec3;

use crate::{
    player::Voxels, render::MeshHandle, survival::items::ItemStack, world::planet::Planet,
};

/// Seconds before a drop despawns.
pub const LIFETIME: f32 = 300.0;
/// Distance from the player's middle at which drops are picked up (m).
pub const PICKUP_RADIUS: f64 = 1.6;
/// Half size of the drop's collision point above the ground (m).
const RADIUS: f64 = 0.15;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Drop {
    /// Tells it apart across the network (see `Survival::new_id`).
    #[serde(default)]
    pub id: u32,
    pub stack: ItemStack,
    /// Planet-relative position (body frame) and velocity.
    pub position: DVec3,
    pub velocity: DVec3,
    pub age: f32,
    /// Seconds until it can be picked up (thrown items fly away first).
    pub pickup_delay: f32,
    /// Seconds until a client asks the host for it again.
    #[serde(skip)]
    pub asked: f32,
    /// Its mesh, once uploaded.
    #[serde(skip)]
    pub mesh: Option<MeshHandle>,
}

impl Drop {
    pub fn new(stack: ItemStack, position: DVec3, velocity: DVec3, pickup_delay: f32) -> Self {
        Self {
            id: 0,
            stack,
            position,
            velocity,
            age: 0.0,
            pickup_delay,
            asked: 0.0,
            mesh: None,
        }
    }

    /// How far it has turned about the up axis (radians): items lying about spin slowly,
    /// each from its own start.
    pub fn spin(&self) -> f32 {
        const SPIN: f32 = 1.4;
        self.age * SPIN + (self.id % 97) as f32 * 0.65
    }

    /// Where to draw it (gently bobbing above where it lies).
    pub fn draw_position(&self) -> DVec3 {
        let up = self.position.normalize_or(DVec3::Y);
        self.position + up * (0.06 + 0.05 * (self.age as f64 * 2.5).sin() - RADIUS)
    }

    pub fn update(&mut self, planet: &Planet, world: &dyn Voxels, gravity: f64, dt: f32) {
        let dt = dt as f64;
        self.age += dt as f32;
        self.pickup_delay -= dt as f32;
        let solid = |p: DVec3| {
            planet
                .cell_at(p)
                .is_some_and(|c| world.block(c.face, c.u, c.v, c.k).collides())
        };
        let up = self.position.normalize_or(DVec3::Y);

        // Stuck inside a block (one was placed on it): pop out on top.
        if solid(self.position)
            && let Some(cell) = planet.cell_at(self.position)
        {
            self.position = up * (planet.layer_radius((cell.k + 1) as f64) + RADIUS);
            self.velocity = DVec3::ZERO;
            return;
        }

        self.velocity -= up * (gravity * dt);
        self.velocity *= (-0.4 * dt).exp(); // air
        let mut next = self.position + self.velocity * dt;

        // Landing: the point just below the item is solid.
        let below = next - up * RADIUS;
        if solid(below)
            && let Some(cell) = planet.cell_at(below)
        {
            let top = planet.layer_radius((cell.k + 1) as f64);
            next = next.normalize() * (top + RADIUS);
            let vertical = self.velocity.dot(up);
            self.velocity -= up * vertical;
            self.velocity *= (-8.0 * dt).exp(); // friction on the ground
        }
        // Hitting a wall sideways: stop there.
        if solid(next) {
            next = self.position;
            self.velocity = up * self.velocity.dot(up).min(0.0);
        }
        self.position = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        survival::items::Item,
        world::{block::Block, planet::Face},
    };

    struct Ground(i32);

    impl Voxels for Ground {
        fn block(&self, _: Face, _: i32, _: i32, k: i32) -> Block {
            if k < self.0 { Block::Stone } else { Block::Air }
        }
    }

    #[test]
    fn falls_and_comes_to_rest() {
        let planet = Planet::new(DVec3::ZERO, 64, 128, 1);
        let ground = planet.sea_layer + 5;
        let up = DVec3::new(0.3, 0.9, 0.2).normalize();
        let start = up * planet.layer_radius(ground as f64 + 3.0);
        let mut drop = Drop::new(
            ItemStack::one(Item::Stick),
            start,
            DVec3::new(1.0, 0.0, -1.0),
            0.5,
        );
        for _ in 0..600 {
            drop.update(&planet, &Ground(ground), 9.81, 1.0 / 60.0);
        }
        let height = drop.position.length() - planet.layer_radius(ground as f64);
        assert!((height - RADIUS).abs() < 1e-6, "resting at {height}");
        assert!(drop.velocity.length() < 1e-3);
    }
}
