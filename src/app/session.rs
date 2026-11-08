//! Worlds as the player knows them: the main menu's backdrop, creating, saving, loading and
//! leaving worlds, the pause menu and the settings.
//!
//! `Game` holds one world at a time. Switching replaces all of the world's parts in place
//! (`install_world`), while the window, renderer and sound stay. The main menu is a world
//! too: the default planet, seen from orbit (`backdrop`).
//!
//! Work that takes more than a frame is announced first: `Opening` shows the loading screen
//! for a frame before the world is built, and the screen stays until chunks and weather are
//! ready, with the world frozen behind it.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use glam::{DVec3, Quat};
use serde::{Deserialize, Serialize};
use winit::window::Fullscreen;

use super::multiplayer::Role;
use super::{
    EYE_HEIGHT, Game, SEED, WorldParts, WorldSpec, build_world, flight::SavedFlight,
    multiplayer::JoinedWorld,
};
use crate::{
    app::save::{self, SAVE_VERSION, WorldMeta},
    audio::Ambience,
    player::{GameMode, SavedPlayer},
    space::bodies::Body,
    survival::{SavedSurvival, SavedWorldState, Survival, WorldState},
    ui::{
        Ui,
        menu::{Action, Menu, MenuContext, MenuSession, Page},
    },
    world::{chunk::BlockCell, edits::WorldEdits},
};

/// Seconds between automatic saves.
const AUTOSAVE: f64 = 180.0;
/// Game seconds per real second behind the main menu.
const BACKDROP_TIME: f64 = 4.0;
/// The menu camera's height above home's sea level (m).
const BACKDROP_ALTITUDE: f64 = 34_000.0;
/// Give up waiting for a world to finish loading after this long (s).
const LOADING_TIMEOUT: f32 = 15.0;
/// The loading screen shows at least this long, so the first frames have had time to ask
/// for what they need (s).
const LOADING_MIN: f32 = 0.5;
/// Chunks must have loaded this far around the player before the world is shown (m).
const LOADED_ENOUGH: f32 = 48.0;

/// The world being played, if it is a saved one.
pub(super) struct Session {
    pub dir: PathBuf,
    pub meta: WorldMeta,
    last_save: Instant,
    /// What the players who joined this world had, by who they are.
    players: BTreeMap<String, String>,
}

impl Session {
    fn new(dir: PathBuf, meta: WorldMeta) -> Self {
        // (Worlds saved before multiplayer have no such file.)
        let players = std::fs::read(dir.join("players.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            dir,
            meta,
            last_save: Instant::now(),
            players,
        }
    }
}

/// Everything about a world except its blocks.
#[derive(Serialize, Deserialize)]
pub(super) struct SavedGame {
    version: u32,
    time: f64,
    active: Body,
    player: SavedPlayer,
    /// Level frame, yaw and pitch.
    camera: (Quat, f32, f32),
    spawn_feet: DVec3,
    fly_speed: f64,
    drag: bool,
    survival: SavedSurvival,
    /// The world the player isn't on.
    other: SavedWorldState,
    flight: SavedFlight,
    /// Lava that hasn't cooled yet.
    molten: Vec<BlockCell>,
}

impl SavedGame {
    pub(super) fn time(&self) -> f64 {
        self.time
    }
}

/// A world about to be opened (see the top of this file).
pub(super) enum Opening {
    Backdrop,
    New(Box<Session>),
    Load {
        session: Box<Session>,
        saved: Box<SavedGame>,
        home: Vec<u8>,
        moon: Vec<u8>,
    },
    /// Someone else's world, downloaded from them.
    Join(Box<JoinedWorld>),
}

impl Game {
    /// A menu is up: the world stands still (except behind the main menu).
    pub(super) fn paused(&self) -> bool {
        // (A game with others goes on behind the menu.)
        self.menu.is_some() && (!self.net.in_session() || self.loading())
    }

    /// The loading screen is hiding an unfinished world.
    pub(super) fn loading(&self) -> bool {
        self.opening.is_some() || self.menu.as_ref().is_some_and(|m| m.is_loading())
    }

    pub(super) fn open_pause(&mut self) {
        self.set_mouse_grab(false);
        self.input.clear();
        self.menu = Some(Menu::new(Page::Pause));
    }

    fn resume(&mut self) {
        self.menu = None;
        self.input.clear();
        if self.first_person() && self.capture.is_none() {
            self.set_mouse_grab(true);
        }
    }

    // ---- The menu's side of a frame ------------------------------------------------------

    /// Draws the menu, if one is open, remembering what was clicked for the next frame.
    pub(super) fn draw_menu(&mut self, ui: &mut Ui, dt: f32) {
        if self.menu.is_none() {
            return;
        }
        let time_rows = self.time_rows();
        let Some(menu) = &mut self.menu else { return };
        let mut context = MenuContext {
            settings: &mut self.settings,
            worlds: &self.worlds,
            now: save::unix_now(),
            in_world: !self.backdrop,
            session: match self.net.role {
                Role::Host => MenuSession::Host,
                Role::Client => MenuSession::Client,
                _ => MenuSession::Offline,
            },
            players: self.net.player_count(),
            steam: self.net.steam.is_some(),
            friends: &self.net.friends,
            time_rows: &time_rows,
        };
        self.menu_actions = menu.draw(ui, &self.input, &mut context, dt);
    }

    /// Carries out what the menu asked for. Returns whether the game should exit.
    pub(super) fn run_menu_actions(&mut self) -> Result<bool> {
        for action in std::mem::take(&mut self.menu_actions) {
            match action {
                Action::ShowMultiplayer => self.show_multiplayer(),
                Action::ShowInvite => {
                    self.net.refresh_friends();
                    if let Some(menu) = &mut self.menu {
                        menu.show(Page::Invite);
                    }
                }
                Action::Host => {
                    self.start_hosting(None);
                    self.resume();
                }
                Action::Join { name, address } => {
                    self.settings.player_name = name;
                    self.settings.last_address = address.clone();
                    self.settings_dirty = true;
                    self.store_settings();
                    self.join_direct(&address);
                }
                Action::JoinSteam(host) => self.join_steam(host),
                Action::Invite(friend) => {
                    if let Some(steam) = &self.net.steam {
                        steam.invite(friend);
                    }
                }
                Action::InviteOverlay => {
                    if let Some(steam) = &self.net.steam {
                        steam.invite_overlay();
                    }
                }
                Action::SyncTime(id) => self.time_sync(id, false),
                Action::FollowTime(id) => self.time_sync(id, true),
                Action::StopFollowing => self.unfollow(),
                Action::CancelJoin => self.cancel_join(),
                Action::ShowWorlds => self.show_worlds(),
                Action::Play(i) => self.play_world(i),
                Action::Create { name, seed, mode } => self.create_world(&name, &seed, mode),
                Action::Delete(i) => self.delete_world(i),
                Action::SettingsChanged => self.apply_settings(),
                Action::Resume => self.resume(),
                Action::LeaveWorld => self.leave_world(),
                Action::Quit => {
                    self.save_on_exit();
                    return Ok(true);
                }
                Action::LoadingDone => self.loading_done(),
            }
        }
        // Whoever is online changes while these pages are open.
        if self
            .menu
            .as_ref()
            .is_some_and(|m| matches!(m.page, Page::Multiplayer | Page::Invite))
        {
            self.net.refresh_friends();
        }
        // Keep the choices once the settings page is closed.
        if self.settings_dirty
            && self
                .menu
                .as_ref()
                .is_none_or(|m| m.page != Page::Settings || !self.input.button_held_left())
        {
            self.store_settings();
        }
        Ok(false)
    }

    fn refresh_worlds(&mut self) {
        self.worlds = self
            .save_root
            .as_deref()
            .map(save::list_worlds)
            .unwrap_or_default();
    }

    fn show_worlds(&mut self) {
        self.refresh_worlds();
        if let Some(menu) = &mut self.menu {
            menu.error = None;
            menu.show(Page::Worlds);
        }
    }

    fn show_multiplayer(&mut self) {
        self.net.refresh_friends();
        let (name, address) = (
            self.settings.player_name.clone(),
            self.settings.last_address.clone(),
        );
        if let Some(menu) = &mut self.menu {
            menu.error = None;
            menu.prepare_join(&name, &address);
            menu.show(Page::Multiplayer);
        }
    }

    fn cancel_join(&mut self) {
        self.net_leave();
        self.show_multiplayer();
    }

    fn fail(&mut self, error: anyhow::Error) {
        log::error!("{error:#}");
        self.refresh_worlds();
        let text = format!("{error}");
        match &mut self.menu {
            Some(menu) => {
                menu.error = Some(text);
                menu.show(Page::Worlds);
            }
            None => self.message = Some((text, Instant::now())),
        }
    }

    fn create_world(&mut self, name: &str, seed: &str, mode: GameMode) {
        let Some(root) = self.save_root.clone() else {
            self.fail(anyhow::anyhow!("there is nowhere to save worlds"));
            return;
        };
        self.refresh_worlds();
        let name = save::unique_name(
            &self.worlds,
            if name.is_empty() { "new world" } else { name },
        );
        let now = save::unix_now();
        let meta = WorldMeta {
            version: SAVE_VERSION,
            name: name.clone(),
            seed: save::parse_seed(seed),
            mode,
            created: now,
            last_played: now,
            played: 0.0,
        };
        let dir = save::new_world_dir(&root, &name);
        self.begin_opening(
            Opening::New(Box::new(Session::new(dir, meta))),
            format!("creating {name}"),
        );
    }

    fn play_world(&mut self, index: usize) {
        let Some(entry) = self.worlds.get(index).cloned() else {
            return;
        };
        match read_world(&entry.dir) {
            Ok((saved, home, moon)) => {
                let name = entry.meta.name.clone();
                self.begin_opening(
                    Opening::Load {
                        session: Box::new(Session::new(entry.dir, entry.meta)),
                        saved: Box::new(saved),
                        home,
                        moon,
                    },
                    format!("loading {name}"),
                );
            }
            Err(e) => self.fail(e.context(format!("could not load {}", entry.meta.name))),
        }
    }

    fn delete_world(&mut self, index: usize) {
        let (Some(root), Some(entry)) = (self.save_root.clone(), self.worlds.get(index).cloned())
        else {
            return;
        };
        if let Err(e) = save::delete_world(&root, &entry.dir) {
            self.fail(e);
        }
        self.refresh_worlds();
    }

    /// Saves and goes back to the main menu.
    pub(super) fn leave_world(&mut self) {
        if let Err(e) = self.save_world(true) {
            // Don't throw away a world that couldn't be saved: stay in it, say why.
            log::error!("{e:#}");
            self.message = Some((format!("could not save: {e}"), Instant::now()));
            self.menu = None;
            return;
        }
        self.net_leave();
        self.begin_opening(Opening::Backdrop, "leaving");
    }

    pub(super) fn begin_opening(&mut self, opening: Opening, text: impl Into<String>) {
        self.menu = Some(Menu::loading(text));
        self.opening = Some((opening, 2));
        self.loading_started = Instant::now();
        self.set_mouse_grab(false);
        self.input.clear();
    }

    /// Builds the world being opened, once the loading screen has been shown; then waits
    /// for it to be ready.
    pub(super) fn advance_opening(&mut self) -> Result<()> {
        if let Some((_, delay)) = &mut self.opening {
            if *delay > 0 {
                *delay -= 1;
                return Ok(());
            }
            let (opening, _) = self.opening.take().expect("just looked");
            self.perform_opening(opening)?;
            self.loading_started = Instant::now();
            return Ok(());
        }
        if !self.menu.as_ref().is_some_and(|m| m.waiting_for_world()) || self.net.connecting() {
            return Ok(());
        }
        let waited = self.loading_started.elapsed().as_secs_f32();
        let ready = waited > LOADING_MIN
            && self.streamer.loaded_radius() >= LOADED_ENOUGH
            && self.streamer.lod_settled()
            && self.weather.started();
        if ready || waited > LOADING_TIMEOUT {
            let next = self.backdrop.then_some(Page::Main);
            if let Some(menu) = &mut self.menu {
                menu.finish_loading(next);
            }
        }
        Ok(())
    }

    fn perform_opening(&mut self, opening: Opening) -> Result<()> {
        match opening {
            Opening::Backdrop => {
                let spec = WorldSpec {
                    seed: SEED,
                    mode: GameMode::Spectator,
                    debug: false,
                    time: None,
                    remote_weather: false,
                };
                self.install_world(build_world(&spec, &self.atmosphere));
                self.backdrop = true;
                self.session = None;
            }
            Opening::New(session) => {
                let spec = WorldSpec {
                    seed: session.meta.seed,
                    mode: session.meta.mode,
                    debug: false,
                    time: None,
                    remote_weather: false,
                };
                self.install_world(build_world(&spec, &self.atmosphere));
                self.backdrop = false;
                self.session = Some(*session);
                // It exists on disk from the start.
                if let Err(e) = self.save_world(true) {
                    log::error!("{e:#}");
                    self.message = Some((format!("could not save: {e}"), Instant::now()));
                }
            }
            Opening::Load {
                session,
                saved,
                home,
                moon,
            } => {
                let spec = WorldSpec {
                    seed: session.meta.seed,
                    mode: session.meta.mode,
                    debug: false,
                    time: Some(saved.time),
                    remote_weather: false,
                };
                self.install_world(build_world(&spec, &self.atmosphere));
                self.backdrop = false;
                self.net.records = session.players.clone();
                self.session = Some(*session);
                self.apply_saved(*saved, &home, &moon)?;
            }
            Opening::Join(joined) => {
                // (What a host sends mustn't be able to end the program.)
                if let Err(e) = self.install_joined(*joined) {
                    self.host_lost(format!("{e:#}"));
                }
            }
        }
        self.started = Instant::now();
        Ok(())
    }

    /// The loading screen has gone.
    fn loading_done(&mut self) {
        if self.backdrop {
            self.join_error_shown();
        } else {
            self.net_world_shown();
            self.resume();
        }
    }

    /// Builds the world someone is hosting (its contents come after).
    pub(super) fn install_remote_world(&mut self, seed: u64, mode: GameMode, time: f64) {
        let spec = WorldSpec {
            seed,
            mode,
            debug: false,
            time: Some(time),
            remote_weather: true,
        };
        self.install_world(build_world(&spec, &self.atmosphere));
    }

    /// Replaces the world's parts. The renderer forgets everything of the old one.
    fn install_world(&mut self, world: WorldParts) {
        self.renderer.forget_world();
        self.planet = world.planet;
        self.terrain = world.terrain;
        self.streamer = world.streamer;
        self.gravity = world.gravity;
        self.system = world.system;
        self.time = world.time;
        self.warp = world.warp;
        self.effective_warp = 1.0;
        self.bodies = world.bodies;
        self.other = world.other;
        self.weather = world.weather;
        self.player = world.player;
        self.camera = world.camera;
        self.last_feet = world.feet;
        self.spawn_feet = world.feet;
        self.active = Body::Planet;
        self.exposure = None;
        self.flight = Default::default();
        self.weather_fx = Default::default();
        self.lava = Default::default();
        self.debris = Default::default();
        self.survival = Survival::default();
        self.drag = true;
        self.was_on_ground = false;
        self.target = None;
        self.last_jump_tap = None;
        self.message = None;
        self.hud_anim = Default::default();
        self.screen_anim = Default::default();
        self.view_model = Default::default();
        self.view_sky = None;
        self.rain = None;
        self.fly_speed = 12.0;
        self.silence();
    }

    /// Stops the looping weather sounds (they fade out by themselves).
    pub(super) fn silence(&mut self) {
        for kind in [Ambience::Rain, Ambience::Wind, Ambience::Roar] {
            self.audio.set_ambience(kind, 0.0);
        }
    }

    // ---- Saving and loading --------------------------------------------------------------

    pub(super) fn saved_game(&self) -> SavedGame {
        SavedGame {
            version: SAVE_VERSION,
            time: self.time,
            active: self.active,
            player: self.player.saved(),
            camera: self.camera.view_state(),
            spawn_feet: self.spawn_feet,
            fly_speed: self.fly_speed,
            drag: self.drag,
            survival: self.survival.saved(),
            other: self.other.state.saved(),
            flight: self.flight.saved(),
            molten: self.lava.molten(),
        }
    }

    /// Saves the world being played, if it is a saved one. `wait`: don't come back until it
    /// is on disk; otherwise the writing happens on another thread.
    pub(super) fn save_world(&mut self, wait: bool) -> Result<()> {
        self.finish_saving()?;
        if self.session.is_none() {
            return Ok(());
        }
        let state = serde_json::to_vec(&self.saved_game())?;
        let (home, moon) = if self.active == Body::Planet {
            (&self.streamer, &self.other.streamer)
        } else {
            (&self.other.streamer, &self.streamer)
        };
        let (home, moon) = (home.edits_bytes(), moon.edits_bytes());
        let mode = self.player.mode;
        let players = serde_json::to_vec(&self.net.records)?;
        let session = self.session.as_mut().expect("checked above");
        session.meta.mode = mode;
        session.meta.last_played = save::unix_now();
        session.last_save = Instant::now();
        let files = vec![
            (
                "world.json".to_string(),
                serde_json::to_vec_pretty(&session.meta)?,
            ),
            ("state.json".to_string(), state),
            ("players.json".to_string(), players),
            ("home.edits".to_string(), home),
            ("moon.edits".to_string(), moon),
        ];
        let dir = session.dir.clone();
        if wait {
            return save::write_files(&dir, files);
        }
        self.save_thread = Some(std::thread::spawn(move || save::write_files(&dir, files)));
        Ok(())
    }

    /// Waits for a save still being written.
    fn finish_saving(&mut self) -> Result<()> {
        match self.save_thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => bail!("the save thread panicked"),
            None => Ok(()),
        }
    }

    /// Saves now and then while playing.
    pub(super) fn autosave(&mut self, dt: f64) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.meta.played += dt;
        if session.last_save.elapsed().as_secs_f64() < AUTOSAVE {
            return;
        }
        if let Err(e) = self.save_world(false) {
            log::error!("{e:#}");
            self.message = Some((format!("could not save: {e}"), Instant::now()));
        }
    }

    /// The window is closing.
    pub(super) fn save_on_exit(&mut self) {
        self.store_settings();
        if let Err(e) = self.save_world(true) {
            log::error!("could not save: {e:#}");
        }
        self.net_leave();
    }

    pub(super) fn apply_saved(&mut self, saved: SavedGame, home: &[u8], moon: &[u8]) -> Result<()> {
        if saved.version > SAVE_VERSION {
            bail!("this world was saved by a newer version of the game");
        }
        self.streamer.load_edits(home).context("home's blocks")?;
        self.other
            .streamer
            .load_edits(moon)
            .context("the moon's blocks")?;
        self.time = saved.time;
        if saved.active == Body::Moon {
            // Built with home active: trade places (nothing has loaded yet).
            let other = &mut self.other;
            std::mem::swap(&mut self.planet, &mut other.planet);
            std::mem::swap(&mut self.terrain, &mut other.terrain);
            std::mem::swap(&mut self.streamer, &mut other.streamer);
            std::mem::swap(&mut self.gravity, &mut other.gravity);
            other.body = Body::Planet;
            self.active = Body::Moon;
        }
        self.player.restore(&saved.player);
        self.flight.restore(saved.flight);
        self.spawn_feet = saved.spawn_feet;
        self.fly_speed = saved.fly_speed;
        self.drag = saved.drag;
        self.other.state = WorldState::restored(saved.other);
        let frame = self.active_frame(self.time);
        let feet = self.player.body_position(&frame);
        self.survival.restore(saved.survival, feet);
        self.lava.resume_cooling(saved.molten, self.time);
        self.last_feet = feet;
        let up = feet.normalize_or(DVec3::Y);
        self.camera.position = self.planet.center + feet + up * EYE_HEIGHT;
        self.camera.set_view_state(saved.camera);
        if !self.first_person() {
            self.camera.position = self.stream_focus();
        }
        Ok(())
    }

    // ---- The main menu's view ------------------------------------------------------------

    /// High above the morning side, always in the same place relative to the sun, with the
    /// planet turning below and filling the right of the screen.
    pub(super) fn update_backdrop(&mut self, dt: f64) {
        self.time += dt * BACKDROP_TIME;
        let sun = self.star_view().normalize();
        let north = DVec3::Y;
        let east = north.cross(sun).try_normalize().unwrap_or(DVec3::X);
        let position = (sun * 0.6 - east * 0.72 + north * 0.34).normalize();
        let radius = self.planet.sea_radius + BACKDROP_ALTITUDE;
        self.camera.position = self.planet.center + position * radius;
        let forward = (-position - east * 0.52 + north * 0.34).normalize();
        self.camera.look_at(forward.as_vec3(), north.as_vec3());
    }

    // ---- Settings ------------------------------------------------------------------------

    /// Makes the settings take effect.
    pub(super) fn apply_settings(&mut self) {
        self.settings_dirty = true;
        let settings = self.settings.clone();
        // (Unattended runs must never take over the screen.)
        if self.capture.is_none() {
            let fullscreen = settings.fullscreen.then_some(Fullscreen::Borderless(None));
            if self.window.fullscreen().is_some() != settings.fullscreen {
                self.window.set_fullscreen(fullscreen);
            }
        }
        self.renderer.set_vsync(settings.vsync);
        self.audio.set_volume(settings.volume);
    }

    fn store_settings(&mut self) {
        if !std::mem::take(&mut self.settings_dirty) {
            return;
        }
        if self.capture.is_some() && std::env::var("ASTRO_SAVE_DIR").is_err() {
            return;
        }
        if let Some(root) = &self.save_root
            && let Err(e) = self.settings.save(root)
        {
            log::error!("could not save the settings: {e:#}");
        }
    }

    /// F11.
    pub(super) fn toggle_fullscreen(&mut self) {
        self.settings.fullscreen = !self.settings.fullscreen;
        self.apply_settings();
        self.store_settings();
    }

    /// Holds the frame rate down to the cap, if there is one.
    pub(super) fn limit_frame_rate(&mut self) {
        if self.settings.fps_cap == 0 {
            self.next_frame = None;
            return;
        }
        let period = Duration::from_secs_f64(1.0 / self.settings.fps_cap as f64);
        let now = Instant::now();
        let target = self.next_frame.unwrap_or(now) + period;
        if let Some(wait) = target.checked_duration_since(now) {
            // Sleeping can oversleep by a millisecond: spin the last bit.
            if let Some(sleep) = wait.checked_sub(Duration::from_millis(1)) {
                std::thread::sleep(sleep);
            }
            while Instant::now() < target {
                std::hint::spin_loop();
            }
            self.next_frame = Some(target);
        } else {
            // Running behind: don't try to catch up with a burst of frames.
            self.next_frame = Some(now);
        }
    }

    // ---- Console commands, mostly for testing ---------------------------------------------

    /// `world create|load`, `save`, `menu`, `settings`. Returns whether it was one of them.
    pub(super) fn session_command(&mut self, command: &str, args: &[&str]) -> bool {
        match command {
            "save" => {
                match self.save_world(true) {
                    Ok(()) if self.session.is_some() => self.console.print("Saved"),
                    Ok(()) => self.console.print("This world isn't a saved one"),
                    Err(e) => self.console.print(format!("Could not save: {e}")),
                }
                true
            }
            "world" => {
                match args {
                    ["create", name, rest @ ..] => {
                        let seed = rest.first().copied().unwrap_or("");
                        let mode = rest
                            .get(1)
                            .and_then(|m| m.parse().ok())
                            .unwrap_or(GameMode::Survival);
                        self.create_world(name, seed, mode);
                    }
                    ["load", name] => {
                        self.refresh_worlds();
                        match self.worlds.iter().position(|w| w.meta.name == *name) {
                            Some(i) => self.play_world(i),
                            None => self.console.print(format!("No world called {name}")),
                        }
                    }
                    ["leave"] => self.leave_world(),
                    _ => self
                        .console
                        .print("Usage: world create <name> [seed] [mode] | load <name> | leave"),
                }
                true
            }
            "menu" => {
                let page = match args.first().copied() {
                    Some("main") => Some(Page::Main),
                    Some("worlds") => Some(Page::Worlds),
                    Some("create") => Some(Page::Create),
                    Some("settings") => Some(Page::Settings),
                    Some("pause") => Some(Page::Pause),
                    Some("multiplayer") => Some(Page::Multiplayer),
                    Some("invite") => Some(Page::Invite),
                    Some("time") => Some(Page::Time),
                    _ => None,
                };
                match page {
                    Some(page) => {
                        self.refresh_worlds();
                        self.set_mouse_grab(false);
                        self.menu
                            .get_or_insert_with(|| Menu::new(Page::Pause))
                            .show(page);
                    }
                    None => self
                        .console
                        .print("Usage: menu <main|worlds|create|settings|pause>"),
                }
                true
            }
            "settings" => {
                let on = |word: &str| matches!(word, "on" | "1" | "true");
                match args {
                    ["vsync", value] => self.settings.vsync = on(value),
                    ["fps", value] => self.settings.fps_cap = value.parse().unwrap_or(0),
                    ["volume", value] => {
                        self.settings.volume =
                            value.parse::<f32>().unwrap_or(100.0).clamp(0.0, 100.0) / 100.0;
                    }
                    ["fullscreen", value] => self.settings.fullscreen = on(value),
                    _ => {
                        self.console.print(format!("{:?}", self.settings));
                        return true;
                    }
                }
                self.apply_settings();
                true
            }
            _ => false,
        }
    }
}

/// A saved world's files. Nothing is changed unless all of them read correctly.
fn read_world(dir: &std::path::Path) -> Result<(SavedGame, Vec<u8>, Vec<u8>)> {
    let state = std::fs::read(dir.join("state.json")).context("reading its save")?;
    let saved: SavedGame = serde_json::from_slice(&state).context("reading its save")?;
    if saved.version > SAVE_VERSION {
        bail!("it was saved by a newer version of the game");
    }
    let home = std::fs::read(dir.join("home.edits")).context("reading home's blocks")?;
    let moon = std::fs::read(dir.join("moon.edits")).context("reading the moon's blocks")?;
    // Find out now, while the world on screen can still stay.
    for edits in [&home, &moon] {
        WorldEdits::default()
            .read_bytes(edits)
            .context("reading its blocks")?;
    }
    Ok((saved, home, moon))
}
