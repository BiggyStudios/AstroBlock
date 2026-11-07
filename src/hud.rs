//! The in-game overlay: crosshair, flight data, hotbar, health, messages and the console.

use crate::{
    console::Console, inventory::Slot, player::GameMode, survival::MAX_HEALTH, ui::Ui,
    world::textures::Tex,
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
    /// Periapsis and apoapsis altitudes (m), when worth showing.
    pub apsides: Option<(f64, f64)>,
    pub hotbar: &'a [Slot],
    pub selected: usize,
    /// Seconds since the selected slot changed (its name fades out).
    pub selected_age: f32,
    pub health: f32,
    /// Hunger bar (0-20) and armor points (0-20).
    pub food: f32,
    pub armor: u32,
    /// Weapon recharge (0..1, 1 = ready).
    pub attack_charge: f32,
    /// Survival mining progress (0..1).
    pub mining: f32,
    /// What the crosshair is on (a block or an animal).
    pub target: Option<String>,
    pub message: Option<&'a str>,
    pub console: &'a Console,
    /// An inventory screen is open (no crosshair).
    pub screen_open: bool,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const DIM: [u8; 4] = [200, 210, 220, 230];
const PANEL: [u8; 4] = [0, 0, 0, 110];

pub fn draw(ui: &mut Ui, info: &HudInfo) {
    let s = ui.scale;
    let (cw, ch) = ui.char_size();
    let (w, h) = (ui.width, ui.height);
    let playing = info.mode != GameMode::Spectator;

    // Crosshair, the attack recharge indicator and the mining bar.
    if !info.screen_open && (playing || info.target.is_some()) {
        let (cx, cy) = ((w / 2.0).round(), (h / 2.0).round());
        let (len, thick) = (5.0 * s, s.max(2.0));
        let color = [255, 255, 255, 200];
        ui.rect(cx - len, cy - thick / 2.0, len * 2.0, thick, color);
        ui.rect(cx - thick / 2.0, cy - len, thick, len * 2.0, color);
        let bar = 16.0 * s;
        if info.mining > 0.0 {
            ui.rect(cx - bar / 2.0, cy + 10.0 * s, bar, 2.0 * s, PANEL);
            ui.rect(
                cx - bar / 2.0,
                cy + 10.0 * s,
                bar * info.mining,
                2.0 * s,
                WHITE,
            );
        } else if info.attack_charge < 1.0 {
            ui.rect(cx - bar / 2.0, cy + 10.0 * s, bar, 2.0 * s, PANEL);
            ui.rect(
                cx - bar / 2.0,
                cy + 10.0 * s,
                bar * info.attack_charge,
                2.0 * s,
                [220, 220, 220, 220],
            );
        }
    }

    // Flight data, top left.
    let mut lines = vec![
        format!(
            "{}{}   {:02}:{:02}   x{}",
            capitalize(info.mode.name()),
            if info.flying { " (flying)" } else { "" },
            info.time_of_day as u32,
            (info.time_of_day.fract() * 60.0) as u32,
            info.warp
        ),
        format!(
            "Alt {}   Surface {:.1} m/s   Orbital {:.0} m/s   g {:.2} m/s2",
            distance(info.altitude),
            info.surface_speed,
            info.orbital_speed,
            info.gravity
        ),
    ];
    if let Some((pe, ap)) = info.apsides {
        lines.push(format!("Pe {}   Ap {}", distance(pe), distance(ap)));
    }
    if let Some(name) = &info.target {
        lines.push(format!("Looking at: {name}"));
    }
    let panel_w = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f32 * cw + 8.0 * s;
    let mut y = 4.0 * s;
    if !info.console.open {
        ui.rect(
            2.0 * s,
            2.0 * s,
            panel_w,
            lines.len() as f32 * (ch + s) + 4.0 * s,
            PANEL,
        );
        for line in &lines {
            ui.text(6.0 * s, y, line, WHITE);
            y += ch + s;
        }
    }

    // Hotbar, bottom centre (Minecraft's: 20-pixel slots).
    if playing {
        let slot = 20.0 * s;
        let n = info.hotbar.len() as f32;
        let x0 = ((w - slot * n) / 2.0).round();
        let y0 = h - slot - 2.0 * s;
        ui.rect(
            x0 - s,
            y0 - s,
            slot * n + 2.0 * s,
            slot + 2.0 * s,
            [0, 0, 0, 160],
        );
        for (i, stack) in info.hotbar.iter().enumerate() {
            let x = x0 + i as f32 * slot;
            ui.frame(x, y0, slot, slot, s, [110, 110, 110, 200]);
            if i == info.selected {
                ui.frame(
                    x - s,
                    y0 - s,
                    slot + 2.0 * s,
                    slot + 2.0 * s,
                    2.0 * s,
                    WHITE,
                );
            }
            if let Some(stack) = stack {
                ui.item(x + 2.0 * s, y0 + 2.0 * s, 16.0 * s, stack);
            }
        }

        // Health: ten hearts above the left half of the hotbar (survival only).
        let mut above = y0 - 2.0 * s;
        if info.mode == GameMode::Survival {
            let heart = 9.0 * s;
            let hy = above - heart;
            for i in 0..(MAX_HEALTH / 2.0) as usize {
                let hp = info.health - i as f32 * 2.0;
                let tex = if hp >= 2.0 {
                    Tex::Heart
                } else if hp >= 1.0 {
                    Tex::HeartHalf
                } else {
                    Tex::HeartEmpty
                };
                ui.sprite(x0 + i as f32 * 8.0 * s, hy, heart, tex, WHITE);
            }
            // Hunger: ten drumsticks above the right half, filling from the right.
            let right_edge = x0 + slot * n;
            for i in 0..10 {
                let food = info.food - i as f32 * 2.0;
                let tex = if food >= 2.0 {
                    Tex::Food
                } else if food >= 1.0 {
                    Tex::FoodHalf
                } else {
                    Tex::FoodEmpty
                };
                ui.sprite(
                    right_edge - (i + 1) as f32 * 8.0 * s - s,
                    hy,
                    heart,
                    tex,
                    WHITE,
                );
            }
            // Armor: above the hearts, only when wearing some.
            if info.armor > 0 {
                let ay = hy - heart - s;
                for i in 0..10 {
                    let points = info.armor as i32 - i * 2;
                    let tex = if points >= 2 {
                        Tex::ArmorIcon
                    } else if points == 1 {
                        Tex::ArmorHalf
                    } else {
                        Tex::ArmorEmpty
                    };
                    ui.sprite(x0 + i as f32 * 8.0 * s, ay, heart, tex, WHITE);
                }
            }
            above = hy - 2.0 * s - if info.armor > 0 { heart + s } else { 0.0 };
        }

        // The selected item's name, fading out after switching.
        if let Some(stack) = info.hotbar.get(info.selected).copied().flatten() {
            let alpha = (1.0 - (info.selected_age - 1.5) / 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                let name = stack.item.name();
                let tw = ui.text_width(&name);
                let color = [255, 255, 255, (alpha * 255.0) as u8];
                ui.text((w - tw) / 2.0, above - ch - 2.0 * s, &name, color);
            }
        }
    }

    // Transient message, above the hotbar.
    if let Some(message) = info.message {
        let tw = ui.text_width(message);
        ui.text(
            (w - tw) / 2.0,
            h - 56.0 * s - ch,
            message,
            [255, 230, 150, 255],
        );
    }

    // Console, top half.
    if info.console.open {
        let rows = ((h * 0.45) / (ch + s)) as usize;
        let height = rows as f32 * (ch + s) + 6.0 * s;
        ui.rect(0.0, 0.0, w, height, [0, 0, 0, 170]);
        let shown = info.console.lines.len().min(rows.saturating_sub(1));
        let mut y = 2.0 * s;
        for line in &info.console.lines[info.console.lines.len() - shown..] {
            ui.text(4.0 * s, y, line, DIM);
            y += ch + s;
        }
        let y = height - ch - 2.0 * s;
        ui.rect(0.0, y - s, w, ch + 2.0 * s, [255, 255, 255, 25]);
        let width = ui.text(4.0 * s, y, &format!("> {}", info.console.input), WHITE);
        ui.rect(4.0 * s + width, y, cw * 0.7, ch - s, [255, 255, 255, 200]);
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn distance(m: f64) -> String {
    if m.abs() >= 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}
