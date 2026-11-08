//! The 3D noise volumes clouds are carved from. Both tile, generated once at startup.
//!
//! - `shape`: Perlin-Worley noise, big billowing masses with rounded cauliflower tops.
//! - `detail`: finer Worley noise, for eroding their edges.

/// A cube of R8 texels and its mip chain (level 0 first).
pub struct Volume {
    pub size: u32,
    pub levels: Vec<Vec<u8>>,
}

fn hash(x: i32, y: i32, z: i32, salt: u32) -> u32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
        .wrapping_add((z as u32).wrapping_mul(0xcb1a_b31f))
        .wrapping_add(salt.wrapping_mul(0x9e37_79b1));
    h ^= h >> 15;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 13;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

fn unit(h: u32) -> f32 {
    h as f32 / u32::MAX as f32
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Tiling gradient noise, about -1..1. `p` in 0..1, `period` lattice cells across.
fn perlin(p: [f32; 3], period: i32, salt: u32) -> f32 {
    let scaled = p.map(|x| x * period as f32);
    let cell = scaled.map(|x| x.floor() as i32);
    let f = [
        scaled[0] - cell[0] as f32,
        scaled[1] - cell[1] as f32,
        scaled[2] - cell[2] as f32,
    ];
    let grad = |ix: i32, iy: i32, iz: i32| {
        let h = hash(
            (cell[0] + ix).rem_euclid(period),
            (cell[1] + iy).rem_euclid(period),
            (cell[2] + iz).rem_euclid(period),
            salt,
        );
        // One of the 12 edge directions.
        let (dx, dy, dz) = (f[0] - ix as f32, f[1] - iy as f32, f[2] - iz as f32);
        match h % 12 {
            0 => dx + dy,
            1 => -dx + dy,
            2 => dx - dy,
            3 => -dx - dy,
            4 => dx + dz,
            5 => -dx + dz,
            6 => dx - dz,
            7 => -dx - dz,
            8 => dy + dz,
            9 => -dy + dz,
            10 => dy - dz,
            _ => -dy - dz,
        }
    };
    let (u, v, w) = (fade(f[0]), fade(f[1]), fade(f[2]));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(grad(0, 0, 0), grad(1, 0, 0), u);
    let x10 = lerp(grad(0, 1, 0), grad(1, 1, 0), u);
    let x01 = lerp(grad(0, 0, 1), grad(1, 0, 1), u);
    let x11 = lerp(grad(0, 1, 1), grad(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

/// Tiling inverted Worley noise: 1 at the feature points, falling to 0 a cell away.
fn worley(p: [f32; 3], period: i32, salt: u32) -> f32 {
    let scaled = p.map(|x| x * period as f32);
    let cell = scaled.map(|x| x.floor() as i32);
    let mut best = f32::MAX;
    for iz in -1..=1 {
        for iy in -1..=1 {
            for ix in -1..=1 {
                let (cx, cy, cz) = (cell[0] + ix, cell[1] + iy, cell[2] + iz);
                let (wx, wy, wz) = (
                    cx.rem_euclid(period),
                    cy.rem_euclid(period),
                    cz.rem_euclid(period),
                );
                let dx = cx as f32 + unit(hash(wx, wy, wz, salt)) - scaled[0];
                let dy = cy as f32 + unit(hash(wx, wy, wz, salt ^ 0x51)) - scaled[1];
                let dz = cz as f32 + unit(hash(wx, wy, wz, salt ^ 0xa3)) - scaled[2];
                best = best.min(dx * dx + dy * dy + dz * dz);
            }
        }
    }
    (1.0 - best.sqrt()).clamp(0.0, 1.0)
}

fn worley_fbm(p: [f32; 3], period: i32, salt: u32) -> f32 {
    worley(p, period, salt) * 0.625
        + worley(p, period * 2, salt ^ 1) * 0.25
        + worley(p, period * 4, salt ^ 2) * 0.125
}

fn perlin_fbm(p: [f32; 3], period: i32, salt: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.6, period);
    for octave in 0..4 {
        sum += perlin(p, freq, salt ^ octave) * amp;
        amp *= 0.55;
        freq *= 2;
    }
    sum * 0.5 + 0.5
}

/// Fills a volume with `f` (on several threads), stretches it to the full 0..1 range and
/// builds its mips.
fn build(size: u32, f: impl Fn([f32; 3]) -> f32 + Sync) -> Volume {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .max(1);
    let size_usize = size as usize;
    let mut values = vec![0f32; size_usize * size_usize * size_usize];
    let slice_len = size_usize * size_usize;
    let slices: Vec<(usize, &mut [f32])> = values.chunks_mut(slice_len).enumerate().collect();
    let per_thread = slices.len().div_ceil(threads);
    let mut slices = slices;
    std::thread::scope(|scope| {
        while !slices.is_empty() {
            let take = per_thread.min(slices.len());
            let batch: Vec<_> = slices.drain(..take).collect();
            let f = &f;
            scope.spawn(move || {
                for (z, slice) in batch {
                    for y in 0..size_usize {
                        for x in 0..size_usize {
                            let p = [x, y, z].map(|c| (c as f32 + 0.5) / size as f32);
                            slice[y * size_usize + x] = f(p);
                        }
                    }
                }
            });
        }
    });
    let (lo, hi) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = (hi - lo).max(1e-6);
    let base: Vec<u8> = values
        .iter()
        .map(|v| (((v - lo) / span) * 255.0 + 0.5) as u8)
        .collect();

    // Mips: a 2x2x2 box filter, wrapping nothing because the box never crosses the edge.
    let mut levels = vec![base];
    let mut s = size_usize;
    while s > 1 {
        let half = s / 2;
        let prev = levels.last().unwrap();
        let mut next = vec![0u8; half * half * half];
        for z in 0..half {
            for y in 0..half {
                for x in 0..half {
                    let mut sum = 0u32;
                    for (dx, dy, dz) in [
                        (0, 0, 0),
                        (1, 0, 0),
                        (0, 1, 0),
                        (1, 1, 0),
                        (0, 0, 1),
                        (1, 0, 1),
                        (0, 1, 1),
                        (1, 1, 1),
                    ] {
                        let i = ((z * 2 + dz) * s + (y * 2 + dy)) * s + x * 2 + dx;
                        sum += prev[i] as u32;
                    }
                    next[(z * half + y) * half + x] = (sum / 8) as u8;
                }
            }
        }
        levels.push(next);
        s = half;
    }
    Volume { size, levels }
}

/// Large billowing masses: Perlin noise with Worley cells stamped into it.
pub fn shape() -> Volume {
    build(128, |p| {
        let perlin = perlin_fbm(p, 4, 11);
        let cells = worley_fbm(p, 5, 77);
        let remap = |x: f32, a: f32, b: f32| ((x - a) / (b - a)).max(0.0);
        remap(perlin, cells - 1.0, 1.0) * 0.72 + cells * 0.28
    })
}

/// Small billows for eroding edges.
pub fn detail() -> Volume {
    build(64, |p| worley_fbm(p, 4, 1234))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles() {
        for (a, b) in [
            ([0.0, 0.3, 0.7], [1.0, 0.3, 0.7]),
            ([0.2, 0.0, 0.4], [0.2, 1.0, 0.4]),
            ([0.6, 0.1, 0.0], [0.6, 0.1, 1.0]),
        ] {
            assert!((perlin(a, 4, 3) - perlin(b, 4, 3)).abs() < 1e-3);
            assert!((worley(a, 4, 3) - worley(b, 4, 3)).abs() < 1e-3);
        }
    }
}
