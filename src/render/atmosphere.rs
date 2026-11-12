//! Physically based atmosphere: Rayleigh and Mie scattering plus ozone absorption.
//!
//! The model follows Hillaire 2020 ("A Scalable and Production Ready Sky and Atmosphere
//! Rendering Technique"), with Bruneton's transmittance parameterisation. Three small lookup
//! tables (LUTs) are precomputed on the CPU at startup and sampled by the shaders:
//!
//! * **transmittance** (r, mu): how much sunlight survives the path from radius `r` to the
//!   top of the atmosphere at zenith cosine `mu`. This is what turns sunsets red.
//! * **multiple scattering** (r, mu_s): Hillaire's isotropic approximation of all scattering
//!   orders >= 2, per unit sun illuminance. It brightens twilight and deep shadows.
//! * **sky irradiance** (r, mu_s): light from the whole sky dome onto an up-facing surface,
//!   per unit sun illuminance. This is the terrain's ambient light.
//!
//! The per-pixel ray march (sky, aerial perspective, the planet's limb seen from space) runs
//! in `shaders/sky.frag`. The functions in `shaders/atmosphere.glsl` must match the ones here.
//!
//! The planet is small (~42 km radius), so its atmosphere is a scaled-down Earth's: scale
//! heights shrink and the coefficients grow, which keeps Earth's vertical optical depth (a
//! blue sky) and gives about the same horizontal haze.

use std::f32::consts::PI;

use glam::Vec3;

pub const TRANSMITTANCE_SIZE: (usize, usize) = (256, 64);
pub const MULTISCATTERING_SIZE: (usize, usize) = (32, 32);
pub const IRRADIANCE_SIZE: (usize, usize) = (64, 16);

#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    /// Sea level radius (m).
    pub bottom_radius: f32,
    pub top_radius: f32,
    /// Per meter, at sea level.
    pub rayleigh_scattering: Vec3,
    pub rayleigh_scale_height: f32,
    pub mie_scattering: f32,
    pub mie_extinction: f32,
    pub mie_scale_height: f32,
    /// Mie phase anisotropy (forward scattering: the bright halo around the sun).
    pub mie_g: f32,
    /// Ozone: absorption per meter at the peak of a tent-shaped layer.
    pub ozone_absorption: Vec3,
    pub ozone_center: f32,
    pub ozone_half_width: f32,
    pub ground_albedo: f32,
}

impl Atmosphere {
    /// An Earth-like atmosphere scaled to a planet of `radius` (m).
    pub fn earth_like(radius: f64) -> Self {
        // Earth: Rayleigh H = 8 km, Mie H = 1.2 km, ozone layer around 25 km. Scaled heights
        // keep twilight reasonably short on this small planet (the sun lights the top of a
        // thick atmosphere long after it has set).
        let rayleigh_h = 1_200.0;
        let mie_h = 300.0;
        // Above Earth's (8 / 1.2 would keep Earth's vertical optical depth): the horizon is
        // so close on this small planet that sunsets need the extra scattering to turn red.
        let rayleigh_scale = 10.0;
        let mie_scale = 1_200.0 / mie_h;
        let ozone_scale = 15_000.0 / 2_000.0;
        Self {
            bottom_radius: radius as f32,
            top_radius: radius as f32 + 9_000.0,
            rayleigh_scattering: Vec3::new(5.802e-6, 13.558e-6, 33.1e-6) * rayleigh_scale,
            rayleigh_scale_height: rayleigh_h,
            mie_scattering: 3.996e-6 * mie_scale,
            mie_extinction: 4.40e-6 * mie_scale,
            mie_scale_height: mie_h,
            mie_g: 0.8,
            ozone_absorption: Vec3::new(0.650e-6, 1.881e-6, 0.085e-6) * ozone_scale,
            ozone_center: 3_500.0,
            ozone_half_width: 2_000.0,
            ground_albedo: 0.3,
        }
    }

    /// (Rayleigh scattering, Mie scattering, extinction) per meter at radius `r`.
    pub fn medium(&self, r: f32) -> (Vec3, f32, Vec3) {
        let h = (r - self.bottom_radius).max(0.0);
        let rayleigh = self.rayleigh_scattering * (-h / self.rayleigh_scale_height).exp();
        let mie_density = (-h / self.mie_scale_height).exp();
        let ozone = (1.0 - (h - self.ozone_center).abs() / self.ozone_half_width).max(0.0);
        let extinction = rayleigh
            + Vec3::splat(self.mie_extinction * mie_density)
            + self.ozone_absorption * ozone;
        (rayleigh, self.mie_scattering * mie_density, extinction)
    }

    /// Distance from radius `r` along zenith cosine `mu` to the top of the atmosphere.
    fn distance_to_top(&self, r: f32, mu: f32) -> f32 {
        let disc = r * r * (mu * mu - 1.0) + self.top_radius * self.top_radius;
        (-r * mu + disc.max(0.0).sqrt()).max(0.0)
    }

    /// Distance to the ground (sea level sphere), if the ray hits it.
    fn distance_to_ground(&self, r: f32, mu: f32) -> Option<f32> {
        let disc = r * r * (mu * mu - 1.0) + self.bottom_radius * self.bottom_radius;
        (mu < 0.0 && disc >= 0.0).then(|| (-r * mu - disc.sqrt()).max(0.0))
    }

    /// Zenith cosine of the geometric horizon seen from radius `r`.
    pub fn horizon_mu(&self, r: f32) -> f32 {
        let s = (self.bottom_radius / r.max(self.bottom_radius)).min(1.0);
        -(1.0 - s * s).max(0.0).sqrt()
    }

    fn h(&self) -> f32 {
        (self.top_radius * self.top_radius - self.bottom_radius * self.bottom_radius).sqrt()
    }

    /// Transmittance LUT coordinates (both 0..1) for (r, mu). Rays below the horizon are not
    /// represented (callers clamp mu to the horizon and apply the planet's shadow).
    pub fn transmittance_coords(&self, r: f32, mu: f32) -> (f32, f32) {
        let r = r.clamp(self.bottom_radius, self.top_radius);
        let h = self.h();
        let rho = (r * r - self.bottom_radius * self.bottom_radius)
            .max(0.0)
            .sqrt();
        let d = self.distance_to_top(r, mu);
        let (d_min, d_max) = (self.top_radius - r, rho + h);
        ((d - d_min) / (d_max - d_min).max(1e-3), rho / h)
    }

    fn transmittance_r_mu(&self, x_mu: f32, x_r: f32) -> (f32, f32) {
        let h = self.h();
        let rho = h * x_r;
        let r = (rho * rho + self.bottom_radius * self.bottom_radius).sqrt();
        let (d_min, d_max) = (self.top_radius - r, rho + h);
        let d = d_min + x_mu * (d_max - d_min);
        let mu = if d == 0.0 {
            1.0
        } else {
            (h * h - rho * rho - d * d) / (2.0 * r * d)
        };
        (r, mu.clamp(-1.0, 1.0))
    }

    fn compute_transmittance(&self, r: f32, mu: f32) -> Vec3 {
        const STEPS: usize = 40;
        let d = self.distance_to_top(r, mu);
        let dt = d / STEPS as f32;
        let mut depth = Vec3::ZERO;
        for i in 0..STEPS {
            let t = (i as f32 + 0.5) * dt;
            let ri = (t * t + 2.0 * r * mu * t + r * r).sqrt();
            depth += self.medium(ri).2 * dt;
        }
        (-depth).exp()
    }

    /// Precomputes all lookup tables.
    pub fn luts(&self) -> AtmosphereLuts {
        let transmittance = Lut::build(TRANSMITTANCE_SIZE, |x, y| {
            let (r, mu) = self.transmittance_r_mu(x, y);
            self.compute_transmittance(r, mu)
        });
        let multiscattering = Lut::build(MULTISCATTERING_SIZE, |x, y| {
            let (r, mu_s) = self.sun_coords_inverse(x, y);
            self.compute_multiscattering(&transmittance, r, mu_s)
        });
        let mut luts = AtmosphereLuts {
            transmittance,
            multiscattering,
            irradiance: Lut::default(),
        };
        luts.irradiance = Lut::build(IRRADIANCE_SIZE, |x, y| {
            let (r, mu_s) = self.sun_coords_inverse(x, y);
            self.compute_irradiance(&luts, r, mu_s)
        });
        luts
    }

    /// (r, mu_s) -> coordinates of the multiple scattering and irradiance LUTs.
    pub fn sun_coords(&self, r: f32, mu_s: f32) -> (f32, f32) {
        let h = (r - self.bottom_radius) / (self.top_radius - self.bottom_radius);
        (mu_s * 0.5 + 0.5, h.clamp(0.0, 1.0))
    }

    fn sun_coords_inverse(&self, x: f32, y: f32) -> (f32, f32) {
        let r = self.bottom_radius + y * (self.top_radius - self.bottom_radius);
        // Stay a hair above the ground so rays don't start inside the planet.
        (r.max(self.bottom_radius + 1.0), x * 2.0 - 1.0)
    }

    /// Transmittance towards the sun at radius `r`, zenith cosine `mu_s`, including the
    /// planet's shadow (softened over the sun's disk).
    pub fn sun_transmittance(&self, luts: &AtmosphereLuts, r: f32, mu_s: f32) -> Vec3 {
        let horizon = self.horizon_mu(r);
        let visible = smoothstep(horizon - 0.01, horizon + 0.01, mu_s);
        let (r, mu) = match self.entering_air(r, mu_s.max(horizon)) {
            Some(entry) => entry,
            None => return Vec3::ONE * visible,
        };
        let (x, y) = self.transmittance_coords(r, mu);
        luts.transmittance.sample(x, y) * visible
    }

    /// A ray from radius `r` at zenith cosine `mu`, from where it enters the air: unchanged
    /// inside it, None if it starts above the air and misses it. (As `transmittance_lut` in
    /// atmosphere.glsl.)
    fn entering_air(&self, r: f32, mu: f32) -> Option<(f32, f32)> {
        let top = self.top_radius;
        if r <= top {
            return Some((r, mu));
        }
        let disc = r * r * (mu * mu - 1.0) + top * top;
        if mu >= 0.0 || disc <= 0.0 {
            return None;
        }
        let d = -r * mu - disc.sqrt();
        Some((top, ((r * mu + d) / top).clamp(-1.0, 1.0)))
    }

    fn compute_multiscattering(&self, transmittance: &Lut, r: f32, mu_s: f32) -> Vec3 {
        const DIRECTIONS: usize = 64;
        const STEPS: usize = 20;
        let luts_t = |r: f32, mu: f32| {
            let horizon = self.horizon_mu(r);
            if mu < horizon {
                return Vec3::ZERO;
            }
            let (x, y) = self.transmittance_coords(r, mu);
            transmittance.sample(x, y)
        };
        let origin = Vec3::new(0.0, r, 0.0);
        let sun = Vec3::new((1.0 - mu_s * mu_s).max(0.0).sqrt(), mu_s, 0.0);
        let (mut l_sum, mut f_sum) = (Vec3::ZERO, Vec3::ZERO);
        for k in 0..DIRECTIONS {
            let dir = fibonacci_sphere(k, DIRECTIONS);
            let mu = dir.y;
            let ground = self.distance_to_ground(r, mu);
            let t_max = ground.unwrap_or_else(|| self.distance_to_top(r, mu));
            let dt = t_max / STEPS as f32;
            let (mut l, mut f, mut throughput) = (Vec3::ZERO, Vec3::ZERO, Vec3::ONE);
            for i in 0..STEPS {
                let p = origin + dir * ((i as f32 + 0.3) * dt);
                let rp = p.length();
                let (rayleigh, mie, extinction) = self.medium(rp);
                let scattering = rayleigh + Vec3::splat(mie);
                let sun_t = luts_t(rp, p.dot(sun) / rp);
                let step_t = (-extinction * dt).exp();
                let integrate = |s: Vec3| (s - s * step_t) / extinction.max(Vec3::splat(1e-12));
                l += throughput * integrate(scattering * sun_t / (4.0 * PI));
                f += throughput * integrate(scattering);
                throughput *= step_t;
            }
            if ground.is_some() {
                let p = origin + dir * t_max;
                let up = p.normalize();
                let mu_g = up.dot(sun);
                l += throughput
                    * luts_t(self.bottom_radius, mu_g)
                    * mu_g.max(0.0)
                    * self.ground_albedo
                    / PI;
            }
            l_sum += l;
            f_sum += f;
        }
        let (l, f) = (l_sum / DIRECTIONS as f32, f_sum / DIRECTIONS as f32);
        l / (Vec3::ONE - f).max(Vec3::splat(1e-3))
    }

    fn compute_irradiance(&self, luts: &AtmosphereLuts, r: f32, mu_s: f32) -> Vec3 {
        const SIDE: usize = 8;
        let origin = Vec3::new(0.0, r, 0.0);
        let sun = Vec3::new((1.0 - mu_s * mu_s).max(0.0).sqrt(), mu_s, 0.0);
        let mut sum = Vec3::ZERO;
        for i in 0..SIDE {
            for j in 0..SIDE {
                // Cosine-weighted hemisphere directions around +Y.
                let (u1, u2) = (
                    (i as f32 + 0.5) / SIDE as f32,
                    (j as f32 + 0.5) / SIDE as f32,
                );
                let (s, phi) = (u1.sqrt(), 2.0 * PI * u2);
                let dir = Vec3::new(s * phi.cos(), (1.0 - u1).sqrt(), s * phi.sin());
                sum += self.sky_radiance(luts, origin, dir, sun, 24);
            }
        }
        sum * PI / (SIDE * SIDE) as f32
    }

    /// Sky radiance (per unit sun illuminance) seen from `origin` along `dir`. Mirrors the
    /// ray march in `sky.frag` (without the planet sphere and aerial perspective).
    pub fn sky_radiance(
        &self,
        luts: &AtmosphereLuts,
        origin: Vec3,
        dir: Vec3,
        sun: Vec3,
        steps: usize,
    ) -> Vec3 {
        let r = origin.length();
        let mu = origin.dot(dir) / r;
        let t_max = self
            .distance_to_ground(r, mu)
            .unwrap_or_else(|| self.distance_to_top(r, mu));
        let cos_theta = dir.dot(sun);
        let phase_r = rayleigh_phase(cos_theta);
        let phase_m = mie_phase(self.mie_g, cos_theta);
        let dt = t_max / steps as f32;
        let (mut l, mut throughput) = (Vec3::ZERO, Vec3::ONE);
        for i in 0..steps {
            let p = origin + dir * ((i as f32 + 0.3) * dt);
            let rp = p.length();
            let mu_s = p.dot(sun) / rp;
            let (rayleigh, mie, extinction) = self.medium(rp);
            let sun_t = self.sun_transmittance(luts, rp, mu_s);
            let (x, y) = self.sun_coords(rp, mu_s);
            let ms = luts.multiscattering.sample(x, y);
            let s = sun_t * (rayleigh * phase_r + Vec3::splat(mie * phase_m))
                + (rayleigh + Vec3::splat(mie)) * ms;
            let step_t = (-extinction * dt).exp();
            l += throughput * (s - s * step_t) / extinction.max(Vec3::splat(1e-12));
            throughput *= step_t;
        }
        l
    }

    /// Sky irradiance on an up-facing surface at radius `r` (per unit sun illuminance).
    pub fn sky_irradiance(&self, luts: &AtmosphereLuts, r: f32, mu_s: f32) -> Vec3 {
        let (x, y) = self.sun_coords(r, mu_s);
        luts.irradiance.sample(x, y)
    }
}

pub struct AtmosphereLuts {
    pub transmittance: Lut,
    pub multiscattering: Lut,
    pub irradiance: Lut,
}

/// An RGB float table addressed with coordinates in 0..1, where 0 and 1 are the centres of
/// the first and last texels - the same convention the shaders use.
#[derive(Default)]
pub struct Lut {
    pub width: usize,
    pub height: usize,
    pub data: Vec<Vec3>,
}

impl Lut {
    pub fn build((width, height): (usize, usize), f: impl Fn(f32, f32) -> Vec3 + Sync) -> Self {
        let mut data = vec![Vec3::ZERO; width * height];
        // Rows in parallel: the irradiance LUT is a few million ray-march steps.
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let rows_per = height.div_ceil(threads);
        std::thread::scope(|scope| {
            for (chunk_index, chunk) in data.chunks_mut(rows_per * width).enumerate() {
                let f = &f;
                scope.spawn(move || {
                    for (i, texel) in chunk.iter_mut().enumerate() {
                        let (x, y) = (i % width, chunk_index * rows_per + i / width);
                        *texel = f(
                            x as f32 / (width - 1) as f32,
                            y as f32 / (height - 1) as f32,
                        );
                    }
                });
            }
        });
        Self {
            width,
            height,
            data,
        }
    }

    /// Bilinear sample.
    pub fn sample(&self, x: f32, y: f32) -> Vec3 {
        let fx = x.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let fy = y.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.data[x + y * self.width];
        at(x0, y0)
            .lerp(at(x1, y0), tx)
            .lerp(at(x0, y1).lerp(at(x1, y1), tx), ty)
    }

    /// RGBA half floats, for upload.
    pub fn to_rgba16f(&self) -> Vec<u16> {
        self.data
            .iter()
            .flat_map(|c| [c.x, c.y, c.z, 1.0].map(f32_to_f16))
            .collect()
    }
}

pub fn rayleigh_phase(cos_theta: f32) -> f32 {
    3.0 / (16.0 * PI) * (1.0 + cos_theta * cos_theta)
}

/// Cornette-Shanks phase function.
pub fn mie_phase(g: f32, cos_theta: f32) -> f32 {
    let k = 3.0 / (8.0 * PI) * (1.0 - g * g) / (2.0 + g * g);
    k * (1.0 + cos_theta * cos_theta) / (1.0 + g * g - 2.0 * g * cos_theta).powf(1.5)
}

fn fibonacci_sphere(i: usize, n: usize) -> Vec3 {
    let golden = PI * (3.0 - 5f32.sqrt());
    let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
    let s = (1.0 - y * y).sqrt();
    let phi = golden * i as f32;
    Vec3::new(s * phi.cos(), y, s * phi.sin())
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// IEEE 754 half precision, round to nearest (no NaN handling needed for LUTs).
fn f32_to_f16(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let m = (mantissa | 0x80_0000) >> (1 - exponent);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exponent >= 31 {
        return sign | 0x7c00;
    }
    let half = ((exponent as u32) << 10) | (mantissa >> 13);
    sign | (half + ((mantissa >> 12) & 1)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        for x in [0.0f32, 1.0, 0.5, 2.0, 1e-3, 65504.0, 0.333] {
            let h = f32_to_f16(x);
            let e = ((h >> 10) & 0x1f) as i32;
            let m = (h & 0x3ff) as f32;
            let back = if e == 0 {
                m / 1024.0 * 2f32.powi(-14)
            } else {
                (1.0 + m / 1024.0) * 2f32.powi(e - 15)
            };
            assert!((back - x).abs() <= x * 1e-3 + 1e-7, "{x} -> {back}");
        }
    }

    #[test]
    fn sky_is_blue_and_sunsets_are_red() {
        let atmosphere = Atmosphere::earth_like(41_700.0);
        let luts = atmosphere.luts();
        let r = atmosphere.bottom_radius + 10.0;
        // Transmittance LUT round trip.
        for (x, y) in [(0.3, 0.2), (0.9, 0.7), (0.05, 0.5)] {
            let (rr, mu) = atmosphere.transmittance_r_mu(x, y);
            let (x2, y2) = atmosphere.transmittance_coords(rr, mu);
            assert!((x - x2).abs() < 1e-3 && (y - y2).abs() < 1e-3);
        }
        // Noon: blue sky, nearly white sun.
        let noon = atmosphere.sun_transmittance(&luts, r, 1.0);
        assert!(noon.min_element() > 0.6, "noon sun {noon}");
        let sky = atmosphere.sky_irradiance(&luts, r, 1.0);
        assert!(sky.z > sky.x * 1.5, "sky irradiance {sky} should be blue");
        // Sunset: the sun is reddened.
        let sunset = atmosphere.sun_transmittance(&luts, r, 0.02);
        assert!(sunset.x > sunset.z * 2.0, "sunset sun {sunset}");
        // Run with --nocapture to see the sun / sky curves when tuning.
        for mu in [1.0f32, 0.5, 0.2, 0.05, 0.0, -0.05, -0.1, -0.2] {
            println!(
                "mu_s {mu}: sun {} sky {}",
                atmosphere.sun_transmittance(&luts, r, mu),
                atmosphere.sky_irradiance(&luts, r, mu)
            );
        }
        // Night: no sun.
        assert_eq!(atmosphere.sun_transmittance(&luts, r, -0.3), Vec3::ZERO);
    }
    #[test]
    fn far_above_the_air_the_sun_is_white_unless_the_air_is_in_the_way() {
        // From the moon's distance, the sun off to the side of the planet: no air between.
        // (It used to be looked up as a grazing ray at the top of the air: a red sun.)
        let atmosphere = Atmosphere::earth_like(41_700.0);
        let luts = atmosphere.luts();
        let r = 880_000.0;
        for mu in [1.0f32, 0.3, 0.0, -0.5, -0.99] {
            let sun = atmosphere.sun_transmittance(&luts, r, mu);
            assert!(sun.min_element() > 0.99, "mu {mu}: {sun}");
        }
        // Just past the planet's limb, through the air: reddened, as at sunset.
        let top = atmosphere.top_radius;
        let bottom = atmosphere.bottom_radius;
        let grazing = (bottom + 0.3 * (top - bottom)) / r;
        let mu = -(1.0 - grazing * grazing).sqrt();
        let sun = atmosphere.sun_transmittance(&luts, r, mu);
        assert!(sun.x > sun.z * 1.5, "through the air {sun}");
        // Low orbit, looking along the horizon: the same as from the top of the air.
        let (a, b) = (
            atmosphere.sun_transmittance(&luts, top + 2_000.0, 0.5),
            atmosphere.sun_transmittance(&luts, top, 0.5),
        );
        assert!((a - b).length() < 0.05, "{a} {b}");
    }
}
