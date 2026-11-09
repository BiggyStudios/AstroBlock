//! Dying. The world goes on and the camera stays with what happened, on the wreck after a
//! crash, until the player chooses to respawn. Items drop at the moment of death.

use std::time::Instant;

use glam::DVec3;
use winit::{event::MouseButton, keyboard::KeyCode};

use super::{Game, WARP_LEVELS};
use crate::{
    flight::{
        hud::{Pointer, button},
        map_view::Planner,
    },
    player::{EYE_HEIGHT, GameMode, PlayerInput},
    space::bodies::Body,
    ui::{Ui, menu::Action},
};

/// How long the words take to come up, and the wait before the button does (s): the crash is
/// what you came to see, and a click that was meant for flying shouldn't respawn you.
const FADE_IN: f32 = 0.7;
const BUTTON_AFTER: f32 = 1.0;
/// On foot the view sinks to the ground over this long (s), to this far above the feet (m).
const FALL_OVER: f32 = 0.7;
const EYE_ON_THE_GROUND: f64 = 0.35;

const TEXT: [u8; 4] = [235, 240, 245, 240];
const DIM: [u8; 4] = [215, 222, 230, 225];

pub(super) struct Death {
    cause: String,
    since: Instant,
    /// Where the vehicle came apart (the active world's frame). The camera circles it; without
    /// one it stays at the player's eyes.
    wreck: Option<DVec3>,
}

impl Death {
    fn age(&self) -> f32 {
        self.since.elapsed().as_secs_f32()
    }

    pub(super) fn wreck(&self) -> Option<DVec3> {
        self.wreck
    }

    /// How far above the feet the eyes are, falling over.
    pub(super) fn eye_height(&self) -> f64 {
        let t = (self.age() / FALL_OVER).min(1.0) as f64;
        EYE_HEIGHT + (EYE_ON_THE_GROUND - EYE_HEIGHT) * t * t
    }
}

impl Game {
    /// The camera is circling a wreck.
    pub(super) fn watching_wreck(&self) -> bool {
        self.death.as_ref().is_some_and(|d| d.wreck.is_some())
    }

    /// The player has died. Everything carried falls here (not in creative), and the game
    /// waits for them to press respawn.
    pub(super) fn die(&mut self, cause: &str, wreck: Option<DVec3>) {
        if self.death.is_some() {
            return;
        }
        let pose = self.pose();
        if self.player.mode == GameMode::Survival {
            self.survival.die(&pose);
        } else {
            self.survival.close(&pose);
        }
        self.survival.dead = true;
        // Killed at the controls of something still in one piece: it flies on without them.
        let mut wreck = wreck;
        if let Some(i) = self.flight.boarded.take() {
            self.release_seat(i);
            wreck.get_or_insert(pose.feet);
        }
        let flight = &mut self.flight;
        flight.aboard = None;
        flight.seat = None;
        flight.cockpit = false;
        flight.map = false;
        flight.pointed = None;
        flight.digging = None;
        flight.planner = Planner::default();
        // Slowed down to see it (a followed clock isn't ours to slow).
        if self.net.follow.is_none() {
            self.warp = 0;
        }
        self.input.clear();
        self.set_mouse_grab(false);
        self.death = Some(Death {
            cause: cause.to_string(),
            since: Instant::now(),
            wreck,
        });
    }

    /// Back to life at the spawn point (if dead).
    pub(super) fn respawn(&mut self) {
        if self.death.take().is_none() {
            return;
        }
        self.survival.dead = false;
        self.flight.eva = None;
        self.flight.aboard = None;
        self.switch_world(Body::Planet);
        let frame = self.active_frame(self.time);
        self.player.teleport(self.spawn_feet, &frame);
        self.player.fallen = 0.0;
        self.reset_camera();
        self.input.clear();
        if self.capture.is_none() && self.menu.is_none() && !self.console.open {
            self.set_mouse_grab(true);
        }
    }

    /// A frame while dead: nothing listens to the player but the world carries on.
    pub(super) fn update_dead(&mut self, dt: f64) {
        if self.input.pressed(KeyCode::Escape) && !self.flight.map {
            self.open_pause();
            return;
        }
        self.keep_view_upright(dt as f32);
        self.flight_input(dt);
        let start = self.time;
        if let Some(at) = self.death.as_ref().and_then(Death::wreck) {
            let warp = WARP_LEVELS[self.warp];
            self.effective_warp = warp;
            self.time += dt * warp;
            // (Chunks load around the player: keep them around the wreck.)
            let frame = self.active_frame(self.time);
            self.player.teleport(at, &frame);
        } else if self.flight.eva.is_some() {
            self.float(dt);
        } else {
            let still = PlayerInput {
                spectator_speed: self.fly_speed,
                ..Default::default()
            };
            self.walk_physics(&still, dt);
        }
        self.update_vessels(start);
        self.follow_craft_world();
        self.place_camera();
        self.update_prediction();
        self.player.fallen = 0.0;
        let warped = self.time - start;
        self.world_goes_on(warped);
        self.play_sounds();
    }

    /// The death screen: low on the screen, so the crash stays in view.
    pub(super) fn draw_death(&mut self, ui: &mut Ui) {
        if self.menu.is_some() {
            return;
        }
        let Some(death) = &self.death else { return };
        let age = death.age();
        let cause = death.cause.clone();
        let hint = if death.wreck.is_some() {
            "right drag to look around, wheel to zoom"
        } else {
            "right drag to look around"
        };
        let (w, h, s) = (ui.width, ui.height, ui.scale);
        let (_, ch) = ui.char_size();
        let start = ui.vertices.len();

        let band = h * 0.5;
        let (dark, clear) = ([6, 8, 12, 225], [6, 8, 12, 0]);
        ui.gradient(0.0, h - band, w, band, [clear, clear, dark, dark]);
        let mut y = h * 0.68;
        ui.scale = s * 2.0;
        let title = "you died";
        ui.text((w - ui.text_width(title)) / 2.0, y, title, TEXT);
        ui.scale = s;
        y += ch * 2.0 + 3.0 * s;
        ui.text((w - ui.text_width(&cause)) / 2.0, y, &cause, DIM);
        y += ch + 3.0 * s;
        ui.text((w - ui.text_width(hint)) / 2.0, y, hint, DIM);
        y += ch + 10.0 * s;
        let alpha = (age / FADE_IN).min(1.0);
        ui.fade_from(start, alpha, 0.0, (1.0 - alpha) * 4.0 * s);

        if age < BUTTON_AFTER {
            return;
        }
        let mut pointer = Pointer {
            pos: self.input.cursor,
            pressed: self.input.button_pressed(MouseButton::Left),
            held: self.input.button_held(MouseButton::Left),
            scroll: 0.0,
            over_ui: false,
        };
        let label = "respawn";
        let x = (w - ui.text_width(label) - 4.0 * s) / 2.0;
        let start = ui.vertices.len();
        let (clicked, _) = button(ui, &mut pointer, x, y, label, true, true);
        ui.fade_from(start, ((age - BUTTON_AFTER) / 0.25).min(1.0), 0.0, 0.0);
        if clicked {
            self.menu_actions.push(Action::Respawn);
        }
    }
}
