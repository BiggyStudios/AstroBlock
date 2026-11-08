//! Particle effects built on the GPU particle system (`render::particles`): block debris,
//! mining chips and placing dust, plus a test rain shower (`rain` console command) that
//! exercises the translucent, streaked, die-on-impact path the weather will use.
//!
//! Emitters here use planet-relative positions; `Effects::flush` makes them absolute.
//! The app fills in each emitter's light (block and sky) at its spot before sending them.

pub mod debris;

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

// ---- Weather -----------------------------------------------------------------------------
// All around `around` (planet-relative), with local up `up` and the surface wind.

/// Rain: streaks at terminal speed, tilted by the wind, that splash where they hit.
pub fn rain_fall(
    around: DVec3,
    up: DVec3,
    wind: Vec3,
    count: u32,
    strength: f32,
) -> ParticleEmitter {
    let fall = 10.0 + 3.5 * strength;
    let velocity = up.as_vec3() * -fall + wind;
    let height = 17.0;
    ParticleEmitter {
        count,
        // Upwind, so the drops arrive around the camera.
        origin: around + up * height as f64 - (wind * (height / fall)).as_dvec3(),
        extent: Vec3::splat(19.0),
        velocity,
        spread: 0.4,
        life: (3.2, 3.8),
        size: (0.011, 0.019),
        gravity: 0.0,
        tint: [150, 170, 200, (60.0 + 50.0 * strength) as u8],
        flags: flags::BLEND | flags::STRETCH | flags::ROUND | flags::DIE_ON_HIT | flags::ROOFED,
        ..ParticleEmitter::default()
    }
}

/// Snowflakes drifting down.
pub fn snow_fall(around: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 12.0 - (wind * 3.0).as_dvec3(),
        extent: Vec3::splat(22.0),
        velocity: up.as_vec3() * -1.9 + wind,
        spread: 0.7,
        life: (7.0, 10.0),
        size: (0.05, 0.1),
        gravity: 0.0,
        drag: 0.5,
        tint: [255, 255, 255, 215],
        flags: flags::BLEND | flags::ROUND | flags::DIE_ON_HIT | flags::ROOFED,
        ..ParticleEmitter::default()
    }
}

/// Snow driven flat by a blizzard.
pub fn snow_streaks(around: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 7.0 - (wind * 0.8).as_dvec3(),
        extent: Vec3::splat(26.0),
        velocity: wind * 2.2 - up.as_vec3() * 2.0,
        spread: 2.0,
        life: (1.5, 2.6),
        size: (0.018, 0.04),
        gravity: 0.0,
        tint: [235, 240, 250, 170],
        flags: flags::BLEND | flags::ROUND | flags::STRETCH | flags::ROOFED,
        ..ParticleEmitter::default()
    }
}

/// Sand blowing past low to the ground.
pub fn sand_streaks(around: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 4.0 - (wind * 0.6).as_dvec3(),
        extent: Vec3::splat(26.0),
        velocity: wind * 2.0 + up.as_vec3() * 0.4,
        spread: 1.8,
        life: (1.2, 2.4),
        size: (0.012, 0.03),
        gravity: 0.0,
        tint: [122, 80, 42, 170],
        flags: flags::BLEND | flags::ROUND | flags::STRETCH | flags::ROOFED,
        ..ParticleEmitter::default()
    }
}

/// Soft clouds of dust rolling by.
pub fn dust_puffs(around: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 5.0 - (wind * 1.5).as_dvec3(),
        extent: Vec3::splat(30.0),
        velocity: wind * 1.6,
        spread: 1.5,
        life: (2.5, 4.5),
        size: (3.5, 8.5),
        gravity: 0.0,
        tint: [122, 82, 46, 60],
        flags: flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// Grey flakes of ash falling after an eruption.
pub fn ash_fall(around: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: around + up * 14.0 - (wind * 4.0).as_dvec3(),
        extent: Vec3::splat(24.0),
        velocity: up.as_vec3() * -1.5 + wind,
        spread: 0.8,
        life: (8.0, 11.0),
        size: (0.05, 0.13),
        gravity: 0.0,
        drag: 0.3,
        tint: [70, 68, 66, 220],
        flags: flags::BLEND | flags::ROUND | flags::DIE_ON_HIT | flags::ROOFED,
        ..ParticleEmitter::default()
    }
}

/// Dust thrown up around a tornado's foot, drawn into the whirl (the simulation's vortex).
pub fn tornado_dust(foot: DVec3, up: DVec3, count: u32, radius: f32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: foot + up * 4.0,
        extent: Vec3::splat(radius * 3.2),
        velocity: up.as_vec3() * 4.0,
        spread: 6.0,
        life: (2.0, 4.5),
        size: (2.0, 5.0),
        gravity: 0.0,
        drag: 3.0,
        tint: [92, 78, 62, 54],
        flags: flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// Bits of ground spinning around a tornado.
pub fn tornado_debris(foot: DVec3, up: DVec3, count: u32, radius: f32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: foot + up * 22.0,
        extent: Vec3::splat(radius * 1.5),
        velocity: up.as_vec3() * 8.0,
        spread: 8.0,
        life: (3.0, 6.0),
        size: (0.25, 0.8),
        gravity: 0.35,
        drag: 2.2,
        bounce: 0.25,
        layer: crate::world::textures::Tex::Dirt as u32,
        piece: 8,
        tint: [210, 200, 190, 255],
        flags: flags::BOUNCE,
        ..ParticleEmitter::default()
    }
}

/// A lightning bolt from `top` down to `bottom`: bright, jagged, with branches. Built from
/// many short-lived glowing particles along the path, one tiny emitter per piece.
pub fn lightning(top: DVec3, bottom: DVec3, seed: u64) -> Vec<ParticleEmitter> {
    let mut rng = seed | 1;
    let mut random = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let up = (top - bottom).normalize();
    let side = up.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
    let other = up.cross(side);
    let mut out = Vec::new();
    // Midpoint displacement, then a glow every so often along the result.
    let trace = |path: &[DVec3], width: f32, out: &mut Vec<ParticleEmitter>| {
        for pair in path.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let length = (b - a).length();
            let pieces = (length / 22.0).ceil() as usize;
            for piece in 0..pieces {
                let t = (piece as f64 + 0.5) / pieces as f64;
                out.push(ParticleEmitter {
                    count: 10,
                    origin: a + (b - a) * t,
                    extent: Vec3::splat(5.0),
                    life: (0.05, 0.16),
                    size: (width, width * 1.7),
                    gravity: 0.0,
                    tint: [235, 240, 255, 255],
                    light: (15, 15),
                    flags: flags::GLOW | flags::BLEND | flags::ROUND,
                    ..ParticleEmitter::default()
                });
            }
        }
    };
    let mut jag = |from: DVec3, to: DVec3, levels: u32, rough: f64| {
        let mut path = vec![from, to];
        let mut amount = (to - from).length() * rough;
        for _ in 0..levels {
            let mut next = Vec::with_capacity(path.len() * 2);
            for pair in path.windows(2) {
                next.push(pair[0]);
                next.push(
                    (pair[0] + pair[1]) * 0.5
                        + side * (random() - 0.5) * amount
                        + other * (random() - 0.5) * amount,
                );
            }
            next.push(*path.last().unwrap());
            path = next;
            amount *= 0.55;
        }
        path
    };
    let main = jag(top, bottom, 5, 0.22);
    trace(&main, 7.0, &mut out);
    // Two branches off the upper part.
    for i in [main.len() / 5, main.len() / 3] {
        let from = main[i];
        let to = from - up * 320.0 + side * 180.0 * (1.0 - 2.0 * (i % 2) as f64) + other * 90.0;
        let branch = jag(from, to, 3, 0.25);
        trace(&branch, 4.0, &mut out);
    }
    out
}

/// Puffs of ash boiling up close to a crater (the column itself is a volume, `clouds.frag`).
pub fn ash_plume(top: DVec3, up: DVec3, wind: Vec3, count: u32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: top + up * 40.0,
        extent: Vec3::splat(70.0),
        velocity: up.as_vec3() * 34.0 + wind * 0.5,
        burst: 8.0,
        spread: 14.0,
        life: (18.0, 30.0),
        size: (36.0, 80.0),
        gravity: 0.0,
        drag: 0.02,
        tint: [34, 32, 30, 66],
        light: (0, 15),
        flags: flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// Glowing lava blasted out of the crater, high above the rim, falling back on arcs.
pub fn lava_fountain(vent: DVec3, up: DVec3, count: u32, scale: f32) -> ParticleEmitter {
    ParticleEmitter {
        count,
        origin: vent + up * 14.0,
        extent: Vec3::new(60.0, 60.0, 60.0),
        velocity: up.as_vec3() * 110.0,
        spread: 42.0,
        life: (8.0, 19.0),
        size: (0.5 * scale, 1.5 * scale),
        gravity: 1.0,
        tint: [150, 46, 8, 235],
        light: (15, 15),
        flags: flags::GLOW | flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// A piece of a river of lava seen from afar: a soft glow creeping downhill.
pub fn lava_stream(at: DVec3, along: Vec3, size: f32, glow: f32) -> ParticleEmitter {
    ParticleEmitter {
        count: 1,
        origin: at,
        extent: Vec3::splat(size * 0.3),
        velocity: along * 6.0,
        spread: 0.5,
        life: (1.5, 2.6),
        size: (size * 0.75, size),
        gravity: 0.0,
        tint: [140, 44, 8, (250.0 * glow) as u8],
        light: (15, 15),
        flags: flags::GLOW | flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// The tip of a river of lava: sparks and smoke.
pub fn lava_front(at: DVec3, up: DVec3, count: u32) -> [ParticleEmitter; 2] {
    let up = up.as_vec3();
    [
        ParticleEmitter {
            count,
            origin: at,
            extent: Vec3::splat(3.0),
            velocity: up * 7.0,
            spread: 4.0,
            life: (0.6, 1.4),
            size: (0.18, 0.4),
            gravity: 0.6,
            tint: [255, 170, 60, 255],
            light: (15, 15),
            flags: flags::GLOW | flags::BLEND | flags::ROUND,
            ..ParticleEmitter::default()
        },
        ParticleEmitter {
            count,
            origin: at + DVec3::from(up) * 2.0,
            extent: Vec3::splat(3.0),
            velocity: up * 5.0,
            spread: 1.5,
            life: (3.0, 6.0),
            size: (3.0, 7.0),
            gravity: 0.0,
            tint: [70, 66, 62, 70],
            light: (0, 15),
            flags: flags::BLEND | flags::ROUND,
            ..ParticleEmitter::default()
        },
    ]
}

/// The smoke a lava bomb trails behind it.
pub fn bomb_trail(at: DVec3, velocity: Vec3) -> ParticleEmitter {
    ParticleEmitter {
        count: 2,
        origin: at,
        extent: Vec3::splat(0.5),
        velocity: velocity * 0.1,
        spread: 1.2,
        life: (1.2, 2.6),
        size: (1.0, 2.6),
        gravity: 0.0,
        tint: [58, 52, 48, 120],
        light: (0, 15),
        flags: flags::BLEND | flags::ROUND,
        ..ParticleEmitter::default()
    }
}

/// A lava bomb coming down.
pub fn lava_splash(at: DVec3, up: DVec3) -> ParticleEmitter {
    ParticleEmitter {
        count: 40,
        origin: at + up * 0.5,
        extent: Vec3::splat(0.6),
        velocity: up.as_vec3() * 7.0,
        burst: 6.0,
        spread: 3.0,
        life: (0.5, 1.5),
        size: (0.15, 0.45),
        gravity: 1.0,
        tint: [255, 140, 40, 255],
        light: (15, 15),
        flags: flags::GLOW | flags::BLEND | flags::ROUND,
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
