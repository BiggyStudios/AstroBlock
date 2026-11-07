//! The home planet's gravity, inside and out, and its atmosphere's density (for drag).
//!
//! The planet is a layered sphere (crust, mantle, outer and inner core, densities like
//! Earth's), scaled so that surface gravity is `SURFACE_GRAVITY`. Outside it pulls like a
//! point mass (`mu / r^2`); inside, by the shell theorem, only the mass below you counts -
//! so gravity weakens towards the centre and vanishes there.

use glam::DVec3;

/// m/s^2 at sea level (Kerbin-like: the planet is small, but heavy enough to feel like Earth).
pub const SURFACE_GRAVITY: f64 = 9.81;

/// Sea-level air density (kg/m^3).
const SEA_LEVEL_DENSITY: f64 = 1.225;

pub struct PlanetGravity {
    /// Gravitational parameter G*M (m^3/s^2).
    pub mu: f64,
    pub radius: f64,
    /// (outer radius, mu of everything inside that radius, density relative units) per
    /// layer, innermost first.
    layers: Vec<(f64, f64, f64)>,
    /// Atmosphere: density scale height and top (m above sea level).
    scale_height: f64,
    atmosphere_top: f64,
}

impl PlanetGravity {
    pub fn new(radius: f64, scale_height: f64, atmosphere_top: f64) -> Self {
        // Layer boundaries match `TerrainGenerator::deep_block`: inner core to 0.2 R, outer
        // core to 0.55 R, mantle up to 3 km below sea level, then crust. Densities in g/cm^3.
        let bounds = [
            (0.2 * radius, 13.0),
            (0.55 * radius, 10.0),
            (radius - 3_000.0, 4.5),
            (radius, 2.7),
        ];
        let mu = SURFACE_GRAVITY * radius * radius;
        // Enclosed mass (relative units) at each boundary, then scale to the total.
        let mut enclosed = 0.0;
        let mut inner = 0.0f64;
        let mut layers = Vec::new();
        for (outer, density) in bounds {
            enclosed += density * (outer.powi(3) - inner.powi(3));
            layers.push((outer, enclosed, density));
            inner = outer;
        }
        let scale = mu / enclosed;
        let layers = layers
            .into_iter()
            .map(|(r, m, d)| (r, m * scale, d * scale))
            .collect();
        Self {
            mu,
            radius,
            layers,
            scale_height,
            atmosphere_top,
        }
    }

    /// G * (mass inside radius r).
    pub fn enclosed_mu(&self, r: f64) -> f64 {
        let mut below = 0.0;
        let mut inner = 0.0f64;
        for &(outer, mu, density) in &self.layers {
            if r < outer {
                return below + density * (r.powi(3) - inner.powi(3));
            }
            below = mu;
            inner = outer;
        }
        self.mu
    }

    /// Gravitational acceleration at `p` (relative to the planet's centre).
    pub fn acceleration(&self, p: DVec3) -> DVec3 {
        let r = p.length();
        if r < 1e-6 {
            return DVec3::ZERO;
        }
        -p * (self.enclosed_mu(r) / (r * r * r))
    }

    /// Air density (kg/m^3) at radius `r`. Caves and tunnels below sea level have
    /// sea-level air.
    pub fn air_density(&self, r: f64) -> f64 {
        let h = r - self.radius;
        if h >= self.atmosphere_top {
            0.0
        } else {
            SEA_LEVEL_DENSITY * (-h.max(0.0) / self.scale_height).exp()
        }
    }

    pub fn atmosphere_top(&self) -> f64 {
        self.radius + self.atmosphere_top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gravity_inside_and_out() {
        let g = PlanetGravity::new(41_700.0, 1_200.0, 9_000.0);
        let surface = g.acceleration(DVec3::new(41_700.0, 0.0, 0.0)).length();
        assert!((surface - SURFACE_GRAVITY).abs() < 1e-9);
        // Inverse square outside.
        let far = g.acceleration(DVec3::new(0.0, 83_400.0, 0.0)).length();
        assert!((far - SURFACE_GRAVITY / 4.0).abs() < 1e-9);
        // Nothing at the centre, continuous at layer boundaries, stronger near the dense core
        // than a uniform planet would be.
        assert_eq!(g.acceleration(DVec3::ZERO), DVec3::ZERO);
        for &(r, _, _) in &g.layers {
            let a = g.acceleration(DVec3::new(r - 1e-3, 0.0, 0.0)).length();
            let b = g.acceleration(DVec3::new(r + 1e-3, 0.0, 0.0)).length();
            assert!((a - b).abs() < 1e-4);
        }
        let mid = g
            .acceleration(DVec3::new(0.0, 0.0, 0.5 * 41_700.0))
            .length();
        assert!(mid > 0.5 * SURFACE_GRAVITY, "{mid}");
    }
}
