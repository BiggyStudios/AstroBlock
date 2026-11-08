//! Whole blocks flying through the air: torn out of the ground by tornadoes and thrown out of
//! volcanoes. Plain CPU objects (a few hundred at most), so that they can land on the real
//! world and hit the player. Each kind is one small cached mesh, drawn turned and in place.

use glam::{DVec3, Quat, Vec3};

use crate::{
    render::{
        MeshHandle,
        models::{self, Light},
    },
    util::FastMap,
    world::block::Block,
};

/// The most blocks in the air at once (the oldest make way).
const MAX_BLOCKS: usize = 260;
/// How hard the air takes hold of a block in a vortex (per second).
const GRIP: f64 = 1.25;

/// A tornado as blocks feel it (planet-relative).
#[derive(Clone, Copy)]
pub struct Vortex {
    pub foot: DVec3,
    pub up: DVec3,
    pub radius: f64,
}

/// The wind of `v` at `p`, and how strongly it holds on there (per second).
fn vortex_wind(v: &Vortex, p: DVec3) -> (DVec3, f64) {
    let d = p - v.foot;
    let h = d.dot(v.up);
    let radial = d - v.up * h;
    let r = radial.length();
    let reach = v.radius * 9.0 + 30.0;
    if r < 1e-3 || r > reach {
        return (DVec3::ZERO, 0.0);
    }
    let inward = -radial / r;
    let around = v.up.cross(inward);
    let core = v.radius.max(4.0);
    let swirl = if r < core { r / core } else { core / r };
    let wind = around * 72.0 * swirl
        + inward * 34.0 * swirl * (-h.max(0.0) / 260.0).exp()
        + v.up * 62.0 * (-(r * r) / (core * core * 5.0)).exp();
    (wind, GRIP * (1.0 - r / reach).sqrt())
}

/// What happens where it comes down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Landing {
    Shatter,
    /// Molten: it splashes and burns.
    Burn,
}

#[derive(Clone, Copy)]
pub struct FlyingBlock {
    pub block: Block,
    /// Planet-relative, and the velocity in the planet's turning frame.
    pub position: DVec3,
    pub velocity: DVec3,
    /// Edge length: 1 or 2 m.
    pub size: u8,
    pub landing: Landing,
    pub age: f32,
    /// Height of the ground below, where chunks aren't loaded (m from the planet's centre).
    pub ground: f64,
    pub ground_in: f32,
    axis: Vec3,
    angle: f32,
    spin: f32,
    /// A vortex lets go of it at this height, or after this long (m, s): it is flung away.
    ceiling: f64,
    hold: f32,
    released: bool,
}

impl FlyingBlock {
    /// `rolls` are random numbers in 0..1 (its tumbling, and how high a vortex can take it).
    pub fn new(
        block: Block,
        position: DVec3,
        velocity: DVec3,
        size: u8,
        landing: Landing,
        rolls: [f32; 4],
    ) -> Self {
        let axis = Vec3::new(rolls[0] - 0.5, rolls[1] - 0.5, rolls[2] - 0.5);
        Self {
            block,
            position,
            velocity,
            size,
            landing,
            age: 0.0,
            ground: 0.0,
            ground_in: 0.0,
            axis: axis.try_normalize().unwrap_or(Vec3::X),
            angle: rolls[3] * std::f32::consts::TAU,
            spin: 3.0 + rolls[3] * 9.0,
            ceiling: 130.0 + rolls[0] as f64 * 240.0,
            hold: 5.0 + rolls[1] * 13.0,
            released: false,
        }
    }

    pub fn step(&mut self, dt: f64, gravity: f64, vortices: &[Vortex]) {
        let mut acceleration = self.position.normalize_or(DVec3::Y) * -gravity;
        if self.age > self.hold {
            self.released = true;
        }
        for v in vortices {
            if (self.position - v.foot).dot(v.up) > self.ceiling {
                self.released = true;
            }
            if !self.released {
                let (wind, hold) = vortex_wind(v, self.position);
                acceleration += (wind - self.velocity) * hold;
            }
        }
        self.velocity += acceleration * dt;
        self.position += self.velocity * dt;
        self.angle += self.spin * dt as f32;
        self.age += dt as f32;
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_axis_angle(self.axis, self.angle)
    }
}

/// Everything in the air, and the cube meshes it is drawn with.
#[derive(Default)]
pub struct Debris {
    pub blocks: Vec<FlyingBlock>,
    pub meshes: FastMap<(Block, u8), MeshHandle>,
}

impl Debris {
    pub fn push(&mut self, block: FlyingBlock) {
        if self.blocks.len() >= MAX_BLOCKS {
            self.blocks.swap_remove(0);
        }
        self.blocks.push(block);
    }
}

/// The cube a flying block is drawn with, centred on the origin. Molten ones light themselves.
pub fn mesh(block: Block, size: u8) -> crate::render::ChunkMesh {
    models::block_cube(
        block,
        size as f64,
        Light {
            sky: 15,
            block: block.glow(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RADIUS: f64 = 43_900.0;

    /// A block on the ground beside a tornado goes up it, round it, and comes down far away.
    #[test]
    fn a_tornado_lifts_blocks_and_throws_them_out() {
        let up = DVec3::Y;
        let foot = up * RADIUS;
        let vortex = Vortex {
            foot,
            up,
            radius: 24.0,
        };
        for (i, roll) in [0.1f32, 0.5, 0.9].into_iter().enumerate() {
            let start = foot + DVec3::X * (12.0 + i as f64 * 14.0) + up * 1.0;
            let mut block = FlyingBlock::new(
                Block::Dirt,
                start,
                up * 12.0,
                1,
                Landing::Shatter,
                [roll, 0.3, 0.8, 0.4],
            );
            let mut top: f64 = 0.0;
            let mut steps = 0;
            while (block.position.length() - RADIUS > 0.0 || steps < 20) && steps < 60 * 90 {
                block.step(1.0 / 60.0, 9.81, &[vortex]);
                top = top.max(block.position.length() - RADIUS);
                steps += 1;
            }
            let landed = block.position - foot;
            let away = (landed - up * landed.dot(up)).length();
            assert!(top > 60.0, "only reached {top:.0} m");
            assert!(block.age < 90.0, "still in the air after {}", block.age);
            assert!(away > 60.0, "came down {away:.0} m from the funnel");
        }
    }

    #[test]
    fn outside_a_vortex_it_just_falls() {
        let start = DVec3::Y * (RADIUS + 50.0);
        let mut block = FlyingBlock::new(
            Block::Dirt,
            start,
            DVec3::ZERO,
            1,
            Landing::Shatter,
            [0.5; 4],
        );
        for _ in 0..60 {
            block.step(1.0 / 60.0, 9.81, &[]);
        }
        assert!((block.velocity.length() - 9.81).abs() < 0.01);
    }
}
