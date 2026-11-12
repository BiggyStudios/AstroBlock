//! Two block worlds: home and the moon.
//!
//! The world the player (or their craft) is at is the *active* one: `Game::planet`,
//! `terrain`, `streamer` and `gravity` are its, and everything the game does with blocks
//! (walking, collision, mining, building, survival) works on it unchanged. Its rotating
//! frame is the frame the game draws in, with its centre at `PLANET_CENTER`. The player's
//! physics state is relative to it too.
//!
//! The other world sits in `Game::other`. It streams no chunks (it is far away), but keeps
//! its LOD terrain for the camera, so it can be seen: drawn turned and moved into the active
//! frame. Its survival state (placed models, chests, drops, animals) waits in
//! `OtherWorld::state`.
//!
//! The worlds swap when the player's craft (rocket, or the player floating) changes sphere
//! of influence (`follow_craft_world`).

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use glam::{DMat3, DVec3, Mat3};

use super::Game;
use crate::{
    render::DrawItem,
    space::{RotatingFrame, bodies::Body, gravity::PlanetGravity},
    survival::WorldState,
    world::{planet::Planet, streaming::WorldStreamer, terrain::TerrainGenerator},
};

pub(super) struct OtherWorld {
    pub body: Body,
    pub planet: Arc<Planet>,
    pub terrain: Arc<TerrainGenerator>,
    pub streamer: WorldStreamer,
    pub gravity: PlanetGravity,
    pub state: WorldState,
}

impl Game {
    /// The active world's rotating frame at `t` (the frame the game draws in).
    pub(super) fn active_frame(&self, t: f64) -> RotatingFrame {
        self.bodies.frame(self.active, t)
    }

    /// The star's position in the active frame (relative to the active body's centre).
    pub(super) fn star_view(&self) -> DVec3 {
        self.bodies
            .view(self.active, Body::Star, DVec3::ZERO, self.time)
    }

    /// Rotating-frame axes -> inertial axes of the active body, as a matrix (stars).
    pub(super) fn active_rotation(&self) -> Mat3 {
        DMat3::from_quat(self.active_frame(self.time).rotation).as_mat3()
    }

    /// A point given in `from`'s frame (relative to its centre) in `to`'s frame.
    fn convert_view(&self, from: Body, to: Body, p: DVec3) -> DVec3 {
        let inertial = self.bodies.frame(from, self.time).to_inertial(p);
        self.bodies.view(to, from, inertial, self.time)
    }

    /// Moves to the world of `body`, if it is the other one: swaps the worlds, and carries
    /// the player's state and the view over into the new frame.
    pub(super) fn switch_world(&mut self, body: Body) {
        if body == self.active || body != self.other.body {
            return;
        }
        let (old, t) = (self.active, self.time);
        // A milestone (and a big change): worth saving soon.
        self.save_soon();
        self.survival.apply_lights(&mut self.streamer);
        let (p, v) = self
            .bodies
            .rebase(old, body, self.player.position, self.player.velocity, t);
        self.player.position = p;
        self.player.velocity = v;
        let turn = self.bodies.view_rotation(body, old, t).as_quat();
        self.camera.rotate_frame(turn);
        let other = &mut self.other;
        std::mem::swap(&mut self.planet, &mut other.planet);
        std::mem::swap(&mut self.terrain, &mut other.terrain);
        std::mem::swap(&mut self.streamer, &mut other.streamer);
        std::mem::swap(&mut self.gravity, &mut other.gravity);
        self.survival.swap_world(&mut other.state);
        other.body = old;
        self.active = body;
        self.target = None;
        self.view_sky = None;
        self.message = Some((
            format!(
                "Arrived at {}",
                if body == Body::Moon {
                    "the moon"
                } else {
                    "home"
                }
            ),
            Instant::now(),
        ));
    }

    /// Loads the world of the body the player's craft is at.
    pub(super) fn follow_craft_world(&mut self) {
        let body = match (self.flight.boarded, &self.flight.eva, &self.flight.aboard) {
            (Some(i), _, _) => Some(self.flight.vessels[i].body),
            (None, Some(eva), _) => Some(eva.body),
            (None, None, Some(a)) => Some(self.flight.vessels[a.vessel].body),
            _ => None,
        };
        if let Some(body @ (Body::Planet | Body::Moon)) = body {
            self.switch_world(body);
        }
    }

    /// Keeps the other world's distant terrain chosen for the camera (no chunks: its focus
    /// is the camera, far away).
    pub(super) fn update_other_world(&mut self) -> Result<()> {
        let camera = self.camera.position - self.planet.center;
        let seen_from =
            self.other.planet.center + self.convert_view(self.active, self.other.body, camera);
        self.other
            .streamer
            .update(seen_from, seen_from, &mut self.renderer)
    }

    /// The other world's terrain, turned and moved into the active frame.
    pub(super) fn other_world_draws(&self) -> Vec<DrawItem> {
        let t = self.time;
        let turn = self.bodies.view_rotation(self.active, self.other.body, t);
        let offset = self
            .bodies
            .view(self.active, self.other.body, DVec3::ZERO, t);
        let turn_f = turn.as_quat();
        let center = self.other.planet.center;
        self.other
            .streamer
            .lod_draw_items()
            .into_iter()
            .chain(self.other.streamer.draw_items())
            .map(|mut d| {
                d.origin = self.planet.center + offset + turn * (d.origin - center);
                d.rotation = turn_f * d.rotation;
                d
            })
            .collect()
    }
}
