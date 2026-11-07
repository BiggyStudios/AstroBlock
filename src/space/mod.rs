//! Celestial mechanics: Keplerian orbits, the star, and the home planet's orbit and spin.
//!
//! The terrain lives in the home planet's **body-fixed (rotating) frame**: world positions
//! (camera, chunks) are planet-fixed, exactly like a player standing on a spinning planet in
//! KSP's rotating reference frame near the surface. Everything in the inertial solar-system
//! frame (the star, the stars in the sky) is converted into that frame each frame - which is
//! what makes the sun rise and set, and the seasons change, as the planet spins and orbits.

pub mod bodies;
pub mod conic;
pub mod gravity;
pub mod orbit;
pub mod trajectory;

use std::f64::consts::TAU;

use glam::{DQuat, DVec3};

use orbit::Orbit;

/// The planet's rotating (body-fixed) frame at one instant, relative to the non-rotating
/// planet-centred inertial frame ("PCI", where orbits live). Both are centred on the planet.
#[derive(Clone, Copy, Debug)]
pub struct RotatingFrame {
    /// Body frame -> inertial frame.
    pub rotation: DQuat,
    /// Angular velocity of the body, in body coordinates.
    pub omega: DVec3,
}

impl RotatingFrame {
    pub fn to_body(self, p: DVec3) -> DVec3 {
        self.rotation.inverse() * p
    }

    pub fn to_inertial(self, p: DVec3) -> DVec3 {
        self.rotation * p
    }

    /// Velocity relative to the ground (body coordinates) of something at inertial
    /// position `p` with inertial velocity `v`.
    pub fn relative_velocity(&self, p: DVec3, v: DVec3) -> DVec3 {
        let body = self.to_body(p);
        self.rotation.inverse() * v - self.omega.cross(body)
    }

    /// Inertial velocity of something at body position `body` moving at `relative` (body
    /// coordinates) over the ground.
    pub fn inertial_velocity(&self, body: DVec3, relative: DVec3) -> DVec3 {
        self.rotation * (relative + self.omega.cross(body))
    }
}

#[derive(Clone)]
pub struct Star {
    /// Gravitational parameter G*M (m^3/s^2).
    pub mu: f64,
    pub radius: f64,
}

#[derive(Clone)]
pub struct Planet {
    /// Orbit around the star.
    pub orbit: Orbit,
    /// Rotation period relative to the stars (s).
    pub sidereal_day: f64,
    /// Angle between the spin axis and the orbit normal.
    pub axial_tilt: f64,
}

/// The star and the home planet.
#[derive(Clone)]
pub struct SolarSystem {
    pub star: Star,
    pub planet: Planet,
}

impl SolarSystem {
    /// A Sun-like star with an Earth-like orbit for the home planet. The day is short (a game
    /// day, like Minecraft's), the year is a real one: seasons come with time warp.
    pub fn new() -> Self {
        Self {
            star: Star {
                mu: 1.327_124_400_18e20,
                radius: 6.957e8,
            },
            planet: Planet {
                // Starts at the March equinox (the planet on +X, the sun on -X), at perihelion.
                orbit: Orbit {
                    semi_major_axis: 1.496e11,
                    eccentricity: 0.0167,
                    inclination: 0.0,
                    longitude_of_ascending_node: 0.0,
                    argument_of_periapsis: 0.0,
                    mean_anomaly_at_epoch: 0.0,
                },
                sidereal_day: 30.0 * 60.0,
                axial_tilt: 23.44f64.to_radians(),
            },
        }
    }

    /// Rotation from the planet's body-fixed frame (spin axis +Y) to the inertial frame.
    pub fn planet_rotation(&self, t: f64) -> DQuat {
        let spin = (TAU * t / self.planet.sidereal_day).rem_euclid(TAU);
        DQuat::from_rotation_x(self.planet.axial_tilt) * DQuat::from_rotation_y(spin)
    }

    /// The planet's rotating frame at time `t`.
    pub fn planet_frame(&self, t: f64) -> RotatingFrame {
        RotatingFrame {
            rotation: self.planet_rotation(t),
            omega: DVec3::Y * (TAU / self.planet.sidereal_day),
        }
    }

    /// The star's position relative to the planet's centre, in the planet's body frame.
    pub fn star_in_body_frame(&self, t: f64) -> DVec3 {
        let (planet, _) = self.planet.orbit.state_at(self.star.mu, t);
        self.planet_rotation(t).inverse() * -planet
    }

    /// Mean length of a solar day (noon to noon), s.
    pub fn solar_day(&self) -> f64 {
        let year = self.planet.orbit.period(self.star.mu);
        1.0 / (1.0 / self.planet.sidereal_day - 1.0 / year)
    }

    /// Local solar time (hours, 0..24; 12 = the sun is highest) at the body-frame direction
    /// `up` from the planet's centre.
    pub fn local_time(&self, t: f64, up: DVec3) -> f64 {
        let sun = self.star_in_body_frame(t);
        let flat = |v: DVec3| DVec3::new(v.x, 0.0, v.z);
        let (s, u) = (flat(sun), flat(up));
        // The sun moves clockwise about +Y in the body frame, so this angle grows with time.
        let hour_angle = s.cross(u).y.atan2(s.dot(u));
        (12.0 + hour_angle * 24.0 / TAU).rem_euclid(24.0)
    }

    /// The first time at or after `start` when the local solar time at `up` is `hours`.
    pub fn time_of_day(&self, start: f64, up: DVec3, hours: f64) -> f64 {
        let day = self.solar_day();
        let mut t = start + (hours - self.local_time(start, up)).rem_euclid(24.0) / 24.0 * day;
        for _ in 0..3 {
            let error = (hours - self.local_time(t, up) + 12.0).rem_euclid(24.0) - 12.0;
            t += error / 24.0 * day;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_night_follow_the_spin() {
        let system = SolarSystem::new();
        let up = DVec3::new(0.3, 0.2, -0.9).normalize();
        let noon = system.time_of_day(0.0, up, 12.0);
        let sun = system.star_in_body_frame(noon).normalize();
        assert!(sun.dot(up) > 0.5, "the sun should be high at noon");
        let midnight = system.time_of_day(noon, up, 0.0);
        assert!(system.star_in_body_frame(midnight).normalize().dot(up) < -0.5);
        // Local time runs forwards and wraps once per solar day.
        let t = noon + 10.0;
        assert!(system.local_time(t, up) > 12.0);
        let next_noon = system.time_of_day(noon + 60.0, up, 12.0);
        assert!((next_noon - noon - system.solar_day()).abs() < 1.0);
        // Distance to the star is about 1 AU.
        let d = system.star_in_body_frame(noon).length();
        assert!((d / 1.496e11 - 1.0).abs() < 0.02);
    }

    #[test]
    fn seasons_follow_the_axial_tilt() {
        let system = SolarSystem::new();
        let year = system.planet.orbit.period(system.star.mu);
        // Sun declination = elevation above the equator (body frame XZ plane).
        let declination = |t: f64| system.star_in_body_frame(t).normalize().y.asin();
        assert!(declination(0.0).abs() < 0.01, "epoch is an equinox");
        let max = (0..400)
            .map(|k| declination(k as f64 * year / 400.0).abs())
            .fold(0.0, f64::max);
        assert!((max - system.planet.axial_tilt).abs() < 0.01);
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn rotating_frame_matches_finite_differences() {
        let system = SolarSystem::new();
        let t = 1234.5;
        let (f0, f1) = (system.planet_frame(t), system.planet_frame(t + 1e-3));
        // A point fixed on the ground moves through inertial space at omega x r.
        let ground = DVec3::new(30_000.0, 20_000.0, -20_000.0);
        let v = (f1.to_inertial(ground) - f0.to_inertial(ground)) / 1e-3;
        assert!((v - f0.inertial_velocity(ground, DVec3::ZERO)).length() < 1e-3);
        assert!(f0.relative_velocity(f0.to_inertial(ground), v).length() < 1e-3);
    }
}
