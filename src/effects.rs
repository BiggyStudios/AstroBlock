//! Particle effects built on the GPU particle system (`render::particles`): block debris,
//! mining chips and placing dust, plus a test rain shower (`rain` console command) that
//! exercises the translucent, streaked, die-on-impact path the weather will use.
//!
//! Emitters here use planet-relative positions; `Effects::flush` makes them absolute.
//! The app fills in each emitter's light (block and sky) at its spot before sending them.

use glam::{DVec3, Vec3};

use crate::{
    render::{ParticleEmitter, particle_flags as flags},
    world::block::{Block, Tint},
};

/// Linear RGBA of a block's biome tint (as on its dropped item).
fn tint(block: Block) -> [u8; 4] {
    match block.tint() {
        Tint::Grass | Tint::Foliage => [0x4c, 0x9a, 0x3c, 0xff],
        Tint::Spruce => [0x3c, 0x5a, 0x3c, 0xff],
        Tint::Water => [0x3c, 0x6e, 0xdc, 0xff],
        Tint::None => [0xff; 4],
    }
}

/// Crumbs of `block`'s texture when it breaks: they start inside its cell (centre `center`,
/// planet-relative, local up `up`), pop up a little, drift slightly and fall. Kept gentle on
/// purpose: nothing should fly at the player.
pub fn block_break(block: Block, center: DVec3, up: DVec3) -> ParticleEmitter {
    let [_, side, _] = block.textures();
    // A torch's texture is a sprite (alpha = transparency), not a tinted block face.
    let look = if block == Block::Torch {
        flags::BOUNCE
    } else {
        flags::BOUNCE | flags::TINT_MASK
    };
    ParticleEmitter {
        count: 24,
        origin: center,
        extent: Vec3::splat(0.3),
        velocity: up.as_vec3() * 0.8,
        burst: 0.35,
        spread: 0.5,
        life: (0.5, 1.1),
        size: (0.05, 0.085),
        gravity: 1.0,
        drag: 0.6,
        bounce: 0.25,
        layer: side as u32,
        piece: 4,
        tint: tint(block),
        flags: look,
        ..ParticleEmitter::default()
    }
}

/// Chips knocked off the face being mined, at `point` (planet-relative) on a face facing
/// `normal`: a small hop off the face, then they fall.
pub fn mining_chips(block: Block, point: DVec3, normal: DVec3, up: DVec3) -> ParticleEmitter {
    let [_, side, _] = block.textures();
    ParticleEmitter {
        count: 4,
        origin: point + normal * 0.05,
        extent: Vec3::splat(0.12),
        velocity: (normal * 0.7 + up * 0.9).as_vec3(),
        spread: 0.5,
        life: (0.45, 0.85),
        size: (0.045, 0.07),
        gravity: 1.0,
        drag: 0.6,
        bounce: 0.2,
        layer: side as u32,
        piece: 3,
        tint: tint(block),
        flags: flags::BOUNCE | flags::TINT_MASK,
        ..ParticleEmitter::default()
    }
}

/// A little dust where a block lands.
pub fn block_place(block: Block, center: DVec3, up: DVec3) -> ParticleEmitter {
    let [_, side, _] = block.textures();
    ParticleEmitter {
        count: 8,
        origin: center - up * 0.45,
        extent: Vec3::new(0.45, 0.45, 0.45),
        velocity: up.as_vec3() * 0.6,
        spread: 0.6,
        life: (0.25, 0.5),
        size: (0.04, 0.07),
        gravity: 0.6,
        drag: 2.0,
        layer: side as u32,
        piece: 2,
        tint: tint(block),
        flags: flags::TINT_MASK,
        ..ParticleEmitter::default()
    }
}

/// Rain falling around `around` (planet-relative) with local up `up`: `count` drops this
/// frame, already at their terminal speed, disappearing where they hit something.
pub fn rain(around: DVec3, up: DVec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 14.0,
        extent: Vec3::splat(22.0),
        velocity: up.as_vec3() * -10.0,
        spread: 0.3,
        life: (4.0, 4.5),
        size: (0.018, 0.024),
        gravity: 0.0,
        tint: [170, 190, 215, 110],
        flags: flags::BLEND | flags::STRETCH | flags::ROUND | flags::DIE_ON_HIT,
        ..ParticleEmitter::default()
    }
}

/// Particle requests gathered during a frame (planet-relative), sent to the renderer once.
#[derive(Default)]
pub struct Effects {
    pub queue: Vec<ParticleEmitter>,
}

impl Effects {
    pub fn push(&mut self, emitter: ParticleEmitter) {
        self.queue.push(emitter);
    }

    /// The queued emitters with absolute origins (`planet_center` added), emptying the queue.
    pub fn flush(&mut self, planet_center: DVec3) -> Vec<ParticleEmitter> {
        self.queue
            .drain(..)
            .map(|mut e| {
                e.origin += planet_center;
                e
            })
            .collect()
    }
}

// ---- Rockets -----------------------------------------------------------------------------
// These are already lit (`light`) and glow by themselves where they burn; send them straight
// to the renderer (`Effects::flush` would relight them by the block light at their spot).

/// The engine's flame at the nozzle `at`, blowing along `down` (unit, away from the nose),
/// for a craft moving at `velocity` over the ground: a wide orange flame and a white core.
pub fn exhaust(at: DVec3, down: DVec3, velocity: DVec3, count: u32) -> [ParticleEmitter; 2] {
    let base = ParticleEmitter {
        origin: at + down * 0.4,
        extent: Vec3::splat(0.25),
        gravity: 0.0,
        light: (15, 15),
        flags: flags::GLOW | flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    };
    [
        ParticleEmitter {
            count,
            velocity: (velocity + down * 38.0).as_vec3(),
            spread: 5.0,
            life: (0.15, 0.35),
            size: (0.7, 1.4),
            tint: [255, 130, 40, 190],
            ..base
        },
        ParticleEmitter {
            count: count / 2,
            velocity: (velocity + down * 45.0).as_vec3(),
            spread: 2.0,
            life: (0.06, 0.16),
            size: (0.4, 0.75),
            tint: [255, 236, 190, 220],
            ..base
        },
    ]
}

/// Exhaust smoke in the air: grey puffs that stop in the air and linger.
pub fn smoke(at: DVec3, down: DVec3, velocity: DVec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: at + down * 2.5,
        extent: Vec3::splat(0.8),
        velocity: (velocity * 0.3 + down * 14.0).as_vec3(),
        spread: 3.0,
        life: (2.0, 4.5),
        size: (1.2, 2.8),
        gravity: 0.0,
        drag: 0.9,
        tint: [205, 205, 210, 110],
        light: (0, 15),
        flags: flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// Reentry plasma around the leading end `at`, streaming back along `flow` (unit, the way
/// the air moves past the craft); `heat` 0..1 makes it brighter and whiter.
pub fn plasma(at: DVec3, flow: DVec3, velocity: DVec3, count: u32, heat: f32) -> ParticleEmitter {
    let g = (70.0 + 120.0 * heat) as u8;
    ParticleEmitter {
        count,
        origin: at,
        extent: Vec3::splat(1.1),
        // Moving with the craft, sliding back along it.
        velocity: (velocity + flow * 14.0).as_vec3(),
        spread: 4.0,
        life: (0.06, 0.3),
        size: (0.5, 1.5),
        gravity: 0.0,
        tint: [255, g, 40, (70.0 + 110.0 * heat) as u8],
        light: (15, 15),
        flags: flags::GLOW | flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// A rocket blowing up at `at`: a fireball, smoke and bits of hull.
pub fn explosion(at: DVec3, velocity: DVec3, up: DVec3) -> Vec<ParticleEmitter> {
    let v = velocity.as_vec3();
    vec![
        ParticleEmitter {
            count: 500,
            origin: at,
            extent: Vec3::splat(2.0),
            velocity: v,
            burst: 12.0,
            spread: 14.0,
            life: (0.4, 1.3),
            size: (1.0, 3.2),
            gravity: 0.0,
            drag: 1.5,
            tint: [255, 140, 50, 200],
            light: (15, 15),
            flags: flags::GLOW | flags::BLEND | flags::ROUND,
            ..ParticleEmitter::default()
        },
        ParticleEmitter {
            count: 160,
            origin: at,
            extent: Vec3::splat(3.0),
            velocity: v + (up * 3.0).as_vec3(),
            spread: 6.0,
            life: (3.0, 6.0),
            size: (2.0, 4.5),
            gravity: 0.0,
            drag: 0.6,
            tint: [70, 68, 66, 150],
            light: (0, 15),
            flags: flags::BLEND | flags::ROUND,
            ..ParticleEmitter::default()
        },
        ParticleEmitter {
            count: 80,
            origin: at,
            extent: Vec3::splat(1.5),
            velocity: v + (up * 4.0).as_vec3(),
            burst: 10.0,
            spread: 12.0,
            life: (2.0, 4.0),
            size: (0.12, 0.4),
            gravity: 1.0,
            drag: 0.2,
            bounce: 0.3,
            layer: crate::world::textures::Tex::Hide as u32,
            tint: [235, 235, 230, 255],
            light: (0, 15),
            flags: flags::BOUNCE,
            ..ParticleEmitter::default()
        },
    ]
}
