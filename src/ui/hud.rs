//! The in-game overlay, minimal and flat: a dot for a crosshair, telemetry as plain text,
//! vitals as a number and a thin line each, the hotbar as a quiet row of items with an
//! underline for the selection. No panels or frames: the world is the star.
//!
//! Everything moves a little (see `HudAnim`): the underline slides between slots, the
//! selected item lifts, items pop when picked up, vitals ease and leave a draining trail
//! when lost, the screen edges flash red when hurt, the reticle closes in on targets, and
//! text and the console fade and slide in and out.

use crate::{
    player::GameMode,
    survival::{MAX_FOOD, MAX_HEALTH, inventory::Slot},
    ui::{
        Ui,
        anim::{Pop, Vital, approach, ease_out, mix},
        console::Console,
    },
};

pub struct HudInfo<'a> {
    pub mode: GameMode,
    pub flying: bool,
    /// Local solar time (hours) and time warp factor.
    pub time_of_day: f64,
    pub warp: f64,
    /// Speed over the ground and in the inertial frame (m/s).
    pub surface_speed: f64,
    pub orbital_speed: f64,
    /// Above sea level (m).
    pub altitude: f64,
    /// Local gravity (m/s^2).
    pub gravity: f64,
    /// What the sky is doing here, in words.
    pub weather: Option<String>,
    /// Where others are in time, in a game with others.
    pub together: Option<String>,
    /// Periapsis and apoapsis altitudes (m), when worth showing.
    pub apsides: Option<(f64, f64)>,
    pub hotbar: &'a [Slot],
    pub selected: usize,
    /// Seconds since the selected slot changed (its name fades out).
    pub selected_age: f32,
    pub health: f32,
    pub food: f32,
    /// Armor points worn (0-20).
    pub armor: u32,
    /// Weapon recharge (0..1, 1 = ready).
    pub attack_charge: f32,
    /// Mining or eating progress (0..1).
    pub mining: f32,
    /// What the crosshair is on (a block or an animal).
    pub target: Option<String>,
    /// A transient message and its age in seconds (shown for `MESSAGE_TIME`).
    pub message: Option<(&'a str, f32)>,
    pub console: &'a Console,
    /// An inventory screen is open (no crosshair).
    pub screen_open: bool,
    /// In a rocket or the map view: their own HUD replaces the crosshair, telemetry,
    /// hotbar and vitals.
    pub flight: bool,
}

/// How long a message stays up (s).
pub const MESSAGE_TIME: f32 = 3.0;

/// The HUD's motion state, carried from frame to frame.
#[derive(Default)]
pub struct HudAnim {
    /// Real seconds since start (pulses, blinking).
    time: f32,
    /// The underline's slot position (fractional while sliding); `None` before the first frame.
    select: Option<f32>,
    /// Per hotbar slot: how far its icon is lifted (1 = selected) and its pickup pop.
    lift: Vec<f32>,
    pops: Vec<Pop>,
    health: Vital,
    food: Vital,
    /// Red flash at the screen edges after damage (0..1).
    hurt: f32,
    /// How far the reticle has closed in on a target (0..1).
    reticle: f32,
    /// The progress line's opacity and the value it shows (kept while it fades out).
    progress: (f32, f32),
    /// How far the console has slid down (0..1).
    console: f32,
}

const TEXT: [u8; 4] = [235, 240, 245, 235];
const DIM: [u8; 4] = [190, 200, 210, 170];
const FAINT: [u8; 4] = [255, 255, 255, 50];
const HEALTH: [u8; 4] = [235, 240, 245, 235];
const FOOD: [u8; 4] = [240, 196, 120, 235];
const ARMOR: [u8; 4] = [150, 200, 240, 220];
const DANGER: [u8; 4] = [240, 90, 70, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

pub fn draw(ui: &mut Ui, info: &HudInfo, anim: &mut HudAnim, dt: f32) {
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let (w, h) = (ui.width, ui.height);
    let playing = info.mode != GameMode::Spectator && !info.flight;
    anim.time += dt;

    // Damage: the screen edges flash red, fading out.
    let survival = info.mode == GameMode::Survival;
    let health = (info.health / MAX_HEALTH).clamp(0.0, 1.0);
    let food = (info.food / MAX_FOOD).clamp(0.0, 1.0);
    let lost = anim.health.update(health, dt);
    anim.food.update(food, dt);
    // Food goes down a point at a time: only its trail shows that, no alarm.
    anim.food.flash = 0.0;
    if lost > 0.0 && survival {
        anim.hurt = (anim.hurt + 0.45 + lost * 3.0).min(1.0);
    }
    anim.hurt = (anim.hurt - dt * 2.2).max(0.0);
    if anim.hurt > 0.0 {
        let a = (ease_out(anim.hurt) * 90.0) as u8;
        let (red, clear) = ([200, 30, 20, a], [200, 30, 20, 0]);
        let e = w.min(h) * 0.14;
        ui.gradient(0.0, 0.0, w, e, [red, red, clear, clear]);
        ui.gradient(0.0, h - e, w, e, [clear, clear, red, red]);
        ui.gradient(0.0, 0.0, e, h, [red, clear, clear, red]);
        ui.gradient(w - e, 0.0, e, h, [clear, red, red, clear]);
    }

    // Crosshair: a dot. On a target, four corner ticks close in around it; mining, eating
    // or recharging shows as a short line under it that fades in and out.
    let crosshair = !info.screen_open && !info.flight && (playing || info.target.is_some());
    anim.reticle = approach(
        anim.reticle,
        if info.target.is_some() { 1.0 } else { 0.0 },
        18.0,
        dt,
    );
    let progress = if info.mining > 0.0 {
        info.mining
    } else if info.attack_charge < 1.0 {
        info.attack_charge
    } else {
        0.0
    };
    if progress > 0.0 {
        anim.progress = (approach(anim.progress.0, 1.0, 25.0, dt), progress);
    } else {
        // Finished (rather than abandoned) shows full while it fades.
        if anim.progress.1 > 0.7 {
            anim.progress.1 = 1.0;
        }
        anim.progress.0 = approach(anim.progress.0, 0.0, 8.0, dt);
    }
    if crosshair {
        let (cx, cy) = ((w / 2.0).round(), (h / 2.0).round());
        let dot = s.max(2.0);
        ui.rect(
            cx - dot / 2.0,
            cy - dot / 2.0,
            dot,
            dot,
            [255, 255, 255, 220],
        );
        let r = ease_out(anim.reticle);
        if r > 0.01 {
            let gap = ((3.0 + 4.0 * (1.0 - r)) * s).round();
            let color = [255, 255, 255, (r * 170.0) as u8];
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                ui.rect(
                    cx + sx * gap - dot / 2.0,
                    cy + sy * gap - dot / 2.0,
                    dot,
                    dot,
                    color,
                );
            }
        }
        let (alpha, value) = anim.progress;
        if alpha > 0.01 {
            let start = ui.vertices.len();
            let len = 12.0 * s;
            let y = cy + 6.0 * s;
            ui.rect(cx - len / 2.0, y, len, s, FAINT);
            ui.rect(cx - len / 2.0, y, (len * value).round(), s, TEXT);
            ui.fade_from(start, alpha, 0.0, (1.0 - alpha) * 2.0 * s);
        }
    }

    // Telemetry, top left: plain text, fading away while the console is down.
    anim.console = approach(
        anim.console,
        if info.console.open { 1.0 } else { 0.0 },
        18.0,
        dt,
    );
    let mode = match info.mode {
        GameMode::Survival => "survival",
        GameMode::Creative => "creative",
        GameMode::Spectator => "spectator",
    };
    let mut lines = vec![format!(
        "{mode}{}   {:02}:{:02}   x{}",
        if info.flying { " / flying" } else { "" },
        info.time_of_day as u32,
        (info.time_of_day.fract() * 60.0) as u32,
        info.warp
    )];
    lines.push(format!(
        "alt {}   gnd {:.1} m/s   orb {:.0} m/s   g {:.2}",
        distance(info.altitude),
        info.surface_speed,
        info.orbital_speed,
        info.gravity
    ));
    if let Some((pe, ap)) = info.apsides {
        lines.push(format!("pe {}   ap {}", distance(pe), distance(ap)));
    }
    if let Some(weather) = &info.weather {
        lines.push(weather.clone());
    }
    if let Some(together) = &info.together {
        lines.push(together.clone());
    }
    if let Some(name) = &info.target {
        lines.push(name.to_lowercase());
    }
    if anim.console < 0.99 && !info.flight {
        let start = ui.vertices.len();
        let mut y = 6.0 * s;
        for (i, line) in lines.iter().enumerate() {
            ui.text(8.0 * s, y, line, if i == 0 { TEXT } else { DIM });
            y += ch + 2.0 * s;
        }
        ui.fade_from(start, 1.0 - anim.console, 0.0, 0.0);
    }

    // Hotbar, bottom centre: items in a row; the underline slides to the selected one, which
    // lifts a little while the others dim. Items pop when picked up.
    if playing {
        let pitch = 22.0 * s;
        let icon = 14.0 * s;
        let n = info.hotbar.len();
        let x0 = ((w - pitch * n as f32) / 2.0).round();
        let y0 = h - icon - 12.0 * s;
        anim.lift.resize(n, 0.0);
        if anim.pops.len() != n {
            anim.pops = info.hotbar.iter().map(Pop::new).collect();
        }
        let select = anim.select.get_or_insert(info.selected as f32);
        *select = approach(*select, info.selected as f32, 22.0, dt);
        let select = *select;

        for (i, stack) in info.hotbar.iter().enumerate() {
            let chosen = i == info.selected;
            let lift = &mut anim.lift[i];
            *lift = approach(*lift, if chosen { 1.0 } else { 0.0 }, 16.0, dt);
            let lift = *lift;
            let x = x0 + i as f32 * pitch + (pitch - icon) / 2.0;
            let y = y0 - (lift * 2.0 * s).round();
            let start = ui.vertices.len();
            let scale = anim.pops[i].observe(stack, dt);
            match stack {
                Some(stack) => {
                    let size = (icon * scale).round();
                    let off = ((size - icon) / 2.0).round();
                    ui.item(x - off, y - off, size, stack);
                }
                None => {
                    let dot = s.max(2.0);
                    ui.rect(x + icon / 2.0 - dot / 2.0, y + icon / 2.0, dot, dot, FAINT);
                }
            }
            ui.fade_from(start, 0.72 + 0.28 * lift, 0.0, 0.0);
        }
        let ux = (x0 + select * pitch + (pitch - icon) / 2.0).round();
        ui.rect(ux, y0 + icon + 4.0 * s, icon, s.max(2.0), TEXT);

        // Vitals above the hotbar: health on the left, food on the right, each a number
        // and a thin line. Armor is a faint second line under health, only when worn.
        let mut above = y0 - 8.0 * s;
        if survival {
            let bar = 64.0 * s;
            let vy = y0 - 10.0 * s - ch;
            let left = x0 + (pitch - icon) / 2.0;
            let right = x0 + pitch * n as f32 - (pitch - icon) / 2.0;
            let pulse = 0.5 + 0.5 * (anim.time * 7.0).sin();
            let health_trail = [240, 90, 70, 200];
            vital(
                ui,
                (left, vy),
                bar,
                &anim.health,
                HEALTH,
                health_trail,
                pulse,
                false,
            );
            let food_trail = [240, 196, 120, 110];
            vital(
                ui,
                (right, vy),
                bar,
                &anim.food,
                FOOD,
                food_trail,
                pulse,
                true,
            );
            if info.armor > 0 {
                let armor = info.armor as f32 / 20.0;
                let line_y = vy + ch / 2.0 + 3.0 * s;
                ui.rect(left + 18.0 * s, line_y, (bar * armor).round(), s, ARMOR);
            }
            above = vy - 4.0 * s;
        }

        // The selected item's name: rises and fades in on switching, fades out after.
        if let Some(stack) = info.hotbar.get(info.selected).copied().flatten() {
            let age = info.selected_age;
            let fade_in = ease_out(age / 0.15);
            let alpha = fade_in * (1.0 - (age - 1.5) / 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                let name = stack.item.name().to_lowercase();
                let tw = ui.text_width(&name);
                let start = ui.vertices.len();
                ui.text((w - tw) / 2.0, above - ch, &name, TEXT);
                ui.fade_from(start, alpha, 0.0, (1.0 - fade_in) * 4.0 * s);
            }
        }
    }

    // Transient message, centred in the lower part of the screen: rises in, fades out.
    if let Some((message, age)) = info.message {
        let fade_in = ease_out(age / 0.2);
        let alpha = fade_in * ((MESSAGE_TIME - age) / 0.5).clamp(0.0, 1.0);
        let tw = ui.text_width(message);
        let start = ui.vertices.len();
        // (Higher in flight, clear of the attitude ball and its buttons.)
        let my = if info.flight { h * 0.2 } else { h * 0.68 };
        ui.text((w - tw) / 2.0, my, message, [240, 214, 160, 255]);
        ui.fade_from(start, alpha, 0.0, (1.0 - fade_in) * 6.0 * s);
    }

    // Console, top half: slides down from the top edge.
    if anim.console > 0.01 {
        let start = ui.vertices.len();
        let rows = ((h * 0.45) / (ch + 2.0 * s)) as usize;
        let height = rows as f32 * (ch + 2.0 * s) + 8.0 * s;
        ui.rect(0.0, 0.0, w, height, [6, 8, 12, 200]);
        let shown = info.console.lines.len().min(rows.saturating_sub(1));
        let mut y = 4.0 * s;
        for line in &info.console.lines[info.console.lines.len() - shown..] {
            ui.text_plain(8.0 * s, y, line, DIM);
            y += ch + 2.0 * s;
        }
        let y = height - ch - 4.0 * s;
        ui.rect(0.0, y - 2.0 * s, w, s, FAINT);
        let width = ui.text_plain(8.0 * s, y, &format!("> {}", info.console.input), TEXT);
        // A softly blinking caret.
        let blink = 0.55 + 0.45 * (anim.time * 5.0).cos();
        let caret = [TEXT[0], TEXT[1], TEXT[2], (TEXT[3] as f32 * blink) as u8];
        ui.rect(8.0 * s + width, y, s.max(2.0), ch - 2.0 * s, caret);
        let slide = 1.0 - ease_out(anim.console);
        ui.fade_from(start, anim.console.min(1.0), 0.0, -slide * height);
    }
}

/// A vital: its value as a number (0-100) and a thin line `len` long, as `vital` shows it
/// (eased, with a trail after losses). `from_right` mirrors it (the number at `x`'s left,
/// the line growing leftwards). A loss flashes the number red and shakes it; a gain
/// brightens it; low values pulse red.
#[allow(clippy::too_many_arguments)]
fn vital(
    ui: &mut Ui,
    (x, y): (f32, f32),
    len: f32,
    vital: &Vital,
    color: [u8; 4],
    trail: [u8; 4],
    pulse: f32,
    from_right: bool,
) {
    let s = ui.scale;
    let (_, ch) = ui.char_size();
    let value = vital.shown;
    let mut color = if value <= 0.25 {
        mix(DANGER, [255, 160, 140, 255], pulse * 0.6)
    } else {
        color
    };
    color = mix(color, DANGER, vital.flash);
    color = mix(color, WHITE, vital.glow * 0.7);
    let shake = ((vital.flash * 40.0).sin() * vital.flash * 1.5 * s).round();
    let text = format!("{:.0}", value * 100.0);
    let tw = ui.text_width(&text);
    let line_y = y + ch / 2.0 - s;
    let (now, was) = ((len * value).round(), (len * vital.trail).round());
    if from_right {
        ui.text(x - tw + shake, y, &text, color);
        let end = x - 18.0 * s;
        ui.rect(end - len, line_y, len, s, FAINT);
        ui.rect(end - was, line_y, was - now, s, trail);
        ui.rect(end - now, line_y, now, s, color);
    } else {
        ui.text(x + shake, y, &text, color);
        let start = x + 18.0 * s;
        ui.rect(start, line_y, len, s, FAINT);
        ui.rect(start + now, line_y, was - now, s, trail);
        ui.rect(start, line_y, now, s, color);
    }
}

fn distance(m: f64) -> String {
    if m.abs() >= 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}
