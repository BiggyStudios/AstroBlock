//! Two-body motion for any conic: ellipses, parabolas and hyperbolas.
//!
//! `orbit::Orbit` describes closed orbits by their elements (the planet, the moon). Craft can
//! also be on escape trajectories, so they are propagated with **universal variables**
//! (Vallado / Curtis): one Kepler equation in the universal anomaly `chi` covers every
//! conic, solved with the Laguerre-Conway iteration, which converges from poor guesses.
//!
//! `Conic` is the trajectory's shape (for drawing it, and for apsides and the times to them).

use std::f64::consts::{PI, TAU};

use glam::DVec3;

/// Stumpff functions C(z) and S(z).
fn stumpff(z: f64) -> (f64, f64) {
    if z.abs() < 1e-3 {
        // Series: exact enough and no cancellation near zero.
        let c = 0.5 - z / 24.0 + z * z / 720.0 - z * z * z / 40_320.0;
        let s = 1.0 / 6.0 - z / 120.0 + z * z / 5040.0 - z * z * z / 362_880.0;
        (c, s)
    } else if z > 0.0 {
        let q = z.sqrt();
        ((1.0 - q.cos()) / z, (q - q.sin()) / (q * q * q))
    } else {
        let q = (-z).sqrt();
        ((q.cosh() - 1.0) / -z, (q.sinh() - q) / (q * q * q))
    }
}

/// Position and velocity `dt` seconds after (`r0`, `v0`) (negative `dt` goes back), around
/// a body with gravitational parameter `mu`.
pub fn propagate(mu: f64, r0: DVec3, v0: DVec3, dt: f64) -> (DVec3, DVec3) {
    if dt == 0.0 {
        return (r0, v0);
    }
    let r0n = r0.length();
    let sqrt_mu = mu.sqrt();
    let vr0 = r0.dot(v0) / r0n;
    // alpha = 1/a (> 0 ellipse, < 0 hyperbola).
    let alpha = 2.0 / r0n - v0.length_squared() / mu;
    let mut dt = dt;
    if alpha > 1e-12 {
        // Whole periods change nothing: keeps chi small for long jumps.
        let period = TAU / (sqrt_mu * alpha.powf(1.5));
        dt = (dt + period / 2.0).rem_euclid(period) - period / 2.0;
    }
    // Initial guess.
    let mut chi = if alpha > 1e-12 {
        sqrt_mu * dt * alpha
    } else if alpha < -1e-12 {
        let a = 1.0 / alpha;
        let num = -2.0 * mu * alpha * dt;
        let den = r0.dot(v0) + dt.signum() * (-mu * a).sqrt() * (1.0 - r0n * alpha);
        let guess = dt.signum() * (-a).sqrt() * (num / den).abs().max(1e-12).ln();
        if guess.is_finite() {
            guess
        } else {
            sqrt_mu * dt / r0n
        }
    } else {
        sqrt_mu * dt / r0n
    };
    let a_term = r0n * vr0 / sqrt_mu;
    let b_term = 1.0 - alpha * r0n;
    for _ in 0..200 {
        let z = alpha * chi * chi;
        let (c, s) = stumpff(z);
        let f = a_term * chi * chi * c + b_term * chi * chi * chi * s + r0n * chi - sqrt_mu * dt;
        let df = a_term * chi * (1.0 - z * s) + b_term * chi * chi * c + r0n;
        let ddf = a_term * (1.0 - z * c) + b_term * chi * (1.0 - z * s);
        // Laguerre-Conway (n = 5).
        let n = 5.0;
        let disc = ((n - 1.0) * (n - 1.0) * df * df - n * (n - 1.0) * f * ddf)
            .abs()
            .sqrt();
        let denom = df + df.signum() * disc;
        let step = if denom.abs() > 1e-300 {
            n * f / denom
        } else {
            f / df
        };
        chi -= step;
        if step.abs() <= 1e-12 * (1.0 + chi.abs()) {
            break;
        }
    }
    let z = alpha * chi * chi;
    let (c, s) = stumpff(z);
    let f = 1.0 - chi * chi / r0n * c;
    let g = dt - chi * chi * chi / sqrt_mu * s;
    let r = r0 * f + v0 * g;
    let rn = r.length();
    let fdot = sqrt_mu / (rn * r0n) * (alpha * chi * chi * chi * s - chi);
    let gdot = 1.0 - chi * chi / rn * c;
    (r, r0 * fdot + v0 * gdot)
}

/// The shape of a trajectory around one body.
#[derive(Clone, Copy, Debug)]
pub struct Conic {
    pub mu: f64,
    /// Eccentricity and semi-latus rectum (m).
    pub e: f64,
    pub p: f64,
    /// Unit vectors: towards periapsis, and 90 degrees ahead of it (in the direction of
    /// motion).
    pub periapsis_dir: DVec3,
    pub ahead: DVec3,
}

impl Conic {
    pub fn from_state(mu: f64, r: DVec3, v: DVec3) -> Conic {
        let h = r.cross(v);
        let normal = h
            .try_normalize()
            .unwrap_or_else(|| r.any_orthonormal_vector());
        let e_vec = v.cross(h) / mu - r.normalize_or_zero();
        let e = e_vec.length();
        let periapsis_dir = if e > 1e-9 {
            e_vec / e
        } else {
            r.normalize_or(DVec3::X)
        };
        Conic {
            mu,
            e,
            p: (h.length_squared() / mu).max(1e-6),
            periapsis_dir,
            ahead: normal.cross(periapsis_dir),
        }
    }

    pub fn closed(&self) -> bool {
        self.e < 1.0
    }

    /// Semi-major axis (negative for hyperbolas).
    pub fn semi_major_axis(&self) -> f64 {
        self.p / (1.0 - self.e * self.e)
    }

    pub fn period(&self) -> Option<f64> {
        self.closed()
            .then(|| TAU * (self.semi_major_axis().powi(3) / self.mu).sqrt())
    }

    pub fn periapsis(&self) -> f64 {
        self.p / (1.0 + self.e)
    }

    /// The apoapsis distance, for closed orbits.
    pub fn apoapsis(&self) -> Option<f64> {
        self.closed().then(|| self.p / (1.0 - self.e))
    }

    pub fn true_anomaly(&self, r: DVec3) -> f64 {
        r.dot(self.ahead).atan2(r.dot(self.periapsis_dir))
    }

    pub fn radius_at(&self, anomaly: f64) -> f64 {
        self.p / (1.0 + self.e * anomaly.cos())
    }

    pub fn position_at(&self, anomaly: f64) -> DVec3 {
        let (s, c) = anomaly.sin_cos();
        (self.periapsis_dir * c + self.ahead * s) * self.radius_at(anomaly)
    }

    /// Time from periapsis to true anomaly `anomaly` (negative before periapsis).
    pub fn time_since_periapsis(&self, anomaly: f64) -> f64 {
        let e = self.e;
        let anomaly = (anomaly + PI).rem_euclid(TAU) - PI;
        if (e - 1.0).abs() < 1e-6 {
            // Parabola: Barker's equation.
            let d = (anomaly / 2.0).tan();
            let q = self.p / 2.0;
            return (2.0 * q.powi(3) / self.mu).sqrt() * (d + d * d * d / 3.0);
        }
        let a = self.semi_major_axis().abs();
        let n = (self.mu / a.powi(3)).sqrt();
        if e < 1.0 {
            let ecc = 2.0 * (((1.0 - e) / (1.0 + e)).sqrt() * (anomaly / 2.0).tan()).atan();
            (ecc - e * ecc.sin()) / n
        } else {
            let h = 2.0 * (((e - 1.0) / (e + 1.0)).sqrt() * (anomaly / 2.0).tan()).atanh();
            (e * h.sinh() - h) / n
        }
    }

    /// Seconds from the state at `r` until the trajectory next reaches true anomaly `target`
    /// (None if a hyperbola never gets there again).
    pub fn time_to_anomaly(&self, r: DVec3, target: f64) -> Option<f64> {
        let now = self.time_since_periapsis(self.true_anomaly(r));
        let then = self.time_since_periapsis(target);
        let mut dt = then - now;
        match self.period() {
            Some(period) => Some(dt.rem_euclid(period)),
            None => {
                if dt < 0.0 {
                    return None;
                }
                dt = dt.max(0.0);
                Some(dt)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::orbit::Orbit;

    const MU: f64 = 1.89e10;

    #[test]
    fn matches_closed_orbits() {
        let o = Orbit {
            semi_major_axis: 90_000.0,
            eccentricity: 0.3,
            inclination: 0.4,
            longitude_of_ascending_node: 1.0,
            argument_of_periapsis: 2.0,
            mean_anomaly_at_epoch: 0.5,
        };
        let (r0, v0) = o.state_at(MU, 10.0);
        for dt in [1.0, 100.0, 3_000.0, -500.0, 1e6] {
            let (r, v) = propagate(MU, r0, v0, dt);
            let (re, ve) = o.state_at(MU, 10.0 + dt);
            assert!((r - re).length() < 1e-3, "dt {dt}: {r} vs {re}");
            assert!((v - ve).length() < 1e-6, "dt {dt}");
        }
    }

    #[test]
    fn hyperbolas_conserve_energy_and_momentum() {
        let r0 = DVec3::new(50_000.0, 0.0, 0.0);
        let v0 = DVec3::new(100.0, 300.0, 1200.0); // well above escape speed
        let energy = |r: DVec3, v: DVec3| v.length_squared() / 2.0 - MU / r.length();
        let conic = Conic::from_state(MU, r0, v0);
        assert!(!conic.closed());
        for dt in [10.0, 600.0, 20_000.0, -300.0] {
            let (r, v) = propagate(MU, r0, v0, dt);
            assert!(
                (energy(r, v) / energy(r0, v0) - 1.0).abs() < 1e-9,
                "dt {dt}"
            );
            assert!((r.cross(v) - r0.cross(v0)).length() / r0.cross(v0).length() < 1e-9);
            // Velocity is the derivative of position.
            let (r2, _) = propagate(MU, r0, v0, dt + 0.01);
            assert!(((r2 - r) / 0.01 - v).length() < 1e-3 * v.length());
            // And the conic agrees on the timing.
            let t = conic.time_since_periapsis(conic.true_anomaly(r))
                - conic.time_since_periapsis(conic.true_anomaly(r0));
            assert!((t - dt).abs() < 1e-3 * dt.abs().max(1.0), "{t} vs {dt}");
        }
    }

    #[test]
    fn conic_geometry() {
        let r0 = DVec3::new(0.0, 0.0, 60_000.0);
        let v = (MU / 60_000.0).sqrt() * 1.1;
        let conic = Conic::from_state(MU, r0, DVec3::X * v);
        // Starting at periapsis, prograde.
        assert!((conic.periapsis() - 60_000.0).abs() < 1e-3);
        assert!(conic.true_anomaly(r0).abs() < 1e-9);
        let ap = conic.apoapsis().unwrap();
        let half = conic.period().unwrap() / 2.0;
        let (r, _) = propagate(MU, r0, DVec3::X * v, half);
        assert!((r.length() - ap).abs() < 1e-3 * ap);
        assert!((conic.time_to_anomaly(r0, PI).unwrap() - half).abs() < 1e-6 * half);
        assert!((conic.position_at(PI) - r).length() < 1e-3 * ap);
    }
}
