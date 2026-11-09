//! The survival game loop on top of the world: the inventory, armor and held item, mining
//! with tools, item drops, placing blocks and models (torches light up), crafting, furnace
//! and chest screens, health and hunger, and the animals.
//!
//! The world itself stays in `WorldStreamer` (blocks, including the cells that model blocks
//! occupy, and block light). This module keeps what lives on top of it: placed models (their
//! meshes), the furnaces' and chests' contents, items lying around, mobs, and the player's
//! inventory. Sounds are collected in `sounds` for the app to play.

pub mod crafting;
pub mod drops;
pub mod furnace;
pub mod inventory;
pub mod items;

use std::time::Instant;

use anyhow::Result;
use glam::DVec3;
use winit::{event::MouseButton, keyboard::KeyCode};

use crate::{
    app::input::Input,
    audio::Sound,
    effects::{self, Effects},
    mobs::{Mob, MobKind},
    net::protocol::{MobState, NetDrop, WorldEvent},
    player::{
        GameMode, HALF_WIDTH, HEIGHT, Voxels,
        interact::{self, Hit},
    },
    render::{
        DrawItem, Fade, MeshHandle, Renderer,
        models::{self, Light, ModelState},
    },
    ui::{
        Ui,
        screen::{self, CHEST_SLOTS, Layout, Screen, ScreenAnim, SlotId, View},
    },
    util::FastMap,
    world::{
        block::Block, chunk::BlockCell, planet::Planet, streaming::WorldStreamer,
        terrain::TerrainGenerator,
    },
};

use drops::Drop;
use furnace::Furnace;
use inventory::{HOTBAR, Inventory, SLOTS, Slot};
use items::{ArmorPiece, Item, ItemStack, Tier, ToolKind};

pub const MAX_HEALTH: f32 = 20.0;
pub const MAX_FOOD: f32 = 20.0;
/// Seconds between repeated actions while a mouse button is held.
const CREATIVE_BREAK_REPEAT: f32 = 0.2;
/// Minecraft's pause between two blocks broken in a row in survival.
const SURVIVAL_BREAK_DELAY: f32 = 0.25;
const PLACE_REPEAT: f32 = 0.25;
/// Seconds to eat something.
const EAT_TIME: f32 = 1.6;
/// A crafting table, furnace or chest screen closes beyond this distance (m).
const SCREEN_REACH: f64 = 8.0;
/// Models, drops and mobs are drawn within this distance (m).
const DRAW_DISTANCE: f64 = 256.0;
/// Animals: how many may be around the player, where they spawn and when they're forgotten.
const MOB_CAP: usize = 10;
const MOB_SPAWN_RANGE: (f64, f64) = (20.0, 56.0);
const MOB_DESPAWN: f64 = 128.0;
/// Torch and lit furnace light levels.
const TORCH_LIGHT: u8 = 14;
const FURNACE_LIGHT: u8 = 13;
/// Meters walked per footstep sound.
const STEP_LENGTH: f64 = 1.6;

/// This player's part in a multiplayer game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetSide {
    #[default]
    Offline,
    Host,
    Client,
}

/// What happened here that the other players need to hear about.
pub enum Outgoing {
    World(WorldEvent),
    /// A client asks for a drop.
    Pickup(u32),
    MobHit {
        owner: u32,
        id: u32,
        damage: f32,
        from: DVec3,
        creative: bool,
    },
    MobGone(u32),
}

/// A placed model block (crafting table, furnace, chest, torch).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Object {
    pub block: Block,
    pub state: ModelState,
    /// The block a torch stands on or hangs from: it falls off without it.
    support: Option<BlockCell>,
    #[serde(skip)]
    mesh: Option<MeshHandle>,
    /// What the mesh was built for (state, block light).
    #[serde(skip)]
    built: Option<(ModelState, u32)>,
    /// The sky light estimate at the model (costly to compute: terrain sampling).
    #[serde(skip)]
    sky: Option<u32>,
}

/// The parts of the world this module changes.
pub struct World<'a> {
    pub planet: &'a Planet,
    pub streamer: &'a mut WorldStreamer,
}

/// Where the player is and looks (planet-relative, body frame).
pub struct Pose {
    pub eye: DVec3,
    pub look: DVec3,
    pub feet: DVec3,
}

impl Pose {
    pub fn up(&self) -> DVec3 {
        self.feet.normalize_or(DVec3::Y)
    }
}

/// A mouse drag across slots that spreads the held stack (left: evenly, right: one each).
struct Drag {
    right: bool,
    slots: Vec<SlotId>,
    /// The held stack when the drag started.
    stack: ItemStack,
}

/// What survival keeps per world (home, the moon): placed models and their contents, items
/// lying around, and animals. The world you are not on keeps its own (see `swap_world`).
#[derive(Default)]
pub struct WorldState {
    furnaces: FastMap<BlockCell, Furnace>,
    chests: FastMap<BlockCell, [Slot; CHEST_SLOTS]>,
    objects: FastMap<BlockCell, Object>,
    drops: Vec<Drop>,
    mobs: Vec<Mob>,
    pending_lights: Vec<(BlockCell, u8)>,
}

/// One world's state in a saved game (maps as lists: JSON keys must be strings).
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct SavedWorldState {
    furnaces: Vec<(BlockCell, Furnace)>,
    chests: Vec<(BlockCell, Vec<Slot>)>,
    objects: Vec<(BlockCell, Object)>,
    drops: Vec<Drop>,
    mobs: Vec<Mob>,
}

impl SavedWorldState {
    fn new(
        furnaces: &FastMap<BlockCell, Furnace>,
        chests: &FastMap<BlockCell, [Slot; CHEST_SLOTS]>,
        objects: &FastMap<BlockCell, Object>,
        drops: &[Drop],
        mobs: &[Mob],
    ) -> Self {
        Self {
            furnaces: furnaces.iter().map(|(c, f)| (*c, *f)).collect(),
            chests: chests.iter().map(|(c, s)| (*c, s.to_vec())).collect(),
            objects: objects.iter().map(|(c, o)| (*c, o.clone())).collect(),
            drops: drops.to_vec(),
            // (Animals someone else is simulating aren't ours to keep.)
            mobs: mobs
                .iter()
                .filter(|m| m.remote.is_none())
                .cloned()
                .collect(),
        }
    }

    /// The light sources this world's models give off.
    fn lights(&self) -> Vec<(BlockCell, u8)> {
        self.objects
            .iter()
            .filter_map(|(cell, object)| match object.block {
                Block::Torch => Some((*cell, TORCH_LIGHT)),
                Block::Furnace if object.state.lit => Some((*cell, FURNACE_LIGHT)),
                _ => None,
            })
            .collect()
    }

    fn into_state(self) -> WorldState {
        let pending_lights = self.lights();
        WorldState {
            furnaces: self.furnaces.into_iter().collect(),
            chests: self
                .chests
                .into_iter()
                .map(|(cell, slots)| {
                    let mut chest = [None; CHEST_SLOTS];
                    for (to, from) in chest.iter_mut().zip(slots) {
                        *to = from;
                    }
                    (cell, chest)
                })
                .collect(),
            objects: self.objects.into_iter().collect(),
            drops: self.drops,
            mobs: self.mobs,
            pending_lights,
        }
    }
}

impl WorldState {
    pub fn clear_mobs(&mut self) {
        self.mobs.clear();
    }

    pub fn saved(&self) -> SavedWorldState {
        SavedWorldState::new(
            &self.furnaces,
            &self.chests,
            &self.objects,
            &self.drops,
            &self.mobs,
        )
    }

    pub fn restored(saved: SavedWorldState) -> Self {
        saved.into_state()
    }
}

/// The player's side of survival in a saved game.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SavedSurvival {
    inventory: Vec<Slot>,
    armor: [Slot; 4],
    selected: usize,
    /// What was held by the mouse or lying in a crafting grid: back in the inventory on load.
    carried: Vec<ItemStack>,
    health: f32,
    food: f32,
    saturation: f32,
    exhaustion: f32,
    food_timer: f32,
    world: SavedWorldState,
}

pub struct Survival {
    pub inventory: Inventory,
    /// Helmet, chestplate, leggings, boots.
    pub armor: [Slot; 4],
    pub selected: usize,
    /// When the selected slot last changed (its name shows above the hotbar for a while).
    pub selected_at: Instant,
    /// The stack held by the mouse in a screen.
    pub cursor: Slot,
    /// The crafting grid of the open screen (2x2 in the inventory, 3x3 at a table).
    pub grid: [Slot; 9],
    pub screen: Option<Screen>,
    pub furnaces: FastMap<BlockCell, Furnace>,
    pub chests: FastMap<BlockCell, [Slot; CHEST_SLOTS]>,
    pub objects: FastMap<BlockCell, Object>,
    pub drops: Vec<Drop>,
    pub mobs: Vec<Mob>,
    pub health: f32,
    /// Minecraft's hunger: food level (0-20, the bar), saturation (hidden reserve, used up
    /// first) and exhaustion (activity; every 4 costs a point of saturation or food).
    pub food: f32,
    pub saturation: f32,
    exhaustion: f32,
    food_timer: f32,
    /// Seconds spent eating the held food.
    pub eating: f32,
    /// Breaking a block: the cell, seconds spent and seconds needed.
    pub mining: Option<(BlockCell, f32, f32)>,
    mining_sound: f32,
    action_cooldown: f32,
    /// Weapon recharge: seconds left, and the full recharge time.
    pub attack: (f32, f32),
    /// What the crosshair is on (a block or an animal), for the HUD.
    pub target_name: Option<String>,
    /// Waiting to respawn: can't be hurt and picks nothing up (not even what just fell here).
    pub dead: bool,
    /// Gestures so far, for the first-person view: swings (hits, swings at nothing,
    /// creative breaks) and uses (placing, opening, putting on). Mining shows as `mining`.
    pub swings: u32,
    pub uses: u32,
    pub catalog: Vec<Item>,
    /// Sounds to play (sound, planet-relative position), drained by the app.
    pub sounds: Vec<(Sound, DVec3)>,
    /// Particle effects to spawn (planet-relative), drained by the app.
    pub effects: Effects,
    /// Light sources to add / remove in the world (cell, level).
    lights: Vec<(BlockCell, u8)>,
    /// Sources of a loaded game, waiting for the chunks around them (light can't spread
    /// through chunks that aren't there yet).
    pending_lights: Vec<(BlockCell, u8)>,
    drag: Option<Drag>,
    last_click: Option<(SlotId, Instant)>,
    walked: f64,
    spawn_timer: f32,
    /// Meshes to free (removed models, drops and mobs).
    garbage: Vec<MeshHandle>,
    rng: u64,
    /// This player's part in a multiplayer game, its number, and what to tell the others.
    pub net: NetSide,
    pub me: u32,
    next_id: u32,
    pub outbox: Vec<Outgoing>,
    /// The open chest or furnace as last seen (to notice changes).
    watched: Option<(BlockCell, Vec<Slot>)>,
}

impl Default for Survival {
    fn default() -> Self {
        Self {
            inventory: Inventory::default(),
            armor: [None; 4],
            selected: 0,
            selected_at: Instant::now(),
            cursor: None,
            grid: [None; 9],
            screen: None,
            furnaces: FastMap::default(),
            chests: FastMap::default(),
            objects: FastMap::default(),
            drops: Vec::new(),
            mobs: Vec::new(),
            health: MAX_HEALTH,
            food: MAX_FOOD,
            saturation: 5.0,
            exhaustion: 0.0,
            food_timer: 0.0,
            eating: 0.0,
            mining: None,
            mining_sound: 0.0,
            action_cooldown: 0.0,
            attack: (0.0, 0.25),
            target_name: None,
            dead: false,
            swings: 0,
            uses: 0,
            catalog: Item::all(),
            sounds: Vec::new(),
            effects: Effects::default(),
            lights: Vec::new(),
            pending_lights: Vec::new(),
            drag: None,
            last_click: None,
            walked: 0.0,
            spawn_timer: 1.0,
            garbage: Vec::new(),
            rng: 0x2545_f491_4f6c_dd1d,
            net: NetSide::Offline,
            me: 0,
            next_id: 0,
            outbox: Vec::new(),
            watched: None,
        }
    }
}

impl Survival {
    /// What a saved game keeps.
    pub fn saved(&self) -> SavedSurvival {
        SavedSurvival {
            inventory: self.inventory.slots.to_vec(),
            armor: self.armor,
            selected: self.selected,
            carried: self
                .cursor
                .iter()
                .chain(self.grid.iter().flatten())
                .copied()
                .collect(),
            health: self.health,
            food: self.food,
            saturation: self.saturation,
            exhaustion: self.exhaustion,
            food_timer: self.food_timer,
            world: SavedWorldState::new(
                &self.furnaces,
                &self.chests,
                &self.objects,
                &self.drops,
                &self.mobs,
            ),
        }
    }

    /// Back from a saved game; anything carried that no longer fits lands at `feet`.
    pub fn restore(&mut self, mut saved: SavedSurvival, feet: DVec3) {
        let state = std::mem::take(&mut saved.world).into_state();
        self.furnaces = state.furnaces;
        self.chests = state.chests;
        self.objects = state.objects;
        self.drops = state.drops;
        self.mobs = state.mobs;
        self.pending_lights = state.pending_lights;
        self.restore_player(saved, feet);
    }

    /// Just the player's side of a saved game.
    pub fn restore_player(&mut self, saved: SavedSurvival, feet: DVec3) {
        for (to, from) in self.inventory.slots.iter_mut().zip(saved.inventory) {
            *to = from;
        }
        self.armor = saved.armor;
        self.selected = saved.selected.min(HOTBAR - 1);
        self.health = saved.health;
        self.food = saved.food;
        self.saturation = saved.saturation;
        self.exhaustion = saved.exhaustion;
        self.food_timer = saved.food_timer;
        for stack in saved.carried {
            if let Some(rest) = self.inventory.add(stack) {
                self.add_drop(Drop::new(rest, feet, DVec3::ZERO, 0.0));
            }
        }
    }

    /// The player's side only (what the host keeps of someone who joined).
    pub fn saved_player(&self) -> SavedSurvival {
        let mut saved = self.saved();
        saved.world = SavedWorldState::default();
        saved
    }

    /// Back to an empty-handed, healthy player (the world stays).
    pub fn clear_player(&mut self) {
        self.inventory = Inventory::default();
        self.armor = [None; 4];
        self.selected = 0;
        self.cursor = None;
        self.grid = [None; 9];
        self.screen = None;
        self.health = MAX_HEALTH;
        self.food = MAX_FOOD;
        self.saturation = 5.0;
        self.exhaustion = 0.0;
        self.food_timer = 0.0;
    }

    pub fn clear_mobs(&mut self) {
        self.garbage
            .extend(self.mobs.drain(..).filter_map(|m| m.mesh));
    }

    // ---- Multiplayer -------------------------------------------------------------------

    /// A number for something new that no other player will pick.
    pub fn new_id(&mut self) -> u32 {
        self.next_id = (self.next_id + 1) & 0xff_ffff;
        self.me << 24 | self.next_id
    }

    /// Gives everything of both worlds a number (when a game opens to others).
    pub fn renumber(&mut self, other: &mut WorldState) {
        let mut drops = std::mem::take(&mut self.drops);
        let mut mobs = std::mem::take(&mut self.mobs);
        for drop in drops.iter_mut().chain(other.drops.iter_mut()) {
            drop.id = self.new_id();
        }
        for mob in mobs.iter_mut().chain(other.mobs.iter_mut()) {
            mob.id = self.new_id();
            mob.owner = self.me;
        }
        self.drops = drops;
        self.mobs = mobs;
    }

    fn tell(&mut self, event: WorldEvent) {
        if self.net != NetSide::Offline {
            self.outbox.push(Outgoing::World(event));
        }
    }

    /// Something else (a tornado) took this block away.
    pub fn announce_broken(&mut self, cell: BlockCell) {
        self.tell(WorldEvent::Broke(cell));
    }

    /// Puts an item on the ground, and tells the others.
    fn add_drop(&mut self, mut drop: Drop) {
        drop.id = self.new_id();
        self.tell(WorldEvent::DropNew(NetDrop {
            id: drop.id,
            stack: drop.stack,
            position: drop.position,
            velocity: drop.velocity,
            pickup_delay: drop.pickup_delay,
        }));
        self.drops.push(drop);
    }

    /// Trades this world's contents for `other`'s, nothing else (see `swap_world`).
    pub fn swap_contents(&mut self, other: &mut WorldState) {
        std::mem::swap(&mut self.furnaces, &mut other.furnaces);
        std::mem::swap(&mut self.chests, &mut other.chests);
        std::mem::swap(&mut self.objects, &mut other.objects);
        std::mem::swap(&mut self.drops, &mut other.drops);
        std::mem::swap(&mut self.mobs, &mut other.mobs);
        std::mem::swap(&mut self.pending_lights, &mut other.pending_lights);
    }

    /// Another player did something to this world. `here`: it is the world around us (so it
    /// can be seen); otherwise it only needs to be remembered.
    pub fn apply_event(&mut self, world: &mut World, event: &WorldEvent, here: bool) {
        match event {
            WorldEvent::Broke(cell) => self.remote_broke(world, *cell, here),
            WorldEvent::Placed {
                cell,
                block,
                state,
                support,
            } => self.remote_placed(world, *cell, *block, *state, *support, here),
            WorldEvent::Chest { cell, slots } => {
                if let Some(chest) = self.chests.get_mut(cell) {
                    for (to, from) in chest.iter_mut().zip(slots) {
                        *to = *from;
                    }
                    if self.watched.as_ref().is_some_and(|(c, _)| c == cell) {
                        self.watched = Some((*cell, chest.to_vec()));
                    }
                }
            }
            WorldEvent::Furnace { cell, furnace } => {
                if let Some(to) = self.furnaces.get_mut(cell) {
                    *to = *furnace;
                    if self.watched.as_ref().is_some_and(|(c, _)| c == cell) {
                        self.watched = Some((*cell, furnace_slots(furnace)));
                    }
                }
            }
            WorldEvent::DropNew(drop) => {
                if !self.drops.iter().any(|d| d.id == drop.id) {
                    let mut new =
                        Drop::new(drop.stack, drop.position, drop.velocity, drop.pickup_delay);
                    new.id = drop.id;
                    self.drops.push(new);
                }
            }
            WorldEvent::DropGone(id) => {
                if let Some(i) = self.drops.iter().position(|d| d.id == *id) {
                    let drop = self.drops.swap_remove(i);
                    self.garbage.extend(drop.mesh);
                }
            }
        }
    }

    fn remote_broke(&mut self, world: &mut World, cell: BlockCell, here: bool) {
        let before = world.streamer.block(cell);
        world.streamer.set_block(cell, Block::Air);
        if here && let Some(block) = before.filter(|b| b.is_solid()) {
            let center = interact::cell_center(world.planet, cell);
            let up = center.normalize_or(DVec3::Y);
            self.effects.push(effects::block_break(block, center, up));
        }
        if let Some(object) = self.objects.remove(&cell) {
            self.garbage.extend(object.mesh);
            if matches!(object.block, Block::Torch | Block::Furnace) {
                world.streamer.set_light(cell, 0);
            }
        }
        self.pending_lights.retain(|(c, _)| *c != cell);
        if matches!(self.screen, Some(Screen::Crafting(c) | Screen::Furnace(c) | Screen::Chest(c)) if c == cell)
        {
            self.screen = None;
        }
        self.furnaces.remove(&cell);
        self.chests.remove(&cell);
    }

    fn remote_placed(
        &mut self,
        world: &mut World,
        cell: BlockCell,
        block: Block,
        state: ModelState,
        support: Option<BlockCell>,
        here: bool,
    ) {
        world.streamer.set_block(cell, block);
        if here && !block.is_model() {
            let center = interact::cell_center(world.planet, cell);
            let up = center.normalize_or(DVec3::Y);
            self.effects.push(effects::block_place(block, center, up));
        }
        if !block.is_model() {
            return;
        }
        self.objects.insert(
            cell,
            Object {
                block,
                state,
                support,
                mesh: None,
                built: None,
                sky: None,
            },
        );
        match block {
            Block::Furnace => {
                self.furnaces.insert(cell, Furnace::default());
            }
            Block::Chest => {
                self.chests.insert(cell, [None; CHEST_SLOTS]);
            }
            // (Waits for the chunks around it, which may not be loaded here.)
            Block::Torch => self.pending_lights.push((cell, TORCH_LIGHT)),
            _ => {}
        }
    }

    /// Takes a drop off the ground (the host, for whoever asked first).
    pub fn take_drop(&mut self, id: u32) -> Option<ItemStack> {
        let i = self.drops.iter().position(|d| d.id == id)?;
        let drop = self.drops.swap_remove(i);
        self.garbage.extend(drop.mesh);
        Some(drop.stack)
    }

    /// The host gave us what we asked for.
    pub fn received(&mut self, stack: ItemStack, pose: &Pose) {
        let center = pose.feet + pose.up() * (HEIGHT * 0.5);
        self.sound(Sound::Pop, center);
        if let Some(rest) = self.inventory.add(stack) {
            self.throw(rest, pose);
        }
    }

    pub fn update_replica(&mut self, owner: u32, state: &MobState) {
        match self.mobs.iter_mut().find(|m| m.id == state.id) {
            Some(mob) => {
                if mob.remote.is_some() {
                    mob.apply_net(owner, state);
                }
            }
            None => self.mobs.push(Mob::replica(owner, state)),
        }
    }

    pub fn remove_replica(&mut self, id: u32) {
        if let Some(i) = self
            .mobs
            .iter()
            .position(|m| m.id == id && m.remote.is_some())
        {
            let mob = self.mobs.swap_remove(i);
            self.garbage.extend(mob.mesh);
        }
    }

    pub fn take_mob(&mut self, i: usize) -> Mob {
        let mut mob = self.mobs.swap_remove(i);
        self.garbage.extend(mob.mesh.take());
        mob
    }

    /// An animal handed to us (it replaces its copy here).
    pub fn adopt(&mut self, mob: Mob) {
        self.remove_replica(mob.id);
        self.mobs.push(mob);
    }

    /// Someone hit an animal we simulate.
    pub fn mob_hit(&mut self, id: u32, damage: f32, from: DVec3, creative: bool) {
        let Some(i) = self
            .mobs
            .iter()
            .position(|m| m.id == id && m.remote.is_none())
        else {
            return;
        };
        let (kind, at) = (self.mobs[i].kind, self.mobs[i].center());
        if self.mobs[i].hit(damage, from) {
            self.kill_mob(i, kind, at, creative);
        }
    }

    fn kill_mob(&mut self, i: usize, kind: MobKind, at: DVec3, creative: bool) {
        let mob = self.mobs.swap_remove(i);
        if self.net != NetSide::Offline {
            self.outbox.push(Outgoing::MobGone(mob.id));
        }
        self.garbage.extend(mob.mesh);
        if !creative {
            let mut roll = |n: u32| if n == 0 { 0 } else { self.rng_u32() % n };
            let loot = kind.loot(&mut roll);
            for stack in loot {
                self.spawn(stack, at, DVec3::ZERO, 0.5);
            }
        }
    }

    fn push_mob(&mut self, kind: MobKind, at: DVec3, seed: u64) {
        let mut mob = Mob::new(kind, at, seed);
        mob.id = self.new_id();
        mob.owner = self.me;
        self.mobs.push(mob);
    }

    /// Notices the open chest or furnace changing, for the others.
    fn watch_container(&mut self) {
        let open = match self.screen {
            Some(Screen::Chest(cell)) => self.chests.get(&cell).map(|c| (cell, c.to_vec())),
            Some(Screen::Furnace(cell)) => {
                self.furnaces.get(&cell).map(|f| (cell, furnace_slots(f)))
            }
            _ => None,
        };
        let Some((cell, slots)) = open else {
            self.watched = None;
            return;
        };
        let changed = self
            .watched
            .as_ref()
            .is_some_and(|(c, old)| *c == cell && *old != slots);
        if changed {
            match self.screen {
                Some(Screen::Furnace(_)) => {
                    if let Some(furnace) = self.furnaces.get(&cell).copied() {
                        self.tell(WorldEvent::Furnace { cell, furnace });
                    }
                }
                _ => self.tell(WorldEvent::Chest {
                    cell,
                    slots: slots.clone(),
                }),
            }
        }
        self.watched = Some((cell, slots));
    }

    /// Lights the sources of a loaded game whose surroundings have loaded. `camera` is
    /// planet-relative.
    pub fn apply_pending_lights(
        &mut self,
        planet: &Planet,
        streamer: &mut WorldStreamer,
        camera: DVec3,
    ) {
        if self.pending_lights.is_empty() {
            return;
        }
        // Everything within this distance of the camera is loaded.
        let loaded = streamer.loaded_radius() as f64;
        self.pending_lights.retain(|&(cell, level)| {
            let ready = (interact::cell_center(planet, cell) - camera).length() + 24.0 < loaded;
            if ready {
                streamer.set_light(cell, level);
            }
            !ready
        });
    }

    /// Trades the current world's state for `other`'s (moving to another world).
    pub fn swap_world(&mut self, other: &mut WorldState) {
        self.swap_contents(other);
        self.mining = None;
        self.target_name = None;
        if matches!(
            self.screen,
            Some(Screen::Furnace(_) | Screen::Chest(_) | Screen::Crafting(_))
        ) {
            self.screen = None;
        }
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn sound(&mut self, sound: Sound, position: DVec3) {
        self.sounds.push((sound, position));
    }

    pub fn held(&self) -> Slot {
        self.inventory.slots[self.selected]
    }

    pub fn tool(&self) -> Option<(ToolKind, Tier)> {
        self.held().and_then(|s| s.item.tool())
    }

    pub fn select(&mut self, slot: usize) {
        if slot != self.selected {
            self.selected = slot;
            self.selected_at = Instant::now();
            self.mining = None;
            self.eating = 0.0;
        }
    }

    /// Mining (or eating) progress (0..1), for the bar under the crosshair.
    pub fn progress(&self) -> f32 {
        if self.eating > 0.0 {
            return (self.eating / EAT_TIME).min(1.0);
        }
        self.mining_progress()
    }

    /// Mining progress (0..1) of the targeted block.
    pub fn mining_progress(&self) -> f32 {
        self.mining
            .map_or(0.0, |(_, spent, total)| (spent / total.max(1e-3)).min(1.0))
    }

    /// Armor points worn (0-20, Minecraft's armor bar).
    pub fn armor_points(&self) -> u32 {
        self.armor
            .iter()
            .flatten()
            .filter_map(|s| s.item.armor())
            .map(|(piece, tier)| tier.points(piece))
            .sum()
    }

    /// Activity that makes you hungry (Minecraft's exhaustion units).
    pub fn exert(&mut self, amount: f32) {
        self.exhaustion += amount;
    }

    /// Spawns items at a planet-relative position, flying apart a little.
    pub fn spawn(&mut self, stack: ItemStack, position: DVec3, velocity: DVec3, delay: f32) {
        let up = position.normalize_or(DVec3::Y);
        let jitter = DVec3::new(self.random(), self.random(), self.random()) - 0.5;
        let spread = (jitter - up * jitter.dot(up)) * 2.0;
        self.add_drop(Drop::new(
            stack,
            position,
            velocity + spread + up * 1.5,
            delay,
        ));
    }

    /// Throws a stack forward from the eyes.
    fn throw(&mut self, stack: ItemStack, pose: &Pose) {
        let position = pose.eye - pose.up() * 0.3 + pose.look * 0.3;
        self.add_drop(Drop::new(
            stack,
            position,
            pose.look * 5.0 + pose.up() * 1.0,
            1.5,
        ));
    }

    /// Throws one of the held item (or the whole stack) forward.
    pub fn throw_held(&mut self, pose: &Pose, all: bool) {
        let n = if all { 64 } else { 1 };
        if let Some(stack) = inventory::take(&mut self.inventory.slots[self.selected], n) {
            self.throw(stack, pose);
        }
    }

    // ---- Interacting with the world ---------------------------------------------------

    /// Breaks a block: it drops what the held tool harvests (nothing in creative), a model
    /// disappears, containers spill their contents, torches on it fall off.
    pub fn break_block(
        &mut self,
        world: &mut World,
        cell: BlockCell,
        block: Block,
        creative: bool,
    ) {
        world.streamer.set_block(cell, Block::Air);
        self.tell(WorldEvent::Broke(cell));
        let center = interact::cell_center(world.planet, cell);
        self.sound(Sound::Break(block.material()), center);
        let up = center.normalize_or(DVec3::Y);
        self.effects.push(effects::block_break(block, center, up));
        if let Some(object) = self.objects.remove(&cell) {
            self.garbage.extend(object.mesh);
            if matches!(object.block, Block::Torch | Block::Furnace) {
                world.streamer.set_light(cell, 0);
            }
        }
        if matches!(self.screen, Some(Screen::Crafting(c) | Screen::Furnace(c) | Screen::Chest(c)) if c == cell)
        {
            self.screen = None;
        }
        let mut loot: Vec<ItemStack> = Vec::new();
        if let Some(furnace) = self.furnaces.remove(&cell) {
            loot.extend(furnace.contents());
        }
        if let Some(chest) = self.chests.remove(&cell) {
            loot.extend(chest.into_iter().flatten());
        }
        if !creative {
            let roll = self.random() as f32;
            loot.extend(items::drops(block, self.tool(), roll));
        }
        for stack in loot {
            self.spawn(stack, center, DVec3::ZERO, 0.5);
        }
        // Torches standing on or hanging from it fall off.
        let orphans: Vec<BlockCell> = self
            .objects
            .iter()
            .filter(|(_, o)| o.support == Some(cell))
            .map(|(&c, _)| c)
            .collect();
        for torch in orphans {
            self.break_block(world, torch, Block::Torch, false);
        }
    }

    /// Puts a block (or model) into `cell`, held against `support`.
    fn place(
        &mut self,
        world: &mut World,
        cell: BlockCell,
        block: Block,
        support: BlockCell,
        pose: &Pose,
    ) -> bool {
        if block.is_vehicle_part() {
            return false; // they go on vehicles: `Game::craft_clicks`
        }
        let planet = world.planet;
        let center = interact::cell_center(planet, cell);
        let mut state = ModelState {
            facing: models::facing_towards(planet, cell, pose.feet - center),
            ..Default::default()
        };
        if block == Block::Torch {
            // Torches go on top of a block or on its side, never underneath.
            let below = planet.cell(cell.face, cell.u, cell.v, cell.k - 1);
            let above = planet.cell(cell.face, cell.u, cell.v, cell.k + 1);
            let solid = world.streamer.block(support).is_some_and(|b| b.is_opaque());
            if !solid || above == Some(support) {
                return false;
            }
            if below != Some(support) {
                let away = center - interact::cell_center(planet, support);
                state.facing = models::facing_towards(planet, cell, away);
                state.wall = true;
            }
        }
        world.streamer.set_block(cell, block);
        self.tell(WorldEvent::Placed {
            cell,
            block,
            state,
            support: (block == Block::Torch).then_some(support),
        });
        self.sound(Sound::Place(block.material()), center);
        if !block.is_model() {
            let up = center.normalize_or(DVec3::Y);
            self.effects.push(effects::block_place(block, center, up));
        }
        if block.is_model() {
            self.objects.insert(
                cell,
                Object {
                    block,
                    state,
                    support: (block == Block::Torch).then_some(support),
                    mesh: None,
                    built: None,
                    sky: None,
                },
            );
            match block {
                Block::Furnace => {
                    self.furnaces.insert(cell, Furnace::default());
                }
                Block::Chest => {
                    self.chests.insert(cell, [None; CHEST_SLOTS]);
                }
                Block::Torch => world.streamer.set_light(cell, TORCH_LIGHT),
                _ => {}
            }
        }
        true
    }

    /// The nearest animal on the line of sight within `reach`, if it's closer than the
    /// targeted block.
    fn mob_target(&self, pose: &Pose, reach: f64, block: Option<&Hit>) -> Option<usize> {
        let block_distance = block.map_or(f64::MAX, |hit| hit.distance);
        self.mobs
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.ray_hit(pose.eye, pose.look, reach).map(|t| (i, t)))
            .filter(|&(_, t)| t < block_distance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// What the crosshair is on: an animal (returned, as an index into `mobs`) beats the
    /// block `target`. Names it in `target_name`.
    pub fn aim(&mut self, pose: &Pose, reach: f64, target: Option<&Hit>) -> Option<usize> {
        let mob = self.mob_target(pose, reach, target);
        self.target_name = match (mob, target) {
            (Some(i), _) => Some(self.mobs[i].kind.name().to_string()),
            (None, Some(hit)) => Some(hit.block.name()),
            _ => None,
        };
        mob
    }

    /// Mouse and keys in the world: break / attack (left), use / place / eat / wear (right),
    /// pick block (middle), drop the held item (Q). Returns a message to show, if any.
    #[allow(clippy::too_many_arguments)]
    pub fn interact(
        &mut self,
        world: &mut World,
        input: &Input,
        mode: GameMode,
        pose: &Pose,
        target: Option<&Hit>,
        reach: f64,
        dt: f32,
    ) -> Option<String> {
        let creative = mode == GameMode::Creative;
        self.action_cooldown -= dt;
        self.attack.0 = (self.attack.0 - dt).max(0.0);
        let mut message = None;
        let mob = self.aim(pose, reach, target);
        let target = if mob.is_some() { None } else { target };

        // Q drops one of the held item, Ctrl+Q the whole stack.
        if input.pressed(KeyCode::KeyQ) {
            self.throw_held(pose, input.held(KeyCode::ControlLeft));
        }

        // Left: attack an animal, break the targeted block, or swing at nothing.
        if input.button_pressed(MouseButton::Left)
            && let Some(i) = mob
        {
            message = self.attack_mob(i, pose, creative);
        }
        if input.button_held(MouseButton::Left) && mob.is_none() {
            match target {
                Some(hit) if creative => {
                    if input.button_pressed(MouseButton::Left) || self.action_cooldown <= 0.0 {
                        self.swings += 1;
                        self.break_block(world, hit.cell, hit.block, true);
                        self.action_cooldown = CREATIVE_BREAK_REPEAT;
                    }
                }
                Some(hit) => {
                    if self.action_cooldown <= 0.0 {
                        let total = items::break_time(hit.block, self.tool());
                        let spent = match self.mining {
                            Some((c, t, _)) if c == hit.cell => t + dt,
                            _ => dt,
                        };
                        self.mining_sound -= dt;
                        if self.mining_sound <= 0.0 {
                            self.mining_sound = 0.25;
                            let at = interact::cell_center(world.planet, hit.cell);
                            self.sound(Sound::Dig(hit.block.material()), at);
                            // Chips fly off the face being hit.
                            let point = pose.eye + pose.look * hit.distance;
                            let normal = hit
                                .before
                                .map(|b| interact::cell_center(world.planet, b) - at)
                                .and_then(DVec3::try_normalize)
                                .unwrap_or(-pose.look);
                            let up = at.normalize_or(DVec3::Y);
                            self.effects
                                .push(effects::mining_chips(hit.block, point, normal, up));
                        }
                        if spent >= total {
                            self.break_block(world, hit.cell, hit.block, false);
                            self.mining = None;
                            self.action_cooldown = SURVIVAL_BREAK_DELAY;
                            self.exert(0.005);
                            // Tools wear 1 per block (swords 2: they aren't for digging).
                            let hard = items::mining(hit.block).hardness > 0.0;
                            let sword = matches!(self.tool(), Some((ToolKind::Sword, _)));
                            let uses = if !hard {
                                0
                            } else if sword {
                                2
                            } else {
                                1
                            };
                            message = self.wear_tool(uses);
                        } else {
                            self.mining = Some((hit.cell, spent, total));
                        }
                    }
                }
                None => {
                    self.mining = None;
                    if input.button_pressed(MouseButton::Left) {
                        self.swing();
                    }
                }
            }
        } else {
            self.mining = None;
        }

        // Right: eat, put on armor, use a crafting table / furnace / chest (sneak to place
        // against it instead), or place the held block.
        let held = self.held();
        let hungry = self.food < MAX_FOOD && mode == GameMode::Survival;
        if let Some(stack) = held
            && stack.item.food().is_some()
            && hungry
            && input.button_held(MouseButton::Right)
            && !target.is_some_and(|h| h.block.is_model() && !input.held(KeyCode::ShiftLeft))
        {
            self.eat(stack, pose, dt);
            return message;
        }
        self.eating = 0.0;
        let right = input.button_pressed(MouseButton::Right)
            || (input.button_held(MouseButton::Right) && self.action_cooldown <= 0.0);
        if input.button_pressed(MouseButton::Right)
            && let Some(stack) = held
            && let Some((piece, _)) = stack.item.armor()
        {
            // Wear it (swapping with what was worn).
            std::mem::swap(
                &mut self.armor[piece as usize],
                &mut self.inventory.slots[self.selected],
            );
            self.sound(Sound::Equip, pose.eye);
            self.uses += 1;
            return message;
        }
        if right && let Some(hit) = target {
            let sneaking = input.held(KeyCode::ShiftLeft);
            let usable = matches!(
                hit.block,
                Block::CraftingTable | Block::Furnace | Block::Chest
            );
            if usable && !sneaking {
                if input.button_pressed(MouseButton::Right) {
                    let screen = match hit.block {
                        Block::Furnace => Screen::Furnace(hit.cell),
                        Block::Chest => Screen::Chest(hit.cell),
                        _ => Screen::Crafting(hit.cell),
                    };
                    self.open(screen);
                    self.uses += 1;
                }
            } else if let (Some(cell), Some(stack)) = (hit.before, held)
                && let Some(block) = stack.item.block()
                && (block == Block::Torch || !overlaps_player(world.planet, cell, pose.feet))
            {
                if self.place(world, cell, block, hit.cell, pose) {
                    self.uses += 1;
                    if !creative {
                        self.inventory.take_one(self.selected);
                    }
                }
                self.action_cooldown = PLACE_REPEAT;
            }
        }

        // Middle: pick the targeted block into the hotbar.
        if input.button_pressed(MouseButton::Middle)
            && let Some(hit) = target
        {
            self.pick_block(Item::Block(hit.block), creative);
        }
        message
    }

    fn eat(&mut self, stack: ItemStack, pose: &Pose, dt: f32) {
        let before = self.eating;
        self.eating += dt;
        // A crunch every quarter second.
        if (before / 0.25).floor() != (self.eating / 0.25).floor() {
            self.sound(Sound::Eat, pose.eye);
        }
        if self.eating >= EAT_TIME {
            self.eating = 0.0;
            let (food, saturation) = stack.item.food().unwrap_or_default();
            self.food = (self.food + food).min(MAX_FOOD);
            self.saturation = (self.saturation + saturation).min(self.food);
            self.inventory.take_one(self.selected);
            self.sound(Sound::Burp, pose.eye);
        }
    }

    /// Hits an animal with the held item: full damage when the weapon has recharged.
    pub fn attack_mob(&mut self, i: usize, pose: &Pose, creative: bool) -> Option<String> {
        self.swings += 1;
        let (damage, speed) = self.held().map_or((1.0, 4.0), |s| s.item.attack());
        let charge = 1.0 - self.attack.0 / self.attack.1.max(1e-3);
        let damage = damage * (0.2 + 0.8 * charge * charge);
        self.attack = (1.0 / speed, 1.0 / speed);
        self.exert(0.1);
        let mob = &mut self.mobs[i];
        let (kind, at) = (mob.kind, mob.center());
        if mob.remote.is_some() {
            // Its owner decides what the hit does.
            let (owner, id) = (mob.owner, mob.id);
            self.outbox.push(Outgoing::MobHit {
                owner,
                id,
                damage,
                from: pose.feet,
                creative,
            });
            self.sound(Sound::MobHurt(kind), at);
        } else {
            let died = mob.hit(damage, pose.feet);
            self.sound(Sound::MobHurt(kind), at);
            if died {
                self.kill_mob(i, kind, at, creative);
            }
        }
        // Swords lose 1 durability per hit, other tools 2.
        match self.tool() {
            Some((ToolKind::Sword, _)) => self.wear_tool(1),
            Some(_) => self.wear_tool(2),
            None => None,
        }
    }

    fn rng_u32(&mut self) -> u32 {
        (self.random() * u32::MAX as f64) as u32
    }

    /// Wears the held tool; returns a message if it broke.
    fn wear_tool(&mut self, uses: u16) -> Option<String> {
        let slot = &mut self.inventory.slots[self.selected];
        let stack = slot.as_mut()?;
        let (kind, tier) = stack.item.tool()?;
        let _ = kind;
        stack.wear += uses;
        if stack.wear >= tier.durability() {
            let name = stack.item.name();
            *slot = None;
            return Some(format!("Your {name} broke!"));
        }
        None
    }

    /// Swinging at nothing still makes the weapon recharge (shown under the crosshair).
    fn swing(&mut self) {
        self.swings += 1;
        let (_, speed) = self.held().map_or((1.0, 4.0), |s| s.item.attack());
        self.attack = (1.0 / speed, 1.0 / speed);
    }

    fn pick_block(&mut self, item: Item, creative: bool) {
        let slots = &mut self.inventory.slots;
        if let Some(i) = slots[..HOTBAR]
            .iter()
            .position(|s| s.is_some_and(|s| s.item == item))
        {
            self.select(i);
        } else if let Some(i) = slots[HOTBAR..]
            .iter()
            .position(|s| s.is_some_and(|s| s.item == item))
        {
            slots.swap(self.selected, HOTBAR + i);
        } else if creative {
            slots[self.selected] = Some(ItemStack::new(item, item.max_stack()));
        }
    }

    // ---- Time passing ------------------------------------------------------------------

    /// Furnaces burn for `dt` game seconds (also what a jump in time skips over).
    pub fn run_furnaces(&mut self, dt: f64) {
        for (cell, furnace) in &mut self.furnaces {
            furnace.update(dt as f32);
            if let Some(object) = self.objects.get_mut(cell)
                && object.state.lit != furnace.lit()
            {
                object.state.lit = furnace.lit();
                let level = if furnace.lit() { FURNACE_LIGHT } else { 0 };
                self.lights.push((*cell, level));
            }
        }
    }

    /// Drops fall and get picked up, animals roam, furnaces burn, hunger and health tick.
    /// `dt` is game time.
    pub fn update(
        &mut self,
        planet: &Planet,
        voxels: &dyn Voxels,
        gravity: f64,
        pose: &Pose,
        mode: GameMode,
        dt: f64,
    ) -> Vec<String> {
        let mut messages = Vec::new();
        // Drops and mobs: a few substeps at most (time warp just makes them settle).
        let steps = (dt / 0.05).ceil().clamp(1.0, 20.0) as usize;
        let step = dt / steps as f64;
        let center = pose.feet + pose.up() * (HEIGHT * 0.5);
        let mut i = 0;
        while i < self.drops.len() {
            let drop = &mut self.drops[i];
            for _ in 0..steps {
                drop.update(planet, voxels, gravity, step as f32);
            }
            // (In a shared game the host says when: clocks differ.)
            let gone = drop.age > drops::LIFETIME && self.net != NetSide::Client;
            let near = (drop.position - center).length() < drops::PICKUP_RADIUS;
            drop.asked -= dt as f32;
            if near && drop.pickup_delay <= 0.0 && mode != GameMode::Spectator && !self.dead {
                if self.net == NetSide::Client {
                    // The host decides who gets it.
                    if drop.asked <= 0.0 {
                        drop.asked = 1.5;
                        let id = drop.id;
                        self.outbox.push(Outgoing::Pickup(id));
                    }
                } else {
                    match self.inventory.add(drop.stack) {
                        None => {
                            let drop = self.drops.swap_remove(i);
                            self.tell(WorldEvent::DropGone(drop.id));
                            self.garbage.extend(drop.mesh);
                            self.sound(Sound::Pop, center);
                            continue;
                        }
                        Some(rest) => self.drops[i].stack = rest,
                    }
                }
            }
            if gone {
                let drop = self.drops.swap_remove(i);
                if self.net == NetSide::Host {
                    self.tell(WorldEvent::DropGone(drop.id));
                }
                self.garbage.extend(drop.mesh);
                continue;
            }
            i += 1;
        }

        let mut i = 0;
        while i < self.mobs.len() {
            if self.mobs[i].remote.is_some() {
                if self.mobs[i].follow(dt.min(0.1)) {
                    i += 1;
                } else {
                    let mob = self.mobs.swap_remove(i);
                    self.garbage.extend(mob.mesh);
                }
                continue;
            }
            if (self.mobs[i].position - pose.feet).length() > MOB_DESPAWN {
                let mob = self.mobs.swap_remove(i);
                self.garbage.extend(mob.mesh);
                continue;
            }
            for _ in 0..steps {
                self.mobs[i].update(planet, voxels, gravity, step);
            }
            // Now and then they say something.
            if self.random() < dt * 0.08 {
                let (kind, at) = (self.mobs[i].kind, self.mobs[i].center());
                self.sound(Sound::Mob(kind), at);
            }
            i += 1;
        }

        self.run_furnaces(dt);
        if self.net != NetSide::Offline {
            self.watch_container();
        }
        if mode == GameMode::Survival
            && let Some(message) = self.metabolism(dt as f32, pose)
        {
            messages.push(message);
        }

        // Walked away from an open crafting table / furnace / chest.
        if let Some(Screen::Crafting(cell) | Screen::Furnace(cell) | Screen::Chest(cell)) =
            self.screen
            && (interact::cell_center(planet, cell) - pose.feet).length() > SCREEN_REACH
        {
            self.close(pose);
            messages.push("Too far away".to_string());
        }
        messages
    }

    /// Minecraft's hunger: exhaustion uses up saturation then food; a full stomach heals,
    /// an empty one hurts (down to half a heart).
    fn metabolism(&mut self, dt: f32, pose: &Pose) -> Option<String> {
        while self.exhaustion >= 4.0 {
            self.exhaustion -= 4.0;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else {
                self.food = (self.food - 1.0).max(0.0);
            }
        }
        self.food_timer += dt;
        let fast = self.saturation > 0.0 && self.food >= MAX_FOOD;
        let interval = if fast { 0.5 } else { 4.0 };
        while self.food_timer >= interval {
            self.food_timer -= interval;
            if self.health < MAX_HEALTH && (fast || self.food >= 18.0) {
                self.health = (self.health + 1.0).min(MAX_HEALTH);
                self.exert(6.0);
            } else if self.food <= 0.0 && self.health > 1.0 {
                self.health -= 1.0;
                self.sound(Sound::Hurt, pose.eye);
                return Some("You are starving!".to_string());
            }
        }
        None
    }

    /// Footsteps, and the hunger walking costs: `distance` walked on `ground` this frame.
    pub fn walked(&mut self, distance: f64, sprinting: bool, ground: Option<Block>, feet: DVec3) {
        if sprinting {
            self.exert(0.1 * distance as f32);
        }
        let Some(ground) = ground else {
            return;
        };
        self.walked += distance;
        if self.walked >= STEP_LENGTH {
            self.walked = 0.0;
            self.sound(Sound::Step(ground.material()), feet);
        }
    }

    /// Takes damage; returns true if the player died. Armor softens attacks (Minecraft's
    /// formula) but not falls or starvation (`bypass_armor`).
    pub fn hurt(&mut self, amount: f32, bypass_armor: bool, at: DVec3) -> bool {
        if self.dead {
            return false;
        }
        let mut amount = amount;
        if !bypass_armor {
            let points = self.armor_points() as f32;
            let toughness: f32 = self
                .armor
                .iter()
                .flatten()
                .filter_map(|s| s.item.armor())
                .map(|(_, tier)| tier.toughness())
                .sum();
            let reduction = (points / 5.0).max(points - amount / (2.0 + toughness / 4.0));
            amount *= 1.0 - reduction.min(20.0) / 25.0;
            for slot in &mut self.armor {
                if let Some(stack) = slot {
                    stack.wear += (amount / 4.0).max(1.0) as u16;
                    if stack.durability().is_some_and(|d| d <= 0.0) {
                        *slot = None;
                    }
                }
            }
        }
        self.health -= amount;
        self.sound(Sound::Hurt, at);
        self.health <= 0.0
    }

    /// Death: everything carried falls where the player was; health and hunger are reset.
    pub fn die(&mut self, pose: &Pose) {
        self.close(pose);
        let mut stacks: Vec<ItemStack> = self
            .inventory
            .slots
            .iter_mut()
            .filter_map(Option::take)
            .collect();
        stacks.extend(self.armor.iter_mut().filter_map(Option::take));
        let at = pose.feet + pose.up() * 0.5;
        for stack in stacks {
            self.spawn(stack, at, DVec3::ZERO, 1.0);
        }
        self.health = MAX_HEALTH;
        self.food = MAX_FOOD;
        self.saturation = 5.0;
        self.exhaustion = 0.0;
        self.mining = None;
    }

    /// Brings animals (in small groups) onto grass around the player, up to `MOB_CAP`.
    pub fn spawn_mobs(
        &mut self,
        planet: &Planet,
        terrain: &TerrainGenerator,
        voxels: &dyn Voxels,
        pose: &Pose,
        dt: f64,
    ) {
        self.spawn_timer -= dt as f32;
        if self.spawn_timer > 0.0 || self.mobs.len() >= MOB_CAP {
            return;
        }
        self.spawn_timer = 2.0;
        let up = pose.up();
        let a = up.any_orthonormal_vector();
        let b = up.cross(a);
        let angle = self.random() * std::f64::consts::TAU;
        let distance = MOB_SPAWN_RANGE.0 + self.random() * (MOB_SPAWN_RANGE.1 - MOB_SPAWN_RANGE.0);
        let spot = (pose.feet + (a * angle.cos() + b * angle.sin()) * distance).normalize();
        let ground = planet.sea_layer + terrain.sample_column(spot).height.floor() as i32;
        let Some(cell) = planet.cell_at(spot * planet.layer_radius(ground as f64 + 0.5)) else {
            return;
        };
        let block = |k: i32| voxels.block(cell.face, cell.u, cell.v, k);
        if block(cell.k) != Block::Grass || block(cell.k + 1) != Block::Air {
            return;
        }
        let kind = MobKind::ALL[(self.random() * 3.0) as usize % 3];
        let count = 1 + (self.random() * 3.0) as usize;
        let feet = spot * planet.layer_radius(cell.k as f64 + 1.0);
        for n in 0..count.min(MOB_CAP - self.mobs.len()) {
            let offset = (a * (n as f64 * 1.3) + b * (self.random() - 0.5)) * 1.5;
            let at = (feet + offset).normalize() * (feet.length() + 0.2);
            let seed = self.rng_u32() as u64 + 1;
            self.push_mob(kind, at, seed);
        }
    }

    /// Puts an animal at a planet-relative position (console `summon`).
    pub fn summon(&mut self, kind: MobKind, at: DVec3) {
        let seed = self.rng_u32() as u64 + 1;
        self.push_mob(kind, at, seed);
    }

    /// Light sources that changed (lit furnaces) go into the world.
    pub fn apply_lights(&mut self, streamer: &mut WorldStreamer) {
        for (cell, level) in self.lights.drain(..) {
            streamer.set_light(cell, level);
        }
    }

    // ---- Meshes ------------------------------------------------------------------------

    /// Uploads meshes for new or changed models, drops and (every frame, they animate) mobs,
    /// frees removed ones, and lists what to draw.
    pub fn draw_items(
        &mut self,
        planet: &Planet,
        terrain: &TerrainGenerator,
        streamer: &WorldStreamer,
        renderer: &mut Renderer,
        camera: DVec3,
    ) -> Result<Vec<DrawItem>> {
        for mesh in self.garbage.drain(..) {
            renderer.destroy_mesh(mesh);
        }
        let near = |p: DVec3| (planet.center + p - camera).length() < DRAW_DISTANCE;
        let light_at_block = |p: DVec3| Light {
            sky: 0,
            block: planet.cell_at(p).map_or(0, |c| streamer.light(c)) as u32,
        };
        let light_at = |p: DVec3| Light {
            sky: sky_light(planet, terrain, planet.cell_at(p)),
            ..light_at_block(p)
        };
        let mut draws = Vec::new();
        let open_chest = match self.screen {
            Some(Screen::Chest(cell)) => Some(cell),
            _ => None,
        };
        for (&cell, object) in &mut self.objects {
            let center = interact::cell_center(planet, cell);
            if !near(center) {
                continue;
            }
            object.state.open = open_chest == Some(cell);
            // Block light can change any time (torches nearby); the sky estimate is fixed.
            let block = planet.cell_at(center).map_or(0, |c| streamer.light(c)) as u32;
            let built = (object.state, block);
            if object.built != Some(built) {
                let sky = *object
                    .sky
                    .get_or_insert_with(|| sky_light(planet, terrain, planet.cell_at(center)));
                let light = Light { sky, block };
                let (mesh, _, _) =
                    models::block_model(planet, cell, object.block, object.state, light);
                if let Some(handle) = renderer.create_mesh(&mesh)? {
                    if let Some(old) = object.mesh.replace(handle) {
                        renderer.destroy_mesh(old);
                    }
                    object.built = Some(built);
                }
            }
            if let Some(mesh) = object.mesh {
                draws.push(DrawItem {
                    rotation: glam::Quat::IDENTITY,
                    mesh,
                    origin: planet.center + center,
                    radius: 1.6,
                    fade: Fade::Visible,
                });
            }
        }
        for drop in &mut self.drops {
            if !near(drop.position) {
                continue;
            }
            if drop.mesh.is_none() {
                let up = drop.position.normalize_or(DVec3::Y);
                let east = up.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
                let north = up.cross(east);
                let mesh =
                    models::drop_model(drop.stack.item, (east, north, up), light_at(drop.position));
                drop.mesh = renderer.create_mesh(&mesh)?;
            }
            if let Some(mesh) = drop.mesh {
                draws.push(DrawItem {
                    rotation: glam::Quat::IDENTITY,
                    mesh,
                    origin: planet.center + drop.draw_position(),
                    radius: 0.5,
                    fade: Fade::Visible,
                });
            }
        }
        for mob in &mut self.mobs {
            if !near(mob.position) {
                continue;
            }
            let up = mob.position.normalize_or(DVec3::Y);
            let mesh = crate::mobs::models::mob_model(
                mob.kind,
                &mob.pose(),
                (mob.heading, up),
                // Animals live on open grass: full sky light.
                Light {
                    sky: 15,
                    ..light_at_block(mob.center())
                },
            );
            // Keep the old mesh if this frame's upload budget is used up.
            if let Some(handle) = renderer.create_mesh(&mesh)?
                && let Some(old) = mob.mesh.replace(handle)
            {
                renderer.destroy_mesh(old);
            }
            if let Some(mesh) = mob.mesh {
                draws.push(DrawItem {
                    rotation: glam::Quat::IDENTITY,
                    mesh,
                    origin: planet.center + mob.position,
                    radius: 2.0,
                    fade: Fade::Visible,
                });
            }
        }
        Ok(draws)
    }

    // ---- Screens -----------------------------------------------------------------------

    pub fn open(&mut self, screen: Screen) {
        self.screen = Some(screen);
        self.mining = None;
        self.drag = None;
    }

    /// Closes the screen: the crafting grid and the held stack go back to the inventory
    /// (or onto the ground if it's full).
    pub fn close(&mut self, pose: &Pose) {
        if self.screen.take().is_none() {
            return;
        }
        self.drag = None;
        let mut back: Vec<ItemStack> = self.grid.iter_mut().filter_map(Option::take).collect();
        back.extend(self.cursor.take());
        for stack in back {
            if let Some(rest) = self.inventory.add(stack) {
                self.throw(rest, pose);
            }
        }
    }

    fn layout(&self, screen: Screen, ui: (f32, f32, f32)) -> Layout {
        Layout::new(screen, ui.0, ui.1, ui.2, self.catalog.len())
    }

    /// The contents of a slot of the open screen.
    fn slot(&self, id: SlotId) -> Slot {
        match id {
            SlotId::Inv(i) => self.inventory.slots[i],
            SlotId::Grid(i) => self.grid[i],
            SlotId::Armor(i) => self.armor[i],
            SlotId::Chest(i) => match self.screen? {
                Screen::Chest(cell) => self.chests.get(&cell)?[i],
                _ => None,
            },
            SlotId::Output => {
                let width = self.screen?.grid_width();
                if width == 0 {
                    return None;
                }
                let items: Vec<Option<Item>> = self.grid[..width * width]
                    .iter()
                    .map(|s| s.map(|s| s.item))
                    .collect();
                crafting::craft(&items, width)
            }
            SlotId::FurnaceIn | SlotId::FurnaceFuel | SlotId::FurnaceOut => {
                let furnace = self.open_furnace()?;
                match id {
                    SlotId::FurnaceIn => furnace.input,
                    SlotId::FurnaceFuel => furnace.fuel,
                    _ => furnace.output,
                }
            }
            SlotId::Catalog(i) => self
                .catalog
                .get(i)
                .map(|&item| ItemStack::new(item, item.max_stack())),
        }
    }

    fn open_furnace(&self) -> Option<&Furnace> {
        match self.screen? {
            Screen::Furnace(cell) => self.furnaces.get(&cell),
            _ => None,
        }
    }

    /// The storage behind a slot (not the crafting result or catalog, which are computed).
    fn slot_mut(&mut self, id: SlotId) -> Option<&mut Slot> {
        let screen = self.screen?;
        match id {
            SlotId::Inv(i) => Some(&mut self.inventory.slots[i]),
            SlotId::Grid(i) => Some(&mut self.grid[i]),
            SlotId::Armor(i) => Some(&mut self.armor[i]),
            SlotId::Chest(i) => match screen {
                Screen::Chest(cell) => self.chests.get_mut(&cell).map(|c| &mut c[i]),
                _ => None,
            },
            SlotId::FurnaceIn | SlotId::FurnaceFuel | SlotId::FurnaceOut => {
                let Screen::Furnace(cell) = screen else {
                    return None;
                };
                let furnace = self.furnaces.get_mut(&cell)?;
                Some(match id {
                    SlotId::FurnaceIn => &mut furnace.input,
                    SlotId::FurnaceFuel => &mut furnace.fuel,
                    _ => &mut furnace.output,
                })
            }
            SlotId::Output | SlotId::Catalog(_) => None,
        }
    }

    /// Whether `stack` may go into slot `id` (fuel slots take fuel, armor slots their piece,
    /// results take nothing).
    fn accepts(&self, id: SlotId, stack: &ItemStack) -> bool {
        match id {
            SlotId::FurnaceFuel => stack.item.fuel().is_some(),
            SlotId::Armor(i) => stack
                .item
                .armor()
                .is_some_and(|(p, _)| p == ArmorPiece::ALL[i]),
            SlotId::Output | SlotId::FurnaceOut | SlotId::Catalog(_) => false,
            _ => true,
        }
    }

    /// Mouse and keys while a screen is open. `ui` is (width, height, GUI scale); returns
    /// true when the screen should close.
    pub fn screen_input(
        &mut self,
        input: &Input,
        mouse: (f32, f32),
        ui: (f32, f32, f32),
        pose: &Pose,
        creative: bool,
    ) -> bool {
        let Some(screen) = self.screen else {
            return false;
        };
        if input.pressed(KeyCode::KeyE) || input.pressed(KeyCode::Escape) {
            return true;
        }
        let layout = self.layout(screen, ui);
        let hovered = layout.slot_at(mouse.0, mouse.1);
        let shift = input.held(KeyCode::ShiftLeft) || input.held(KeyCode::ShiftRight);

        // A drag in progress: spread over the slots it crosses, finish when released.
        if let Some(drag) = &self.drag {
            let button = if drag.right {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            if input.button_held(button) {
                if let Some(id) = hovered
                    && !drag.slots.contains(&id)
                    && self.drag_accepts(id, &drag.stack)
                {
                    let right = drag.right;
                    if let Some(d) = &mut self.drag {
                        d.slots.push(id);
                    }
                    if right {
                        self.click(id, true, false);
                    }
                }
            } else {
                self.finish_drag();
            }
            return false;
        }

        for (button, right) in [(MouseButton::Left, false), (MouseButton::Right, true)] {
            if !input.button_pressed(button) {
                continue;
            }
            match hovered {
                Some(id) => {
                    // Double click: gather that item onto the cursor.
                    let double = !right
                        && self
                            .last_click
                            .is_some_and(|(s, t)| s == id && t.elapsed().as_secs_f32() < 0.3);
                    self.last_click = (!right).then(|| (id, Instant::now()));
                    if double && self.cursor.is_some() && !shift {
                        self.gather();
                        self.last_click = None;
                    } else if !shift
                        && let Some(stack) = self.cursor
                        && self.drag_accepts(id, &stack)
                    {
                        // With a stack held, a press starts a (possible) drag.
                        self.drag = Some(Drag {
                            right,
                            slots: vec![id],
                            stack,
                        });
                        if right {
                            self.click(id, true, false);
                        }
                    } else {
                        self.click(id, right, shift);
                    }
                }
                // Outside the panel: drop (creative: delete) the held stack.
                None if !layout.contains(mouse.0, mouse.1) => {
                    let n = if right { 1 } else { 64 };
                    if let Some(stack) = inventory::take(&mut self.cursor, n)
                        && !creative
                    {
                        self.throw(stack, pose);
                    }
                }
                None => {}
            }
        }
        // Middle click sorts the inventory.
        if input.button_pressed(MouseButton::Middle) {
            self.inventory.sort();
        }
        if let Some(id) = hovered {
            // Number keys swap the hovered slot with that hotbar slot.
            for (n, key) in DIGITS.iter().enumerate() {
                if input.pressed(*key) {
                    self.swap_with_hotbar(id, n);
                }
            }
            // Q drops from the hovered slot (Ctrl: the whole stack).
            if input.pressed(KeyCode::KeyQ)
                && !matches!(id, SlotId::Output | SlotId::Catalog(_))
                && let Some(slot) = self.slot_mut(id)
            {
                let n = if input.held(KeyCode::ControlLeft) {
                    64
                } else {
                    1
                };
                if let Some(stack) = inventory::take(slot, n) {
                    self.throw(stack, pose);
                }
            }
        }
        false
    }

    /// Whether a drag carrying `stack` can include slot `id`.
    fn drag_accepts(&self, id: SlotId, stack: &ItemStack) -> bool {
        self.accepts(id, stack)
            && match self.slot(id) {
                None => true,
                Some(s) => s.stacks_with(stack) && s.count < s.item.max_stack(),
            }
    }

    /// Ends a drag: a left drag over several slots splits the held stack evenly between them
    /// (the rest stays held); over one slot it was just a click.
    fn finish_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if drag.right {
            return; // already placed one per slot
        }
        if drag.slots.len() == 1 {
            self.click(drag.slots[0], false, false);
            return;
        }
        let Some(held) = self.cursor else { return };
        let each = (held.count as usize / drag.slots.len()).max(1) as u8;
        for id in drag.slots {
            let Some(mut held) = self.cursor else { break };
            let n = each.min(held.count);
            let Some(slot) = self.slot_mut(id) else {
                continue;
            };
            let placed = match slot {
                None => {
                    *slot = Some(ItemStack { count: n, ..held });
                    n
                }
                Some(s) => {
                    let room = s.item.max_stack() - s.count;
                    let n = n.min(room);
                    s.count += n;
                    n
                }
            };
            held.count -= placed;
            self.cursor = (held.count > 0).then_some(held);
        }
    }

    /// Double click: pulls every matching item in the screen onto the held stack.
    fn gather(&mut self) {
        let Some(mut held) = self.cursor else { return };
        let ids: Vec<SlotId> = (0..SLOTS)
            .map(SlotId::Inv)
            .chain((0..CHEST_SLOTS).map(SlotId::Chest))
            .collect();
        for id in ids {
            if held.count >= held.item.max_stack() {
                break;
            }
            let Some(slot) = self.slot_mut(id) else {
                continue;
            };
            if let Some(s) = slot
                && s.stacks_with(&held)
            {
                let n = (held.item.max_stack() - held.count).min(s.count);
                held.count += n;
                inventory::take(slot, n);
            }
        }
        self.cursor = Some(held);
    }

    fn click(&mut self, id: SlotId, right: bool, shift: bool) {
        match id {
            SlotId::Output => self.take_crafted(shift),
            SlotId::Catalog(i) => {
                let Some(&item) = self.catalog.get(i) else {
                    return;
                };
                let full = ItemStack::new(item, item.max_stack());
                if shift {
                    self.inventory.add(full);
                } else if self.cursor.is_some() {
                    self.cursor = None; // clicking the catalog with a stack deletes it
                } else {
                    self.cursor = Some(if right { ItemStack::one(item) } else { full });
                }
            }
            _ if shift => self.quick_move(id),
            SlotId::FurnaceOut => {
                // Results can be taken, not put in.
                let mut cursor = self.cursor.take();
                if let Some(slot) = self.slot_mut(id) {
                    match (&mut cursor, *slot) {
                        (None, _) => cursor = slot.take(),
                        (Some(held), Some(out)) if held.stacks_with(&out) => {
                            *slot = inventory::merge(held, out);
                        }
                        _ => {}
                    }
                }
                self.cursor = cursor;
            }
            _ => {
                if let Some(held) = self.cursor
                    && !self.accepts(id, &held)
                {
                    return;
                }
                let mut cursor = self.cursor.take();
                if let Some(slot) = self.slot_mut(id) {
                    inventory::click(slot, &mut cursor, right);
                }
                self.cursor = cursor;
            }
        }
    }

    /// Shift-click: move a stack across, between the inventory and the screen's other slots
    /// (into a chest, onto the furnace, into an armor slot, or between hotbar and inventory).
    fn quick_move(&mut self, id: SlotId) {
        let Some(stack) = self.slot_mut(id).and_then(|s| s.take()) else {
            return;
        };
        let main_then_hotbar: Vec<usize> = (HOTBAR..SLOTS).chain(0..HOTBAR).collect();
        let rest = match (id, self.screen) {
            (SlotId::Inv(i), Some(Screen::Chest(cell))) => {
                let order: Vec<usize> = (0..CHEST_SLOTS).collect();
                match self.chests.get_mut(&cell) {
                    Some(chest) => inventory::insert(chest, stack, &order),
                    None => self.move_within_inventory(i, stack),
                }
            }
            (SlotId::Inv(i), Some(Screen::Furnace(_)))
                if stack.item.smelted().is_some() || stack.item.fuel().is_some() =>
            {
                let target = if stack.item.smelted().is_some() {
                    SlotId::FurnaceIn
                } else {
                    SlotId::FurnaceFuel
                };
                let rest = match self.slot_mut(target) {
                    Some(slot) => inventory::insert(std::slice::from_mut(slot), stack, &[0]),
                    None => Some(stack),
                };
                // A full furnace slot: move within the inventory instead.
                match rest {
                    Some(rest) => self.move_within_inventory(i, rest),
                    None => None,
                }
            }
            (SlotId::Inv(_), Some(Screen::Inventory))
                if stack
                    .item
                    .armor()
                    .is_some_and(|(p, _)| self.armor[p as usize].is_none()) =>
            {
                let (piece, _) = stack.item.armor().expect("checked");
                self.armor[piece as usize] = Some(stack);
                None
            }
            (SlotId::Inv(i), _) => self.move_within_inventory(i, stack),
            _ => inventory::insert(&mut self.inventory.slots, stack, &main_then_hotbar),
        };
        if let Some(rest) = rest
            && let Some(slot) = self.slot_mut(id)
        {
            *slot = Some(rest);
        }
    }

    /// Hotbar -> main inventory and back.
    fn move_within_inventory(&mut self, from: usize, stack: ItemStack) -> Option<ItemStack> {
        let order: Vec<usize> = if from < HOTBAR {
            (HOTBAR..SLOTS).collect()
        } else {
            (0..HOTBAR).collect()
        };
        inventory::insert(&mut self.inventory.slots, stack, &order)
    }

    fn swap_with_hotbar(&mut self, id: SlotId, n: usize) {
        match id {
            SlotId::Catalog(_) => self.inventory.slots[n] = self.slot(id),
            SlotId::Output => {
                if self.inventory.slots[n].is_none() {
                    let result = self.slot(id);
                    if result.is_some() {
                        self.consume_grid();
                        self.inventory.slots[n] = result;
                    }
                }
            }
            SlotId::Inv(i) => self.inventory.slots.swap(i, n),
            _ => {
                if self.inventory.slots[n].is_some_and(|s| !self.accepts(id, &s)) {
                    return;
                }
                let hotbar = self.inventory.slots[n].take();
                if let Some(slot) = self.slot_mut(id) {
                    let other = std::mem::replace(slot, hotbar);
                    self.inventory.slots[n] = other;
                } else {
                    self.inventory.slots[n] = hotbar;
                }
            }
        }
    }

    /// Uses up one of each item in the crafting grid.
    fn consume_grid(&mut self) {
        for slot in &mut self.grid {
            inventory::take(slot, 1);
        }
    }

    /// Takes the crafting result: onto the cursor, or (shift) as many as possible into the
    /// inventory.
    fn take_crafted(&mut self, shift: bool) {
        if shift {
            let order: Vec<usize> = (HOTBAR..SLOTS).chain(0..HOTBAR).collect();
            for _ in 0..64 {
                let Some(result) = self.slot(SlotId::Output) else {
                    break;
                };
                let mut trial = self.inventory.slots;
                if inventory::insert(&mut trial, result, &order).is_some() {
                    break;
                }
                self.inventory.slots = trial;
                self.consume_grid();
            }
            return;
        }
        let Some(result) = self.slot(SlotId::Output) else {
            return;
        };
        match &mut self.cursor {
            None => self.cursor = Some(result),
            Some(held)
                if held.stacks_with(&result)
                    && held.count + result.count <= held.item.max_stack() =>
            {
                held.count += result.count;
            }
            _ => return,
        }
        self.consume_grid();
    }

    /// Draws the open screen.
    /// The open screen, or the last one fading out.
    pub fn draw_screen(&self, ui: &mut Ui, mouse: (f32, f32), anim: &mut ScreenAnim, dt: f32) {
        let Some(screen) = self.screen else {
            anim.draw_closing(ui, dt);
            return;
        };
        let layout = self.layout(screen, (ui.width, ui.height, ui.scale));
        let slot = |id: SlotId| self.slot(id);
        let view = View {
            slot: &slot,
            cursor: self.cursor,
            mouse,
            furnace: self.open_furnace(),
            catalog: &self.catalog,
        };
        screen::draw(ui, screen, &layout, &view, anim, dt);
    }
}

fn furnace_slots(furnace: &Furnace) -> Vec<Slot> {
    vec![furnace.input, furnace.fuel, furnace.output]
}

const DIGITS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Whether a block in `cell` would overlap the player standing at `feet`.
fn overlaps_player(planet: &Planet, cell: BlockCell, feet: DVec3) -> bool {
    let up = feet.normalize_or(DVec3::Y);
    let offset = interact::cell_center(planet, cell) - feet;
    let vertical = offset.dot(up);
    let horizontal = (offset - up * vertical).length();
    vertical > -0.5 && vertical < HEIGHT + 0.5 && horizontal < HALF_WIDTH + 0.5
}

/// The mesher's fake sky light (15 in the open, darker below the column's surface).
pub(crate) fn sky_light(
    planet: &Planet,
    terrain: &TerrainGenerator,
    cell: Option<BlockCell>,
) -> u32 {
    let Some(cell) = cell else { return 15 };
    if planet.res_at(cell.k) != planet.face_blocks {
        return 1;
    }
    let column = terrain.sample_column(planet.column_dir(cell.face, cell.u, cell.v));
    let depth = planet.sea_layer + column.height.floor() as i32 - cell.k;
    if depth < 0 {
        15
    } else {
        (13 - depth).clamp(1, 13) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survival::items::ArmorTier;

    fn stack(item: Item, n: u8) -> Slot {
        Some(ItemStack::new(item, n))
    }

    fn pose() -> Pose {
        Pose {
            eye: DVec3::Y * 100.0,
            look: DVec3::X,
            feet: DVec3::Y * 98.0,
        }
    }

    #[test]
    fn crafting_in_the_inventory_grid() {
        let mut s = Survival {
            screen: Some(Screen::Inventory),
            ..Default::default()
        };
        s.grid[1] = stack(Item::Block(Block::OakLog), 3);
        assert_eq!(
            s.slot(SlotId::Output),
            stack(Item::Block(Block::OakPlanks), 4)
        );
        // Click: 4 planks on the cursor, one log used.
        s.click(SlotId::Output, false, false);
        assert_eq!(s.cursor, stack(Item::Block(Block::OakPlanks), 4));
        assert_eq!(s.grid[1], stack(Item::Block(Block::OakLog), 2));
        // Shift-click crafts the rest straight into the inventory.
        s.click(SlotId::Output, false, true);
        assert_eq!(s.grid[1], None);
        assert_eq!(
            s.inventory.slots[HOTBAR],
            stack(Item::Block(Block::OakPlanks), 8)
        );
        // Closing puts the cursor stack back too (merging into the stack already there).
        s.close(&pose());
        assert_eq!(s.cursor, None);
        assert_eq!(
            s.inventory.slots[HOTBAR],
            stack(Item::Block(Block::OakPlanks), 12)
        );
    }

    #[test]
    fn shift_click_feeds_the_furnace_and_fills_chests() {
        let cell = BlockCell {
            face: crate::world::planet::Face(0),
            u: 1,
            v: 2,
            k: 3,
        };
        let mut s = Survival {
            screen: Some(Screen::Furnace(cell)),
            ..Default::default()
        };
        s.furnaces.insert(cell, Furnace::default());
        s.inventory.slots[0] = stack(Item::RawIron, 5);
        s.inventory.slots[1] = stack(Item::Coal, 2);
        s.click(SlotId::Inv(0), false, true);
        s.click(SlotId::Inv(1), false, true);
        let f = s.furnaces[&cell];
        assert_eq!(f.input, stack(Item::RawIron, 5));
        assert_eq!(f.fuel, stack(Item::Coal, 2));
        // Non-fuel can't go into the fuel slot.
        s.cursor = stack(Item::Block(Block::Dirt), 1);
        s.click(SlotId::FurnaceFuel, false, false);
        assert_eq!(s.cursor, stack(Item::Block(Block::Dirt), 1));

        // Chests: shift-click moves stacks in and out.
        s.cursor = None;
        s.chests.insert(cell, [None; CHEST_SLOTS]);
        s.screen = Some(Screen::Chest(cell));
        s.inventory.slots[3] = stack(Item::Stick, 10);
        s.click(SlotId::Inv(3), false, true);
        assert_eq!(s.chests[&cell][0], stack(Item::Stick, 10));
        assert_eq!(s.inventory.slots[3], None);
        s.click(SlotId::Chest(0), false, true);
        assert_eq!(s.inventory.slots[HOTBAR], stack(Item::Stick, 10));
    }

    #[test]
    fn dragging_spreads_stacks() {
        let mut s = Survival {
            screen: Some(Screen::Inventory),
            ..Default::default()
        };
        // Left drag over 3 slots with 10 items: 3 each, 1 left held.
        s.cursor = stack(Item::Stick, 10);
        s.drag = Some(Drag {
            right: false,
            slots: vec![SlotId::Inv(9), SlotId::Inv(10), SlotId::Inv(11)],
            stack: ItemStack::new(Item::Stick, 10),
        });
        s.finish_drag();
        for i in 9..12 {
            assert_eq!(s.inventory.slots[i], stack(Item::Stick, 3));
        }
        assert_eq!(s.cursor, stack(Item::Stick, 1));
        // Double click gathers them back.
        s.gather();
        assert_eq!(s.cursor, stack(Item::Stick, 10));
        assert!(s.inventory.slots.iter().all(Option::is_none));
    }

    #[test]
    fn hunger_and_armor() {
        let mut s = Survival::default();
        // Sprinting 400 m burns 40 exhaustion: the saturation reserve, then 5 food.
        s.walked(400.0, true, None, DVec3::Y);
        s.metabolism(0.0, &pose());
        assert_eq!((s.saturation, s.food), (0.0, 15.0));
        // Diamond armor (20 points, toughness 8) cuts a 10 point hit to 3.
        for piece in ArmorPiece::ALL {
            s.armor[piece as usize] = stack(Item::Armor(piece, ArmorTier::Diamond), 1);
        }
        assert_eq!(s.armor_points(), 20);
        s.hurt(10.0, false, DVec3::Y);
        assert!((s.health - 17.0).abs() < 1e-4, "{}", s.health);
        // Falls ignore armor.
        s.hurt(5.0, true, DVec3::Y);
        assert!((s.health - 12.0).abs() < 1e-4);
    }
}
