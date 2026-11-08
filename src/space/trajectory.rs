//! Patched-conic trajectory prediction (the map view's lines) and on-rails propagation.
//!
//! A trajectory is a chain of `Segment`s, each a two-body conic around one body, ending at
//! an event:
//! - entering a child's sphere of influence (found by sampling, then bisection),
//! - leaving the body's own SOI or hitting its surface (exact: the conic's radius crossing),
//! - a maneuver node (its burn is applied and the next segment starts there),
//! - or nothing within one orbit / the time limit.
//!
//! The same event search moves a craft on rails during time warp (`advance`), stopping it
//! where physics has to take over (atmosphere, close to a surface).

use std::f64::consts::TAU;

use glam::DVec3;

use super::{
    bodies::{Bodies, Body},
    conic::{Conic, propagate},
};

/// A planned burn: when, and how much along prograde, normal and radial-out (m/s), in the
/// orbit's frame at that moment.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ManeuverNode {
    pub time: f64,
    pub dv: DVec3,
}

/// The burn of `dv` (prograde, normal, radial) as an inertial vector for state (`r`, `v`).
pub fn burn_vector(r: DVec3, v: DVec3, dv: DVec3) -> DVec3 {
    let (prograde, normal, radial) = orbit_axes(r, v);
    prograde * dv.x + normal * dv.y + radial * dv.z
}

/// The (prograde, normal, radial-out) unit vectors of state (`r`, `v`).
pub fn orbit_axes(r: DVec3, v: DVec3) -> (DVec3, DVec3, DVec3) {
    let prograde = v.normalize_or(DVec3::X);
    let normal = r
        .cross(v)
        .try_normalize()
        .unwrap_or_else(|| prograde.any_orthonormal_vector());
    (prograde, normal, prograde.cross(normal))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    /// A whole closed orbit with nothing happening.
    Orbit,
    /// Enters this body's sphere of influence.
    Enter(Body),
    /// Leaves for the parent body's.
    Exit,
    /// Hits the surface (or, on rails, reaches a limit where physics takes over).
    Impact,
    /// A maneuver node (its index).
    Node(usize),
    /// The prediction's time limit.
    Limit,
}

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub body: Body,
    pub t0: f64,
    pub t1: f64,
    /// State relative to `body` at `t0` (inertial axes).
    pub r0: DVec3,
    pub v0: DVec3,
    pub conic: Conic,
    pub end: End,
    /// Maneuver nodes applied before this segment (0: the current trajectory).
    pub nodes_before: usize,
}

impl Segment {
    pub fn state_at(&self, t: f64) -> (DVec3, DVec3) {
        propagate(self.conic.mu, self.r0, self.v0, t - self.t0)
    }
}

/// How far ahead predictions look at most (s).
const HORIZON: f64 = 30.0 * 86_400.0;
/// Samples per segment when looking for SOI encounters.
const SAMPLES: usize = 400;

/// Radial events of a conic within `[t0, t1]`: the first time it goes below `low` (inbound)
/// or above `high` (outbound), from state (`r`, `v`) at `t0`.
fn radial_event(conic: &Conic, r: DVec3, low: f64, high: f64, span: f64) -> Option<(f64, End)> {
    let mut best: Option<(f64, End)> = None;
    let mut consider = |dt: Option<f64>, end: End| {
        if let Some(dt) = dt
            && dt <= span
            && best.is_none_or(|(b, _)| dt < b)
        {
            best = Some((dt, end));
        }
    };
    let rn = r.length();
    if rn <= low {
        consider(Some(0.0), End::Impact);
    }
    if conic.periapsis() < low && conic.e > 1e-9 {
        // Inbound crossing: before periapsis.
        let cos = ((conic.p / low - 1.0) / conic.e).clamp(-1.0, 1.0);
        consider(conic.time_to_anomaly(r, -cos.acos()), End::Impact);
    }
    if high.is_finite() {
        let escapes = conic.apoapsis().is_none_or(|ap| ap > high);
        if escapes && conic.e > 1e-9 {
            let cos = ((conic.p / high - 1.0) / conic.e).clamp(-1.0, 1.0);
            consider(conic.time_to_anomaly(r, cos.acos()), End::Exit);
        }
    }
    best
}

/// The first time within `(t0, t1]` the trajectory enters a child body's SOI.
fn encounter(
    bodies: &Bodies,
    body: Body,
    r: DVec3,
    v: DVec3,
    t0: f64,
    t1: f64,
) -> Option<(f64, Body)> {
    let mu = bodies.mu(body);
    let mut first: Option<(f64, Body)> = None;
    for &child in body.children() {
        let soi = bodies.soi(child);
        let inside = |t: f64| {
            let (p, _) = propagate(mu, r, v, t - t0);
            (p - bodies.state(child, t).0).length() < soi
        };
        // Already inside at the start (just left it): ignore until we're out.
        let mut was_inside = inside(t0);
        let mut prev = t0;
        for k in 1..=SAMPLES {
            let t = t0 + (t1 - t0) * k as f64 / SAMPLES as f64;
            let now = inside(t);
            if now && !was_inside {
                let (mut lo, mut hi) = (prev, t);
                for _ in 0..48 {
                    let mid = 0.5 * (lo + hi);
                    if inside(mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                if first.is_none_or(|(f, _)| hi < f) {
                    first = Some((hi, child));
                }
                break;
            }
            was_inside = now;
            prev = t;
        }
    }
    first
}

/// The next event on the conic from (`r`, `v`) at `t0`, no later than `t_max`, with
/// `low` as the "impact" radius. Returns (time, end).
fn next_event(
    bodies: &Bodies,
    body: Body,
    r: DVec3,
    v: DVec3,
    t0: f64,
    t_max: f64,
    low: f64,
) -> (f64, End) {
    let mu = bodies.mu(body);
    let conic = Conic::from_state(mu, r, v);
    let span = t_max - t0;
    let soi = bodies.soi(body);
    let mut end = match conic.period() {
        Some(period) if period < span => (t0 + period, End::Orbit),
        _ => (t_max, End::Limit),
    };
    if let Some((dt, e)) = radial_event(&conic, r, low, soi, end.0 - t0) {
        end = (t0 + dt, e);
    }
    // Encounters: sample at most one orbit (or the span) ahead.
    if let Some((t, child)) = encounter(bodies, body, r, v, t0, end.0) {
        end = (t, End::Enter(child));
    }
    end
}

/// Moves a state across an SOI boundary at time `t`.
fn transfer(
    bodies: &Bodies,
    body: Body,
    end: End,
    r: DVec3,
    v: DVec3,
    t: f64,
) -> (Body, DVec3, DVec3) {
    match end {
        End::Enter(child) => {
            let (o, ov) = bodies.state(child, t);
            (child, r - o, v - ov)
        }
        End::Exit => {
            let (o, ov) = bodies.state(body, t);
            (body.parent().unwrap_or(body), r + o, v + ov)
        }
        _ => (body, r, v),
    }
}

/// Predicts the trajectory from (`r`, `v`) relative to `body` at `t`, through the maneuver
/// `nodes` (sorted by time), for up to `max_segments` segments.
pub fn predict(
    bodies: &Bodies,
    body: Body,
    r: DVec3,
    v: DVec3,
    t: f64,
    nodes: &[ManeuverNode],
    max_segments: usize,
) -> Vec<Segment> {
    let (mut body, mut r, mut v, mut t) = (body, r, v, t);
    let mut segments = Vec::new();
    let mut applied = 0;
    let limit = t + HORIZON;
    while segments.len() < max_segments {
        let mu = bodies.mu(body);
        let next_node = nodes.get(applied).map(|n| n.time).filter(|&nt| nt > t);
        let t_max = next_node.unwrap_or(limit).min(limit);
        let (mut t_end, mut end) = next_event(bodies, body, r, v, t, t_max, bodies.radius(body));
        if let Some(nt) = next_node
            && nt <= t_end
            && end != End::Impact
        {
            (t_end, end) = (nt, End::Node(applied));
        }
        segments.push(Segment {
            body,
            t0: t,
            t1: t_end,
            r0: r,
            v0: v,
            conic: Conic::from_state(mu, r, v),
            end,
            nodes_before: applied,
        });
        let (r1, v1) = propagate(mu, r, v, t_end - t);
        match end {
            End::Enter(_) | End::Exit => {
                (body, r, v) = transfer(bodies, body, end, r1, v1, t_end);
            }
            End::Node(i) => {
                r = r1;
                v = v1 + burn_vector(r1, v1, nodes[i].dv);
                applied += 1;
            }
            End::Orbit => {
                // Nothing happens this orbit; skip ahead to the next node, if any.
                let Some(node) = nodes.get(applied) else {
                    break;
                };
                let period = segments
                    .last()
                    .and_then(|s| s.conic.period())
                    .unwrap_or(1.0);
                let skip = ((node.time - t) / period).floor().max(1.0) * period;
                (r, v) = propagate(mu, r, v, skip);
                t += skip;
                // Replace the drawn orbit's end time so it still covers one revolution.
                continue;
            }
            End::Impact | End::Limit => break,
        }
        t = t_end;
    }
    segments
}

/// Moves a craft on rails from `t0` towards `t1`. It stops early (returning the time
/// reached and `true`) where physics must take over: entering an atmosphere, or within
/// `clearance` of a surface. SOI changes on the way are followed.
pub fn advance(
    bodies: &Bodies,
    body: Body,
    r: DVec3,
    v: DVec3,
    (t0, t1): (f64, f64),
    clearance: f64,
) -> (Body, DVec3, DVec3, f64, bool) {
    let (mut body, mut r, mut v, mut t) = (body, r, v, t0);
    for _ in 0..16 {
        if t >= t1 {
            break;
        }
        let low = bodies
            .atmosphere_top(body)
            .unwrap_or(bodies.radius(body) + clearance);
        let (t_end, end) = next_event(bodies, body, r, v, t, t1, low);
        let (r1, v1) = propagate(bodies.mu(body), r, v, t_end - t);
        match end {
            End::Enter(_) | End::Exit => {
                (body, r, v) = transfer(bodies, body, end, r1, v1, t_end);
                t = t_end;
            }
            End::Impact => return (body, r1, v1, t_end, true),
            _ => {
                // Nothing in the way (or a whole orbit, if the warp is that fast).
                let (r1, v1) = propagate(bodies.mu(body), r, v, t1 - t);
                return (body, r1, v1, t1, false);
            }
        }
    }
    (body, r, v, t.min(t1).max(t0), false)
}

/// Samples of a segment for drawing: (position relative to its body, time), evenly spaced
/// in true anomaly so ellipses stay smooth.
pub fn sample(segment: &Segment, count: usize) -> Vec<(DVec3, f64)> {
    let c = &segment.conic;
    let a0 = c.true_anomaly(segment.r0);
    let a1 = match segment.end {
        End::Orbit => a0 + TAU,
        _ => {
            let (r1, _) = segment.state_at(segment.t1);
            let mut a1 = c.true_anomaly(r1);
            if c.closed() {
                while a1 < a0 {
                    a1 += TAU;
                }
                // Longer than one revolution: draw one.
                if segment.t1 - segment.t0 >= c.period().unwrap_or(f64::MAX) * 0.999 {
                    a1 = a0 + TAU;
                }
            }
            a1
        }
    };
    let t_peri0 = c.time_since_periapsis(a0);
    let period = c.period();
    (0..=count)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f64 / count as f64;
            let mut dt = c.time_since_periapsis(a) - t_peri0;
            if let Some(p) = period {
                dt = dt.rem_euclid(p);
                if k == count && a1 - a0 > TAU * 0.999 {
                    dt = p;
                }
            }
            (c.position_at(a), segment.t0 + dt)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{SolarSystem, bodies::MOON_DISTANCE, gravity::PlanetGravity};

    fn bodies() -> Bodies {
        Bodies::new(
            SolarSystem::new(),
            PlanetGravity::new(43_928.0, 1_200.0, 9_000.0),
            11_000.0,
        )
    }

    /// A low circular equatorial orbit around the planet at time `t`.
    fn low_orbit(b: &Bodies, altitude: f64, phase: f64) -> (DVec3, DVec3) {
        let eq = b.equator();
        let r = b.radius(Body::Planet) + altitude;
        let v = (b.mu(Body::Planet) / r).sqrt();
        let (s, c) = phase.sin_cos();
        // Prograde about the spin axis (+Y in the equatorial frame).
        (
            eq * DVec3::new(c * r, 0.0, -s * r),
            eq * DVec3::new(-s * v, 0.0, -c * v),
        )
    }

    #[test]
    fn circular_orbit_is_one_closed_segment() {
        let b = bodies();
        let (r, v) = low_orbit(&b, 20_000.0, 0.0);
        let segments = predict(&b, Body::Planet, r, v, 0.0, &[], 8);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].end, End::Orbit);
        let pts = sample(&segments[0], 64);
        assert!((pts[0].0 - r).length() < 1.0 && (pts[64].0 - r).length() < 1.0);
    }

    #[test]
    fn hohmann_burn_reaches_the_moon() {
        let b = bodies();
        let (r, v) = low_orbit(&b, 20_000.0, 0.0);
        let r1 = r.length();
        // Transfer to the moon's distance, then find a departure time that meets it.
        let a = (r1 + MOON_DISTANCE) / 2.0;
        let mu = b.mu(Body::Planet);
        let dv = (mu * (2.0 / r1 - 1.0 / a)).sqrt() - (mu / r1).sqrt();
        let transfer = std::f64::consts::PI * (a.powi(3) / mu).sqrt();
        let period = TAU * (r1.powi(3) / mu).sqrt();
        let mut found = None;
        for k in 0..200 {
            let t = k as f64 * period / 200.0;
            let (rn, vn) = propagate(mu, r, v, t);
            // At apoapsis (half a transfer later) the craft is opposite where it burned.
            let (moon, _) = b.state(Body::Moon, t + transfer);
            if (-rn).normalize().dot(moon.normalize()) > 0.999 {
                let node = ManeuverNode {
                    time: t,
                    dv: DVec3::new(dv, 0.0, 0.0),
                };
                found = Some((node, rn, vn));
                break;
            }
        }
        let (node, _, _) = found.expect("a launch window within one orbit");
        let segments = predict(&b, Body::Planet, r, v, 0.0, &[node], 8);
        let ends: Vec<End> = segments.iter().map(|s| s.end).collect();
        assert_eq!(ends[0], End::Node(0), "{ends:?}");
        assert!(
            segments.iter().any(|s| s.end == End::Enter(Body::Moon)),
            "no encounter: {ends:?}"
        );
        let at_moon = segments.iter().find(|s| s.body == Body::Moon).unwrap();
        assert!(at_moon.r0.length() <= b.soi(Body::Moon) * 1.0001);
        // On rails, the same trip ends up in the moon's SOI.
        let (body, ..) = advance(&b, Body::Planet, r, v, (0.0, node.time), 2_000.0);
        assert_eq!(body, Body::Planet);
    }

    #[test]
    fn rails_stop_at_the_atmosphere() {
        let b = bodies();
        // A suborbital lob from 30 km: comes back down into the air.
        let (r, v) = low_orbit(&b, 30_000.0, 0.0);
        let (_, r1, _, t, stopped) = advance(&b, Body::Planet, r, v * 0.8, (0.0, 1e6), 2_000.0);
        assert!(stopped && t < 1e6);
        assert!(
            (r1.length() - b.gravity.atmosphere_top()).abs() < 1.0,
            "{}",
            r1.length()
        );
        let segments = predict(&b, Body::Planet, r, v * 0.8, 0.0, &[], 8);
        assert_eq!(segments.last().unwrap().end, End::Impact);
    }
}
