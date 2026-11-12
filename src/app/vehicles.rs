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
    player::{GameMode, HALF_WIDTH, HEIGHT, PlayerInput, interact},
    render::{DrawItem, Fade, ParticleEmitter, models::Light},
    survival::items::{self, Item, ItemStack},
    world::{
        block::{Block, DoorKind, Material},
        chunk::BlockCell,
    },
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
    // Parts that may be listed before what they hold on to: placed as they can be, out from
    // the core.
    let grow = |c: &mut Craft, cells: &[At], block: Block, facing: DVec3| {
        let mut left: Vec<At> = cells.to_vec();
        loop {
            let before = left.len();
            left.retain(|&at| {
                !c.place(Part {
                    at,
                    block,
                    facing: facing_of(facing),
                })
            });
            if left.len() == before {
                break;
            }
        }
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
            // Its wheels run on electricity: a battery, and panels on the back to charge it.
            place(&mut c, [0, 1, 2], Block::Accumulator, DVec3::NEG_Z);
            place(&mut c, [1, 1, 2], Block::SolarPanel, DVec3::Y);
            place(&mut c, [-1, 1, 2], Block::SolarPanel, DVec3::Y);
        }
        "digger" => {
            // A rover with a wall of drills in front: it bores a tunnel three wide and three
            // high. A generator burns the tanks' fuel to keep the batteries up.
            let mut deck = Vec::new();
            for x in -1..=1 {
                for z in -1..=3 {
                    if [x, z] != [0, 0] {
                        deck.push([x, 0, z]);
                    }
                }
                deck.push([x, 1, 3]);
                deck.push([x, 2, 3]);
            }
            grow(&mut c, &deck, Block::Hull, DVec3::X);
            let drills: Vec<At> = (-1..=1)
                .flat_map(|x| (0..=2).map(move |y| [x, y, 4]))
                .collect();
            grow(&mut c, &drills, Block::VehicleDrill, DVec3::Z);
            place(&mut c, [0, 1, 1], Block::Seat, DVec3::Z);
            place(&mut c, [0, 1, 0], Block::Accumulator, DVec3::NEG_Z);
            place(&mut c, [1, 1, 0], Block::Accumulator, DVec3::NEG_Z);
            place(&mut c, [-1, 1, 0], Block::CoalGenerator, DVec3::NEG_Z);
            place(&mut c, [-1, 1, -1], Block::FuelTank, DVec3::X);
            place(&mut c, [0, 1, -1], Block::FuelTank, DVec3::X);
            for z in [-1, 1, 3] {
                place(&mut c, [2, 0, z], Block::Wheel, DVec3::X);
                place(&mut c, [-2, 0, z], Block::Wheel, DVec3::X);
            }
        }
        "probe" => {
            // An ion probe for space: a battery, panels on two wings and four ion engines.
            c = Craft::new(2);
            for y in 1..=3 {
                place(&mut c, [0, y, 0], Block::Hull, DVec3::X);
            }
            place(&mut c, [0, 4, 0], Block::Seat, DVec3::Z);
            place(&mut c, [0, 2, 1], Block::Accumulator, DVec3::Z);
            place(&mut c, [1, 2, 0], Block::Gyro, DVec3::X);
            for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                place(&mut c, [x, 0, z], Block::IonEngine, DVec3::NEG_Y);
            }
            for x in [1, 2, 3, -1, -2, -3] {
                place(&mut c, [x, 3, 0], Block::Hull, DVec3::X);
                place(&mut c, [x, 3, 1], Block::SolarPanel, DVec3::Z);
                place(&mut c, [x, 3, -1], Block::SolarPanel, DVec3::NEG_Z);
            }
            place(&mut c, [0, -1, 0], Block::LandingGear, DVec3::X);
        }
        "gate" => {
            // A flatbed with a big door across its back (a door on a vehicle).
            let mut deck = Vec::new();
            for x in -2..=2 {
                for z in -3..=2 {
                    if [x, z] != [0, 0] {
                        deck.push([x, 0, z]);
                    }
                }
            }
            for y in 1..=3 {
                deck.push([-2, y, -3]);
                deck.push([2, y, -3]);
            }
            for x in -2..=2 {
                deck.push([x, 4, -3]);
            }
            grow(&mut c, &deck, Block::Hull, DVec3::X);
            for (x, z) in [(3, -2), (-3, -2), (3, 1), (-3, 1)] {
                place(&mut c, [x, 0, z], Block::Wheel, DVec3::X);
            }
            let big = craft::door_cells(DoorKind::Big, [1, 1, -3], facing_of(DVec3::NEG_Z));
            let parts: Vec<Part> = big
                .iter()
                .map(|&(at, facing)| Part {
                    at,
                    block: Block::BigDoor,
                    facing,
                })
                .collect();
            c.place_all(&parts);
            place(&mut c, [0, 1, 1], Block::Seat, DVec3::Z);
            place(&mut c, [1, 1, 2], Block::Accumulator, DVec3::Z);
            place(&mut c, [1, 2, 2], Block::SolarPanel, DVec3::Y);
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

/// How fast a door on a vehicle opens (share of the way a second).
fn vehicle_door_rate(kind: DoorKind) -> f32 {
    match kind {
        DoorKind::Hinged => 2.2,
        DoorKind::Sliding => 1.8,
        DoorKind::Big => 0.33,
    }
}

/// The items parts taken off a vehicle are: one each, and one for all the cells of a door.
fn vehicle_part_items(parts: &[Part]) -> Vec<ItemStack> {
    let mut doors: Vec<At> = Vec::new();
    let mut items = Vec::new();
    for p in parts {
        if p.block.door().is_some() {
            let (col, row) = craft::door_piece(p.facing);
            let across = craft::door_across(p.facing);
            let anchor = [
                p.at[0] - across[0] * col,
                p.at[1] - row,
                p.at[2] - across[2] * col,
            ];
            if doors.contains(&anchor) {
                continue;
            }
            doors.push(anchor);
        }
        items.push(ItemStack::new(Item::Block(p.block.as_item()), 1));
    }
    items
}

/// How high a vehicle's core must sit for its lowest part to rest on the ground (m).
fn rest_height(c: &Craft) -> f64 {
    c.parts()
        .iter()
        .map(|p| craft::wheel_radius(p.block).unwrap_or(0.5) - p.at[1] as f64)
        .fold(0.5, f64::max)
}

impl Game {
    /// Where a vehicle's grid is, in the frame the game draws in: its origin and the rotation
    /// taking grid axes into it.
    pub(super) fn grid_pose(&self, v: &Vessel) -> (DVec3, DQuat) {
        let frame = self.active_frame(self.time);
        let to_world = (frame.rotation.inverse() * v.shown_attitude()).normalize();
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
        if above.is_some_and(|c| self.vehicle_in_cell(c)) {
            self.say("There is a vehicle there");
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

    /// Some vehicle has a part in (or reaching well into) this block cell: nothing can be
    /// placed there.
    pub(super) fn vehicle_in_cell(&self, cell: BlockCell) -> bool {
        let center = interact::cell_center(&self.planet, cell);
        self.flight.vessels.iter().any(|v| {
            let Some(craft) = &v.craft else {
                return false;
            };
            let (grid, to_world) = self.grid_pose(v);
            if (center - grid).length() > craft.stats.radius + 2.0 {
                return false;
            }
            let local = to_world.inverse() * (center - grid);
            // The middle of the cell and its corners drawn in a little (cells are about a
            // meter, and the grids are turned against each other).
            let inset = 0.38;
            std::iter::once(DVec3::ZERO)
                .chain((0..8).map(|i| {
                    DVec3::new(
                        if i & 1 == 0 { -inset } else { inset },
                        if i & 2 == 0 { -inset } else { inset },
                        if i & 4 == 0 { -inset } else { inset },
                    )
                }))
                .any(|d| {
                    let p = local + d;
                    craft
                        .get([p.x.round() as i16, p.y.round() as i16, p.z.round() as i16])
                        .is_some()
                })
        })
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
        let sneaking = self.input.held(KeyCode::ShiftLeft);
        if part.block == Block::Seat && !sneaking {
            self.sit_down(hit.vessel, hit.at);
            return;
        }
        if part.block.door().is_some() && !sneaking {
            self.toggle_vehicle_door(hit.vessel, hit.at);
            return;
        }
        self.place_vehicle_part(hit);
    }

    /// Opens or closes a door on a vehicle (and tells the others). A big door's motor takes
    /// its electricity from the vehicle's batteries.
    pub(super) fn toggle_vehicle_door(&mut self, i: usize, at: At) {
        let infinite = self.infinite_power;
        let v = &mut self.flight.vessels[i];
        let id = v.id;
        let Some((n, door)) = v.craft.as_ref().and_then(|c| c.door_at(at)) else {
            return;
        };
        let open = !v.craft.as_ref().is_some_and(|c| c.door_open(n));
        if door.kind == DoorKind::Big && !infinite && v.charge >= 0.0 {
            if v.charge < craft::BIG_DOOR_ENERGY {
                self.say("Not enough power to move the big door");
                return;
            }
            v.charge -= craft::BIG_DOOR_ENERGY;
        }
        if let Some(c) = v.craft.as_mut() {
            c.set_door(at, open);
        }
        let (grid, to_world) = self.grid_pose(&self.flight.vessels[i]);
        let material = door.kind.closed().material();
        self.survival
            .sounds
            .push((Sound::Place(material), grid + to_world * craft::center(at)));
        self.survival.uses += 1;
        self.send_craft_change(id, CraftChange::Door { at, open });
    }

    pub(super) fn break_vehicle_part(&mut self, hit: CraftHit) {
        let v = &mut self.flight.vessels[hit.vessel];
        if v.pilot.is_some() && hit.at == [0, 0, 0] {
            self.say("Someone is flying it");
            return;
        }
        let id = v.id;
        // A door comes off whole: all its cells, and it is one door again.
        let cells: Vec<At> = match v.craft.as_ref().and_then(|c| c.door_at(hit.at)) {
            Some((_, door)) => craft::door_cells(door.kind, door.anchor, door.front)
                .into_iter()
                .map(|(at, _)| at)
                .filter(|&at| at != hit.at)
                .collect(),
            None => Vec::new(),
        };
        let mut removed = craft::Removed::default();
        v.rebuild_craft(|c| {
            removed = c.remove(hit.at);
            for &at in &cells {
                let more = c.remove(at);
                removed.core_gone |= more.core_gone;
                removed.parts.extend(more.parts);
            }
        });
        let (grid, to_world) = self.grid_pose(&self.flight.vessels[hit.vessel]);
        let at = grid + to_world * craft::center(hit.at);
        self.survival
            .sounds
            .push((Sound::Break(Material::Stone), at));
        if self.player.mode != GameMode::Creative {
            for stack in vehicle_part_items(&removed.parts) {
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
        for at in cells {
            self.send_craft_change(id, CraftChange::Take(at));
        }
    }

    fn place_vehicle_part(&mut self, hit: CraftHit) {
        let Some(stack) = self.survival.held() else {
            return;
        };
        let Some(block) = stack.item.block() else {
            return;
        };
        let fits = !block.is_model() || craft::is_vehicle_model(block);
        if !fits || matches!(block, Block::Air | Block::Water | Block::VehicleCore) {
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
        if let Some((kind, _)) = block.door() {
            self.place_vehicle_door(hit, at, kind, outward);
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

    /// A door onto a vehicle: all its cells at once, `at` its bottom (the bottom middle of a
    /// big door). It faces out from what it is put on, or (put on a floor) towards whoever
    /// puts it there.
    fn place_vehicle_door(&mut self, hit: CraftHit, at: At, kind: DoorKind, outward: DVec3) {
        let v = &self.flight.vessels[hit.vessel];
        let (grid, to_world) = self.grid_pose(v);
        let frame = self.active_frame(self.time);
        let look = to_world.inverse() * self.camera.forward().as_dvec3();
        let flat_out = DVec3::new(outward.x, 0.0, outward.z);
        let front = if flat_out.length() > 0.5 {
            facing_of(flat_out)
        } else {
            facing_of(DVec3::new(-look.x, 0.0, -look.z))
        };
        let across = craft::door_across(front);
        let (w, _) = kind.size();
        let back = if w > 1 { 1 } else { 0 };
        let anchor = [at[0] - across[0] * back, at[1], at[2] - across[2] * back];
        // In order out from where it was put, so each touches one before it (the network
        // places them one at a time).
        let mut parts: Vec<Part> = craft::door_cells(kind, anchor, front)
            .iter()
            .map(|&(at, facing)| Part {
                at,
                block: kind.closed(),
                facing,
            })
            .collect();
        parts.sort_by_key(|p| (0..3).map(|i| (p.at[i] - at[i]).abs()).sum::<i16>());
        // Not inside whoever is placing it.
        let feet = to_world.inverse() * (frame.to_body(self.player.position) - grid);
        let inside = parts.iter().any(|p| {
            let c = craft::center(p.at);
            (feet.x - c.x).abs() < 0.5 + HALF_WIDTH
                && (feet.z - c.z).abs() < 0.5 + HALF_WIDTH
                && feet.y < c.y + 0.5
                && feet.y + HEIGHT > c.y - 0.5
        });
        if inside {
            return;
        }
        let v = &mut self.flight.vessels[hit.vessel];
        let id = v.id;
        let mut placed = false;
        v.rebuild_craft(|c| placed = c.place_all(&parts));
        if !placed {
            self.say("There isn't room for the door there");
            return;
        }
        if self.player.mode != GameMode::Creative {
            self.survival.inventory.take_one(self.survival.selected);
        }
        self.survival.uses += 1;
        let (grid, to_world) = self.grid_pose(&self.flight.vessels[hit.vessel]);
        self.survival.sounds.push((
            Sound::Place(kind.closed().material()),
            grid + to_world * craft::center(at),
        ));
        for part in parts {
            self.send_craft_change(id, CraftChange::Put(part));
        }
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
        let Some(i) = self.flight.vessels.iter().position(|v| v.id == vessel) else {
            return;
        };
        if let CraftChange::Crash(at) = change
            && self.flight.vessels[i]
                .craft
                .as_ref()
                .is_some_and(|c| c.get(at).is_some())
        {
            // The bang the one who saw it break made.
            log::debug!("vehicle {vessel} lost a part in a crash");
            let (grid, to_world) = self.grid_pose(&self.flight.vessels[i]);
            self.crash_bang(grid + to_world * craft::center(at));
        }
        let v = &mut self.flight.vessels[i];
        match change {
            CraftChange::Put(part) => {
                v.rebuild_craft(|c| {
                    c.place(part);
                });
            }
            CraftChange::Take(at) | CraftChange::Crash(at) => {
                v.rebuild_craft(|c| {
                    c.remove(at);
                });
            }
            CraftChange::Door { at, open } => {
                if let Some(c) = v.craft.as_mut() {
                    c.set_door(at, open);
                }
            }
        }
    }

    /// The console's `place` and `break`, on a vehicle (a test without a mouse). Returns
    /// whether there was one to act on.
    pub(super) fn console_vehicle_click(&mut self, place: bool) -> bool {
        let count = self.flight.vessels.len();
        if place && self.place_core(true) {
            let text = if self.flight.vessels.len() > count {
                "Started a vehicle".to_string()
            } else {
                self.message
                    .as_ref()
                    .map_or_else(String::new, |(m, _)| m.clone())
            };
            self.console.print(text);
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
                Some(p) if p.block.door().is_some() => self.toggle_vehicle_door(hit.vessel, hit.at),
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

    /// A seat broken (by someone else, or knocked off) while sat in: whoever was in it stands
    /// up where it was, still aboard (the pilot gives up the controls).
    pub(super) fn lose_missing_seats(&mut self) {
        let is_seat = |v: &crate::flight::vessel::Vessel, at: At| {
            v.craft
                .as_ref()
                .and_then(|c| c.get(at))
                .is_some_and(|p| p.block == Block::Seat)
        };
        if let Some(i) = self.flight.boarded
            && self.flight.vessels[i].craft.is_some()
        {
            let v = &self.flight.vessels[i];
            let gone = match self.flight.seat {
                Some(seat) => !is_seat(v, seat),
                None => v.craft.as_ref().is_some_and(|c| c.stats.seats.is_empty()),
            };
            if gone {
                self.exit_vessel();
                self.say("Your seat is gone");
            }
        }
        if let Some(a) = &mut self.flight.aboard
            && let Some(seat) = a.seat
            && !is_seat(&self.flight.vessels[a.vessel], seat)
        {
            a.seat = None;
            self.say("Your seat is gone");
        }
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

    /// Where someone aboard (by what they last said) is in the world: their feet, the deck's
    /// up, and, in a seat, the way it faces.
    pub(super) fn aboard_world(&self, pose: &AboardPose) -> Option<(DVec3, DVec3, Option<DVec3>)> {
        let v = self.flight.vessels.iter().find(|v| v.id == pose.vessel)?;
        let (grid, to_world) = self.grid_pose(v);
        let feet = pose.feet.as_dvec3();
        let facing = pose
            .seat
            .then(|| {
                let at = feet + DVec3::Y * 0.3;
                let at = [
                    at.x.round() as i16,
                    at.y.round() as i16,
                    at.z.round() as i16,
                ];
                v.craft.as_ref()?.get(at).filter(|p| p.block == Block::Seat)
            })
            .flatten()
            .map(|p| {
                let d = craft::direction(p.facing);
                to_world * if d.y.abs() > 0.5 { DVec3::Z } else { d }
            });
        Some((grid + to_world * feet, to_world * DVec3::Y, facing))
    }

    /// Flying a vehicle seen from outside: yourself, sitting in the seat.
    pub(super) fn own_figure_draw_item(
        &mut self,
        objects: &mut Vec<DrawItem>,
    ) -> anyhow::Result<()> {
        let seated = if self.flight.cockpit || self.flight.map || self.death.is_some() {
            None
        } else {
            self.flight
                .boarded
                .filter(|&i| self.flight.vessels[i].craft.is_some())
                .and_then(|_| self.aboard_pose())
                .and_then(|pose| self.aboard_world(&pose))
        };
        let Some((feet, up, facing)) = seated else {
            if let Some(old) = self.flight.figure.take() {
                self.renderer.destroy_mesh(old);
            }
            return Ok(());
        };
        let up = up.normalize_or(DVec3::Y);
        let forward = facing.unwrap_or(up.any_orthonormal_vector());
        let light = self.light_at(feet + up);
        let mesh = crate::player::avatar::avatar_model(
            &crate::player::avatar::AvatarPose {
                sitting: facing.is_some(),
                ..Default::default()
            },
            (forward, up),
            light,
        );
        if let Some(handle) = self.renderer.create_mesh(&mesh)?
            && let Some(old) = self.flight.figure.replace(handle)
        {
            self.renderer.destroy_mesh(old);
        }
        if let Some(mesh) = self.flight.figure {
            objects.push(DrawItem {
                mesh,
                origin: self.planet.center + feet,
                rotation: glam::Quat::IDENTITY,
                radius: 2.0,
                fade: Fade::Visible,
            });
        }
        Ok(())
    }

    /// One vehicle's mesh (rebuilt when its parts change) and its moving pieces (propellers,
    /// wheels, flaps), turned by what they are doing (`dt`: this frame, 0 when paused).
    pub(super) fn vehicle_draw_item(
        &mut self,
        i: usize,
        objects: &mut Vec<DrawItem>,
        dt: f32,
    ) -> anyhow::Result<()> {
        let v = &self.flight.vessels[i];
        let Some(craft) = &v.craft else {
            return Ok(());
        };
        let (grid, to_world) = self.grid_pose(v);
        let radius = craft.stats.radius as f32 + 2.0;
        // Lit as where it is (underground, in the core), rebuilt when that changes.
        let light = self.light_at(grid + to_world * v.com());
        let packed = light.sky << 4 | light.block;
        if v.mesh_version != craft.version || v.mesh_light != packed || v.mesh.is_none() {
            let version = craft.version;
            let (mesh, moving) = craft_model::craft_model(craft, light);
            if let Some(handle) = self.renderer.create_mesh(&mesh)? {
                let mut handles = Vec::with_capacity(moving.len());
                for (pivot, mesh) in &moving {
                    if let Some(h) = self.renderer.create_mesh(mesh)? {
                        handles.push((*pivot, h));
                    }
                }
                if handles.len() < moving.len() {
                    // Out of upload room this frame: all of it again next frame (the old
                    // meshes are drawn meanwhile).
                    self.renderer.destroy_mesh(handle);
                    for (_, h) in handles {
                        self.renderer.destroy_mesh(h);
                    }
                } else {
                    let v = &mut self.flight.vessels[i];
                    for old in v.take_meshes() {
                        self.renderer.destroy_mesh(old);
                    }
                    v.mesh = Some(handle);
                    v.moving = handles;
                    v.mesh_version = version;
                    v.mesh_light = packed;
                }
            }
        }

        // What it is doing: the engines, the ground going by, the stick.
        let v = &self.flight.vessels[i];
        let ground_speed = match v.landed {
            Some(_) => 0.0,
            None => {
                let frame = self.bodies.frame(v.body, self.time);
                let omega = frame.rotation * frame.omega;
                let over_ground = v.velocity - omega.cross(v.position);
                let ahead = v.attitude * v.craft.as_ref().map_or(DVec3::Z, |c| c.stats.forward);
                over_ground.dot(ahead) as f32
            }
        };
        let flaps = std::mem::take(&mut self.flight.vessels[i].flaps);
        let v = &mut self.flight.vessels[i];
        let doors: Vec<(bool, f32)> = v.craft.as_ref().map_or(Vec::new(), |c| {
            (c.stats.doors.iter().enumerate())
                .map(|(n, d)| (c.door_open(n), vehicle_door_rate(d.kind)))
                .collect()
        });
        if !v.looks_started {
            v.looks.settle_doors(&doors);
            v.looks_started = true;
        }
        let doing = craft_model::Doing {
            power: if v.engines_on() {
                v.throttle as f32
            } else {
                0.0
            },
            touching: v.touching || v.landed.is_some(),
            ground_speed,
            steer: v.steer,
            flaps: &flaps,
            drilling: v.drilling && v.power_share > 0.0,
            gimbal: if v.engines_on() {
                v.gimbal
            } else {
                DVec3::ZERO
            },
            doors: &doors,
        };
        v.looks.advance(&doing, dt);
        v.flaps = flaps;

        let v = &self.flight.vessels[i];
        let up = v.craft.as_ref().map_or(DVec3::Y, |c| c.stats.up);
        if let Some(mesh) = v.mesh {
            objects.push(DrawItem {
                mesh,
                origin: self.planet.center + grid,
                rotation: to_world.as_quat(),
                radius,
                fade: Fade::Visible,
            });
        }
        for (pivot, mesh) in &v.moving {
            if !v.looks.shown(pivot) {
                continue;
            }
            let turn = (to_world * v.looks.turn(pivot, up)).normalize();
            let at = pivot.at + v.looks.offset(pivot);
            objects.push(DrawItem {
                mesh: *mesh,
                origin: self.planet.center + grid + to_world * at,
                rotation: turn.as_quat(),
                radius: 1.6,
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
        if v.craft.is_none() {
            return;
        }
        let running = v.running_engines();
        if running.is_empty() {
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
        for (e, power) in running.iter().take(10) {
            if matches!(e.kind.air, craft::AirUse::Propeller(_)) {
                sound_at.get_or_insert(grid + to_world * e.position);
                continue;
            }
            if e.kind.electric() {
                // (Quiet: no rumble.)
                let nozzle = grid + to_world * (e.position - e.push * 0.6);
                let count = ((60.0 * *power as f32) * dt).ceil() as u32;
                out.push(effects::ion_exhaust(
                    nozzle,
                    to_world * -e.push,
                    speed,
                    count,
                    *power as f32,
                ));
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

    /// The flown vehicle's drills, while they run (G, or the HUD's button) and have power: each
    /// bores through the block in front of its head, as fast as an iron tool would (slower on
    /// less power). What it cuts goes to the driver.
    pub(super) fn run_drills(&mut self, dt: f32) {
        let Some(i) = self.flight.boarded else {
            self.flight.drill_work.clear();
            return;
        };
        let v = &self.flight.vessels[i];
        let Some(craft) = &v.craft else { return };
        if !v.drilling || v.power_share <= 0.0 || craft.stats.drills.is_empty() || dt <= 0.0 {
            self.flight.drill_work.clear();
            return;
        }
        let share = v.power_share as f32;
        let drills = craft.stats.drills.clone();
        let (grid, to_world) = self.grid_pose(v);
        self.flight.drill_work.resize(drills.len(), None);
        let mut done = Vec::new();
        for (n, d) in drills.iter().enumerate() {
            let out = to_world * craft::direction(d.facing);
            let tip = grid + to_world * craft::center(d.at) + out;
            let Some(cell) = self.planet.cell_at(tip) else {
                continue;
            };
            let block = self.streamer.block(cell).unwrap_or(Block::Air);
            if !block.is_solid() || block.is_model() || block == Block::Lava {
                self.flight.drill_work[n] = None;
                continue;
            }
            let tool = items::mining(block)
                .tool
                .unwrap_or(items::ToolKind::Pickaxe);
            let time = items::break_time(block, Some((tool, items::Tier::Iron))).max(0.15);
            let work = match self.flight.drill_work[n] {
                Some((c, w)) if c == cell => w + dt * share,
                _ => dt * share,
            };
            // Chips off the face now and then, and the sound of it.
            let before = work - dt * share;
            if (before / 0.12).floor() != (work / 0.12).floor() {
                let up = tip.normalize_or(DVec3::Y);
                self.survival
                    .effects
                    .push(effects::drill_chips(block, tip - out * 0.5, -out, up));
            }
            if (before / 0.3).floor() != (work / 0.3).floor() {
                self.survival
                    .sounds
                    .push((Sound::Dig(block.material()), tip));
            }
            if work >= time {
                self.flight.drill_work[n] = None;
                done.push((cell, block));
            } else {
                self.flight.drill_work[n] = Some((cell, work));
            }
        }
        done.dedup();
        for (cell, block) in done {
            if self.streamer.block(cell) != Some(block) {
                continue;
            }
            let mut world = crate::survival::World {
                planet: &self.planet,
                streamer: &mut self.streamer,
            };
            let got = self.survival.drill_block(&mut world, cell, block);
            if self.player.mode != GameMode::Creative
                && let Some(stack) = got
                && let Some(rest) = self.survival.inventory.add(stack)
            {
                let at = interact::cell_center(&self.planet, cell);
                self.survival.spawn(rest, at, DVec3::ZERO, 0.5);
            }
        }
    }

    /// G while flying: the drills on or off.
    pub(super) fn toggle_drills(&mut self) {
        let Some(i) = self.flight.boarded else { return };
        let v = &mut self.flight.vessels[i];
        if v.craft.as_ref().is_none_or(|c| c.stats.drills.is_empty()) {
            return;
        }
        v.drilling = !v.drilling;
        let text = if v.drilling {
            "Drills on"
        } else {
            "Drills off"
        };
        self.say(text);
    }

    /// `vehicle <plane|rover|rocket|digger|probe|gate>`: a ready-made one a few metres ahead.
    pub(super) fn spawn_preset(&mut self, name: &str, distance: f64) {
        let Some(craft) = preset(name) else {
            self.console
                .print("vehicle <plane|rover|rocket|digger|probe|gate> [m]");
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

        // The newer ones have everything they are about.
        assert!(rover.stats.battery > craft::CORE_BATTERY && rover.stats.solar.len() == 2);
        let digger = preset("digger").unwrap();
        assert_eq!(digger.stats.drills.len(), 9, "a wall of drills");
        assert_eq!(digger.stats.generators, 1);
        assert_eq!(digger.stats.wheels.len(), 6);
        let probe = preset("probe").unwrap();
        assert_eq!(probe.stats.engines.len(), 4);
        assert!(probe.stats.engines.iter().all(|e| e.kind.electric()));
        assert_eq!(probe.stats.solar.len(), 12);
        let gate = preset("gate").unwrap();
        assert_eq!(gate.stats.doors.len(), 1);
        let door_cells = gate
            .parts()
            .iter()
            .filter(|p| p.block.door().is_some())
            .count();
        assert_eq!(door_cells, 9);
        assert_eq!(gate.stats.wheels.len(), 4);
        for name in ["digger", "probe", "gate"] {
            assert!(preset(name).unwrap().stats.seats.len() == 1, "{name}");
        }
    }

    /// Taking a door off a vehicle gives one door, however many cells it had.
    #[test]
    fn a_door_comes_off_as_one_door() {
        let gate = preset("gate").unwrap();
        let parts: Vec<Part> = gate
            .parts()
            .iter()
            .filter(|p| p.block.door().is_some() || p.block == Block::Seat)
            .copied()
            .collect();
        let items = vehicle_part_items(&parts);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|s| s.item == Item::Block(Block::BigDoor)));
    }

    #[test]
    fn a_rover_rests_on_its_wheels() {
        let rover = preset("rover").unwrap();
        assert!((rest_height(&rover) - craft::WHEEL_RADIUS).abs() < 1e-9);
    }
}
