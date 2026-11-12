//! The bodies a craft can fly around: the star, the home planet and its moon, with their
//! gravity, spheres of influence and where they are at any time.
//!
//! Flight uses **patched conics**, like KSP: a craft only feels the body whose sphere of
//! influence (SOI) it is in, and its state is kept relative to that body. Every body's
//! inertial frame has the same axes (the star's ecliptic frame, +Y north); only the origins
//! differ. "PCI" is the planet-centred inertial frame the rest of the game uses.
//!
//! The moon orbits in the planet's equatorial plane (so launches from the equator reach it
//! without a plane change), tidally locked: one face always turns towards the planet.

use glam::{DQuat, DVec3};

use super::{RotatingFrame, SolarSystem, gravity::PlanetGravity, orbit::Orbit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Body {
    Star,
    Planet,
    Moon,
}

impl Body {
    pub fn name(self) -> &'static str {
        match self {
            Body::Star => "sun",
            Body::Planet => "home",
            Body::Moon => "moon",
        }
    }

    pub fn parent(self) -> Option<Body> {
        match self {
            Body::Star => None,
            Body::Planet => Some(Body::Star),
            Body::Moon => Some(Body::Planet),
        }
    }

    pub fn children(self) -> &'static [Body] {
        match self {
            Body::Star => &[Body::Planet],
            Body::Planet => &[Body::Moon],
            Body::Moon => &[],
        }
    }
}

/// Surface gravity on the moon (m/s^2): like KSP's Mun.
pub const MOON_GRAVITY: f64 = 1.63;
/// The moon's orbit radius around the planet: 20 planet radii, like the Mun's.
pub const MOON_DISTANCE: f64 = 880_000.0;
/// A craft closer to the star than this many of its radii burns up (its sunlit side passes
/// the hull's limit, as in a reentry). The light grows as 1 / distance^2.
pub const SUN_BURN_RADII: f64 = 3.5;

pub struct Moon {
    pub radius: f64,
    pub mu: f64,
    /// Around the planet, in the planet's equatorial frame.
    pub orbit: Orbit,
}

pub struct Bodies {
    pub system: SolarSystem,
    /// The planet's gravity (layered inside) and atmosphere.
    pub gravity: PlanetGravity,
    pub moon: Moon,
}

impl Bodies {
    /// `moon_radius`: the moon's mean (datum) radius, from its block grid.
    pub fn new(system: SolarSystem, gravity: PlanetGravity, moon_radius: f64) -> Self {
        let radius = moon_radius;
        Self {
            system,
            gravity,
            moon: Moon {
                radius,
                mu: MOON_GRAVITY * radius * radius,
                orbit: Orbit {
                    semi_major_axis: MOON_DISTANCE,
                    eccentricity: 0.012,
                    inclination: 0.0,
                    longitude_of_ascending_node: 0.0,
                    argument_of_periapsis: 0.0,
                    mean_anomaly_at_epoch: 2.0,
                },
            },
        }
    }

    pub fn mu(&self, body: Body) -> f64 {
        match body {
            Body::Star => self.system.star.mu,
            Body::Planet => self.gravity.mu,
            Body::Moon => self.moon.mu,
        }
    }

    /// Mean surface radius (sea level for the planet).
    pub fn radius(&self, body: Body) -> f64 {
        match body {
            Body::Star => self.system.star.radius,
            Body::Planet => self.gravity.radius,
            Body::Moon => self.moon.radius,
        }
    }

    /// Radius of the sphere of influence (Laplace's a * (m / M)^(2/5)).
    pub fn soi(&self, body: Body) -> f64 {
        match body {
            Body::Star => f64::INFINITY,
            Body::Planet => {
                self.system.planet.orbit.semi_major_axis
                    * (self.gravity.mu / self.system.star.mu).powf(0.4)
            }
            Body::Moon => {
                self.moon.orbit.semi_major_axis * (self.moon.mu / self.gravity.mu).powf(0.4)
            }
        }
    }

    /// Top of the atmosphere (radius), if the body has one.
    pub fn atmosphere_top(&self, body: Body) -> Option<f64> {
        (body == Body::Planet).then(|| self.gravity.atmosphere_top())
    }

    /// Near the star its light alone burns a craft up (`SUN_BURN_RADII`); None elsewhere.
    pub fn hot_zone(&self, body: Body) -> Option<f64> {
        (body == Body::Star).then_some(self.system.star.radius * SUN_BURN_RADII)
    }

    /// How hot sunlight alone makes a craft at `distance` from the star (K): it reaches
    /// `max_temperature` at the edge of the hot zone (and, as T^4 goes with the light, as
    /// 1 / sqrt(distance)): about 220 K out here.
    pub fn sunlit_temperature(&self, distance: f64, max_temperature: f64) -> f64 {
        let edge = self.system.star.radius * SUN_BURN_RADII;
        max_temperature * (edge / distance.max(1.0)).sqrt()
    }

    /// Sunlight at `r` (relative to `body`, inertial axes): the way to the star (inertial,
    /// unit) and how strong it is, 1 at the home planet's distance (as 1 / distance^2) and 0
    /// where the planet or the moon is in the way.
    pub fn sunlight(&self, body: Body, r: DVec3, t: f64) -> (DVec3, f64) {
        let at = self.offset(body, t).0 + r;
        let to_star = self.offset(Body::Star, t).0 - at;
        let distance = to_star.length().max(1.0);
        let dir = to_star / distance;
        let home = self.state(Body::Planet, t).0.length();
        let mut strength = (home / distance).powi(2).min(20.0);
        for b in [Body::Planet, Body::Moon] {
            let c = self.offset(b, t).0 - at;
            let along = c.dot(dir);
            if along > 0.0 && (c - dir * along).length() < self.radius(b) {
                strength = 0.0;
            }
        }
        (dir, strength)
    }

    /// The way to the star from `body`'s centre, in its own rotating frame (unit).
    pub fn sun_in_frame(&self, body: Body, t: f64) -> DVec3 {
        let to_star = self.offset(Body::Star, t).0 - self.offset(body, t).0;
        (self.frame(body, t).rotation.inverse() * to_star).normalize_or(DVec3::Y)
    }

    /// Rails (time warp beyond physics) stop at this radius: the top of the air, the star's
    /// hot zone, or `clearance` above an airless surface.
    pub fn rails_floor(&self, body: Body, clearance: f64) -> f64 {
        self.atmosphere_top(body)
            .or(self.hot_zone(body))
            .unwrap_or(self.radius(body) + clearance)
    }

    /// The planet's equatorial frame -> inertial axes.
    pub fn equator(&self) -> DQuat {
        DQuat::from_rotation_x(self.system.planet.axial_tilt)
    }

    /// Position and velocity of `body` relative to its parent at time `t`.
    pub fn state(&self, body: Body, t: f64) -> (DVec3, DVec3) {
        match body {
            Body::Star => (DVec3::ZERO, DVec3::ZERO),
            Body::Planet => self.system.planet.orbit.state_at(self.system.star.mu, t),
            Body::Moon => {
                let (r, v) = self.moon.orbit.state_at(self.gravity.mu, t);
                let eq = self.equator();
                (eq * r, eq * v)
            }
        }
    }

    /// Position and velocity of `body` relative to the planet's centre (PCI).
    pub fn offset(&self, body: Body, t: f64) -> (DVec3, DVec3) {
        match body {
            Body::Planet => (DVec3::ZERO, DVec3::ZERO),
            Body::Moon => self.state(Body::Moon, t),
            Body::Star => {
                let (r, v) = self.state(Body::Planet, t);
                (-r, -v)
            }
        }
    }

    /// A state relative to `body` as a state relative to the planet (PCI).
    pub fn to_pci(&self, body: Body, r: DVec3, v: DVec3, t: f64) -> (DVec3, DVec3) {
        let (o, ov) = self.offset(body, t);
        (r + o, v + ov)
    }

    /// A position relative to `body` (inertial axes) in the frame the game draws in: the
    /// `active` body's rotating frame, relative to its centre.
    pub fn view(&self, active: Body, body: Body, r: DVec3, t: f64) -> DVec3 {
        let pci = self.offset(body, t).0 + r;
        self.frame(active, t)
            .to_body(pci - self.offset(active, t).0)
    }

    /// A state relative to `body` as a velocity over the `active` body's ground (its axes).
    pub fn view_velocity(&self, active: Body, body: Body, r: DVec3, v: DVec3, t: f64) -> DVec3 {
        let (o, ov) = self.offset(body, t);
        let (a, av) = self.offset(active, t);
        self.frame(active, t)
            .relative_velocity(o + r - a, ov + v - av)
    }

    /// Turns `body`'s rotating axes into the `active` body's.
    pub fn view_rotation(&self, active: Body, body: Body, t: f64) -> DQuat {
        self.frame(active, t).rotation.inverse() * self.frame(body, t).rotation
    }

    /// A state relative to `from` as one relative to `to` (both inertial axes).
    pub fn rebase(&self, from: Body, to: Body, r: DVec3, v: DVec3, t: f64) -> (DVec3, DVec3) {
        let (o, ov) = self.offset(from, t);
        let (a, av) = self.offset(to, t);
        (r + o - a, v + ov - av)
    }

    /// The body's own rotating frame at `t` (body-fixed axes -> inertial axes).
    pub fn frame(&self, body: Body, t: f64) -> RotatingFrame {
        match body {
            Body::Star => RotatingFrame {
                rotation: DQuat::IDENTITY,
                omega: DVec3::ZERO,
            },
            Body::Planet => self.system.planet_frame(t),
            Body::Moon => {
                // Tidally locked: its +X axis always points at the planet.
                let (r, v) = self.moon.orbit.state_at(self.gravity.mu, t);
                let to_planet = -r.normalize();
                let angle = (-to_planet.z).atan2(to_planet.x);
                let rate = r.cross(v).y / r.length_squared();
                RotatingFrame {
                    rotation: self.equator() * DQuat::from_rotation_y(angle),
                    omega: DVec3::Y * rate,
                }
            }
        }
    }

    /// Gravitational acceleration at `r` relative to `body` (inertial axes). The planet's is
    /// layered (weaker inside); the others are point masses.
    pub fn gravity(&self, body: Body, r: DVec3) -> DVec3 {
        match body {
            Body::Planet => self.gravity.acceleration(r),
            _ => {
                let d = r.length().max(1.0);
                -r * (self.mu(body) / (d * d * d))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies() -> Bodies {
        let gravity = PlanetGravity::new(43_928.0, 1_200.0, 9_000.0);
        Bodies::new(SolarSystem::new(), gravity, 11_000.0)
    }

    #[test]
    fn moon_is_kerbal_sized() {
        let b = bodies();
        let soi = b.soi(Body::Moon);
        assert!((100_000.0..200_000.0).contains(&soi), "moon SOI {soi}");
        let planet_soi = b.soi(Body::Planet);
        assert!(planet_soi > 10.0 * MOON_DISTANCE, "planet SOI {planet_soi}");
        // Orbits in the equatorial plane: perpendicular to the planet's spin axis.
        let axis = b.frame(Body::Planet, 0.0).to_inertial(DVec3::Y);
        for t in [0.0, 5_000.0, 20_000.0] {
            let (r, v) = b.state(Body::Moon, t);
            assert!(r.normalize().dot(axis).abs() < 1e-9);
            assert!(r.cross(v).normalize().dot(axis) > 0.999, "prograde");
        }
    }

    #[test]
    fn moon_is_tidally_locked() {
        let b = bodies();
        for t in [0.0, 1_234.0, 30_000.0] {
            let (r, _) = b.state(Body::Moon, t);
            let face = b.frame(Body::Moon, t).to_inertial(DVec3::X);
            assert!(face.dot(-r.normalize()) > 0.9999, "t {t}");
            // The frame's spin matches how fast that face turns.
            let f0 = b.frame(Body::Moon, t);
            let f1 = b.frame(Body::Moon, t + 1.0);
            let p = DVec3::new(0.0, 0.0, 11_000.0);
            let v = f1.to_inertial(p) - f0.to_inertial(p);
            assert!((v - f0.inertial_velocity(p, DVec3::ZERO)).length() < 1e-3);
        }
    }
}
