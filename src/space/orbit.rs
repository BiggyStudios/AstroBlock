//! Keplerian (two-body) orbits.
//!
//! Frame convention: an orbit is given relative to its parent body in the parent's inertial
//! frame, whose reference plane (the "ecliptic") is XZ with +Y as north. Prograde motion is
//! counter-clockwise seen from +Y, i.e. a right-handed rotation about +Y - the same sense
//! the planets spin in.

use std::f64::consts::{PI, TAU};

use glam::DVec3;

/// Classical orbital elements of an elliptic orbit (angles in radians).
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub semi_major_axis: f64,
    pub eccentricity: f64,
    pub inclination: f64,
    pub longitude_of_ascending_node: f64,
    pub argument_of_periapsis: f64,
    /// Mean anomaly at time 0.
    pub mean_anomaly_at_epoch: f64,
}

impl Orbit {
    /// Radians per second, for a parent with gravitational parameter `mu` (m^3/s^2).
    pub fn mean_motion(&self, mu: f64) -> f64 {
        (mu / self.semi_major_axis.powi(3)).sqrt()
    }

    pub fn period(&self, mu: f64) -> f64 {
        TAU / self.mean_motion(mu)
    }

    /// Position and velocity relative to the parent at time `t` (s).
    pub fn state_at(&self, mu: f64, t: f64) -> (DVec3, DVec3) {
        let (a, e) = (self.semi_major_axis, self.eccentricity);
        let n = self.mean_motion(mu);
        let mean = (self.mean_anomaly_at_epoch + n * t).rem_euclid(TAU);
        let ecc = eccentric_anomaly(mean, e);
        let (sin_e, cos_e) = ecc.sin_cos();
        let b = a * (1.0 - e * e).sqrt();
        let e_dot = n / (1.0 - e * cos_e);

        // Perifocal frame: x towards periapsis, y 90 degrees ahead in the orbit plane.
        let position = (a * (cos_e - e), b * sin_e);
        let velocity = (-a * sin_e * e_dot, b * cos_e * e_dot);
        (
            self.perifocal_to_parent(position),
            self.perifocal_to_parent(velocity),
        )
    }

    /// The orbit through position `r` with velocity `v` at time `t` (relative to the parent,
    /// in its inertial frame). `None` unless the orbit is elliptic.
    pub fn from_state(mu: f64, r: DVec3, v: DVec3, t: f64) -> Option<Orbit> {
        // Work in the textbook Z-north frame (inverse of the mapping in `perifocal_to_parent`).
        let to_std = |g: DVec3| DVec3::new(g.x, -g.z, g.y);
        let (r, v) = (to_std(r), to_std(v));
        let h = r.cross(v);
        let rl = r.length();
        let e_vec = v.cross(h) / mu - r / rl;
        let e = e_vec.length();
        let energy = v.length_squared() / 2.0 - mu / rl;
        if energy >= 0.0 || e >= 1.0 || h.length() < 1e-9 {
            return None;
        }
        let a = -mu / (2.0 * energy);
        let inclination = (h.z / h.length()).clamp(-1.0, 1.0).acos();
        let node = DVec3::Z.cross(h);
        let (lan, node_dir) = if node.length() > 1e-9 * h.length() {
            (node.y.atan2(node.x).rem_euclid(TAU), node.normalize())
        } else {
            (0.0, DVec3::X) // equatorial: measure from +X
        };
        // Periapsis direction (any direction for a circular orbit: use the node).
        let peri_dir = if e > 1e-10 { e_vec / e } else { node_dir };
        let mut arg_periapsis = node_dir.dot(peri_dir).clamp(-1.0, 1.0).acos();
        if node_dir.cross(peri_dir).dot(h) < 0.0 {
            arg_periapsis = TAU - arg_periapsis;
        }
        let mut true_anomaly = peri_dir.dot(r / rl).clamp(-1.0, 1.0).acos();
        if peri_dir.cross(r).dot(h) < 0.0 {
            true_anomaly = TAU - true_anomaly;
        }
        let ecc = 2.0 * (((1.0 - e) / (1.0 + e)).sqrt() * (true_anomaly / 2.0).tan()).atan();
        let mean = ecc - e * ecc.sin();
        let orbit = Orbit {
            semi_major_axis: a,
            eccentricity: e,
            inclination,
            longitude_of_ascending_node: lan,
            argument_of_periapsis: arg_periapsis,
            mean_anomaly_at_epoch: 0.0,
        };
        Some(Orbit {
            mean_anomaly_at_epoch: (mean - orbit.mean_motion(mu) * t).rem_euclid(TAU),
            ..orbit
        })
    }

    /// Periapsis and apoapsis distances from the parent's centre.
    pub fn apsides(&self) -> (f64, f64) {
        let (a, e) = (self.semi_major_axis, self.eccentricity);
        (a * (1.0 - e), a * (1.0 + e))
    }

    /// Rotates a perifocal (x, y) vector into the parent frame.
    fn perifocal_to_parent(&self, (x, y): (f64, f64)) -> DVec3 {
        let (sin_o, cos_o) = self.longitude_of_ascending_node.sin_cos();
        let (sin_w, cos_w) = self.argument_of_periapsis.sin_cos();
        let (sin_i, cos_i) = self.inclination.sin_cos();
        // Textbook rotation Rz(node) * Rx(inclination) * Rz(periapsis), with Z as north...
        let px = (cos_o * cos_w - sin_o * sin_w * cos_i) * x
            + (-cos_o * sin_w - sin_o * cos_w * cos_i) * y;
        let py = (sin_o * cos_w + cos_o * sin_w * cos_i) * x
            + (-sin_o * sin_w + cos_o * cos_w * cos_i) * y;
        let pz = sin_w * sin_i * x + cos_w * sin_i * y;
        // ...then mapped onto our Y-north frame (a proper rotation: x -> x, y -> -z, z -> y).
        DVec3::new(px, pz, -py)
    }
}

/// Solves Kepler's equation `M = E - e sin E` for the eccentric anomaly (elliptic orbits).
pub fn eccentric_anomaly(mean_anomaly: f64, e: f64) -> f64 {
    let m = mean_anomaly.rem_euclid(TAU);
    let mut ecc = if e < 0.8 { m } else { PI };
    for _ in 0..50 {
        let step = (ecc - e * ecc.sin() - m) / (1.0 - e * ecc.cos());
        ecc -= step;
        if step.abs() < 1e-15 {
            break;
        }
    }
    ecc
}

#[cfg(test)]
mod tests {
    use super::*;

    const MU_SUN: f64 = 1.327_124_400_18e20;

    fn orbit() -> Orbit {
        Orbit {
            semi_major_axis: 1.496e11,
            eccentricity: 0.2,
            inclination: 0.3,
            longitude_of_ascending_node: 1.1,
            argument_of_periapsis: 2.0,
            mean_anomaly_at_epoch: 0.4,
        }
    }

    #[test]
    fn kepler_equation_is_solved() {
        for e in [0.0, 0.0167, 0.5, 0.95] {
            for i in 0..32 {
                let m = i as f64 * TAU / 32.0;
                let ecc = eccentric_anomaly(m, e);
                assert!((ecc - e * ecc.sin() - m).abs() < 1e-12, "e {e}, M {m}");
            }
        }
    }

    #[test]
    fn orbit_is_periodic_and_conserves_energy() {
        let o = orbit();
        let period = o.period(MU_SUN);
        let (p0, v0) = o.state_at(MU_SUN, 1234.0);
        let (p1, v1) = o.state_at(MU_SUN, 1234.0 + period);
        assert!((p0 - p1).length() / p0.length() < 1e-9);
        assert!((v0 - v1).length() / v0.length() < 1e-9);
        for k in 0..20 {
            let (p, v) = o.state_at(MU_SUN, k as f64 * period / 20.0);
            // Vis-viva: v^2 = mu (2/r - 1/a).
            let expected = MU_SUN * (2.0 / p.length() - 1.0 / o.semi_major_axis);
            assert!((v.length_squared() - expected).abs() / expected < 1e-9);
            // Velocity is the derivative of position.
            let dt = 10.0;
            let (p2, _) = o.state_at(MU_SUN, k as f64 * period / 20.0 + dt);
            assert!(((p2 - p) / dt - v).length() / v.length() < 1e-5);
        }
        // Periapsis distance.
        let rp = (0..1000)
            .map(|k| o.state_at(MU_SUN, k as f64 * period / 1000.0).0.length())
            .fold(f64::MAX, f64::min);
        assert!((rp / (o.semi_major_axis * (1.0 - o.eccentricity)) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn state_vectors_round_trip() {
        let mu = 1.7e10;
        for (i, o) in [
            orbit(),
            Orbit {
                semi_major_axis: 50_000.0,
                eccentricity: 0.0,
                inclination: 0.0,
                ..orbit()
            },
            Orbit {
                semi_major_axis: 60_000.0,
                eccentricity: 0.3,
                inclination: 1.2,
                ..orbit()
            },
        ]
        .into_iter()
        .enumerate()
        {
            let o = Orbit {
                semi_major_axis: if i == 0 { 70_000.0 } else { o.semi_major_axis },
                ..o
            };
            let t = 123.0;
            let (r, v) = o.state_at(mu, t);
            let back = Orbit::from_state(mu, r, v, t).expect("elliptic");
            for dt in [0.0, 50.0, 400.0] {
                let (r1, v1) = o.state_at(mu, t + dt);
                let (r2, v2) = back.state_at(mu, t + dt);
                assert!((r1 - r2).length() < 1e-3, "orbit {i}: {r1} vs {r2}");
                assert!((v1 - v2).length() < 1e-6, "orbit {i}");
            }
        }
    }

    #[test]
    fn prograde_is_right_handed_about_y() {
        let o = Orbit {
            inclination: 0.0,
            longitude_of_ascending_node: 0.0,
            ..orbit()
        };
        let (p, v) = o.state_at(MU_SUN, 0.0);
        assert!(p.cross(v).normalize().dot(DVec3::Y) > 0.999);
    }
}
