//! Passive animals: cows, pigs and sheep. They wander around grassland, graze, step up
//! single blocks, hop away and panic when hit, and drop leather, meat or wool. Their models
//! are in `models`.
//!
//! Like dropped items, mobs live in the planet's body (rotating) frame with local gravity
//! (times `WEIGHT`), and collide with blocks using the player's sweep (`player::collide`).

pub mod models;

use glam::DVec3;

use crate::{
    net::protocol::MobState,
    player::{self, Voxels},
    render::MeshHandle,
    survival::items::{Item, ItemStack},
    world::{block::Block, planet::Planet},
};

/// Animals fall this many times faster than real gravity (like the player's hops), so
/// hops and knockback are brisk rather than floaty.
const WEIGHT: f64 = 2.0;
/// Take-off speed of a hop up one block under `WEIGHT` x gravity (about 1.2 m high).
const HOP_SPEED: f64 = 6.9;
/// A hit knocks an animal this fast away (m/s) in a low hop (0.3 s): about 1.3 m in all.
const KNOCKBACK: f64 = 3.2;
const KNOCKBACK_LIFT: f64 = 3.0;
/// An animal someone else simulates is forgotten after this long without news (s).
const REMOTE_TIMEOUT: f32 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MobKind {
    Cow,
    Pig,
    Sheep,
}

impl MobKind {
    pub const ALL: [MobKind; 3] = [MobKind::Cow, MobKind::Pig, MobKind::Sheep];

    pub fn max_health(self) -> f32 {
        match self {
            MobKind::Sheep => 8.0,
            _ => 10.0,
        }
    }

    /// Collision box: half width and height (m).
    pub fn size(self) -> (f64, f64) {
        match self {
            MobKind::Cow => (0.45, 1.4),
            MobKind::Pig => (0.45, 0.9),
            MobKind::Sheep => (0.45, 1.3),
        }
    }

    /// The body's length along its heading for hits: half the length and how far its middle
    /// is ahead of the feet (m). The head sticks out in front (see `models`).
    fn length(self) -> (f64, f64) {
        match self {
            MobKind::Cow => (1.06, 0.3),
            _ => (0.83, 0.14),
        }
    }

    /// Walking speed (m/s).
    fn speed(self) -> f64 {
        match self {
            MobKind::Pig => 1.1,
            _ => 0.9,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MobKind::Cow => "Cow",
            MobKind::Pig => "Pig",
            MobKind::Sheep => "Sheep",
        }
    }

    /// What it drops when killed; `roll(n)` is a random number in 0..n.
    pub fn loot(self, roll: &mut dyn FnMut(u32) -> u32) -> Vec<ItemStack> {
        let mut out = Vec::new();
        let mut add = |item, n: u32| {
            if n > 0 {
                out.push(ItemStack::new(item, n as u8));
            }
        };
        match self {
            MobKind::Cow => {
                add(Item::Leather, roll(3));
                add(Item::RawBeef, 1 + roll(3));
            }
            MobKind::Pig => add(Item::RawPorkchop, 1 + roll(3)),
            MobKind::Sheep => {
                add(Item::Block(Block::WhiteWool), 1);
                add(Item::RawMutton, 1 + roll(2));
            }
        }
        out
    }
}

/// The animation state a model is posed from (see `models`).
#[derive(Clone, Copy, Debug, Default)]
pub struct MobPose {
    /// Walk cycle phase (radians) and how strongly the legs swing (0..1).
    pub walk: f32,
    pub swing: f32,
    /// Flashing red after a hit.
    pub hurt: bool,
    /// Head down grazing (0..1).
    pub graze: f32,
    /// Seconds alive, for idle motion.
    pub age: f32,
    /// Coat colour variant (0..3).
    pub coat: u8,
}

/// The last known state of an animal someone else simulates.
#[derive(Clone, Copy)]
pub struct RemoteMob {
    pub position: DVec3,
    pub heading: DVec3,
    pub graze: f32,
    pub silent: f32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Mob {
    /// Tells it apart across the network.
    #[serde(default)]
    pub id: u32,
    /// Who simulates it (others only see where it is).
    #[serde(default)]
    pub owner: u32,
    /// Someone else simulates it: where they say it is, and for how long that's been the
    /// latest news.
    #[serde(skip)]
    pub remote: Option<RemoteMob>,
    pub kind: MobKind,
    /// Feet position and velocity (body frame, planet-relative).
    pub position: DVec3,
    pub velocity: DVec3,
    /// Unit direction it faces (along the ground).
    pub heading: DVec3,
    pub health: f32,
    /// Seconds of the red hurt flash left (also invulnerability, as in Minecraft).
    pub hurt: f32,
    /// Seconds of running around in panic after being hit.
    panic: f32,
    /// Where it wants to go (unit direction), or None to stand still, and for how long.
    goal: Option<DVec3>,
    goal_time: f32,
    /// Walk cycle phase (radians) and how strongly the legs swing (0..1).
    pub walk: f32,
    pub swing: f32,
    on_ground: bool,
    /// Seconds alive (with a random start), for idle motion: tail, ears, looking around.
    pub age: f32,
    /// How far the head is down grazing (0..1), and whether it wants to graze.
    pub graze: f32,
    grazing: bool,
    /// Coat colour variant.
    pub coat: u8,
    #[serde(skip)]
    pub mesh: Option<MeshHandle>,
    rng: u64,
}

impl Mob {
    pub fn new(kind: MobKind, position: DVec3, seed: u64) -> Self {
        let up = position.normalize_or(DVec3::Y);
        let heading = up.any_orthonormal_vector();
        let mut mob = Self {
            id: 0,
            owner: 0,
            remote: None,
            kind,
            position,
            velocity: DVec3::ZERO,
            heading,
            health: kind.max_health(),
            hurt: 0.0,
            panic: 0.0,
            goal: None,
            goal_time: 0.0,
            walk: 0.0,
            swing: 0.0,
            on_ground: false,
            age: 0.0,
            graze: 0.0,
            grazing: false,
            coat: 0,
            mesh: None,
            rng: seed | 1,
        };
        mob.goal_time = mob.random() as f32 * 3.0;
        mob.age = mob.random() as f32 * 100.0;
        mob.coat = (mob.random() * 4.0) as u8;
        mob
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn random_heading(&mut self, up: DVec3) -> DVec3 {
        let a = up.any_orthonormal_vector();
        let b = up.cross(a);
        let angle = self.random() * std::f64::consts::TAU;
        a * angle.cos() + b * angle.sin()
    }

    /// Takes a hit from `from` (planet-relative position of the attacker). Returns true if
    /// it died. Hits during the hurt flash don't count.
    pub fn hit(&mut self, damage: f32, from: DVec3) -> bool {
        if self.hurt > 0.0 {
            return false;
        }
        self.health -= damage;
        self.hurt = 0.5;
        self.panic = 5.0;
        let up = self.position.normalize_or(DVec3::Y);
        let away = self.position - from;
        let away = (away - up * away.dot(up)).normalize_or(self.heading);
        // A short hop away (replacing its own motion, so hits don't add up to a flight).
        self.velocity = away * KNOCKBACK + up * KNOCKBACK_LIFT;
        self.grazing = false;
        self.health <= 0.0
    }

    /// What the model needs to pose it.
    pub fn pose(&self) -> MobPose {
        MobPose {
            walk: self.walk,
            swing: self.swing,
            hurt: self.hurt > 0.0,
            graze: self.graze,
            age: self.age,
            coat: self.coat,
        }
    }

    /// Where it is in the middle (planet-relative).
    pub fn center(&self) -> DVec3 {
        self.position + self.position.normalize_or(DVec3::Y) * (self.kind.size().1 * 0.5)
    }

    /// Distance along a ray (planet-relative origin, unit direction) to its box, if hit.
    pub fn ray_hit(&self, origin: DVec3, dir: DVec3, reach: f64) -> Option<f64> {
        let up = self.position.normalize_or(DVec3::Y);
        let forward = self.heading;
        let right = forward.cross(up);
        let (half_width, height) = self.kind.size();
        // Animals are longer than they are wide.
        let (half_length, ahead) = self.kind.length();
        let half = [half_width, half_length, height * 0.5];
        let rel = origin - (self.center() + forward * ahead);
        let axes = [right, forward, up];
        let (mut t0, mut t1) = (0.0f64, reach);
        for (axis, h) in axes.iter().zip(half) {
            let (o, d) = (rel.dot(*axis), dir.dot(*axis));
            if d.abs() < 1e-9 {
                if o.abs() > h {
                    return None;
                }
                continue;
            }
            let (a, b) = ((-h - o) / d, (h - o) / d);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }

    /// What others need to see of it.
    pub fn net_state(&self) -> MobState {
        let heading = self.heading.as_vec3() * 127.0;
        MobState {
            id: self.id,
            kind: self.kind,
            coat: self.coat,
            position: self.position.as_vec3(),
            heading: [heading.x as i8, heading.y as i8, heading.z as i8],
            graze: (self.graze.clamp(0.0, 1.0) * 255.0) as u8,
            hurt: self.hurt > 0.0,
        }
    }

    /// A copy of someone else's animal, from what they sent.
    pub fn replica(owner: u32, state: &MobState) -> Self {
        let position = state.position.as_dvec3();
        let mut mob = Self::new(state.kind, position, state.id as u64 + 1);
        mob.id = state.id;
        mob.coat = state.coat;
        mob.owner = owner;
        mob.apply_net(owner, state);
        mob.remote = mob.remote.map(|r| RemoteMob { position, ..r });
        mob
    }

    /// News about an animal someone else simulates.
    pub fn apply_net(&mut self, owner: u32, state: &MobState) {
        let [x, y, z] = state.heading.map(|v| v as f64 / 127.0);
        self.owner = owner;
        self.remote = Some(RemoteMob {
            position: state.position.as_dvec3(),
            heading: DVec3::new(x, y, z).normalize_or(self.heading),
            graze: state.graze as f32 / 255.0,
            silent: 0.0,
        });
        if state.hurt {
            self.hurt = 0.15;
        }
    }

    /// Moves towards where its owner says it is, walking the walk. False: it hasn't been
    /// heard of in a while.
    pub fn follow(&mut self, dt: f64) -> bool {
        let Some(remote) = &mut self.remote else {
            return true;
        };
        remote.silent += dt as f32;
        let remote = *remote;
        let before = self.position;
        let ease = 1.0 - (-dt * 12.0).exp();
        self.position += (remote.position - self.position) * ease;
        self.heading =
            (self.heading + (remote.heading - self.heading) * ease).normalize_or(remote.heading);
        self.graze += (remote.graze - self.graze) * ease as f32;
        self.hurt = (self.hurt - dt as f32).max(0.0);
        self.age += dt as f32;
        let up = self.position.normalize_or(DVec3::Y);
        let moved = self.position - before;
        let ground_speed = (moved - up * moved.dot(up)).length() / dt.max(1e-4);
        self.walk += (ground_speed * dt * 6.0) as f32;
        let target = ground_speed.min(1.0) as f32;
        self.swing += (target - self.swing) * (dt as f32 * 8.0).min(1.0);
        remote.silent < REMOTE_TIMEOUT
    }

    /// Wandering, panicking, walking and falling, for `dt` seconds.
    pub fn update(&mut self, planet: &Planet, world: &dyn Voxels, gravity: f64, dt: f64) {
        let up = self.position.normalize_or(DVec3::Y);
        self.hurt = (self.hurt - dt as f32).max(0.0);
        self.panic = (self.panic - dt as f32).max(0.0);
        self.goal_time -= dt as f32;
        self.age += dt as f32;
        let gravity = gravity * WEIGHT;

        // Decide where to go.
        if self.goal_time <= 0.0 {
            if self.panic > 0.0 {
                self.goal = Some(self.random_heading(up));
                self.goal_time = 0.5 + self.random() as f32;
            } else if self.random() < 0.6 {
                self.goal = Some(self.random_heading(up));
                self.goal_time = 2.0 + self.random() as f32 * 3.0;
            } else {
                self.goal = None;
                self.goal_time = 3.0 + self.random() as f32 * 5.0;
                // Standing still is mostly grazing.
                self.grazing = self.random() < 0.7;
            }
            if self.goal.is_some() {
                self.grazing = false;
            }
        }
        let head_down = if self.grazing && self.panic <= 0.0 {
            1.0
        } else {
            0.0
        };
        self.graze += (head_down - self.graze) * (dt as f32 * 2.5).min(1.0);
        let speed = self.kind.speed() * if self.panic > 0.0 { 2.0 } else { 1.0 };
        let desired = match self.goal {
            Some(goal) => {
                // Turn towards the goal, then walk the way it faces.
                let goal = (goal - up * goal.dot(up)).normalize_or(self.heading);
                let turn = (dt * 4.0).min(1.0);
                self.heading = (self.heading + (goal - self.heading) * turn).normalize_or(goal);
                self.heading * speed
            }
            None => DVec3::ZERO,
        };
        self.heading =
            (self.heading - up * self.heading.dot(up)).normalize_or(up.any_orthonormal_vector());

        // Walk (on the ground), fall, collide.
        let vertical = self.velocity.dot(up);
        let horizontal = self.velocity - up * vertical;
        let horizontal = if self.on_ground {
            player::approach(horizontal, desired, 20.0 * dt)
        } else {
            horizontal
        };
        self.velocity = horizontal + up * (vertical - gravity * dt);
        let target = self.position + self.velocity * dt;
        let (position, blocked, axes) =
            player::collide(planet, world, self.position, target, self.kind.size());
        self.position = position;
        self.velocity = player::remove_blocked(self.velocity, axes, &blocked);
        self.on_ground = blocked.down;
        // Bumped into a step: hop up it.
        if self.on_ground && (blocked.u || blocked.v) && desired != DVec3::ZERO {
            self.velocity += up * (HOP_SPEED - self.velocity.dot(up));
            self.on_ground = false;
        }

        // Leg swing follows the speed over the ground.
        let ground_speed = (self.velocity - up * self.velocity.dot(up)).length();
        self.walk += (ground_speed * dt * 6.0) as f32;
        let target_swing = (ground_speed / 1.0).min(1.0) as f32;
        self.swing += (target_swing - self.swing) * (dt as f32 * 8.0).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::planet::Face;

    struct Ground(i32);

    impl Voxels for Ground {
        fn block(&self, _: Face, _: i32, _: i32, k: i32) -> Block {
            if k < self.0 { Block::Grass } else { Block::Air }
        }
    }

    #[test]
    fn wanders_on_the_ground_and_dies_to_hits() {
        let planet = Planet::new(DVec3::ZERO, 64, 128, 1);
        let ground = planet.sea_layer + 4;
        let up = DVec3::new(0.2, 0.9, 0.3).normalize();
        let start = up * planet.layer_radius(ground as f64 + 1.0);
        let mut cow = Mob::new(MobKind::Cow, start, 42);
        for _ in 0..1200 {
            cow.update(&planet, &Ground(ground), 9.81, 1.0 / 60.0);
        }
        let height = cow.position.length() - planet.layer_radius(ground as f64);
        assert!(height.abs() < 1e-3, "standing {height} m above the ground");
        // Two sword hits (6 each), half a second apart, kill a cow (10 health).
        let attacker = cow.position + cow.heading * 2.0;
        assert!(!cow.hit(6.0, attacker));
        assert!(!cow.hit(6.0, attacker), "invulnerable while flashing");
        for _ in 0..40 {
            cow.update(&planet, &Ground(ground), 9.81, 1.0 / 60.0);
        }
        assert!(cow.hit(6.0, attacker));
        // A ray at its middle hits it.
        let eye = cow.center() + cow.heading.cross(up) * 3.0;
        let hit = cow.ray_hit(eye, -cow.heading.cross(up), 5.0);
        assert!(hit.is_some_and(|t| (2.0..3.0).contains(&t)), "{hit:?}");
    }
}
