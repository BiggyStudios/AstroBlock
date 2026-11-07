//! Deterministic f64 3D simplex noise and fractal helpers.
//!
//! Noise is always sampled in 3D at positions on (or in) the planet sphere, so there are no
//! seams or pole pinching. Gradients come from an integer hash instead of a 256-entry
//! permutation table, so patterns never repeat (a permutation table would tile every
//! 256 noise units - that's only ~12 km for cave-sized noise).

use glam::DVec3;

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    seed: u64,
}

const GRAD3: [[f64; 3]; 12] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
];

/// SplitMix64 finalizer: a fast, well-distributed 64-bit mixer.
#[inline]
pub fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[inline]
pub fn hash3(seed: u64, i: i64, j: i64, k: i64) -> u64 {
    mix64(
        seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (j as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ (k as u64).wrapping_mul(0x1656_67b1_9e37_79f9),
    )
}

/// Hash -> uniform f64 in [0, 1).
#[inline]
pub fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self { seed: mix64(seed) }
    }

    #[inline]
    fn grad(&self, i: i64, j: i64, k: i64, x: f64, y: f64, z: f64) -> f64 {
        let g = GRAD3[(hash3(self.seed, i, j, k) % 12) as usize];
        g[0] * x + g[1] * y + g[2] * z
    }

    /// 3D simplex noise in roughly [-1, 1].
    pub fn noise(&self, p: DVec3) -> f64 {
        const F3: f64 = 1.0 / 3.0;
        const G3: f64 = 1.0 / 6.0;
        let (x, y, z) = (p.x, p.y, p.z);

        let s = (x + y + z) * F3;
        let (i, j, k) = ((x + s).floor(), (y + s).floor(), (z + s).floor());
        let t = (i + j + k) * G3;
        let (x0, y0, z0) = (x - (i - t), y - (j - t), z - (k - t));

        let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
            if y0 >= z0 {
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 < z0 {
            (0, 0, 1, 0, 1, 1)
        } else if x0 < z0 {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 1, 0, 1, 1, 0)
        };

        let (x1, y1, z1) = (
            x0 - i1 as f64 + G3,
            y0 - j1 as f64 + G3,
            z0 - k1 as f64 + G3,
        );
        let (x2, y2, z2) = (
            x0 - i2 as f64 + 2.0 * G3,
            y0 - j2 as f64 + 2.0 * G3,
            z0 - k2 as f64 + 2.0 * G3,
        );
        let (x3, y3, z3) = (
            x0 - 1.0 + 3.0 * G3,
            y0 - 1.0 + 3.0 * G3,
            z0 - 1.0 + 3.0 * G3,
        );
        let (i, j, k) = (i as i64, j as i64, k as i64);

        let mut n = 0.0;
        let mut corner = |dx: f64, dy: f64, dz: f64, ci: i64, cj: i64, ck: i64| {
            let t = 0.6 - dx * dx - dy * dy - dz * dz;
            if t > 0.0 {
                let t2 = t * t;
                n += t2 * t2 * self.grad(ci, cj, ck, dx, dy, dz);
            }
        };
        corner(x0, y0, z0, i, j, k);
        corner(x1, y1, z1, i + i1, j + j1, k + k1);
        corner(x2, y2, z2, i + i2, j + j2, k + k2);
        corner(x3, y3, z3, i + 1, j + 1, k + 1);
        32.0 * n
    }

    fn octave(&self, o: u32) -> Noise {
        Noise {
            seed: mix64(self.seed.wrapping_add(o as u64 + 1)),
        }
    }

    /// Fractal Brownian motion, normalized to roughly [-1, 1] (typically within +-0.6).
    pub fn fbm(&self, p: DVec3, octaves: u32) -> f64 {
        let (mut sum, mut amp, mut norm, mut freq) = (0.0, 1.0, 0.0, 1.0);
        for o in 0..octaves {
            sum += self.octave(o).noise(p * freq) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }

    /// Ridged multifractal in [0, 1]: sharp crests, ideal for mountain ranges and dunes.
    pub fn ridged(&self, p: DVec3, octaves: u32) -> f64 {
        let (mut sum, mut amp, mut norm, mut freq, mut weight) = (0.0, 1.0, 0.0, 1.0, 1.0);
        for o in 0..octaves {
            let n = 1.0 - self.octave(o).noise(p * freq).abs();
            let signal = n * n * weight;
            weight = (signal * 2.0).clamp(0.0, 1.0);
            sum += signal * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
}

pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
