//! The flight HUD (inside a rocket), in the game's minimal flat style:
//! - an attitude ball at the bottom centre: the sphere of directions around the craft seen
//!   from behind its nose (sky above the horizon, ground below, pitch rings, compass
//!   letters), with markers for prograde / retrograde, normal / antinormal, radial out / in
//!   and the maneuver burn, and the nose fixed in the middle,
//! - throttle (left) and hull temperature (right) as thin vertical lines,
//! - the SAS switch and its modes as small text buttons above the ball,
//! - telemetry as plain text, top left, and the next maneuver, top centre.
//!
//! Also the small pieces of UI the map view shares: `Pointer` and `button`.

use glam::DVec3;

use crate::{
    flight::vessel::{MAX_TEMPERATURE, SasMode},
    space::bodies::Body,
    ui::Ui,
};

pub const TEXT: [u8; 4] = [235, 240, 245, 235];
pub const DIM: [u8; 4] = [190, 200, 210, 170];
pub const FAINT: [u8; 4] = [255, 255, 255, 50];
pub const AMBER: [u8; 4] = [240, 196, 120, 240];
pub const DANGER: [u8; 4] = [240, 90, 70, 255];
pub const PROGRADE: [u8; 4] = [226, 222, 110, 255];
pub const NORMAL: [u8; 4] = [214, 120, 226, 255];
pub const RADIAL: [u8; 4] = [110, 210, 232, 255];
pub const MANEUVER: [u8; 4] = [110, 160, 255, 255];

/// The mouse, for clickable UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    pub pos: (f32, f32),
    /// Left button went down this frame / is down.
    pub pressed: bool,
    pub held: bool,
    pub scroll: f32,
    /// Set by widgets under the cursor (so clicks there don't reach the map).
    pub over_ui: bool,
}

impl Pointer {
    pub fn inside(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let (px, py) = self.pos;
        px >= x && px < x + w && py >= y && py < y + h
    }
}

/// A text button at (x, y): lowercase label, a faint backing when hovered, amber with an
/// underline when `active`. Returns (clicked, width).
pub fn button(
    ui: &mut Ui,
    pointer: &mut Pointer,
    x: f32,
    y: f32,
    label: &str,
    active: bool,
    enabled: bool,
) -> (bool, f32) {
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let w = ui.text_width(label) + 4.0 * s;
    let h = ch + 3.0 * s;
    let hover = enabled && pointer.inside(x, y, w, h);
    if hover {
        pointer.over_ui = true;
        ui.rect(x, y, w, h, [255, 255, 255, 28]);
    }
    let color = if !enabled {
        [150, 160, 170, 90]
    } else if active {
        AMBER
    } else if hover {
        [255, 255, 255, 255]
    } else {
        DIM
    };
    ui.text(x + 2.0 * s, y + s, label, color);
    if active {
        ui.rect(x + 2.0 * s, y + h - s, w - 4.0 * s, s, AMBER);
    }
    (hover && pointer.pressed, w)
}

/// Directions for the attitude ball, all in one (inertial) frame.
pub struct Attitude {
    /// The craft's right, nose and dorsal axes.
    pub right: DVec3,
    pub nose: DVec3,
    pub dorsal: DVec3,
    /// Radial up, north and east at the craft.
    pub up: DVec3,
    pub north: DVec3,
    pub east: DVec3,
    /// Prograde, normal and radial out (orbital or surface, see `surface`).
    pub prograde: Option<DVec3>,
    pub normal: Option<DVec3>,
    pub radial: Option<DVec3>,
    pub maneuver: Option<DVec3>,
}

pub struct FlightInfo {
    pub body: Body,
    /// Above the mean surface / sea level (m), and vertical speed (m/s).
    pub altitude: f64,
    pub vertical_speed: f64,
    /// Speed shown on the ball (surface or orbital, see `surface`).
    pub speed: f64,
    pub surface: bool,
    /// Apoapsis / periapsis altitudes and the time to them (s).
    pub apoapsis: Option<(f64, Option<f64>)>,
    pub periapsis: (f64, Option<f64>),
    pub throttle: f64,
    /// How full the tank is (0..1), and the velocity change that is worth (m/s).
    pub fuel: f64,
    pub delta_v: f64,
    pub temperature: f64,
    /// Dynamic pressure (Pa).
    pub pressure: f64,
    pub sas: bool,
    pub sas_mode: SasMode,
    pub landed: bool,
    pub attitude: Attitude,
    /// The next maneuver: seconds until its burn should start, remaining dv, burn time.
    pub node: Option<(f64, f64, f64)>,
    pub warp: f64,
    pub time_of_day: f64,
    pub map: bool,
}

pub enum FlightAction {
    ToggleSas,
    Sas(SasMode),
    ToggleMap,
}

pub fn draw(ui: &mut Ui, info: &FlightInfo, pointer: &mut Pointer, time: f32) -> Vec<FlightAction> {
    let mut actions = Vec::new();
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let (w, h) = (ui.width, ui.height);

    // Telemetry, top left.
    let mut lines = vec![format!(
        "{}{}   {:02}:{:02}   x{}",
        info.body.name(),
        if info.landed { " / landed" } else { "" },
        info.time_of_day as u32,
        (info.time_of_day.fract() * 60.0) as u32,
        info.warp
    )];
    lines.push(format!(
        "alt {}   vs {:+.1} m/s   q {:.1} kpa",
        distance(info.altitude),
        info.vertical_speed,
        info.pressure / 1000.0
    ));
    let mut apsides = String::new();
    if let Some((ap, t)) = info.apoapsis {
        apsides += &format!("ap {}{}   ", distance(ap), until(t));
    } else {
        apsides += "escape   ";
    }
    let (pe, t) = info.periapsis;
    apsides += &format!("pe {}{}", distance(pe), until(t));
    lines.push(apsides);
    let mut y = 6.0 * s;
    for (i, line) in lines.iter().enumerate() {
        ui.text(8.0 * s, y, line, if i == 0 { TEXT } else { DIM });
        y += ch + 2.0 * s;
    }

    // The next maneuver, top centre.
    if let Some((start_in, dv, burn)) = info.node {
        let text = if start_in > 0.0 {
            format!(
                "burn in {}   dv {:.1} m/s   {:.0} s",
                clock(start_in),
                dv,
                burn
            )
        } else {
            format!("burn now   dv {:.1} m/s   {:.0} s", dv, burn)
        };
        let color = if start_in <= 0.0 && start_in > -burn - 5.0 {
            let pulse = 0.6 + 0.4 * (time * 6.0).sin();
            [110, 160, 255, (255.0 * pulse) as u8]
        } else {
            MANEUVER
        };
        let tw = ui.text_width(&text);
        ui.text((w - tw) / 2.0, 6.0 * s, &text, color);
    }

    // The attitude ball.
    let radius = (52.0 * s).round();
    let (cx, cy) = ((w / 2.0).round(), h - radius - 10.0 * s);
    navball(ui, (cx, cy), radius, &info.attitude);

    // Speed above it.
    let label = format!(
        "{} {:.1} m/s",
        if info.surface { "surface" } else { "orbit" },
        info.speed
    );
    let tw = ui.text_width(&label);
    let sas_y = cy - radius - 2.0 * ch - 9.0 * s;
    ui.text(cx - tw / 2.0, cy - radius - ch - 4.0 * s, &label, TEXT);

    // SAS buttons above that.
    let labels: Vec<&str> = std::iter::once("sas")
        .chain(SasMode::ALL.iter().map(|m| m.label()))
        .collect();
    let total: f32 = labels
        .iter()
        .map(|l| ui.text_width(l) + 4.0 * s)
        .sum::<f32>()
        + (labels.len() - 1) as f32 * 2.0 * s;
    let mut x = (cx - total / 2.0).round();
    let (clicked, bw) = button(ui, pointer, x, sas_y, "sas", info.sas, true);
    if clicked {
        actions.push(FlightAction::ToggleSas);
    }
    x += bw + 2.0 * s;
    for mode in SasMode::ALL {
        let enabled = info.sas && (mode != SasMode::Maneuver || info.attitude.maneuver.is_some());
        let active = info.sas && info.sas_mode == mode;
        let (clicked, bw) = button(ui, pointer, x, sas_y, mode.label(), active, enabled);
        if clicked && enabled {
            actions.push(FlightAction::Sas(mode));
        }
        x += bw + 2.0 * s;
    }

    // Throttle, left of the ball; temperature, right.
    let gauge_h = radius * 1.6;
    let gy = cy - gauge_h / 2.0;
    let gx = cx - radius - 14.0 * s;
    ui.rect(gx, gy, s, gauge_h, FAINT);
    let filled = (gauge_h * info.throttle as f32).round();
    ui.rect(gx - s, gy + gauge_h - filled, 3.0 * s, filled, AMBER);
    let text = format!("{:.0}", info.throttle * 100.0);
    ui.text(
        gx - ui.text_width(&text) - 4.0 * s,
        cy - ch / 2.0,
        &text,
        DIM,
    );
    // Fuel, further left, with what is left of it in m/s.
    let fx = gx - 38.0 * s;
    ui.rect(fx, gy, s, gauge_h, FAINT);
    let filled = (gauge_h * info.fuel as f32).round();
    let low = info.fuel < 0.15;
    let color = if low {
        let pulse = 0.6 + 0.4 * (time * 9.0).sin();
        [240, 90, 70, (255.0 * pulse) as u8]
    } else {
        TEXT
    };
    ui.rect(fx - s, gy + gauge_h - filled, 3.0 * s, filled, color);
    let text = format!("{:.0} m/s", info.delta_v);
    ui.text(
        fx - ui.text_width(&text) - 4.0 * s,
        cy - ch / 2.0,
        &text,
        if low { DANGER } else { DIM },
    );
    let label = "fuel";
    ui.text(fx - ui.text_width(label) / 2.0, gy - ch - s, label, DIM);
    let heat = ((info.temperature - 290.0) / (MAX_TEMPERATURE - 290.0)).clamp(0.0, 1.0) as f32;
    let hx = cx + radius + 14.0 * s;
    ui.rect(hx, gy, s, gauge_h, FAINT);
    let filled = (gauge_h * heat).round();
    let hot = heat > 0.7;
    let color = if hot {
        let pulse = 0.6 + 0.4 * (time * 9.0).sin();
        [240, 90, 70, (255.0 * pulse) as u8]
    } else if heat > 0.35 {
        [240, 160, 90, 240]
    } else {
        TEXT
    };
    ui.rect(hx - s, gy + gauge_h - filled, 3.0 * s, filled, color);
    let text = format!("{:.0} k", info.temperature);
    ui.text(
        hx + 5.0 * s,
        cy - ch / 2.0,
        &text,
        if hot { DANGER } else { DIM },
    );

    // Map toggle, and the keys, bottom left.
    let (clicked, _) = button(
        ui,
        pointer,
        8.0 * s,
        h - ch - 10.0 * s,
        "map",
        info.map,
        true,
    );
    if clicked {
        actions.push(FlightAction::ToggleMap);
    }
    ui.text(
        8.0 * s,
        y + 2.0 * s,
        "wasd qe steer   shift/ctrl throttle   z/x full/cut   space wheel brake   t sas   m map   f get out",
        [190, 200, 210, 110],
    );
    actions
}

/// The attitude ball at `center`, `radius` pixels.
fn navball(ui: &mut Ui, center: (f32, f32), radius: f32, a: &Attitude) {
    let (cx, cy) = center;
    // A point on the disc (unit coordinates, y up) seen as a direction around the craft.
    let direction = |x: f32, y: f32| {
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        a.right * x as f64 + a.dorsal * y as f64 + a.nose * z as f64
    };
    let shade = |x: f32, y: f32| {
        let d = direction(x, y);
        let e = d.dot(a.up) as f32;
        let t = ((e + 0.03) / 0.06).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        let sky = [96.0, 132.0, 170.0];
        let ground = [140.0, 98.0, 66.0];
        // Darker towards the rim, like a lit sphere.
        let rim = 0.62 + 0.38 * (1.0 - x * x - y * y).max(0.0).sqrt();
        let c: [f32; 3] = std::array::from_fn(|i| (ground[i] + (sky[i] - ground[i]) * t) * rim);
        [c[0] as u8, c[1] as u8, c[2] as u8, 235]
    };
    let rings = 12;
    let segments = 40;
    let at = |ring: usize, seg: usize| {
        let rho = ring as f32 / rings as f32;
        let th = seg as f32 / segments as f32 * std::f32::consts::TAU;
        (rho * th.cos(), rho * th.sin())
    };
    for ring in 0..rings {
        for seg in 0..segments {
            let corners = [
                (ring, seg),
                (ring + 1, seg),
                (ring + 1, seg + 1),
                (ring, seg + 1),
            ];
            let pts = corners.map(|(r, sg)| at(r, sg));
            let points = pts.map(|(x, y)| [cx + x * radius, cy - y * radius]);
            ui.poly(points, pts.map(|(x, y)| shade(x * 0.999, y * 0.999)));
        }
    }
    ui.circle([cx, cy], radius, ui.scale, [255, 255, 255, 70]);

    // Where a direction shows on the ball (front half only).
    let project = |d: DVec3| -> Option<[f32; 2]> {
        let z = d.dot(a.nose);
        (z > 0.0).then(|| {
            [
                cx + d.dot(a.right) as f32 * radius,
                cy - d.dot(a.dorsal) as f32 * radius,
            ]
        })
    };
    // Horizon and pitch rings every 30 degrees.
    for (elevation, width, alpha) in [
        (0.0f64, 1.5, 200u8),
        (30.0, 1.0, 70),
        (60.0, 1.0, 70),
        (-30.0, 1.0, 70),
        (-60.0, 1.0, 70),
    ] {
        let el = elevation.to_radians();
        let point = |k: usize| {
            let th = k as f64 / 96.0 * std::f64::consts::TAU;
            (a.east * th.cos() + a.north * th.sin()) * el.cos() + a.up * el.sin()
        };
        for k in 0..96 {
            if let (Some(p), Some(q)) = (project(point(k)), project(point(k + 1))) {
                ui.line(p, q, width * ui.scale, [255, 255, 255, alpha]);
            }
        }
    }
    // Compass letters on the horizon.
    let (_, ch) = ui.char_size();
    for (label, d) in [
        ("n", a.north),
        ("e", a.east),
        ("s", -a.north),
        ("w", -a.east),
    ] {
        if let Some(p) = project(d) {
            let tw = ui.text_width(label);
            ui.text(
                p[0] - tw / 2.0,
                p[1] - ch - ui.scale,
                label,
                [255, 255, 255, 200],
            );
        }
    }

    // Markers.
    let s = ui.scale;
    let r = 5.0 * s;
    let markers = [
        (a.prograde, Marker::Prograde),
        (a.prograde.map(|d| -d), Marker::Retrograde),
        (a.normal, Marker::Normal),
        (a.normal.map(|d| -d), Marker::Antinormal),
        (a.radial, Marker::RadialOut),
        (a.radial.map(|d| -d), Marker::RadialIn),
        (a.maneuver, Marker::Maneuver),
    ];
    for (dir, kind) in markers {
        if let Some(p) = dir.and_then(project) {
            marker(ui, p, r, kind);
        }
    }
    // The nose, fixed in the middle: a small wing shape.
    let c = AMBER;
    let w = 1.5 * s;
    ui.line([cx - 9.0 * s, cy], [cx - 3.0 * s, cy], w, c);
    ui.line([cx - 3.0 * s, cy], [cx, cy + 3.0 * s], w, c);
    ui.line([cx, cy + 3.0 * s], [cx + 3.0 * s, cy], w, c);
    ui.line([cx + 3.0 * s, cy], [cx + 9.0 * s, cy], w, c);
}

#[derive(Clone, Copy)]
pub enum Marker {
    Prograde,
    Retrograde,
    Normal,
    Antinormal,
    RadialOut,
    RadialIn,
    Maneuver,
}

/// Draws a direction marker centred at `p` with radius `r`.
pub fn marker(ui: &mut Ui, p: [f32; 2], r: f32, kind: Marker) {
    let w = (ui.scale * 1.2).max(1.5);
    let [x, y] = p;
    match kind {
        Marker::Prograde | Marker::Retrograde => {
            let c = PROGRADE;
            ui.circle(p, r, w, c);
            if matches!(kind, Marker::Prograde) {
                ui.line([x, y - r], [x, y - r * 1.8], w, c);
                ui.line([x - r, y], [x - r * 1.8, y], w, c);
                ui.line([x + r, y], [x + r * 1.8, y], w, c);
                ui.rect(x - w / 2.0, y - w / 2.0, w, w, c);
            } else {
                let k = r * 0.7;
                ui.line([x - k, y - k], [x + k, y + k], w, c);
                ui.line([x - k, y + k], [x + k, y - k], w, c);
            }
        }
        Marker::Normal | Marker::Antinormal => {
            let c = NORMAL;
            let sign = if matches!(kind, Marker::Normal) {
                -1.0
            } else {
                1.0
            };
            let a = [x, y + sign * r];
            let b = [x - r * 0.9, y - sign * r * 0.6];
            let d = [x + r * 0.9, y - sign * r * 0.6];
            ui.line(a, b, w, c);
            ui.line(b, d, w, c);
            ui.line(d, a, w, c);
            if matches!(kind, Marker::Antinormal) {
                ui.rect(x - w / 2.0, y - w / 2.0, w, w, c);
            }
        }
        Marker::RadialOut | Marker::RadialIn => {
            let c = RADIAL;
            ui.circle(p, r, w, c);
            let k = r * 0.7;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                if matches!(kind, Marker::RadialOut) {
                    ui.line(
                        [x + dx * k, y + dy * k],
                        [x + dx * k * 1.8, y + dy * k * 1.8],
                        w,
                        c,
                    );
                } else {
                    ui.line(
                        [x + dx * k, y + dy * k],
                        [x + dx * k * 0.35, y + dy * k * 0.35],
                        w,
                        c,
                    );
                }
            }
        }
        Marker::Maneuver => {
            let c = MANEUVER;
            ui.circle(p, r * 1.1, w, c);
            for (dx, dy) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
                ui.line(
                    [x + dx * r * 1.1, y + dy * r * 1.1],
                    [x + dx * r * 0.5, y + dy * r * 0.5],
                    w,
                    c,
                );
            }
        }
    }
}

/// Meters as "850 m" or "12.3 km".
pub fn distance(m: f64) -> String {
    if m.abs() >= 1e7 {
        // Whole kilometres with thousands separators: "880,000 km".
        let km = (m.abs() / 1000.0).round() as u64;
        let digits = km.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("{}{out} km", if m < 0.0 { "-" } else { "" })
    } else if m.abs() >= 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}

/// Seconds as "45s", "12:34" or "3h 12m" (or days).
pub fn clock(t: f64) -> String {
    let t = t.max(0.0);
    if t < 60.0 {
        format!("{t:.0}s")
    } else if t < 3600.0 {
        format!("{}:{:02}", (t / 60.0) as u64, (t % 60.0) as u64)
    } else if t < 86_400.0 {
        format!(
            "{}h {:02}m",
            (t / 3600.0) as u64,
            ((t % 3600.0) / 60.0) as u64
        )
    } else {
        format!(
            "{}d {}h",
            (t / 86_400.0) as u64,
            ((t % 86_400.0) / 3600.0) as u64
        )
    }
}

fn until(t: Option<f64>) -> String {
    t.map_or(String::new(), |t| format!(" in {}", clock(t)))
}
