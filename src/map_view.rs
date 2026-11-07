//! The map view's overlay and the maneuver planner.
//!
//! The map itself is the real world seen from far away (the planet's LOD terrain and
//! atmosphere, the moon's mesh); this draws on top of it:
//! - the predicted trajectory (patched conics), cyan for the current path and amber after
//!   a planned maneuver, dimmed where a body hides it,
//! - apoapsis / periapsis markers with altitudes, sphere-of-influence encounters and exits,
//!   impacts, the craft, the bodies' names and the moon's orbit,
//! - maneuver nodes. Clicking the path adds one there; the panel on the right edits the
//!   selected node (prograde, normal, radial, time: hold the buttons or scroll over a row)
//!   and shows where the burn leads.
//!
//! Moon-relative parts of the path are drawn around where the moon is when the craft enters
//! its sphere of influence (so they join the path leading in).

use glam::{DVec3, Mat4};

use crate::{
    flight_hud::{
        AMBER, DIM, FAINT, MANEUVER, Marker, Pointer, TEXT, button, clock, distance, marker,
    },
    space::{
        bodies::{Bodies, Body},
        trajectory::{End, ManeuverNode, Segment, sample},
    },
    ui::Ui,
};

const PATH: [u8; 4] = [120, 200, 240, 230];
const PLANNED: [u8; 4] = [240, 170, 80, 230];

/// Projects world positions (absolute, planet body frame) onto the screen.
pub struct View {
    pub view_proj: Mat4,
    pub camera: DVec3,
    pub size: (f32, f32),
}

impl View {
    pub fn project(&self, world: DVec3) -> Option<[f32; 2]> {
        let rel = (world - self.camera).as_vec3();
        let clip = self.view_proj * rel.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        if x.abs() > 4.0 || y.abs() > 4.0 {
            return None;
        }
        Some([(x * 0.5 + 0.5) * self.size.0, (y * 0.5 + 0.5) * self.size.1])
    }
}

pub struct MapContext<'a> {
    pub bodies: &'a Bodies,
    /// The body whose frame the world is drawn in.
    pub active: Body,
    pub time: f64,
    /// Where the active body's centre is drawn (absolute).
    pub planet_center: DVec3,
    pub view: View,
    pub segments: &'a [Segment],
    /// The craft's world position.
    pub craft: DVec3,
    /// Whether maneuvers can be planned (in a rocket).
    pub editable: bool,
    /// The engine's acceleration (m/s^2), for burn times.
    pub accel: f64,
}

impl MapContext<'_> {
    /// A position relative to `body` (inertial axes) at the current time, in the world.
    fn world(&self, body: Body, rel: DVec3, t_ref: f64) -> DVec3 {
        let pci = match body {
            Body::Planet => rel,
            Body::Moon => self.bodies.offset(Body::Moon, t_ref).0 + rel,
            Body::Star => rel + self.bodies.offset(Body::Star, self.time).0,
        };
        // Drawn in the frame of the body the game is at.
        let active = self.bodies.offset(self.active, self.time).0;
        self.planet_center
            + self
                .bodies
                .frame(self.active, self.time)
                .to_body(pci - active)
    }

    /// The reference time a segment is drawn at (see the module docs).
    fn reference(&self, s: &Segment) -> f64 {
        if s.body == Body::Moon {
            s.t0
        } else {
            self.time
        }
    }

    /// Whether a body hides `world` from the camera.
    fn hidden(&self, world: DVec3) -> bool {
        let cam = self.view.camera;
        let to = world - cam;
        let dist = to.length();
        let dir = to / dist.max(1e-9);
        let spheres = [
            (
                self.world(Body::Planet, DVec3::ZERO, self.time),
                self.bodies.radius(Body::Planet),
            ),
            (
                self.world(Body::Moon, DVec3::ZERO, self.time),
                self.bodies.radius(Body::Moon),
            ),
        ];
        spheres.iter().any(|&(c, r)| {
            let oc = cam - c;
            let b = oc.dot(dir);
            let disc = b * b - (oc.length_squared() - r * r);
            disc > 0.0 && {
                let t = -b - disc.sqrt();
                t > 0.0 && t < dist - r * 0.002
            }
        })
    }
}

/// The planner's state: nodes for the boarded craft, which one is being edited, and the
/// buttons being held.
#[derive(Default)]
pub struct Planner {
    pub nodes: Vec<ManeuverNode>,
    pub selected: Option<usize>,
    /// (row, direction, seconds held).
    hold: Option<(usize, f64, f32)>,
    /// Time warp should stop at this time.
    pub warp_to: Option<f64>,
}

impl Planner {
    fn sort(&mut self) {
        let selected = self.selected.map(|i| self.nodes[i]);
        self.nodes.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.selected = selected.and_then(|n| self.nodes.iter().position(|m| *m == n));
    }

    pub fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take() {
            self.nodes.remove(i);
        }
    }
}

pub fn draw(ui: &mut Ui, ctx: &MapContext, planner: &mut Planner, pointer: &mut Pointer, dt: f32) {
    let s = ui.scale;
    let (_, ch) = ui.char_size();

    // The node panel first, so it takes the clicks over it.
    if ctx.editable {
        node_panel(ui, ctx, planner, pointer, dt);
    }

    // The moon's orbit, faintly.
    let mu = ctx.bodies.mu(Body::Planet);
    let period = ctx.bodies.moon.orbit.period(mu);
    let mut last: Option<[f32; 2]> = None;
    for k in 0..=160 {
        let t = ctx.time + period * k as f64 / 160.0;
        let w = ctx.world(Body::Planet, ctx.bodies.offset(Body::Moon, t).0, ctx.time);
        let p = (!ctx.hidden(w)).then(|| ctx.view.project(w)).flatten();
        if let (Some(a), Some(b)) = (last, p) {
            ui.line(a, b, s, [200, 200, 210, 45]);
        }
        last = p;
    }

    // Body names, and a ring for bodies too small to see from out here.
    for body in [Body::Planet, Body::Moon] {
        let w = ctx.world(body, DVec3::ZERO, ctx.time);
        if let Some(p) = ctx.view.project(w) {
            let distance = (w - ctx.view.camera).length();
            let pixels = ctx.bodies.radius(body) / distance * ctx.view.size.1 as f64 * 0.7;
            if pixels < 6.0 {
                ui.circle(p, 4.0 * s, s, [220, 220, 230, 160]);
            }
            let tw = ui.text_width(body.name());
            let below = (pixels as f32).max(6.0 * s);
            ui.text(
                p[0] - tw / 2.0,
                p[1] + below,
                body.name(),
                [255, 255, 255, 150],
            );
        }
    }

    // The trajectory; remember the closest point to the mouse.
    let mut hover: Option<(f32, [f32; 2], f64)> = None;
    for seg in ctx.segments {
        let color = if seg.nodes_before == 0 { PATH } else { PLANNED };
        let t_ref = ctx.reference(seg);
        let points: Vec<(Option<[f32; 2]>, bool, f64)> = sample(seg, 220)
            .into_iter()
            .map(|(rel, t)| {
                let w = ctx.world(seg.body, rel, t_ref);
                (ctx.view.project(w), ctx.hidden(w), t)
            })
            .collect();
        for pair in points.windows(2) {
            let ((Some(a), ha, _), (Some(b), hb, t)) = (pair[0], pair[1]) else {
                continue;
            };
            let alpha = if ha || hb { 60 } else { color[3] };
            ui.line(a, b, s.max(1.5), [color[0], color[1], color[2], alpha]);
            if !(ha || hb) && t > ctx.time + 1.0 {
                let d = ((pointer.pos.0 - b[0]).powi(2) + (pointer.pos.1 - b[1]).powi(2)).sqrt();
                if d < 10.0 * s && hover.is_none_or(|(best, ..)| d < best) {
                    hover = Some((d, b, t));
                }
            }
        }
        segment_markers(ui, ctx, seg);
        // Where the moon will be: its path is drawn around that spot.
        if seg.body == Body::Moon && seg.t0 > ctx.time + 1.0 {
            let w = ctx.world(Body::Moon, DVec3::ZERO, seg.t0);
            if let Some(p) = ctx.view.project(w) {
                ui.circle(p, 4.0 * s, s, [220, 220, 230, 90]);
            }
        }
    }

    // The craft.
    if let Some(p) = ctx.view.project(ctx.craft) {
        let c = [255, 255, 255, 230];
        ui.line(
            [p[0] - 5.0 * s, p[1] + 4.0 * s],
            [p[0], p[1] - 5.0 * s],
            s.max(1.5),
            c,
        );
        ui.line(
            [p[0], p[1] - 5.0 * s],
            [p[0] + 5.0 * s, p[1] + 4.0 * s],
            s.max(1.5),
            c,
        );
        ui.line(
            [p[0] + 5.0 * s, p[1] + 4.0 * s],
            [p[0] - 5.0 * s, p[1] + 4.0 * s],
            s.max(1.5),
            c,
        );
    }

    // Nodes: markers, click to select.
    let mut clicked_node = false;
    for seg in ctx.segments {
        if let End::Node(i) = seg.end {
            let (rel, _) = seg.state_at(seg.t1);
            let w = ctx.world(seg.body, rel, ctx.reference(seg));
            if let Some(p) = ctx.view.project(w) {
                let selected = planner.selected == Some(i);
                marker(
                    ui,
                    p,
                    if selected { 7.0 } else { 5.0 } * s,
                    Marker::Maneuver,
                );
                let near = ((pointer.pos.0 - p[0]).powi(2) + (pointer.pos.1 - p[1]).powi(2)).sqrt()
                    < 9.0 * s;
                if near && ctx.editable {
                    pointer.over_ui = true;
                    if pointer.pressed {
                        planner.selected = Some(i);
                        clicked_node = true;
                    }
                }
            }
        }
    }

    // Hovering the path: where a node would go; clicking adds one.
    if let Some((_, p, t)) = hover
        && !pointer.over_ui
        && ctx.editable
    {
        ui.circle(p, 4.0 * s, s, [255, 255, 255, 200]);
        let label = format!("+{}", clock(t - ctx.time));
        ui.text(p[0] + 8.0 * s, p[1] - ch - 2.0 * s, &label, DIM);
        if pointer.pressed && !clicked_node {
            planner.nodes.push(ManeuverNode {
                time: t,
                dv: DVec3::ZERO,
            });
            planner.selected = Some(planner.nodes.len() - 1);
            planner.sort();
        }
    }
}

/// Apoapsis / periapsis and how a segment ends.
fn segment_markers(ui: &mut Ui, ctx: &MapContext, seg: &Segment) {
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let c = &seg.conic;
    let radius = ctx.bodies.radius(seg.body);
    let t_ref = ctx.reference(seg);
    let span = seg.t1 - seg.t0;
    let reaches = |anomaly: f64| {
        seg.end == End::Orbit
            || c.time_to_anomaly(seg.r0, anomaly)
                .is_some_and(|dt| dt <= span)
    };
    let mut label = |anomaly: f64, text: &str| {
        let w = ctx.world(seg.body, c.position_at(anomaly), t_ref);
        if ctx.hidden(w) {
            return;
        }
        if let Some(p) = ctx.view.project(w) {
            let d = 3.0 * s;
            let color = [255, 255, 255, 220];
            ui.line([p[0] - d, p[1]], [p[0], p[1] - d], s, color);
            ui.line([p[0], p[1] - d], [p[0] + d, p[1]], s, color);
            ui.line([p[0] + d, p[1]], [p[0], p[1] + d], s, color);
            ui.line([p[0], p[1] + d], [p[0] - d, p[1]], s, color);
            ui.text(p[0] + 6.0 * s, p[1] - ch / 2.0, text, TEXT);
        }
    };
    if c.e > 1e-3 && reaches(0.0) {
        label(0.0, &format!("pe {}", distance(c.periapsis() - radius)));
    }
    if let Some(ap) = c.apoapsis()
        && c.e > 1e-3
        && ap < ctx.bodies.soi(seg.body)
        && reaches(std::f64::consts::PI)
    {
        label(
            std::f64::consts::PI,
            &format!("ap {}", distance(ap - radius)),
        );
    }
    let (end, _) = seg.state_at(seg.t1);
    let w = ctx.world(seg.body, end, t_ref);
    let Some(p) = ctx.view.project(w) else { return };
    let text = match seg.end {
        End::Enter(b) => format!("enter {}", b.name()),
        End::Exit => format!("leave {}", seg.body.name()),
        End::Impact => "impact".to_string(),
        _ => return,
    };
    let color = if seg.end == End::Impact {
        [240, 90, 70, 255]
    } else {
        [220, 220, 255, 230]
    };
    ui.circle(p, 3.0 * s, s, color);
    ui.text(p[0] + 6.0 * s, p[1] + 2.0 * s, &text, color);
}

/// The selected node's editor, on the right.
fn node_panel(
    ui: &mut Ui,
    ctx: &MapContext,
    planner: &mut Planner,
    pointer: &mut Pointer,
    dt: f32,
) {
    let Some(i) = planner.selected.filter(|&i| i < planner.nodes.len()) else {
        planner.selected = None;
        return;
    };
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let row_h = ch + 6.0 * s;
    let panel_w = 150.0 * s;
    let x0 = ui.width - panel_w - 10.0 * s;
    let mut y = 40.0 * s;
    let rows = 9.0;
    ui.rect(
        x0 - 6.0 * s,
        y - 6.0 * s,
        panel_w + 12.0 * s,
        row_h * rows + 12.0 * s,
        [8, 10, 16, 150],
    );
    if pointer.inside(
        x0 - 6.0 * s,
        y - 6.0 * s,
        panel_w + 12.0 * s,
        row_h * rows + 12.0 * s,
    ) {
        pointer.over_ui = true;
    }
    let title = format!("maneuver {} of {}", i + 1, planner.nodes.len());
    ui.text(x0, y, &title, MANEUVER);
    y += row_h;

    let node = planner.nodes[i];
    let period = ctx
        .segments
        .iter()
        .find(|seg| seg.end == End::Node(i))
        .and_then(|seg| seg.conic.period())
        .unwrap_or(3_600.0);
    // (label, value text, step per press, rate per second held)
    let rows_spec = [
        ("prograde", format!("{:+.1}", node.dv.x), 0.1, 1.0),
        ("normal", format!("{:+.1}", node.dv.y), 0.1, 1.0),
        ("radial", format!("{:+.1}", node.dv.z), 0.1, 1.0),
        (
            "time",
            format!("+{}", clock(node.time - ctx.time)),
            1.0,
            period / 400.0,
        ),
    ];
    let mut change = [0.0f64; 4];
    let mut still_held = false;
    for (row, (label, value, step, rate)) in rows_spec.iter().enumerate() {
        ui.text(x0, y + s, label, DIM);
        let vx = x0 + 58.0 * s;
        let (minus, mw) = button(ui, pointer, vx, y, "-", false, true);
        let tw = ui.text_width(value);
        let field = 52.0 * s;
        ui.text(vx + mw + (field - tw) / 2.0, y + s, value, TEXT);
        let px = vx + mw + field;
        let (plus, pw) = button(ui, pointer, px, y, "+", false, true);
        let over_minus = pointer.inside(vx, y, mw, row_h);
        let over_plus = pointer.inside(px, y, pw, row_h);
        if minus || plus {
            let sign = if plus { 1.0 } else { -1.0 };
            change[row] += sign * step;
            planner.hold = Some((row, sign, 0.0));
        }
        if let Some((held_row, sign, secs)) = planner.hold
            && held_row == row
            && pointer.held
            && (over_minus || over_plus)
        {
            let secs = secs + dt;
            planner.hold = Some((row, sign, secs));
            still_held = true;
            if secs > 0.25 {
                // Faster the longer it is held.
                let speed = rate * 4f64.powf((secs - 0.25) as f64).min(300.0);
                change[row] += sign * speed * dt as f64;
            }
        }
        // Scrolling over a row: one step per notch (ten for dv).
        if pointer.inside(x0, y, panel_w, row_h) && pointer.scroll != 0.0 {
            let notch = if row < 3 { 1.0 } else { rate * 4.0 };
            change[row] += pointer.scroll as f64 * notch;
        }
        y += row_h;
    }
    if !still_held {
        planner.hold = None;
    }
    if change != [0.0; 4] {
        let n = &mut planner.nodes[i];
        n.dv += DVec3::new(change[0], change[1], change[2]);
        n.time = (n.time + change[3]).max(ctx.time + 1.0);
        planner.sort();
    }

    // Burn summary and where it leads.
    let node = planner.nodes[planner.selected.unwrap_or(i)];
    let dv = node.dv.length();
    let burn = dv / ctx.accel.max(1e-6);
    ui.text(
        x0,
        y + s,
        &format!("dv {dv:.1} m/s   burn {burn:.0} s"),
        TEXT,
    );
    y += row_h;
    let after = ctx.segments.iter().find(|seg| seg.nodes_before == i + 1);
    let result = match after {
        Some(seg) => {
            let c = &seg.conic;
            let r = ctx.bodies.radius(seg.body);
            let encounter = ctx.segments.iter().find(|x| {
                x.nodes_before == i + 1 && x.body == Body::Moon && seg.body != Body::Moon
            });
            if let Some(m) = encounter {
                format!(
                    "moon pe {}",
                    distance(m.conic.periapsis() - ctx.bodies.radius(Body::Moon))
                )
            } else {
                match c.apoapsis() {
                    Some(ap) if ap < ctx.bodies.soi(seg.body) => format!(
                        "pe {}  ap {}",
                        distance(c.periapsis() - r),
                        distance(ap - r)
                    ),
                    _ => format!("pe {}  escape", distance(c.periapsis() - r)),
                }
            }
        }
        None => String::from("..."),
    };
    ui.text(x0, y + s, &result, AMBER);
    y += row_h;
    let (warp, ww) = button(ui, pointer, x0, y, "warp to", false, true);
    if warp {
        planner.warp_to = Some(node.time - burn / 2.0 - 15.0);
    }
    let (delete, dw) = button(ui, pointer, x0 + ww + 4.0 * s, y, "delete", false, true);
    if delete {
        planner.delete_selected();
    }
    let (close, _) = button(ui, pointer, x0 + ww + dw + 8.0 * s, y, "close", false, true);
    if close {
        planner.selected = None;
    }
    let _ = FAINT;
}
