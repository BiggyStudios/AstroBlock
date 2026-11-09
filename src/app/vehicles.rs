//! Vehicles on the game's side: starting one with a core, building on it part by part,
//! climbing aboard, walking around inside and sitting down.
//!
//! - What a vehicle is and how it flies: `flight::craft` and `flight::craft_physics`. It is a
//!   `Vessel`, so the map, time warp, saving and the network all carry on working.
//! - **Aboard** means standing on a vehicle's own grid. You move in its coordinates, with
//!   "down" always its deck's -Y, so it can roll, climb or orbit with you inside. Walking off
//!   the edge puts you back in the world (falling, with the vehicle's speed).
//! - The first to sit in a seat flies it. Anyone else who sits or stands aboard rides along,
//!   and their clock follows the pilot's (see `timeline::keep_ride`).

use std::time::Instant;

use glam::{DMat3, DQuat, DVec3};
use serde::{Deserialize, Serialize};
use winit::{event::MouseButton, keyboard::KeyCode};

use super::{Game, MAX_PHYSICS_WARP, WARP_LEVELS};
use crate::{
    audio::Sound,
    effects,
    flight::{
        craft::{self, At, Craft, Part, direction, facing_of},
        craft_model,
        vessel::Vessel,
    },
    net::protocol::{AboardPose, CraftChange, Msg},
    player::{GameMode, HALF_WIDTH, HEIGHT, PlayerInput},
    render::{DrawItem, Fade, ParticleEmitter, models::Light},
    survival::items::{self, Item, ItemStack},
    world::block::{Block, Material},
};

/// Walking aboard: speeds (m/s), accelerations (m/s^2), a jump, and the deck's pull, which is
/// the walking world's whatever the vehicle is doing.
const WALK: f64 = 4.3;
const SPRINT: f64 = 5.6;
const ACCEL: f64 = 50.0;
const AIR_ACCEL: f64 = 10.0;
const JUMP: f64 = 7.67;
const DECK_GRAVITY: f64 = 9.81 * 2.4;
/// How fast you can be going relative to a vehicle and still land on it (m/s).
const BOARDING_SPEED: f64 = 15.0;
/// Eye height while sitting (m).
const SEATED_EYE: f64 = 1.05;
/// Survival: if a part has no break time of its own.
const MIN_BREAK: f32 = 0.15;

/// Standing or sitting on a vehicle.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Aboard {
    /// Index into `Flight::vessels`.
    pub vessel: usize,
    /// Feet, in the vehicle's grid coordinates.
    pub feet: DVec3,
    /// Grid axes.
    pub velocity: DVec3,
    pub on_floor: bool,
    pub seat: Option<At>,
}

/// What the crosshair is on.
#[derive(Clone, Copy)]
pub(super) struct CraftHit {
    pub vessel: usize,
    pub at: At,
    /// The side it was hit on (a direction index).
    pub face: u8,
    pub distance: f64,
}

fn at_offset(at: At, face: u8) -> At {
    let d = direction(face);
    [at[0] + d.x as i16, at[1] + d.y as i16, at[2] + d.z as i16]
}

/// A few ready-made vehicles, for trying things out (`vehicle plane`).
pub(super) fn preset(name: &str) -> Option<Craft> {
    let mut c = Craft::new(4);
    let place = |c: &mut Craft, at: At, block: Block, facing: DVec3| {
        c.place(Part {
            at,
            block,
            facing: facing_of(facing),
        });
    };
    match name {
        "rover" => {
            for (x, z) in [
                (1, 0),
                (-1, 0),
                (1, 1),
                (-1, 1),
                (0, 1),
                (1, 2),
                (-1, 2),
                (0, 2),
            ] {
                place(&mut c, [x, 0, z], Block::Hull, DVec3::X);
            }
            place(&mut c, [0, 1, 1], Block::Seat, DVec3::Z);
            place(&mut c, [1, 1, 0], Block::Seat, DVec3::Z);
            place(&mut c, [-1, 1, 0], Block::Seat, DVec3::Z);
            for (x, z) in [(2, 0), (-2, 0), (2, 2), (-2, 2)] {
                place(&mut c, [x, 0, z], Block::Wheel, DVec3::X);
            }
        }
        "plane" => {
            for z in 1..=6 {
                place(&mut c, [0, 0, z], Block::Hull, DVec3::X);
            }
            for z in (-3..=-1).rev() {
                place(&mut c, [0, 0, z], Block::Hull, DVec3::X);
            }
            for x in 1..=7 {
                // The outer ends are the ailerons.
                let block = if x >= 5 { Block::Flap } else { Block::Wing };
                for z in 2..=3 {
                    place(&mut c, [x, 0, z], block, DVec3::Y);
                    place(&mut c, [-x, 0, z], block, DVec3::Y);
                }
            }
            for x in 1..=3 {
                place(&mut c, [x, 0, -3], Block::Flap, DVec3::Y);
                place(&mut c, [-x, 0, -3], Block::Flap, DVec3::Y);
            }
            place(&mut c, [0, 1, -3], Block::Flap, DVec3::X);
            place(&mut c, [0, 2, -3], Block::Flap, DVec3::X);
            place(&mut c, [0, 1, 1], Block::Seat, DVec3::Z);
            place(&mut c, [0, 1, 3], Block::FuelTank, DVec3::X);
            place(&mut c, [0, 0, 7], Block::Propeller, DVec3::NEG_Z);
            place(&mut c, [0, -1, 5], Block::LandingGear, DVec3::X);
            for x in [1, -1] {
                place(&mut c, [x, 0, 1], Block::Hull, DVec3::X);
                place(&mut c, [x, -1, 1], Block::LandingGear, DVec3::X);
            }
        }
        "rocket" => {
            c = Craft::new(2);
            for y in 1..=8 {
                place(&mut c, [0, y, 0], Block::FuelTank, DVec3::X);
            }
            place(&mut c, [0, 9, 0], Block::Seat, DVec3::Z);
            place(&mut c, [1, 4, 0], Block::Gyro, DVec3::X);
            place(&mut c, [0, -1, 0], Block::RocketEngine, DVec3::NEG_Y);
            for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let side = DVec3::new(x as f64, 0.0, z as f64);
                let across = DVec3::new(-side.z, 0.0, side.x);
                place(&mut c, [x, 0, z], Block::Flap, across);
            }
        }
        _ => return None,
    }
    Some(c)
}

/// How high a vehicle's core must sit for its lowest part to rest on the ground (m).
fn rest_height(c: &Craft) -> f64 {
    c.parts()
        .iter()
        .map(|p| {
            let reach = match p.block {
                Block::Wheel | Block::LandingGear => craft::WHEEL_RADIUS,
                _ => 0.5,
            };
            reach - p.at[1] as f64
        })
        .fold(0.5, f64::max)
}

impl Game {
    /// Where a vehicle's grid is, in the frame the game draws in: its origin and the rotation
    /// taking grid axes into it.
    pub(super) fn grid_pose(&self, v: &Vessel) -> (DVec3, DQuat) {
        let frame = self.active_frame(self.time);
        let to_world = (frame.rotation.inverse() * v.attitude).normalize();
        let com = v.world_position(&self.bodies, self.active, self.time);
        (com - to_world * v.com(), to_world)
    }

    /// Puts a vehicle in the world, and tells the others.
    fn add_vehicle(&mut self, craft: Craft, core: DVec3, rotation: DQuat) {
        self.flight.vessels.push(Vessel::built(
            &self.bodies,
            self.active,
            core,
            rotation,
            self.time,
            craft,
        ));
        self.net_vessel_added();
    }

    /// The axes of a new vehicle: up where it stands, its front the way the player looks.
    fn level_axes(&self, up: DVec3) -> (DQuat, bool) {
        let look = self.camera.forward().as_dvec3();
        let along = (look - up * look.dot(up)).normalize_or(up.any_orthonormal_vector());
        let x = up.cross(along);
        let rotation = DQuat::from_mat3(&DMat3::from_cols(x, up, along)).normalize();
        (rotation, look.dot(up).abs() > 0.75)
    }

    /// A right click with a vehicle core on top of a block starts a vehicle there. Returns
    /// whether the click was used.
    pub(super) fn place_core(&mut self, clicked: bool) -> bool {
        let holding = self
            .survival
            .held()
            .is_some_and(|s| s.item == Item::Block(Block::VehicleCore));
        if !holding || !clicked {
            return false;
        }
        let Some(hit) = self.target.as_ref() else {
            return false;
        };
        let cell = hit.cell;
        let above = self.planet.cell(cell.face, cell.u, cell.v, cell.k + 1);
        if hit.before != above || !hit.block.collides() {
            self.say("Start a vehicle on top of a block");
            return true;
        }
        let n = self.planet.res_at(cell.k);
        let up = self
            .planet
            .dir(cell.face, cell.u as f64 + 0.5, cell.v as f64 + 0.5, n);
        let core = up * (self.planet.layer_radius(cell.k as f64 + 1.0) + 0.5);
        let (rotation, upright) = self.level_axes(up);
        // Looking up or down, it is going to fly that way: a rocket.
        let craft = Craft::new(if upright { 2 } else { 4 });
        self.add_vehicle(craft, core, rotation);
        if self.player.mode != GameMode::Creative {
            self.survival.inventory.take_one(self.survival.selected);
        }
        self.survival.uses += 1;
        self.survival
            .sounds
            .push((Sound::Place(Material::Stone), core));
        self.say("A new vehicle: build onto the core. R turns engines and wings");
        true
    }

    fn say(&mut self, text: &str) {
        self.message = Some((text.to_string(), Instant::now()));
    }

    /// The part under the crosshair, if nearer than the targeted block.
    pub(super) fn craft_target(&self, reach: f64) -> Option<CraftHit> {
        let origin = self.camera.position - self.planet.center;
        let dir = self.camera.forward().as_dvec3();
        let block = self.target.as_ref().map_or(f64::MAX, |h| h.distance);
        let sitting = self
            .flight
            .aboard
            .as_ref()
            .is_some_and(|a| a.seat.is_some());
        if sitting {
            return None;
        }
        let mut best: Option<CraftHit> = None;
        for (i, v) in self.flight.vessels.iter().enumerate() {
            let Some(craft) = &v.craft else { continue };
            let (grid, to_world) = self.grid_pose(v);
            if (origin - grid).length() > craft.stats.radius + reach + 1.0 {
                continue;
            }
            let back = to_world.inverse();
            let Some((at, face, distance)) = craft.ray(back * (origin - grid), back * dir, reach)
            else {
                continue;
            };
            if distance < block && best.is_none_or(|b| distance < b.distance) {
                best = Some(CraftHit {
                    vessel: i,
                    at,
                    face,
                    distance,
                });
            }
        }
        best
    }

    /// Breaking and placing parts, and sitting down.
    pub(super) fn craft_clicks(&mut self, hit: CraftHit, dt: f32) {
        let Some(part) = self.flight.vessels[hit.vessel]
            .craft
            .as_ref()
            .and_then(|c| c.get(hit.at))
        else {
            return;
        };
        self.survival.target_name = Some(part.block.name());
        let creative = self.player.mode == GameMode::Creative;

        if self.input.button_held(MouseButton::Left) {
            let total = if creative {
                0.0
            } else {
                items::break_time(part.block, self.survival.tool()).max(MIN_BREAK)
            };
            let spent = match self.flight.digging {
                Some((v, at, spent)) if v == hit.vessel && at == hit.at => spent + dt,
                _ => dt,
            };
            let ready = if creative {
                self.input.button_pressed(MouseButton::Left)
            } else {
                spent >= total
            };
            if self.input.button_pressed(MouseButton::Left) {
                self.survival.swings += 1;
            }
            if ready {
                self.flight.digging = None;
                self.break_vehicle_part(hit);
            } else {
                self.flight.digging = Some((hit.vessel, hit.at, spent));
                self.survival.mining = None;
            }
        } else {
            self.flight.digging = None;
        }

        if !self.input.button_pressed(MouseButton::Right) {
            return;
        }
        if part.block == Block::Seat && !self.input.held(KeyCode::ShiftLeft) {
            self.sit_down(hit.vessel, hit.at);
            return;
        }
        self.place_vehicle_part(hit);
    }

    fn break_vehicle_part(&mut self, hit: CraftHit) {
        let v = &mut self.flight.vessels[hit.vessel];
        if v.pilot.is_some() && hit.at == [0, 0, 0] {
            self.say("Someone is flying it");
            return;
        }
        let id = v.id;
        let mut removed = craft::Removed::default();
        v.rebuild_craft(|c| removed = c.remove(hit.at));
        let (grid, to_world) = self.grid_pose(&self.flight.vessels[hit.vessel]);
        let at = grid + to_world * craft::center(hit.at);
        self.survival
            .sounds
            .push((Sound::Break(Material::Stone), at));
        if self.player.mode != GameMode::Creative {
            let parts = removed.parts.iter().map(|p| p.block);
            for block in parts {
                let stack = ItemStack::new(Item::Block(block), 1);
                if let Some(rest) = self.survival.inventory.add(stack) {
                    self.survival.spawn(rest, at, DVec3::ZERO, 0.0);
                }
            }
        }
        if removed.core_gone {
            self.net_vessel_gone(id, None);
            self.remove_vessel(hit.vessel);
            self.say("The vehicle came apart");
            return;
        }
        self.send_craft_change(id, CraftChange::Take(hit.at));
    }

    fn place_vehicle_part(&mut self, hit: CraftHit) {
        let Some(stack) = self.survival.held() else {
            return;
        };
        let Some(block) = stack.item.block() else {
            return;
        };
        if block.is_model() || matches!(block, Block::Air | Block::Water | Block::VehicleCore) {
            self.say("That doesn't go on a vehicle");
            return;
        }
        let at = at_offset(hit.at, hit.face);
        let outward = direction(hit.face);
        let v = &self.flight.vessels[hit.vessel];
        let Some(craft) = &v.craft else { return };
        if !craft.can_place(at) {
            self.say("Too far from the core");
            return;
        }
        // Not inside whoever is placing it.
        let (grid, to_world) = self.grid_pose(v);
        let frame = self.active_frame(self.time);
        let feet = to_world.inverse() * (frame.to_body(self.player.position) - grid);
        let center = craft::center(at);
        let inside = (feet.x - center.x).abs() < 0.5 + HALF_WIDTH
            && (feet.z - center.z).abs() < 0.5 + HALF_WIDTH
            && feet.y < center.y + 0.5
            && feet.y + HEIGHT > center.y - 0.5;
        if inside && craft::is_walkable_solid(block) {
            return;
        }
        let facing = self.part_facing(block, outward, craft);
        let part = Part { at, block, facing };
        let v = &mut self.flight.vessels[hit.vessel];
        let id = v.id;
        let mut placed = false;
        v.rebuild_craft(|c| placed = c.place(part));
        if !placed {
            return;
        }
        if self.player.mode != GameMode::Creative {
            self.survival.inventory.take_one(self.survival.selected);
        }
        self.survival.uses += 1;
        let (grid, to_world) = self.grid_pose(&self.flight.vessels[hit.vessel]);
        self.survival
            .sounds
            .push((Sound::Place(block.material()), grid + to_world * center));
        self.send_craft_change(id, CraftChange::Put(part));
    }

    /// The way a new part faces, unless R has said otherwise.
    fn part_facing(&self, block: Block, outward: DVec3, craft: &Craft) -> u8 {
        match self.flight.part_turn {
            Some(turn) if craft::can_turn(block) => turn,
            _ => craft::auto_facing(block, outward, &craft.stats),
        }
    }

    pub(super) fn send_craft_change(&mut self, vessel: u32, change: CraftChange) {
        if self.net.in_session() {
            self.send_everyone(&Msg::CraftEdit { vessel, change }, true);
        }
    }

    /// News of someone changing a vehicle.
    pub(super) fn apply_craft_change(&mut self, vessel: u32, change: CraftChange) {
        let Some(v) = self.flight.vessels.iter_mut().find(|v| v.id == vessel) else {
            return;
        };
        match change {
            CraftChange::Put(part) => {
                v.rebuild_craft(|c| {
                    c.place(part);
                });
            }
            CraftChange::Take(at) => {
                let mut gone = false;
                v.rebuild_craft(|c| gone = c.remove(at).core_gone);
                let _ = gone;
            }
        }
    }

    /// The console's `place` and `break`, on a vehicle (a test without a mouse). Returns
    /// whether there was one to act on.
    pub(super) fn console_vehicle_click(&mut self, place: bool) -> bool {
        if place && self.place_core(true) {
            self.console.print("Started a vehicle");
            return true;
        }
        let Some(hit) = self.craft_target(8.0) else {
            return false;
        };
        let part = self.flight.vessels[hit.vessel]
            .craft
            .as_ref()
            .and_then(|c| c.get(hit.at));
        if place {
            match part {
                Some(p) if p.block == Block::Seat => self.sit_down(hit.vessel, hit.at),
                _ => self.place_vehicle_part(hit),
            }
        } else {
            self.break_vehicle_part(hit);
        }
        let text = match self
            .flight
            .vessels
            .get(hit.vessel)
            .and_then(|v| v.craft.as_ref())
        {
            Some(c) => format!(
                "Vehicle: {} parts, {:.0} kg, forward {:?}",
                c.parts().len(),
                c.stats.dry_mass,
                c.stats.forward
            ),
            None => "The vehicle is gone".to_string(),
        };
        self.console.print(text);
        true
    }

    /// R, holding a part that can point: which way the next one goes.
    pub(super) fn turn_part(&mut self) {
        let turning = self
            .survival
            .held()
            .and_then(|s| s.item.block())
            .is_some_and(craft::can_turn);
        if !turning {
            return;
        }
        self.flight.part_turn = match self.flight.part_turn {
            None => Some(0),
            Some(5) => None,
            Some(n) => Some(n + 1),
        };
        let text = match self.flight.part_turn {
            None => "Parts face by where you place them".to_string(),
            Some(n) => {
                const NAMES: [&str; 6] = ["+x", "-x", "up", "down", "+z", "-z"];
                format!("Next part faces {}", NAMES[n as usize])
            }
        };
        self.say(&text);
    }

    /// Sits in a seat: flying it if nobody is, otherwise along for the ride.
    pub(super) fn sit_down(&mut self, i: usize, seat: At) {
        let v = &self.flight.vessels[i];
        let (id, pilot) = (v.id, v.pilot);
        let taken = self.net.players.values().any(|r| {
            r.aboard_pose()
                .is_some_and(|a| a.vessel == id && a.seat && a.feet == seat_feet(seat).as_vec3())
        });
        if taken {
            self.say("Someone is sitting there");
            return;
        }
        self.flight.digging = None;
        if pilot.is_none() && self.player.mode != GameMode::Spectator {
            self.flight.seat = Some(seat);
            if self.request_seat(i) {
                self.enter_vessel(i);
            }
            return;
        }
        self.flight.aboard = Some(Aboard {
            vessel: i,
            feet: seat_feet(seat),
            velocity: DVec3::ZERO,
            on_floor: true,
            seat: Some(seat),
        });
        self.say("Along for the ride: f to stand up");
    }

    /// Gets up from flying: stands by the seat, still aboard.
    pub(super) fn stand_from_controls(&mut self, i: usize) {
        let craft = self.flight.vessels[i].craft.as_ref();
        let seat = self
            .flight
            .seat
            .or_else(|| craft.and_then(|c| c.stats.seats.first().copied()))
            .unwrap_or([0, 1, 0]);
        self.flight.aboard = Some(Aboard {
            vessel: i,
            feet: seat_feet(seat),
            velocity: DVec3::ZERO,
            on_floor: true,
            seat: None,
        });
        self.flight.turned_with = None;
        self.place_aboard();
    }

    /// Time, walking and sitting for someone aboard.
    pub(super) fn aboard_physics(&mut self, input: &PlayerInput, dt: f64) {
        let Some(mut a) = self.flight.aboard.clone() else {
            return;
        };
        let v = &self.flight.vessels[a.vessel];
        let mut warp = WARP_LEVELS[self.warp];
        if warp > MAX_PHYSICS_WARP && !v.can_rail_warp(&self.bodies) {
            warp = MAX_PHYSICS_WARP;
        }
        self.effective_warp = warp;
        self.time += dt * warp;

        if a.seat.is_some() {
            if self.input.pressed(KeyCode::KeyF) {
                a.seat = None;
                self.flight.aboard = Some(a);
            }
            return;
        }
        let Some(craft) = &v.craft else {
            self.flight.aboard = None;
            return;
        };
        let (_, to_world) = self.grid_pose(v);
        let mut walk = to_world.inverse() * input.walk;
        walk.y = 0.0;
        let walk = walk.normalize_or_zero();
        let speed = if input.sprint { SPRINT } else { WALK };
        let accel = if a.on_floor { ACCEL } else { AIR_ACCEL };
        let flat = DVec3::new(a.velocity.x, 0.0, a.velocity.z);
        let flat = crate::player::approach(flat, walk * speed, accel * dt);
        a.velocity.x = flat.x;
        a.velocity.z = flat.z;
        if input.jump && a.on_floor {
            a.velocity.y = JUMP;
            a.on_floor = false;
        }
        a.velocity.y -= DECK_GRAVITY * dt;
        let target = a.feet + a.velocity * dt;
        let (feet, bumped) = craft.sweep(a.feet, target, HALF_WIDTH, HEIGHT);
        for axis in 0..3 {
            if bumped.axes[axis] {
                a.velocity[axis] = 0.0;
            }
        }
        a.feet = feet;
        a.on_floor = bumped.floor;
        let off = !a.on_floor && !craft.over_something(a.feet, HALF_WIDTH);
        self.flight.aboard = Some(a);
        // Walked off the side: gravity and the world take over, once `place_aboard` has put
        // the player where the vehicle is at this frame's time.
        self.flight.stepping_off = off;
    }

    /// Puts the player where the vehicle has carried them (after it has moved).
    pub(super) fn place_aboard(&mut self) {
        let Some(a) = &self.flight.aboard else {
            self.flight.turned_with = None;
            self.flight.stepping_off = false;
            return;
        };
        let (vessel, feet, relative, on_floor) = (a.vessel, a.feet, a.velocity, a.on_floor);
        let v = &self.flight.vessels[vessel];
        let inertial = v.grid_to_inertial(feet);
        let moving = v.point_velocity(feet) + v.attitude * relative;
        let (pos, vel) = self
            .bodies
            .rebase(v.body, self.active, inertial, moving, self.time);
        let (_, to_world) = self.grid_pose(v);
        self.player.position = pos;
        self.player.velocity = vel;
        self.player.on_ground = on_floor;
        self.player.flying = false;
        // Turn with it, so what you look at inside stays put.
        let now = to_world.as_quat();
        if let Some(before) = self.flight.turned_with {
            self.camera.rotate_frame(now * before.inverse());
        }
        self.flight.turned_with = Some(now);
        // (Only now: left any earlier, the player would stay at last frame's position, which
        // the planet's turn has carried a metre or more from where the deck is.)
        if std::mem::take(&mut self.flight.stepping_off) {
            self.flight.aboard = None;
            self.player.on_ground = false;
        }
    }

    /// Where the feet are on each vehicle's grid (before this frame's physics), one entry per
    /// vessel in order, for `meet_vehicles`. (Not by id: they are all 0 outside a shared game.)
    pub(super) fn feet_in_grids(&self) -> Vec<Option<DVec3>> {
        let feet = self.player.body_position(&self.active_frame(self.time));
        self.flight
            .vessels
            .iter()
            .map(|v| {
                v.craft.as_ref().map(|_| {
                    let (grid, to_world) = self.grid_pose(v);
                    to_world.inverse() * (feet - grid)
                })
            })
            .collect()
    }

    /// Creative flight, on or off. Taking off from a vehicle leaves it.
    pub(super) fn toggle_flight(&mut self) {
        self.player.toggle_flight();
        if self.player.flying {
            self.flight.aboard = None;
            self.player.on_ground = false;
        }
    }

    /// On foot: vehicles are solid, and landing on one puts you aboard. `before` is from
    /// `feet_in_grids`. This has to run after the vehicles have moved to this frame's time:
    /// a parked one turns with the planet, over a metre per frame, and against last frame's
    /// position you would collide with a ghost beside it and jump across when landing.
    pub(super) fn meet_vehicles(&mut self, before: &[Option<DVec3>]) {
        if self.player.mode == GameMode::Spectator || self.flight.vessels.is_empty() {
            return;
        }
        // (One was destroyed just now, so the list no longer lines up: next frame's will.)
        if before.len() != self.flight.vessels.len() {
            return;
        }
        let frame = self.active_frame(self.time);
        let after = frame.to_body(self.player.position);
        let radial = after.normalize_or(DVec3::Y);
        for (i, from) in before.iter().enumerate() {
            let v = &self.flight.vessels[i];
            let Some(craft) = &v.craft else { continue };
            let Some(from) = *from else { continue };
            let (grid, to_world) = self.grid_pose(v);
            if (after - grid).length() > craft.stats.radius + 4.0 {
                continue;
            }
            let back = to_world.inverse();
            let to = back * (after - grid);
            let (reached, bumped) = craft.sweep(from, to, HALF_WIDTH, HEIGHT);
            if !bumped.axes.iter().any(|&b| b) {
                continue;
            }
            let world = grid + to_world * reached;
            let relative = frame.relative_velocity(self.player.position, self.player.velocity);
            let mut in_grid = back * relative;
            for axis in 0..3 {
                if bumped.axes[axis] {
                    in_grid[axis] = 0.0;
                }
            }
            self.player.position = frame.to_inertial(world);
            self.player.velocity = frame.inertial_velocity(world, to_world * in_grid);
            let upright = (to_world * DVec3::Y).dot(radial) > 0.5;
            // Flying, a vehicle is only something to bump into: to land, stop flying.
            let landing = !self.player.flying;
            if bumped.floor && landing && upright && in_grid.length() < BOARDING_SPEED {
                self.flight.aboard = Some(Aboard {
                    vessel: i,
                    feet: reached,
                    velocity: DVec3::new(in_grid.x, 0.0, in_grid.z),
                    on_floor: true,
                    seat: None,
                });
                self.player.on_ground = true;
                return;
            }
            return;
        }
    }

    /// How far above the feet the eyes are right now.
    pub(super) fn eye_height(&self) -> f64 {
        if let Some(death) = &self.death {
            return death.eye_height();
        }
        match &self.flight.aboard {
            Some(a) if a.seat.is_some() => SEATED_EYE,
            _ => crate::player::EYE_HEIGHT,
        }
    }

    /// What the others are told about where this player is aboard.
    pub(super) fn aboard_pose(&self) -> Option<AboardPose> {
        if let Some(i) = self.flight.boarded {
            let v = &self.flight.vessels[i];
            let craft = v.craft.as_ref()?;
            let seat = self
                .flight
                .seat
                .or_else(|| craft.stats.seats.first().copied())?;
            return Some(AboardPose {
                vessel: v.id,
                feet: seat_feet(seat).as_vec3(),
                seat: true,
            });
        }
        let a = self.flight.aboard.as_ref()?;
        Some(AboardPose {
            vessel: self.flight.vessels[a.vessel].id,
            feet: a.feet.as_vec3(),
            seat: a.seat.is_some(),
        })
    }

    /// Where someone aboard (by what they last said) is in the world.
    pub(super) fn aboard_world(&self, pose: &AboardPose) -> Option<(DVec3, DVec3)> {
        let v = self.flight.vessels.iter().find(|v| v.id == pose.vessel)?;
        let (grid, to_world) = self.grid_pose(v);
        Some((grid + to_world * pose.feet.as_dvec3(), to_world * DVec3::Y))
    }

    /// One vehicle's mesh (rebuilt when its parts change), and the outline on the part
    /// the crosshair is on.
    pub(super) fn vehicle_draw_item(
        &mut self,
        i: usize,
        objects: &mut Vec<DrawItem>,
    ) -> anyhow::Result<()> {
        let v = &self.flight.vessels[i];
        let Some(craft) = &v.craft else {
            return Ok(());
        };
        let (grid, to_world) = self.grid_pose(v);
        let radius = craft.stats.radius as f32 + 2.0;
        if v.mesh_version != craft.version || v.mesh.is_none() {
            let version = craft.version;
            let mesh = craft_model::craft_model(craft, Light { sky: 15, block: 0 });
            if let Some(handle) = self.renderer.create_mesh(&mesh)? {
                let v = &mut self.flight.vessels[i];
                if let Some(old) = v.mesh.replace(handle) {
                    self.renderer.destroy_mesh(old);
                }
                v.mesh_version = version;
            }
        }
        if let Some(mesh) = self.flight.vessels[i].mesh {
            objects.push(DrawItem {
                mesh,
                origin: self.planet.center + grid,
                rotation: to_world.as_quat(),
                radius,
                fade: Fade::Visible,
            });
        }
        Ok(())
    }

    /// The outline on the targeted part.
    pub(super) fn vehicle_outline(&mut self, objects: &mut Vec<DrawItem>) -> anyhow::Result<()> {
        let Some(hit) = self.flight.pointed else {
            return Ok(());
        };
        let Some(v) = self.flight.vessels.get(hit.vessel) else {
            return Ok(());
        };
        let (grid, to_world) = self.grid_pose(v);
        if self.flight.outline_for != Some(hit.at) || self.flight.outline.is_none() {
            let mesh = craft_model::outline(hit.at, Light { sky: 15, block: 15 });
            if let Some(handle) = self.renderer.create_mesh(&mesh)? {
                if let Some(old) = self.flight.outline.replace(handle) {
                    self.renderer.destroy_mesh(old);
                }
                self.flight.outline_for = Some(hit.at);
            }
        }
        if let Some(mesh) = self.flight.outline {
            objects.push(DrawItem {
                mesh,
                origin: self.planet.center + grid,
                rotation: to_world.as_quat(),
                radius: 80.0,
                fade: Fade::Visible,
            });
        }
        Ok(())
    }

    /// Flames from each engine that is running.
    pub(super) fn vehicle_flames(&mut self, i: usize, dt: f32, out: &mut Vec<ParticleEmitter>) {
        let v = &self.flight.vessels[i];
        let Some(craft) = &v.craft else { return };
        if v.throttle <= 0.0 || v.fuel <= 0.0 {
            return;
        }
        let t = self.time;
        let (grid, to_world) = self.grid_pose(v);
        let speed = self
            .bodies
            .view_velocity(self.active, v.body, v.position, v.velocity, t);
        let throttle = v.throttle as f32;
        let n = ((180.0 + 260.0 * throttle) * dt).ceil() as u32;
        let mut sound_at = None;
        for e in craft.stats.engines.iter().take(10) {
            if matches!(e.kind.air, craft::AirUse::Propeller(_)) {
                sound_at.get_or_insert(grid + to_world * e.position);
                continue;
            }
            let nozzle = grid + to_world * (e.position - e.push * 0.8);
            let back = to_world * -e.push;
            out.extend(effects::exhaust(nozzle, back, speed, n));
            sound_at.get_or_insert(nozzle);
        }
        if let Some(at) = sound_at {
            self.flight.engine_timer -= dt;
            if self.flight.engine_timer <= 0.0 {
                self.flight.engine_timer = 0.42;
                let listener = self.camera.position - self.planet.center;
                let right = self.camera.right().as_dvec3();
                self.audio
                    .play_gain(Sound::Engine, at - listener, right, 0.35 + 0.65 * throttle);
            }
        }
    }

    /// `vehicle <plane|rover|rocket>`: a ready-made one a few metres ahead.
    pub(super) fn spawn_preset(&mut self, name: &str, distance: f64) {
        let Some(craft) = preset(name) else {
            self.console.print("vehicle <plane|rover|rocket> [m]");
            return;
        };
        let frame = self.active_frame(self.time);
        let feet = self.player.body_position(&frame);
        let up = feet.normalize_or(DVec3::Y);
        let look = self.camera.forward().as_dvec3();
        let ahead = (look - up * look.dot(up)).normalize_or(up.any_orthonormal_vector());
        let spot = (feet + ahead * distance).normalize();
        let ground = Self::ground_position(&self.planet, &self.terrain, spot);
        let up = ground.normalize_or(DVec3::Y);
        let (rotation, _) = self.level_axes(up);
        let height = rest_height(&craft);
        self.add_vehicle(craft, ground + up * height, rotation);
        self.console.print(format!("Placed a {name}"));
    }

    /// Faces the nearest vehicle's seat from its side, then sits (a console test).
    pub(super) fn sit_in_nearest(&mut self) {
        let here = self.player.body_position(&self.active_frame(self.time));
        let nearest = (0..self.flight.vessels.len())
            .filter(|&i| {
                self.flight.vessels[i]
                    .craft
                    .as_ref()
                    .is_some_and(|c| !c.stats.seats.is_empty())
            })
            .min_by(|&a, &b| {
                let d = |i: usize| {
                    (self.flight.vessels[i].world_position(&self.bodies, self.active, self.time)
                        - here)
                        .length()
                };
                d(a).total_cmp(&d(b))
            });
        match nearest {
            Some(i) => {
                let craft = self.flight.vessels[i].craft.as_ref();
                let seats = craft.map_or(Vec::new(), |c| c.stats.seats.clone());
                let free = seats.into_iter().find(|s| {
                    !self.net.players.values().any(|r| {
                        r.aboard_pose()
                            .is_some_and(|a| a.seat && a.feet == seat_feet(*s).as_vec3())
                    })
                });
                if let Some(seat) = free {
                    self.sit_down(i, seat);
                }
            }
            None => self.console.print("No seat nearby"),
        }
    }

    /// Stands the player on the nearest vehicle's deck (a console test).
    pub(super) fn step_aboard(&mut self, status: bool) {
        if status {
            let text = match &self.flight.aboard {
                Some(a) => format!(
                    "Aboard vehicle {}: feet {:.2}, on floor {}, seat {:?}",
                    a.vessel, a.feet, a.on_floor, a.seat
                ),
                None => "Not aboard".to_string(),
            };
            self.console.print(text);
            return;
        }
        let here = self.player.body_position(&self.active_frame(self.time));
        let nearest = (0..self.flight.vessels.len())
            .filter(|&i| self.flight.vessels[i].craft.is_some())
            .min_by(|&a, &b| {
                let d = |i: usize| {
                    (self.flight.vessels[i].world_position(&self.bodies, self.active, self.time)
                        - here)
                        .length()
                };
                d(a).total_cmp(&d(b))
            });
        let Some(i) = nearest else {
            self.console.print("No vehicle nearby");
            return;
        };
        let top = self.flight.vessels[i]
            .craft
            .as_ref()
            .and_then(|c| c.parts().iter().map(|p| p.at[1]).max())
            .unwrap_or(0) as f64;
        self.flight.aboard = Some(Aboard {
            vessel: i,
            feet: DVec3::new(0.0, top + 0.6, 0.0),
            velocity: DVec3::ZERO,
            on_floor: false,
            seat: None,
        });
        self.console.print("Aboard");
    }
}

/// Where the feet are, sitting in `seat`.
pub(super) fn seat_feet(seat: At) -> DVec3 {
    craft::center(seat) - DVec3::Y * 0.3
}

/// The cockpit's view (`Flight::cockpit`), for the pilot: which way is ahead and up.
pub(super) fn cockpit_turn(yaw: f64, pitch: f64, up: DVec3, right: DVec3) -> DQuat {
    DQuat::from_axis_angle(up, -yaw) * DQuat::from_axis_angle(right, pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::craft::Stats;

    #[test]
    fn the_ready_made_vehicles_are_whole() {
        for name in ["rover", "plane", "rocket"] {
            let c = preset(name).expect(name);
            let s: &Stats = &c.stats;
            assert!(s.has_core);
            assert!(!s.seats.is_empty(), "{name} has a seat");
            assert!(c.parts().len() > 8, "{name}");
        }
        let rover = preset("rover").unwrap();
        assert_eq!(rover.stats.wheels.len(), 4);
        assert!(rover.stats.wheels.iter().all(|w| w.driven));
        let plane = preset("plane").unwrap();
        assert_eq!(plane.stats.engines.len(), 1);
        assert!(plane.stats.surfaces.len() > 20);
        assert_eq!(plane.stats.forward, DVec3::Z);
        let rocket = preset("rocket").unwrap();
        assert_eq!(rocket.stats.forward, DVec3::Y);
    }

    #[test]
    fn a_rover_rests_on_its_wheels() {
        let rover = preset("rover").unwrap();
        assert!((rest_height(&rover) - craft::WHEEL_RADIUS).abs() < 1e-9);
    }
}
