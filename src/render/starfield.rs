//! The Milky Way, precomputed into an equirectangular map of the inertial sky.
//!
//! It's smooth, so a texture is far cheaper than evaluating noise per pixel every frame.
//! Individual stars stay procedural in `sky.frag` (they must stay pixel-sharp).

use std::f32::consts::{PI, TAU};

use glam::{DVec3, Vec3};

use super::atmosphere::Lut;
use crate::world::noise::Noise;

pub const GALAXY_SIZE: (usize, usize) = (1024, 512);

/// Galaxy radiance (relative to the sun's illuminance) per direction. x is the longitude
/// around +Y (atan2(z, x)), y the latitude; texel centres at 0 and 1, like the other LUTs.
pub fn galaxy_map() -> Lut {
    let clouds = Noise::new(0x6a1a_c7e5);
    let dust = Noise::new(0xd057_1a9e);
    let pole = Vec3::new(0.25, 0.85, 0.46).normalize();
    let core = pole.cross(Vec3::X).normalize();
    Lut::build(GALAXY_SIZE, |x, y| {
        let (lon, lat) = ((x - 0.5) * TAU, (y - 0.5) * PI);
        let d = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
        let p = d.as_dvec3();
        // Distance from the galactic plane, and closeness to the bright core.
        let b = d.dot(pole);
        let band = (-b * b / 0.018).exp();
        let bulge = d.dot(core).max(0.0).powi(6);
        let cloud = (clouds.fbm(p * 5.0, 4) as f32 * 0.9 + 0.5).clamp(0.0, 1.2);
        let lanes = smoothstep(0.05, 0.3, dust.fbm(p * 9.0 + DVec3::splat(7.0), 4) as f32)
            * (-b * b / 0.003).exp();
        let glow = band * (0.25 + cloud) * (0.6 + 2.5 * bulge) * (1.0 - 0.8 * lanes);
        let color = Vec3::new(0.75, 0.82, 1.0).lerp(Vec3::new(1.0, 0.85, 0.65), bulge);
        color * glow * 2e-4
    })
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
