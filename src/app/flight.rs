//! Flying, on the game's side: rockets standing around and the one the player is in, its
//! controls, the chase and map cameras, the maneuver planner, the flight HUD and map overlay,
//! the rocket's effects and sounds, and the console commands for testing all of it.
//!
//! - Physics, SAS and heating: `vessel`. Orbits and predictions: `space::trajectory`.
//! - Drawing: `flight::rocket_model`, `flight::hud`, `flight::map_view`.
//!
//! In a rocket the mouse is free (for the HUD's buttons and the map); the right button drags
//! the camera around and the wheel zooms.

use std::{f64::consts::PI, time::Instant};

use anyhow::Result;
use glam::{DVec3, Quat};
use winit::{event::MouseButton, keyboard::KeyCode};

use super::{Game, MAX_PHYSICS_WARP, WARP_LEVELS, WorldVoxels};
use crate::{
    audio::Sound,
    effects,
    flight::{
        hud::{self, Attitude, FlightAction, FlightInfo, Pointer},
        map_view::{self, MapContext, Planner, View},
        rocket_model::rocket_model,
        vessel::{self, COM_HEIGHT, Controls, FUEL_CAPACITY, SasMode, Vessel},
    },
    player::{EYE_HEIGHT, GameMode},
    render::{DrawItem, Fade, ParticleEmitter, camera::Camera, models::Light},
    space::{
        bodies::Body,
        conic::propagate,
        trajectory::{self, ManeuverNode, Segment, burn_vector, orbit_axes},
    },
    survival::items::{Item, ItemStack},
    ui::Ui,
    world::block::Material,
};

/// Throttle change per second while Shift / Ctrl is held.
const THROTTLE_RATE: f64 = 0.7;
/// Rad per pixel of mouse drag.
const DRAG_SENSITIVITY: f64 = 0.006;

#[derive(Clone, Copy)]
struct Orbiting {
    yaw: f64,
    pitch: f64,
    distance: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Craft,
    Body(Body),
}

/// The player floating outside, above the air (after getting out in space, or drifting up
/// there). Like an empty rocket it follows its conic exactly (patched conics, so the moon
/// and the sun pull too); you can look around but not move.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Eva {
    pub body: Body,
    /// Feet, relative to `body`, inertial axes.
    pub position: DVec3,
    pub velocity: DVec3,
}

pub(super) struct Flight {
    pub vessels: Vec<Vessel>,
    /// The vessel the player is in.
    pub boarded: Option<usize>,
    pub eva: Option<Eva>,
    pub map: bool,
    chase: Orbiting,
    map_cam: Orbiting,
    focus: Focus,
    pub planner: Planner,
    pub prediction: Vec<Segment>,
    controls: Controls,
    last_cursor: (f32, f32),
    /// The mouse was over flight UI last frame (the wheel then doesn't zoom).
    over_ui: bool,
    engine_timer: f32,
    /// Fractions of particles carried to the next frame: flame, smoke, plasma.
    carry: [f32; 3],
    /// Explosions etc. to send with this frame's particles.
    pending: Vec<ParticleEmitter>,
    time: f32,
}

impl Default for Flight {
    fn default() -> Self {
        Self {
            vessels: Vec::new(),
            boarded: None,
            eva: None,
            map: false,
            chase: Orbiting {
                yaw: PI,
                pitch: 0.2,
                distance: 24.0,
            },
            map_cam: Orbiting {
                yaw: 0.3,
                pitch: 0.45,
                distance: 150_000.0,
            },
            focus: Focus::Craft,
            planner: Planner::default(),
            prediction: Vec::new(),
            controls: Controls::default(),
            last_cursor: (0.0, 0.0),
            over_ui: false,
            engine_timer: 0.0,
            carry: [0.0; 3],
            pending: Vec::new(),
            time: 0.0,
        }
    }
}

/// The flying side of a saved game.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SavedFlight {
    vessels: Vec<Vessel>,
    boarded: Option<usize>,
    eva: Option<Eva>,
    nodes: Vec<ManeuverNode>,
}

impl Flight {
    pub(super) fn saved(&self) -> SavedFlight {
        SavedFlight {
            vessels: self.vessels.clone(),
            boarded: self.boarded,
            eva: self.eva.clone(),
            nodes: self.planner.nodes.clone(),
        }
    }

    pub(super) fn restore(&mut self, saved: SavedFlight) {
        self.boarded = saved.boarded.filter(|&i| i < saved.vessels.len());
        self.vessels = saved.vessels;
        self.eva = saved.eva;
        self.planner.nodes = saved.nodes;
    }
}

impl Game {
    /// The camera is the player's eyes (not in a rocket, not the map).
    pub(super) fn first_person(&self) -> bool {
        self.flight.boarded.is_none() && !self.flight.map
    }

    /// The craft the map and predictions are about: the rocket, or the player.
    fn craft_state(&self) -> (Body, DVec3, DVec3) {
        match self.flight.boarded {
            Some(i) => {
                let v = &self.flight.vessels[i];
                (v.body, v.position, v.velocity)
            }
            None => match &self.flight.eva {
                Some(e) => (e.body, e.position, e.velocity),
                None => (self.active, self.player.position, self.player.velocity),
            },
        }
    }

    /// The craft's world position (planet body frame, planet-relative).
    fn craft_world(&self) -> DVec3 {
        match self.flight.boarded {
            Some(i) => self.flight.vessels[i].world_position(&self.bodies, self.active, self.time),
            None => {
                let (feet, up) = self.feet_and_up();
                feet + up * EYE_HEIGHT
            }
        }
    }

    /// The player's feet (world, planet-relative) and "up" there: away from the body
    /// whose sphere of influence it is in.
    pub(super) fn feet_and_up(&self) -> (DVec3, DVec3) {
        let frame = self.active_frame(self.time);
        let feet = self.player.body_position(&frame);
        let center = match &self.flight.eva {
            Some(e) => self
                .bodies
                .view(self.active, e.body, DVec3::ZERO, self.time),
            None => DVec3::ZERO,
        };
        (feet, (feet - center).normalize_or(DVec3::Y))
    }

    /// Below this radius of the active body, floating hands over to walking (the player's
    /// own physics): home's air, or the moon's block world.
    fn handover_radius(&self) -> f64 {
        self.gravity
            .atmosphere_top()
            .max(self.planet.layer_radius(self.planet.layers as f64) + 50.0)
    }

    /// Floating outside (see `Eva`): follows its conic; near the ground of the world loaded
    /// now, the player's own physics takes over (falling, landing, walking).
    pub(super) fn float(&mut self, dt: f64) {
        let Some(eva) = &mut self.flight.eva else {
            return;
        };
        let warp = WARP_LEVELS[self.warp];
        self.effective_warp = warp;
        let (t0, t1) = (self.time, self.time + dt * warp);
        let (body, r, v, t, stopped) = trajectory::advance(
            &self.bodies,
            eva.body,
            eva.position,
            eva.velocity,
            (t0, t1),
            vessel::RAILS_CLEARANCE,
        );
        (eva.body, eva.position, eva.velocity) = (body, r, v);
        self.time = t;
        if stopped {
            self.warp = 0;
        }
        let (pos, vel) = self.bodies.rebase(body, self.active, r, v, t);
        self.player.position = pos;
        self.player.velocity = vel;
        self.player.on_ground = false;
        if body == self.active && r.length() <= self.handover_radius() {
            self.flight.eva = None;
        }
    }

    /// On foot above the air and the blocks (launched, or jumped off the moon): start
    /// floating.
    pub(super) fn start_floating(&mut self) {
        let p = &self.player;
        if p.mode == GameMode::Spectator
            || p.flying
            || p.on_ground
            || p.position.length() <= self.handover_radius() + 10.0
        {
            return;
        }
        self.flight.eva = Some(Eva {
            body: self.active,
            position: p.position,
            velocity: p.velocity,
        });
    }

    /// Where chunks load around (absolute world position).
    pub(super) fn stream_focus(&self) -> DVec3 {
        if self.first_person() {
            self.camera.position
        } else {
            self.planet.center + self.craft_world()
        }
    }

    /// Keys and mouse for flying, the cameras and the map (before physics).
    pub(super) fn flight_input(&mut self, dt: f64) {
        let input = &self.input;
        let in_screen = self.survival.screen.is_some();
        if !in_screen && input.pressed(KeyCode::KeyM) {
            self.toggle_map();
        }
        // Mouse drags (right button) turn the camera, the wheel zooms.
        let cursor = self.input.cursor;
        let (dx, dy) = (
            (cursor.0 - self.flight.last_cursor.0) as f64,
            (cursor.1 - self.flight.last_cursor.1) as f64,
        );
        self.flight.last_cursor = cursor;
        let dragging = self.input.button_held(MouseButton::Right);
        let scroll = if self.flight.over_ui {
            0.0
        } else {
            self.input.scroll as f64
        };
        if self.flight.map {
            let min = match self.flight.focus {
                Focus::Craft => 30.0,
                Focus::Body(b) => self.bodies.radius(b) * 1.1,
            };
            let cam = &mut self.flight.map_cam;
            if dragging {
                cam.yaw -= dx * DRAG_SENSITIVITY;
                cam.pitch = (cam.pitch + dy * DRAG_SENSITIVITY).clamp(-1.5, 1.5);
            }
            cam.distance = (cam.distance * 0.85f64.powf(scroll)).clamp(min, 5e11);
            if self.input.pressed(KeyCode::Tab) {
                self.flight.focus = match self.flight.focus {
                    Focus::Craft => Focus::Body(Body::Planet),
                    Focus::Body(Body::Planet) => Focus::Body(Body::Moon),
                    _ => Focus::Craft,
                };
                self.flight.map_cam.distance = match self.flight.focus {
                    Focus::Body(b) => self.bodies.radius(b) * 4.0,
                    Focus::Craft => self.flight.map_cam.distance,
                };
            }
            if self.input.pressed(KeyCode::Delete) || self.input.pressed(KeyCode::Backspace) {
                self.flight.planner.delete_selected();
            }
            if self.input.pressed(KeyCode::Escape) {
                self.toggle_map();
            }
        } else if self.flight.boarded.is_some() {
            let cam = &mut self.flight.chase;
            if dragging {
                cam.yaw += dx * DRAG_SENSITIVITY;
                cam.pitch = (cam.pitch + dy * DRAG_SENSITIVITY).clamp(-1.4, 1.4);
            }
            cam.distance = (cam.distance * 0.88f64.powf(scroll)).clamp(9.0, 600.0);
        }

        let Some(i) = self.flight.boarded else {
            return;
        };
        let held = |k: KeyCode| self.input.held(k) as i32 as f64;
        self.flight.controls = Controls {
            pitch: held(KeyCode::KeyS) - held(KeyCode::KeyW),
            yaw: held(KeyCode::KeyD) - held(KeyCode::KeyA),
            roll: held(KeyCode::KeyE) - held(KeyCode::KeyQ),
        };
        let v = &mut self.flight.vessels[i];
        let mut throttle = v.throttle;
        throttle += (held(KeyCode::ShiftLeft) - held(KeyCode::ControlLeft)) * THROTTLE_RATE * dt;
        if self.input.pressed(KeyCode::KeyZ) {
            throttle = 1.0;
        }
        if self.input.pressed(KeyCode::KeyX) {
            throttle = 0.0;
        }
        v.throttle = throttle.clamp(0.0, 1.0);
        if self.input.pressed(KeyCode::KeyT) {
            v.sas = !v.sas;
        }
        if self.input.pressed(KeyCode::KeyF) {
            self.exit_vessel();
        }
    }

    fn toggle_map(&mut self) {
        self.flight.map = !self.flight.map;
        if self.flight.map {
            self.set_mouse_grab(false);
            self.flight.focus = Focus::Craft;
            let (body, r, _) = self.craft_state();
            let scale = r.length().max(self.bodies.radius(body));
            self.flight.map_cam.distance = scale * 2.6;
        } else {
            self.flight.planner.selected = None;
            if self.first_person() && self.capture.is_none() {
                self.set_mouse_grab(true);
            }
        }
    }

    /// The next maneuver's burn direction (inertial), from the boarded rocket's orbit.
    fn maneuver_direction(&self) -> Option<DVec3> {
        let v = &self.flight.vessels[self.flight.boarded?];
        let node = self.flight.planner.nodes.first()?;
        let (r, vel) = propagate(
            self.bodies.mu(v.body),
            v.position,
            v.velocity,
            node.time - self.time,
        );
        Some(burn_vector(r, vel, node.dv))
    }

    /// Physics (or rails) for the boarded rocket, which carries the player. Advances time.
    pub(super) fn fly(&mut self, dt: f64) {
        let Some(i) = self.flight.boarded else { return };
        // "Warp to": the fastest warp that won't overshoot the target.
        let mut warp = WARP_LEVELS[self.warp];
        if let Some(target) = self.flight.planner.warp_to {
            let left = target - self.time;
            if left <= 1.0 {
                self.flight.planner.warp_to = None;
                self.warp = 0;
            } else {
                self.warp = WARP_LEVELS
                    .iter()
                    .rposition(|&l| l * dt * 4.0 < left)
                    .unwrap_or(0);
            }
            warp = WARP_LEVELS[self.warp];
        }
        let mut rails = warp > MAX_PHYSICS_WARP;
        if rails && !self.flight.vessels[i].can_rail_warp(&self.bodies) {
            warp = MAX_PHYSICS_WARP;
            rails = false;
            let why = if self.flight.vessels[i].throttle > 0.0 {
                "while the engine burns"
            } else {
                "this close to the ground or in the air"
            };
            self.message = Some((
                format!("Time warp limited to x{MAX_PHYSICS_WARP} {why}"),
                Instant::now(),
            ));
        }
        self.effective_warp = warp;
        let (t0, t1) = (self.time, self.time + dt * warp);
        let maneuver = self.maneuver_direction();
        let world = WorldVoxels {
            planet: &self.planet,
            streamer: &self.streamer,
            terrain: &self.terrain,
        };
        let vessel = &mut self.flight.vessels[i];
        if rails {
            let (t, stopped) = vessel.rails(&self.bodies, (t0, t1));
            self.time = t;
            if stopped {
                self.warp = 0;
                self.flight.planner.warp_to = None;
                self.message = Some(("Time warp stopped".to_string(), Instant::now()));
            }
        } else {
            let env = vessel::Env {
                bodies: &self.bodies,
                ground: Some(vessel::Ground {
                    body: self.active,
                    planet: &self.planet,
                    world: &world,
                }),
                drag: self.drag,
            };
            let target = if vessel.sas {
                vessel.sas_direction(&self.bodies, t0, maneuver)
            } else {
                None
            };
            vessel.update(&env, &self.flight.controls, (t0, t1), target);
            self.time = t1;
        }

        if std::mem::take(&mut vessel.flameout) {
            self.message = Some(("out of fuel".to_string(), Instant::now()));
        }
        // The next maneuver counts down with what the engine delivers.
        let burned = std::mem::take(&mut vessel.burned);
        let (body, r, v) = (vessel.body, vessel.position, vessel.velocity);
        if burned.length() > 0.0
            && let Some(node) = self.flight.planner.nodes.first_mut()
        {
            let burn = node.dv.length() / vessel.acceleration();
            if (node.time - self.time).abs() < burn + 120.0 {
                let (rn, vn) = propagate(self.bodies.mu(body), r, v, node.time - self.time);
                let (p, n, rad) = orbit_axes(rn, vn);
                let before = node.dv;
                node.dv -= DVec3::new(burned.dot(p), burned.dot(n), burned.dot(rad));
                if node.dv.length() < 0.1 || node.dv.dot(before) <= 0.0 {
                    self.flight.planner.nodes.remove(0);
                    self.flight.planner.selected = None;
                    let vessel = &mut self.flight.vessels[i];
                    if vessel.sas_mode == SasMode::Maneuver {
                        vessel.sas_mode = SasMode::Stability;
                    }
                    vessel.throttle = 0.0;
                    self.message = Some(("Maneuver complete".to_string(), Instant::now()));
                }
            }
        }

        if let Some(reason) = self.flight.vessels[i].destroyed {
            self.destroy_vessel(i, reason);
            return;
        }
        // The player rides along (for streaming, survival, and getting out).
        let v = &self.flight.vessels[i];
        let (pos, vel) = self
            .bodies
            .rebase(v.body, self.active, v.position, v.velocity, self.time);
        self.player.position = pos;
        self.player.velocity = vel;
        self.player.on_ground = false;
    }

    /// The rockets nobody is in, from `t0` to now: landed ones ride the turning ground,
    /// the rest coast on their conics (or fall through the air with physics).
    pub(super) fn update_vessels(&mut self, t0: f64) {
        let t1 = self.time;
        let world = WorldVoxels {
            planet: &self.planet,
            streamer: &self.streamer,
            terrain: &self.terrain,
        };
        let env = vessel::Env {
            bodies: &self.bodies,
            ground: Some(vessel::Ground {
                body: self.active,
                planet: &self.planet,
                world: &world,
            }),
            drag: self.drag,
        };
        let mut destroyed = Vec::new();
        let client = self.net.is_client();
        for (i, v) in self.flight.vessels.iter_mut().enumerate() {
            if Some(i) == self.flight.boarded {
                continue;
            }
            if v.landed.is_some() {
                v.sync_landed(&self.bodies, t1);
                continue;
            }
            // Someone else's to fly (or the host's, for a client): carry on until the next
            // news of it.
            if v.pilot.is_some() {
                if t1 - t0 > 1.0 {
                    v.rails(&self.bodies, (t0, t1));
                } else if t1 >= t0 - 1.0 {
                    v.position += v.velocity * (t1 - t0);
                }
                continue;
            }
            v.throttle = 0.0;
            let mut from = t0;
            if v.can_rail_warp(&self.bodies) {
                from = v.rails(&self.bodies, (t0, t1)).0;
            }
            if client {
                if (t1 - from).abs() <= 1.0 {
                    v.position += v.velocity * (t1 - from);
                }
                continue;
            }
            if from < t1 {
                v.update(&env, &Controls::default(), (from, t1), None);
            }
            if let Some(reason) = v.destroyed {
                destroyed.push((i, reason));
            }
        }
        for (i, reason) in destroyed.into_iter().rev() {
            self.destroy_vessel(i, reason);
        }
    }

    fn destroy_vessel(&mut self, i: usize, reason: &str) {
        let id = self.flight.vessels[i].id;
        self.net_vessel_gone(id, Some(reason));
        self.explode_vessel(i, reason);
    }

    /// A rocket stops existing, with a bang.
    pub(super) fn explode_vessel(&mut self, i: usize, reason: &str) {
        let v = self.flight.vessels.remove(i);
        let at = v.world_position(&self.bodies, self.active, self.time);
        let ground =
            self.bodies
                .view_velocity(self.active, v.body, v.position, v.velocity, self.time);
        let up = at.normalize_or(DVec3::Y);
        self.flight
            .pending
            .extend(effects::explosion(at, ground * 0.2, up));
        if let Some(mesh) = v.mesh {
            self.renderer.destroy_mesh(mesh);
        }
        let listener = self.camera.position - self.planet.center;
        let right = self.camera.right().as_dvec3();
        self.audio.play(Sound::Explosion, at - listener, right);
        self.message = Some((format!("The rocket {reason}!"), Instant::now()));
        match self.flight.boarded {
            Some(b) if b == i => {
                self.flight.boarded = None;
                self.flight.map = false;
                self.flight.planner = Planner::default();
                self.reset_camera();
                if self.player.mode == GameMode::Survival {
                    self.respawn();
                } else {
                    let frame = self.active_frame(self.time);
                    self.player.teleport(self.spawn_feet, &frame);
                }
                if self.capture.is_none() {
                    self.set_mouse_grab(true);
                }
            }
            Some(b) if b > i => self.flight.boarded = Some(b - 1),
            _ => {}
        }
    }

    /// Takes a rocket away quietly (someone picked it up).
    pub(super) fn remove_vessel(&mut self, i: usize) {
        let v = self.flight.vessels.remove(i);
        if let Some(mesh) = v.mesh {
            self.renderer.destroy_mesh(mesh);
        }
        match self.flight.boarded {
            Some(b) if b > i => self.flight.boarded = Some(b - 1),
            _ => {}
        }
    }

    /// Asks to get in (in a game with others, the host says who flies it).
    fn board(&mut self, i: usize) {
        if self.player.mode != GameMode::Spectator && self.request_seat(i) {
            self.enter_vessel(i);
        }
    }

    pub(super) fn enter_vessel(&mut self, i: usize) {
        if self.player.mode == GameMode::Spectator {
            return;
        }
        self.flight.boarded = Some(i);
        self.flight.eva = None;
        self.flight.planner = Planner::default();
        self.flight.chase = Orbiting {
            yaw: PI,
            pitch: 0.2,
            distance: 24.0,
        };
        self.survival.mining = None;
        self.set_mouse_grab(false);
        self.message = Some((
            "In the rocket: shift / ctrl throttle, wasd qe steer, m map, f to get out".to_string(),
            Instant::now(),
        ));
    }

    /// Climbs out next to a landed rocket.
    fn exit_vessel(&mut self) {
        let Some(i) = self.flight.boarded else { return };
        self.release_seat(i);
        let v = &self.flight.vessels[i];
        let landed_here = v.landed.is_some() && v.body == self.active;
        if !landed_here {
            // Out into space (or the air): beside the hatch, moving with the rocket.
            let right = v.attitude * DVec3::X;
            let (r, vel, body) = (v.position + right * 4.0, v.velocity, v.body);
            let (pos, pvel) = self.bodies.rebase(body, self.active, r, vel, self.time);
            self.player.position = pos;
            self.player.velocity = pvel;
            self.player.on_ground = false;
            self.flight.eva = Some(Eva {
                body,
                position: r,
                velocity: vel,
            });
            self.flight.boarded = None;
            self.flight.map = false;
            self.flight.planner = Planner::default();
            // (Near the ground this hands straight over to the player's own physics.)
            self.float(0.0);
            self.reset_camera();
            if self.capture.is_none() {
                self.set_mouse_grab(true);
            }
            return;
        }
        let frame = self.active_frame(self.time);
        let com = v.world_position(&self.bodies, self.active, self.time);
        let up = com.normalize();
        let right = frame.to_body(v.attitude * DVec3::X);
        let base = com - up * COM_HEIGHT;
        // Beside it, on the first open ground up from the rocket's base.
        let spot = base + right * 3.0;
        let mut feet = spot;
        if let Some(cell) = self.planet.cell_at(spot - up * 0.5) {
            let free = |k: i32| {
                self.planet
                    .cell(cell.face, cell.u, cell.v, k)
                    .and_then(|c| self.streamer.block(c))
                    .is_none_or(|b| !b.collides())
            };
            for dk in 0..24 {
                let k = cell.k + dk;
                if !free(k) {
                    continue;
                }
                if free(k + 1) && !free(k - 1) {
                    feet = spot.normalize() * self.planet.layer_radius(k as f64);
                    break;
                }
            }
        }
        self.player.teleport(feet, &frame);
        self.flight.boarded = None;
        self.flight.map = false;
        self.flight.planner = Planner::default();
        self.reset_camera();
        if self.capture.is_none() {
            self.set_mouse_grab(true);
        }
    }

    /// Back to first person at the player's eyes.
    pub(super) fn reset_camera(&mut self) {
        let (feet, up) = self.feet_and_up();
        let mut camera = Camera::new(self.planet.center + feet + up * EYE_HEIGHT, up.as_vec3());
        camera.fov_y = self.camera.fov_y;
        self.camera = camera;
    }

    /// Places the chase or map camera (after physics).
    pub(super) fn place_flight_camera(&mut self) {
        let t = self.time;
        let frame = self.active_frame(t);
        if self.flight.map {
            let focus = match self.flight.focus {
                Focus::Craft => self.craft_world(),
                Focus::Body(b) => self.bodies.view(self.active, b, DVec3::ZERO, t),
            };
            // The map turns with the stars, not with the planet.
            let north = self.bodies.equator() * DVec3::Y;
            let e1 = self.bodies.equator() * DVec3::X;
            let e2 = north.cross(e1);
            let c = self.flight.map_cam;
            let dir = (e1 * c.yaw.cos() + e2 * c.yaw.sin()) * c.pitch.cos() + north * c.pitch.sin();
            let offset = frame.to_body(dir * c.distance);
            self.camera.position = self.planet.center + focus + offset;
            self.camera
                .look_at(-offset.as_vec3(), frame.to_body(north).as_vec3());
        } else if let Some(i) = self.flight.boarded {
            let v = &self.flight.vessels[i];
            let com = v.world_position(&self.bodies, self.active, t);
            let center = self.bodies.view(self.active, v.body, DVec3::ZERO, t);
            let up = (com - center).normalize_or(DVec3::Y);
            let axis = self.bodies.view_rotation(self.active, v.body, t) * DVec3::Y;
            let east = axis
                .cross(up)
                .try_normalize()
                .unwrap_or_else(|| up.any_orthonormal_vector());
            let north = up.cross(east);
            let c = self.flight.chase;
            let dir =
                (north * c.yaw.cos() + east * c.yaw.sin()) * c.pitch.cos() + up * c.pitch.sin();
            // Aim a little below the centre of mass: the rocket sits high, its flame in view.
            let target = com - up * 2.5;
            self.camera.position = self.planet.center + target + dir * c.distance;
            self.camera.look_at(-dir.as_vec3(), up.as_vec3());
        }
    }

    /// Recomputes the predicted trajectory (for the map, and maneuver nodes).
    pub(super) fn update_prediction(&mut self) {
        let landed = match self.flight.boarded {
            Some(i) => self.flight.vessels[i].landed.is_some(),
            None => self.player.on_ground,
        };
        if landed || !(self.flight.map || !self.flight.planner.nodes.is_empty()) {
            self.flight.prediction.clear();
            return;
        }
        let (body, r, v) = self.craft_state();
        let nodes: &[_] = if self.flight.boarded.is_some() {
            &self.flight.planner.nodes
        } else {
            &[]
        };
        self.flight.prediction =
            trajectory::predict(&self.bodies, body, r, v, self.time, nodes, 10);
    }

    /// The rocket under the crosshair (index, distance), if nearer than the targeted block.
    pub(super) fn rocket_target(&self, reach: f64) -> Option<usize> {
        let origin = self.camera.position - self.planet.center;
        let dir = self.camera.forward().as_dvec3();
        let block = self.target.as_ref().map_or(f64::MAX, |h| h.distance);
        self.flight
            .vessels
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                v.ray_hit(&self.bodies, self.active, self.time, origin, dir, reach)
                    .map(|d| (i, d))
            })
            .filter(|&(_, d)| d < block)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Clicks on a rocket: right boards it, left picks it up (if it stands on the ground).
    pub(super) fn rocket_clicks(&mut self, i: usize) {
        if self.input.button_pressed(MouseButton::Right) {
            self.board(i);
        } else if self.input.button_pressed(MouseButton::Left)
            && self.flight.vessels[i].landed.is_some()
            && self.flight.vessels[i].pilot.is_none()
        {
            let id = self.flight.vessels[i].id;
            self.net_vessel_gone(id, None);
            self.remove_vessel(i);
            let pose = self.pose();
            if let Some(rest) = self.survival.inventory.add(ItemStack::new(Item::Rocket, 1)) {
                self.survival
                    .spawn(rest, pose.feet + pose.up(), DVec3::ZERO, 0.0);
            }
            self.survival.swings += 1;
        }
    }

    /// A right click (`clicked`) with a rocket in hand on the top of a block stands one
    /// there. Returns whether the click was used.
    pub(super) fn place_rocket(&mut self, clicked: bool) -> bool {
        let holding = self.survival.held().is_some_and(|s| s.item == Item::Rocket);
        if !holding || !clicked {
            return false;
        }
        let Some(hit) = self.target.as_ref() else {
            return false;
        };
        let cell = hit.cell;
        let above = self.planet.cell(cell.face, cell.u, cell.v, cell.k + 1);
        if hit.before != above || !hit.block.collides() {
            self.message = Some((
                "Stand the rocket on top of a block".to_string(),
                Instant::now(),
            ));
            return true;
        }
        let blocked = (1..=9).any(|dk| {
            self.planet
                .cell(cell.face, cell.u, cell.v, cell.k + dk)
                .and_then(|c| self.streamer.block(c))
                .is_some_and(|b| b.collides())
        });
        if blocked {
            self.message = Some(("Not enough room for the rocket".to_string(), Instant::now()));
            return true;
        }
        let n = self.planet.res_at(cell.k);
        let base = self
            .planet
            .dir(cell.face, cell.u as f64 + 0.5, cell.v as f64 + 0.5, n)
            * self.planet.layer_radius(cell.k as f64 + 1.0);
        self.flight
            .vessels
            .push(Vessel::on_pad(&self.bodies, self.active, base, self.time));
        self.net_vessel_added();
        if self.player.mode != GameMode::Creative {
            self.survival.inventory.take_one(self.survival.selected);
        }
        self.survival.uses += 1;
        self.survival
            .sounds
            .push((Sound::Place(Material::Stone), base));
        true
    }

    /// Meshes for the rockets (rebuilt every frame: they move and glow).
    pub(super) fn flight_draw_items(&mut self, objects: &mut Vec<DrawItem>) -> Result<()> {
        let t = self.time;
        let frame = self.active_frame(t);
        for v in &mut self.flight.vessels {
            let com = v.world_position(&self.bodies, self.active, t);
            let to_body = frame.rotation.inverse() * v.attitude;
            let axes = (to_body * DVec3::X, to_body * DVec3::Y, to_body * DVec3::Z);
            let heat = ((v.temperature - 750.0) / (vessel::MAX_TEMPERATURE - 750.0)).clamp(0.0, 1.0)
                as f32;
            let block = self
                .planet
                .cell_at(com)
                .map_or(0, |c| self.streamer.light(c)) as u32;
            let mesh = rocket_model(axes, heat, Light { sky: 15, block });
            if let Some(handle) = self.renderer.create_mesh(&mesh)?
                && let Some(old) = v.mesh.replace(handle)
            {
                self.renderer.destroy_mesh(old);
            }
            if let Some(mesh) = v.mesh {
                objects.push(DrawItem {
                    mesh,
                    origin: self.planet.center + com,
                    rotation: Quat::IDENTITY,
                    radius: 9.0,
                    fade: Fade::Visible,
                });
            }
        }
        Ok(())
    }

    /// Flames, smoke, plasma and engine sound of the rockets being flown, and explosions.
    pub(super) fn flight_effects(&mut self, dt: f32) -> Vec<ParticleEmitter> {
        let mut out = std::mem::take(&mut self.flight.pending);
        if let Some(i) = self.flight.boarded {
            self.vessel_effects(i, dt, &mut out);
        }
        let eye = self.camera.position - self.planet.center;
        for i in 0..self.flight.vessels.len() {
            let v = &self.flight.vessels[i];
            let near = v.world_position(&self.bodies, self.active, self.time) - eye;
            if Some(i) != self.flight.boarded && v.pilot.is_some() && near.length() < 6_000.0 {
                self.vessel_effects(i, dt, &mut out);
            }
        }
        out
    }

    fn vessel_effects(&mut self, i: usize, dt: f32, out: &mut Vec<ParticleEmitter>) {
        let t = self.time;
        let frame = self.active_frame(t);
        let v = &self.flight.vessels[i];
        let com = v.world_position(&self.bodies, self.active, t);
        let ground = self
            .bodies
            .view_velocity(self.active, v.body, v.position, v.velocity, t);
        let nose = frame.to_body(v.nose());
        let nozzle = com - nose * COM_HEIGHT;
        let carry = &mut self.flight.carry;
        let mut count = |k: usize, rate: f32| {
            let n = rate * dt + carry[k];
            carry[k] = n.fract();
            n as u32
        };
        if v.throttle > 0.0 {
            let n = count(0, 260.0 + 340.0 * v.throttle as f32);
            out.extend(effects::exhaust(nozzle, -nose, ground, n));
            let air = if v.body == Body::Planet {
                self.bodies.gravity.air_density(v.position.length())
            } else {
                0.0
            };
            if air > 0.02 {
                let n = count(1, 90.0 * v.throttle as f32);
                out.push(effects::smoke(nozzle, -nose, ground, n));
            }
            self.flight.engine_timer -= dt;
            if self.flight.engine_timer <= 0.0 {
                self.flight.engine_timer = 0.42;
                let listener = self.camera.position - self.planet.center;
                let right = self.camera.right().as_dvec3();
                let gain = 0.35 + 0.65 * v.throttle as f32;
                self.audio
                    .play_gain(Sound::Engine, nozzle - listener, right, gain);
            }
        }
        if v.heating > 2.0e5 {
            let heat = ((v.heating - 2.0e5) / 3.0e6).clamp(0.0, 1.0) as f32;
            let flow = -ground.normalize_or(-nose);
            let lead = com - flow * 2.5;
            let n = count(2, 150.0 + 600.0 * heat);
            out.push(effects::plasma(lead, flow, ground, n, heat));
        }
    }

    /// The flight HUD (in a rocket) and the map overlay, with their clicks.
    pub(super) fn flight_overlay(&mut self, ui: &mut Ui, dt: f32) {
        self.flight.time += dt;
        let mut pointer = Pointer {
            pos: self.input.cursor,
            pressed: self.input.button_pressed(MouseButton::Left),
            held: self.input.button_held(MouseButton::Left),
            scroll: self.input.scroll,
            over_ui: false,
        };
        let t = self.time;
        if self.flight.map {
            let (w, h) = self.renderer.screen_size();
            let view = View {
                view_proj: self.camera.view_projection(w / h),
                camera: self.camera.position,
                size: (w, h),
            };
            let ctx = MapContext {
                bodies: &self.bodies,
                active: self.active,
                time: t,
                planet_center: self.planet.center,
                view,
                segments: &self.flight.prediction,
                craft: self.planet.center + self.craft_world(),
                editable: self.flight.boarded.is_some(),
                accel: self
                    .flight
                    .boarded
                    .map_or(vessel::THRUST / (vessel::DRY_MASS + FUEL_CAPACITY), |i| {
                        self.flight.vessels[i].acceleration()
                    }),
            };
            map_view::draw(ui, &ctx, &mut self.flight.planner, &mut pointer, dt);
        }
        if let Some(i) = self.flight.boarded {
            let info = self.flight_info(i);
            for action in hud::draw(ui, &info, &mut pointer, self.flight.time) {
                let v = &mut self.flight.vessels[i];
                match action {
                    FlightAction::ToggleSas => v.sas = !v.sas,
                    FlightAction::Sas(mode) => {
                        v.sas = true;
                        v.sas_mode = mode;
                    }
                    FlightAction::ToggleMap => self.toggle_map(),
                }
            }
        }
        self.flight.over_ui = pointer.over_ui;
    }

    fn flight_info(&self, i: usize) -> FlightInfo {
        let t = self.time;
        let bodies = &self.bodies;
        let v = &self.flight.vessels[i];
        let r = v.position;
        let radius = bodies.radius(v.body);
        let surface = v.surface_mode(bodies);
        let vel = if surface {
            v.surface_velocity(bodies, t)
        } else {
            v.velocity
        };
        let conic = v.conic(bodies);
        let up = r.normalize_or(DVec3::Y);
        let axis = bodies.frame(v.body, t).rotation * DVec3::Y;
        let east = axis
            .cross(up)
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let moving = vel.length() > 0.5;
        let (prograde, normal, radial) = orbit_axes(r, vel);
        let node = self.flight.planner.nodes.first().map(|n| {
            let dv = n.dv.length();
            let burn = dv / v.acceleration();
            (n.time - burn / 2.0 - t, dv, burn)
        });
        let feet = self.player.body_position(&self.active_frame(t));
        FlightInfo {
            body: v.body,
            altitude: r.length() - radius,
            vertical_speed: v.surface_velocity(bodies, t).dot(up),
            speed: vel.length(),
            surface,
            apoapsis: conic
                .apoapsis()
                .filter(|&ap| ap < bodies.soi(v.body))
                .map(|ap| (ap - radius, conic.time_to_anomaly(r, PI))),
            periapsis: (conic.periapsis() - radius, conic.time_to_anomaly(r, 0.0)),
            throttle: v.throttle,
            fuel: v.fuel / FUEL_CAPACITY,
            delta_v: v.delta_v(),
            temperature: v.temperature,
            pressure: v.pressure,
            sas: v.sas,
            sas_mode: v.sas_mode,
            landed: v.landed.is_some(),
            attitude: Attitude {
                right: v.attitude * DVec3::X,
                nose: v.nose(),
                dorsal: v.attitude * DVec3::Z,
                up,
                north: up.cross(east),
                east,
                prograde: moving.then_some(prograde),
                normal: moving.then_some(normal),
                radial: moving.then_some(radial),
                maneuver: self.maneuver_direction().and_then(DVec3::try_normalize),
            },
            node,
            warp: self.effective_warp,
            time_of_day: self.system.local_time(t, feet.normalize_or(DVec3::Y)),
            map: self.flight.map,
        }
    }

    /// Plans a moon transfer: a prograde burn up to the moon's distance, at the time in
    /// the next orbit that meets it with the lowest periapsis above 3 km.
    fn plan_transfer(&mut self) {
        let Some(i) = self.flight.boarded else {
            self.console.print("Not in a rocket");
            return;
        };
        let v = &self.flight.vessels[i];
        let conic = v.conic(&self.bodies);
        let (Body::Planet, Some(period)) = (v.body, conic.period()) else {
            self.console.print("Get into an orbit around home first");
            return;
        };
        let mu = self.bodies.mu(Body::Planet);
        let (pos, vel) = (v.position, v.velocity);
        let mut best: Option<(trajectory::ManeuverNode, f64)> = None;
        for k in 0..240 {
            let time = self.time + 30.0 + period * k as f64 / 240.0;
            let (r, rv) = propagate(mu, pos, vel, time - self.time);
            let a = (r.length() + crate::space::bodies::MOON_DISTANCE) / 2.0;
            let dv = (mu * (2.0 / r.length() - 1.0 / a)).sqrt() - rv.length();
            let node = trajectory::ManeuverNode {
                time,
                dv: DVec3::new(dv, 0.0, 0.0),
            };
            let segments =
                trajectory::predict(&self.bodies, Body::Planet, pos, vel, self.time, &[node], 6);
            if let Some(m) = segments.iter().find(|s| s.body == Body::Moon) {
                let pe = m.conic.periapsis() - self.bodies.radius(Body::Moon);
                if pe > 3_000.0 && best.is_none_or(|(_, b)| pe < b) {
                    best = Some((node, pe));
                }
            }
        }
        match best {
            Some((node, pe)) => {
                self.flight.planner.nodes = vec![node];
                self.flight.planner.selected = Some(0);
                self.console.print(format!(
                    "Moon transfer: {:.0} m/s in {:.0} s, moon periapsis {:.1} km",
                    node.dv.x,
                    node.time - self.time,
                    pe / 1000.0
                ));
            }
            None => self.console.print("No moon encounter within one orbit"),
        }
    }

    /// Console commands for flying (and testing it). Returns whether `command` was one.
    pub(super) fn flight_command(&mut self, command: &str, args: &[&str]) -> bool {
        let number = |i: usize| args.get(i).and_then(|a| a.parse::<f64>().ok());
        match command {
            "rocket" => {
                // Stand one a few metres ahead (like `summon`).
                let frame = self.active_frame(self.time);
                let feet = self.player.body_position(&frame);
                let up = feet.normalize_or(DVec3::Y);
                let look = self.camera.forward().as_dvec3();
                let ahead = (look - up * look.dot(up)).normalize_or(up.any_orthonormal_vector());
                let distance = number(0).unwrap_or(7.0);
                let spot = (feet + ahead * distance).normalize();
                let base = Self::ground_position(&self.planet, &self.terrain, spot);
                self.flight.vessels.push(Vessel::on_pad(
                    &self.bodies,
                    self.active,
                    base,
                    self.time,
                ));
                self.net_vessel_added();
                self.console.print("Placed a rocket");
            }
            "board" => {
                let here = self.craft_world();
                let nearest = self
                    .flight
                    .vessels
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        (
                            i,
                            (v.world_position(&self.bodies, self.active, self.time) - here)
                                .length(),
                        )
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match nearest {
                    Some((i, d)) if d < 60.0 => self.board(i),
                    _ => self.console.print("No rocket nearby (`rocket` places one)"),
                }
            }
            "exit" => self.exit_vessel(),
            "goto" => {
                // goto <moon|home> [lat lon]: stand on the ground there (on the moon, by
                // default where home is overhead).
                let body = match args.first().copied() {
                    Some("moon") => Body::Moon,
                    Some("home") => Body::Planet,
                    _ => {
                        self.console.print("Usage: goto <moon|home> [lat lon]");
                        return true;
                    }
                };
                if self.flight.boarded.is_some() {
                    self.console.print("Get out of the rocket first");
                    return true;
                }
                self.flight.eva = None;
                self.switch_world(body);
                let dir = match (number(1), number(2)) {
                    (Some(lat), Some(lon)) => super::direction(lat, lon),
                    _ if body == Body::Moon => DVec3::X,
                    _ => (self.spawn_feet).normalize(),
                };
                let feet = Self::ground_position(&self.planet, &self.terrain, dir);
                let frame = self.active_frame(self.time);
                self.player.teleport(feet, &frame);
                self.reset_camera();
                self.console.print(format!("On {}", body.name()));
            }
            "lookat" if self.first_person() => {
                // lookat <moon|home|sun|rocket>: turn the first-person camera towards it.
                let target = match args.first().copied() {
                    Some("sun") => self.star_view(),
                    Some("home") => {
                        self.bodies
                            .view(self.active, Body::Planet, DVec3::ZERO, self.time)
                    }
                    Some("player") => match self.nearest_player() {
                        Some(p) => p,
                        None => return true,
                    },
                    Some(what @ ("tornado" | "volcano")) => match self.weather_target(what) {
                        Some(p) => p,
                        None => return true,
                    },
                    Some("rocket") => {
                        let eye = self.camera.position - self.planet.center;
                        let nearest = self
                            .flight
                            .vessels
                            .iter()
                            .map(|v| v.world_position(&self.bodies, self.active, self.time))
                            .min_by(|a, b| (*a - eye).length().total_cmp(&(*b - eye).length()));
                        match nearest {
                            Some(p) => p,
                            None => return true,
                        }
                    }
                    _ => self
                        .bodies
                        .view(self.active, Body::Moon, DVec3::ZERO, self.time),
                };
                let eye = self.camera.position - self.planet.center;
                let dir = (target - eye).normalize().as_vec3();
                let level = self.camera.orientation()
                    * glam::Quat::from_rotation_x(-self.camera.pitch)
                    * glam::Quat::from_rotation_y(self.camera.yaw);
                let local = level.inverse() * dir;
                self.camera.pitch = local.y.clamp(-1.0, 1.0).asin();
                self.camera.yaw = local.x.atan2(-local.z);
                self.console.print(format!(
                    "Looking {:.0} deg up",
                    self.camera.pitch.to_degrees()
                ));
            }
            "transfer" => self.plan_transfer(),
            "map" => self.toggle_map(),
            "throttle" | "sas" | "node" | "chase" | "dive" | "fuel" => {
                let Some(i) = self.flight.boarded else {
                    self.console.print("Not in a rocket");
                    return true;
                };
                match command {
                    "throttle" => {
                        self.flight.vessels[i].throttle = number(0).unwrap_or(1.0).clamp(0.0, 1.0)
                    }
                    "fuel" => {
                        let v = &mut self.flight.vessels[i];
                        if let Some(percent) = number(0) {
                            v.fuel = FUEL_CAPACITY * (percent / 100.0).clamp(0.0, 1.0);
                        }
                        let text = format!(
                            "Fuel {:.0} kg, {:.0} m/s, {:.1} m/s^2",
                            v.fuel,
                            v.delta_v(),
                            v.acceleration()
                        );
                        self.console.print(text);
                    }
                    "sas" => {
                        let v = &mut self.flight.vessels[i];
                        match args.first().copied() {
                            Some("off") => v.sas = false,
                            Some(name) => {
                                if let Some(mode) = SasMode::ALL.iter().find(|m| m.label() == name)
                                {
                                    v.sas = true;
                                    v.sas_mode = *mode;
                                }
                            }
                            None => v.sas = !v.sas,
                        }
                    }
                    "node" => {
                        let dv = DVec3::new(
                            number(0).unwrap_or(0.0),
                            number(1).unwrap_or(0.0),
                            number(2).unwrap_or(0.0),
                        );
                        let time = self.time + number(3).unwrap_or(60.0);
                        self.flight
                            .planner
                            .nodes
                            .push(trajectory::ManeuverNode { time, dv });
                        self.flight
                            .planner
                            .nodes
                            .sort_by(|a, b| a.time.total_cmp(&b.time));
                        self.flight.planner.selected = Some(0);
                    }
                    "dive" => {
                        // dive <m/s> [deg]: head down into the air (testing reentry).
                        let v = &mut self.flight.vessels[i];
                        let speed = number(0).unwrap_or(900.0);
                        let angle = number(1).unwrap_or(15.0).to_radians();
                        let up = v.position.normalize();
                        let along = (v.velocity - up * v.velocity.dot(up))
                            .try_normalize()
                            .unwrap_or_else(|| up.any_orthonormal_vector());
                        v.landed = None;
                        v.velocity = (along * angle.cos() - up * angle.sin()) * speed;
                        v.attitude =
                            glam::DQuat::from_rotation_arc(DVec3::Y, v.velocity.normalize());
                        v.spin = DVec3::ZERO;
                    }
                    _ => {
                        // chase <yaw deg> <pitch deg> <distance m>
                        let c = &mut self.flight.chase;
                        c.yaw = number(0).map_or(c.yaw, f64::to_radians);
                        c.pitch = number(1).map_or(c.pitch, f64::to_radians);
                        c.distance = number(2).unwrap_or(c.distance);
                    }
                }
            }
            "mapcam" => {
                // mapcam <yaw deg> <pitch deg> <distance km> [craft|home|moon]
                let c = &mut self.flight.map_cam;
                c.yaw = number(0).map_or(c.yaw, f64::to_radians);
                c.pitch = number(1).map_or(c.pitch, f64::to_radians);
                c.distance = number(2).map_or(c.distance, |d| d * 1000.0);
                self.flight.focus = match args.get(3).copied() {
                    Some("home") => Focus::Body(Body::Planet),
                    Some("moon") => Focus::Body(Body::Moon),
                    _ => Focus::Craft,
                };
            }
            "orbit" if self.flight.boarded.is_some() => {
                // A circular, equatorial, prograde orbit around home (or the moon).
                let i = self.flight.boarded.unwrap_or(0);
                let body = if args.get(1).copied() == Some("moon") {
                    Body::Moon
                } else {
                    Body::Planet
                };
                let altitude = number(0).unwrap_or(30.0) * 1000.0;
                let r = self.bodies.radius(body) + altitude;
                let speed = (self.bodies.mu(body) / r).sqrt();
                let eq = self.bodies.equator();
                // Start where the craft is above the equator (or anywhere on the moon).
                let (pos, _) = self.flight.vessels[i].pci(&self.bodies, self.time);
                let flat = eq.inverse() * pos;
                let angle = (-flat.z).atan2(flat.x);
                let (s, c) = angle.sin_cos();
                let v = &mut self.flight.vessels[i];
                v.landed = None;
                v.body = body;
                v.position = eq * DVec3::new(c * r, 0.0, -s * r);
                v.velocity = eq * DVec3::new(-s * speed, 0.0, -c * speed);
                v.attitude = glam::DQuat::from_rotation_arc(DVec3::Y, v.velocity.normalize());
                v.spin = DVec3::ZERO;
                v.temperature = vessel::START_TEMPERATURE;
                self.console.print(format!(
                    "Rocket in a circular orbit {:.1} km above {}, {speed:.0} m/s",
                    altitude / 1000.0,
                    body.name()
                ));
            }
            _ => return false,
        }
        true
    }
}
