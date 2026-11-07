//! Window, event loop glue and the per-frame game loop.

use std::{f32::consts::PI, path::PathBuf, sync::Arc, time::Instant};

use anyhow::Result;
use glam::{DVec3, Vec3};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use crate::{
    audio::Audio,
    camera::Camera,
    console::{self, Console},
    hud::{self, HudInfo},
    input::Input,
    interact::{self, CREATIVE_REACH, Hit, SURVIVAL_REACH},
    inventory,
    items::{Item, ItemStack},
    mobs::MobKind,
    player::{EYE_HEIGHT, GameMode, MAX_PHYSICS_WARP, Physics, Player, PlayerInput, Voxels},
    render::{
        BlockTarget, FrameParams, RenderStats, Renderer,
        atmosphere::{Atmosphere, AtmosphereLuts},
    },
    screen::Screen,
    space::{SolarSystem, gravity::PlanetGravity},
    survival::{self, Pose, Survival},
    ui::Ui,
    world::{
        block::Block,
        planet::{Face, Planet},
        streaming::{UNDERGROUND_RADIUS, WorldStreamer},
        terrain::TerrainGenerator,
        textures,
    },
};

/// The home planet sits ~1 AU from the world origin. At that distance f32 has a resolution
/// of ~16 km, so this keeps us honest: everything must go through f64 + camera-relative
/// rendering to work at all.
const PLANET_CENTER: DVec3 = DVec3::new(1.496e11, 0.0, 0.0);
const SEED: u64 = 0x5eed_a57b;
/// 2048 chunks per face edge -> 65,536 blocks -> ~45.9 km radius, ~288 km circumference.
const CHUNKS_PER_FACE: i32 = 2048;
/// Build limit: layers above sea level. Below sea level the planet is solid all the way
/// to its centre (see `world::planet` for how the block grid coarsens with depth).
const HEIGHT_LIMIT: i32 = 1024;
const LOAD_RADIUS: f64 = 12.0 * 32.0;
/// Sun illuminance above the atmosphere, in scene units (all HDR radiance is relative to
/// it; the auto exposure maps it to the display).
const SUN_ILLUMINANCE: f32 = 3.0;
/// Irradiance of full block light (torches), in scene units. Keep in sync with
/// `TORCH_IRRADIANCE` in `shaders/atmosphere.glsl`.
const TORCH_IRRADIANCE: f32 = 0.6;
/// Starlight / airglow, relative to the sun: nights are dark but not black.
const NIGHT_AMBIENT: f32 = 0.004;
/// Time warp steps (`,` / `.`), like KSP.
const WARP_LEVELS: [f64; 9] = [
    1.0, 5.0, 10.0, 50.0, 100.0, 1_000.0, 10_000.0, 100_000.0, 1e6,
];
/// Local solar time at the spawn point when the game starts.
const START_HOUR: f64 = 9.0;
/// Two jump presses within this time toggle creative flight.
const DOUBLE_TAP: f64 = 0.3;

const MOUSE_SENSITIVITY: f32 = 0.0022;

#[derive(Default)]
pub struct App {
    state: Option<Game>,
    /// Fatal error that stopped the event loop; reported by `main`.
    pub error: Option<anyhow::Error>,
}

struct Game {
    window: Arc<Window>,
    camera: Camera,
    input: Input,
    planet: Arc<Planet>,
    terrain: Arc<TerrainGenerator>,
    streamer: WorldStreamer,
    /// The star and the planet's orbit and spin: they drive the day/night cycle.
    system: SolarSystem,
    /// Universal time (s since the epoch).
    time: f64,
    warp: usize,
    /// The warp actually applied (limited while the player can.t go on rails).
    effective_warp: f64,
    atmosphere: Atmosphere,
    atmosphere_luts: AtmosphereLuts,
    /// Smoothed auto exposure (None until the first frame).
    exposure: Option<f32>,
    gravity: PlanetGravity,
    player: Player,
    /// Air drag on the player (console: `drag on|off`).
    drag: bool,
    console: Console,
    /// Inventory, held item, screens, drops, placed models, furnaces, health.
    survival: Survival,
    /// Sound effects (silent in capture mode).
    audio: Audio,
    /// Feet position and whether the player stood on something last frame (footsteps,
    /// the hunger cost of walking and jumping).
    last_feet: DVec3,
    was_on_ground: bool,
    /// Where the player respawns (body-frame feet position).
    spawn_feet: DVec3,
    /// The block being looked at.
    target: Option<Hit>,
    last_jump_tap: Option<Instant>,
    /// A short message shown above the hotbar.
    message: Option<(String, Instant)>,
    /// Spectator fly speed in m/s; the scroll wheel scales it exponentially.
    fly_speed: f64,
    mouse_grabbed: bool,
    last_frame: Instant,
    stats: FrameStats,
    render_stats: RenderStats,
    /// Smoothed CPU time per frame (ms): (total excluding GPU wait, world streaming).
    cpu_ms: (f64, f64),
    /// Debug: `ASTRO_CAPTURE="file.bmp;seconds"` saves a screenshot after a delay and exits.
    capture: Option<Capture>,
    /// Debug: `ASTRO_COMMANDS="cmd;cmd"` runs console commands once the world has loaded.
    startup_commands: Vec<String>,
    started: Instant,
    // Declared last: dropped after the streamer, so worker threads are gone first.
    renderer: Renderer,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match Game::new(event_loop) {
            Ok(game) => self.state = Some(game),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(game) = &mut self.state else { return };
        // In capture mode the game runs unattended: ignore all user input.
        if game.capture.is_some()
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::CursorMoved { .. }
            )
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                game.renderer.resize()
            }
            WindowEvent::Focused(false) => {
                game.set_mouse_grab(false);
                game.input.clear();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                if pressed && code == KeyCode::Backquote && !event.repeat {
                    game.toggle_console();
                    return;
                }
                if game.console.open {
                    if pressed {
                        match code {
                            KeyCode::Enter | KeyCode::NumpadEnter => {
                                if let Some(line) = game.console.submit() {
                                    game.run_command(&line);
                                }
                            }
                            KeyCode::Escape => game.toggle_console(),
                            KeyCode::Backspace => game.console.backspace(),
                            KeyCode::ArrowUp => game.console.browse(true),
                            KeyCode::ArrowDown => game.console.browse(false),
                            _ => {
                                if let Some(text) = &event.text {
                                    game.console.type_text(text);
                                }
                            }
                        }
                    }
                    return;
                }
                if pressed {
                    game.input.key_down(code);
                } else {
                    game.input.key_up(code);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if game.console.open {
                    return;
                }
                match state {
                    // The first click only captures the mouse (unless a screen uses it).
                    ElementState::Pressed
                        if !game.mouse_grabbed && game.survival.screen.is_none() =>
                    {
                        game.set_mouse_grab(true)
                    }
                    ElementState::Pressed => game.input.button_down(button),
                    ElementState::Released => game.input.button_up(button),
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                game.input.cursor = (position.x as f32, position.y as f32);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                game.input.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
            }
            WindowEvent::RedrawRequested => match game.frame() {
                Ok(true) => event_loop.exit(),
                Ok(false) => {}
                Err(e) => {
                    self.error = Some(e);
                    event_loop.exit();
                }
            },
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(game) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta } = event
            && game.mouse_grabbed
        {
            game.input.mouse_delta.0 += delta.0;
            game.input.mouse_delta.1 += delta.1;
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(game) = &self.state {
            game.window.request_redraw();
        }
    }
}

/// The world as the player's collision sees it: loaded chunks, or - where nothing is loaded
/// yet - a guess from the terrain height (so you don't fall through unloaded ground).
struct WorldVoxels<'a> {
    planet: &'a Planet,
    streamer: &'a WorldStreamer,
    terrain: &'a TerrainGenerator,
}

impl Voxels for WorldVoxels<'_> {
    fn block(&self, face: Face, u: i32, v: i32, k: i32) -> Block {
        let Some(cell) = self.planet.cell(face, u, v, k) else {
            return if k < 0 { Block::Stone } else { Block::Air };
        };
        if let Some(block) = self.streamer.block(cell) {
            return block;
        }
        if self.planet.res_at(k) != self.planet.face_blocks {
            return Block::Stone; // deep interior
        }
        let column = self
            .terrain
            .sample_column(self.planet.column_dir(cell.face, cell.u, cell.v));
        let ground = self.planet.sea_layer + column.height.floor() as i32;
        if k <= ground {
            Block::Stone
        } else if k < self.planet.sea_layer {
            Block::Water
        } else {
            Block::Air
        }
    }
}

fn direction(lat: f64, lon: f64) -> DVec3 {
    let (lat, lon) = (lat.to_radians(), lon.to_radians());
    DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin())
}

impl Game {
    fn new(event_loop: &ActiveEventLoop) -> Result<Self> {
        let capture = Capture::from_env();
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Astro Block")
                    .with_inner_size(LogicalSize::new(1600.0, 900.0))
                    // Unattended captures shouldn't steal keyboard focus.
                    .with_active(capture.is_none()),
            )?,
        );

        let planet = Arc::new(Planet::new(
            PLANET_CENTER,
            CHUNKS_PER_FACE,
            HEIGHT_LIMIT,
            SEED,
        ));
        let atmosphere = Atmosphere::earth_like(planet.sea_radius);
        let lut_start = Instant::now();
        let atmosphere_luts = atmosphere.luts();
        log::info!("atmosphere LUTs computed in {:?}", lut_start.elapsed());
        let renderer = Renderer::new(
            window.clone(),
            &textures::generate(),
            &atmosphere,
            &atmosphere_luts,
        )?;
        let terrain = Arc::new(TerrainGenerator::new(planet.clone()));
        log::info!(
            "planet: radius {:.1} km, {} chunks per face edge at the surface, {} layers, {} shells",
            planet.sea_radius / 1000.0,
            planet.face_blocks / 32,
            planet.layers,
            planet.shells.len()
        );
        let gravity = PlanetGravity::new(
            planet.sea_radius,
            atmosphere.rayleigh_scale_height as f64,
            (atmosphere.top_radius - atmosphere.bottom_radius) as f64,
        );

        // Spawn standing on the ground. Debug: ASTRO_SPAWN="lat,lon[,altitude[,pitch[,yaw]]]"
        // (degrees; eye height in meters above sea level).
        let mut spawn_dir = terrain.find_spawn();
        let mut eye_altitude = None;
        let mut pitch = -0.25;
        let mut yaw = 0.0;
        if let Ok(spec) = std::env::var("ASTRO_SPAWN") {
            let v: Vec<f64> = spec
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if let [lat, lon, rest @ ..] = v.as_slice() {
                spawn_dir = direction(*lat, *lon);
                eye_altitude = rest.first().copied();
                pitch = rest.get(1).map_or(pitch, |p| p.to_radians() as f32);
                yaw = rest.get(2).map_or(yaw, |y| y.to_radians() as f32);
            }
        }
        let feet = match eye_altitude {
            Some(alt) => spawn_dir * (planet.sea_radius + alt - EYE_HEIGHT),
            None => Self::ground_position(&planet, &terrain, spawn_dir),
        };
        let mut camera = Camera::new(
            planet.center + feet + spawn_dir * EYE_HEIGHT,
            spawn_dir.as_vec3(),
        );
        camera.pitch = pitch;
        camera.yaw = yaw;

        let streamer = WorldStreamer::new(planet.clone(), terrain.clone(), LOAD_RADIUS);
        log::info!(
            "world streaming with {} worker threads",
            streamer.stats().workers
        );

        // Start in the morning at the spawn point. Debug: ASTRO_TIME=<hours> (local solar
        // time at the spawn), ASTRO_WARP=<factor> (initial time warp), ASTRO_MODE=<mode>.
        let system = SolarSystem::new();
        let hour = std::env::var("ASTRO_TIME")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(START_HOUR);
        let time = system.time_of_day(0.0, spawn_dir, hour);
        // Debug: ASTRO_FACE_SUN=1 turns the camera towards the sun's compass direction.
        if std::env::var("ASTRO_FACE_SUN").is_ok_and(|v| v == "1") {
            let sun = system.star_in_body_frame(time).normalize().as_vec3();
            camera.yaw = (0..360)
                .map(|d| (d as f32).to_radians())
                .max_by(|&a, &b| {
                    let facing = |y: f32| {
                        let mut c = Camera::new(camera.position, spawn_dir.as_vec3());
                        c.yaw = y;
                        c.forward().dot(sun)
                    };
                    facing(a).total_cmp(&facing(b))
                })
                .unwrap_or(0.0);
        }
        let warp = std::env::var("ASTRO_WARP")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map_or(0, |w| {
                WARP_LEVELS
                    .iter()
                    .rposition(|&l| l <= w.max(1.0))
                    .unwrap_or(0)
            });
        let mode = std::env::var("ASTRO_MODE")
            .ok()
            .and_then(|m| m.parse().ok())
            .unwrap_or(GameMode::Survival);
        let player = Player::new(mode, feet, &system.planet_frame(time));
        let mut console = Console::default();
        console.print("Welcome! Type `help` for commands, ` closes the console.");
        console.open = std::env::var("ASTRO_CONSOLE").is_ok_and(|v| v == "1");
        let startup_commands = std::env::var("ASTRO_COMMANDS")
            .map(|c| c.split(';').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        Ok(Self {
            window,
            camera,
            input: Input::default(),
            planet,
            terrain,
            streamer,
            system,
            time,
            warp,
            effective_warp: 1.0,
            atmosphere,
            atmosphere_luts,
            exposure: None,
            gravity,
            player,
            drag: true,
            console,
            survival: Survival::default(),
            audio: Audio::new(capture.is_none()),
            last_feet: feet,
            was_on_ground: false,
            spawn_feet: feet,
            target: None,
            last_jump_tap: None,
            message: None,
            fly_speed: 12.0,
            mouse_grabbed: false,
            last_frame: Instant::now(),
            stats: FrameStats::default(),
            render_stats: RenderStats::default(),
            cpu_ms: (0.0, 0.0),
            capture,
            startup_commands,
            started: Instant::now(),
            renderer,
        })
    }

    /// Body-frame feet position standing on the terrain (or the sea) along `dir`.
    fn ground_position(planet: &Planet, terrain: &TerrainGenerator, dir: DVec3) -> DVec3 {
        let ground = terrain.sample_column(dir).height.floor().max(0.0);
        dir * (planet.sea_radius + ground + 1.0)
    }

    /// Runs one frame. Returns `true` when the game should exit.
    fn frame(&mut self) -> Result<bool> {
        let now = Instant::now();
        // Clamp so a stall (window drag, breakpoint) doesn't teleport the camera.
        let dt = (now - self.last_frame).as_secs_f64().min(0.1);
        self.last_frame = now;

        // One startup command per frame, so each sees the previous one's effect.
        if !self.startup_commands.is_empty() && self.started.elapsed().as_secs_f64() > 4.0 {
            let line = self.startup_commands.remove(0);
            self.console.print(format!("> {line}"));
            self.run_command(&line);
        }
        self.update(dt);

        let stream_start = Instant::now();
        self.streamer
            .update(self.camera.position, &mut self.renderer)?;
        let stream_ms = stream_start.elapsed().as_secs_f64() * 1000.0;

        let mut exit = false;
        if let Some(capture) = &mut self.capture {
            if capture.requested {
                exit = true; // the previous frame was captured
            } else if capture.start.elapsed().as_secs_f64() >= capture.after_secs {
                capture.requested = true;
                self.renderer.request_capture(capture.path.clone());
                log::info!("capture: {}", self.title(self.stats.last_fps));
            }
        }

        let draws = self.streamer.draw_items();
        let (mut params, underground) = self.frame_params(dt);
        if underground {
            // No LOD: chunk water must not hand over to it.
            params.far_discard_radius = f32::INFINITY;
        }
        // Underground the surface LOD terrain would only show through unloaded rock.
        let lod = if underground {
            Vec::new()
        } else {
            self.streamer.lod_draw_items()
        };
        let objects = self.survival.draw_items(
            &self.planet,
            &self.terrain,
            &self.streamer,
            &mut self.renderer,
            self.camera.position,
        )?;
        let ui = self.overlay();
        let camera = &self.camera;
        self.render_stats =
            self.renderer
                .render(camera, &params, &draws, &lod, &objects, &ui.vertices)?;
        self.input.end_frame();

        // Exponentially smoothed CPU timings (excluding time spent waiting on the GPU).
        let cpu = now.elapsed().as_secs_f64() * 1000.0 - self.render_stats.gpu_wait_ms as f64;
        self.cpu_ms.0 += (cpu - self.cpu_ms.0) * 0.05;
        self.cpu_ms.1 += (stream_ms - self.cpu_ms.1) * 0.05;

        if let Some(fps) = self.stats.tick(dt) {
            self.window.set_title(&self.title(fps));
        }
        Ok(exit)
    }

    fn update(&mut self, dt: f64) {
        // Keep the camera upright relative to the planet surface below it.
        let up = (self.camera.position - self.planet.center).as_vec3();
        self.camera.set_up(up);

        // Inventory screens: E opens one; while open they get the mouse and keys.
        let pose = self.pose();
        let creative = self.player.mode == GameMode::Creative;
        if self.survival.screen.is_some() {
            let (w, h) = self.renderer.screen_size();
            let scale = Ui::new(w, h).scale;
            if self.survival.screen_input(
                &self.input,
                self.input.cursor,
                (w, h, scale),
                &pose,
                creative,
            ) {
                self.close_screen();
            }
        } else {
            if self.input.pressed(KeyCode::Escape) {
                self.set_mouse_grab(false);
            }
            if self.input.pressed(KeyCode::KeyE) && self.player.mode != GameMode::Spectator {
                let screen = if creative {
                    Screen::Creative
                } else {
                    Screen::Inventory
                };
                self.survival.open(screen);
            }
        }
        let in_screen = self.survival.screen.is_some();
        if in_screen && self.mouse_grabbed {
            self.set_mouse_grab(false);
        }

        // Time warp, KSP style: `.` faster, `,` slower.
        if self.input.pressed(KeyCode::Period) {
            self.warp = (self.warp + 1).min(WARP_LEVELS.len() - 1);
        }
        if self.input.pressed(KeyCode::Comma) {
            self.warp = self.warp.saturating_sub(1);
        }

        // Hotbar: number keys and the scroll wheel (which sets the fly speed in spectator).
        let digits = [
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
        if !in_screen && let Some(i) = digits.iter().position(|&k| self.input.pressed(k)) {
            self.survival.select(i);
        }
        let scroll = self.input.scroll;
        if scroll != 0.0 && !in_screen {
            if self.player.mode == GameMode::Spectator {
                self.fly_speed = (self.fly_speed * 1.25f64.powf(scroll as f64)).clamp(0.5, 3.0e8);
            } else {
                let n = inventory::HOTBAR as i32;
                let next = (self.survival.selected as i32 - scroll.signum() as i32).rem_euclid(n);
                self.survival.select(next as usize);
            }
        }

        if self.mouse_grabbed {
            let (dx, dy) = self.input.mouse_delta;
            self.camera.rotate(
                dx as f32 * MOUSE_SENSITIVITY,
                -dy as f32 * MOUSE_SENSITIVITY,
            );
        }

        // Double-tapping jump toggles creative flight.
        if self.input.pressed(KeyCode::Space) {
            let now = Instant::now();
            if self
                .last_jump_tap
                .is_some_and(|t| now.duration_since(t).as_secs_f64() < DOUBLE_TAP)
            {
                self.player.toggle_flight();
                self.last_jump_tap = None;
            } else {
                self.last_jump_tap = Some(now);
            }
        }

        let input = self.player_input();

        // Time warp beyond physics speed needs the player landed or in a stable orbit.
        let physics = Physics {
            planet: &self.planet,
            gravity: &self.gravity,
            drag: self.drag,
        };
        let frame0 = self.system.planet_frame(self.time);
        // (The chosen warp is kept; it applies again once possible.)
        let mut warp = WARP_LEVELS[self.warp];
        let mut rails = warp > MAX_PHYSICS_WARP;
        if rails && !self.player.can_rail_warp(&physics, &frame0, self.time) {
            warp = MAX_PHYSICS_WARP;
            rails = false;
            self.message = Some((
                format!("Time warp limited to x{MAX_PHYSICS_WARP} while moving in the atmosphere"),
                Instant::now(),
            ));
        }
        self.effective_warp = warp;
        let (t0, t1) = (self.time, self.time + dt * warp);
        let world = WorldVoxels {
            planet: &self.planet,
            streamer: &self.streamer,
            terrain: &self.terrain,
        };
        let system = &self.system;
        self.player
            .update(&physics, &world, &input, (t0, t1), dt, rails, &|t| {
                system.planet_frame(t)
            });
        self.time = t1;

        // The camera rides at the player's eyes.
        let feet = self
            .player
            .body_position(&self.system.planet_frame(self.time));
        self.camera.position = self.planet.center + feet + feet.normalize() * EYE_HEIGHT;

        // Fall damage, death and respawn.
        let fallen = std::mem::take(&mut self.player.fallen);
        if self.player.mode == GameMode::Survival {
            let damage = Player::fall_damage(fallen);
            let at = self.camera.position - self.planet.center;
            if damage > 0.0 && self.survival.hurt(damage, true, at) {
                self.respawn();
            }
        }

        // Drops, furnaces, regeneration.
        let pose = self.pose();
        let world = WorldVoxels {
            planet: &self.planet,
            streamer: &self.streamer,
            terrain: &self.terrain,
        };
        let gravity = self.gravity.acceleration(self.player.position).length();
        let messages = self.survival.update(
            &self.planet,
            &world,
            gravity,
            &pose,
            self.player.mode,
            dt * warp,
        );
        self.survival
            .spawn_mobs(&self.planet, &self.terrain, &world, &pose, dt * warp);
        if let Some(message) = messages.into_iter().last() {
            self.message = Some((message, Instant::now()));
        }
        if self.survival.screen.is_none() && in_screen {
            self.close_screen();
        }

        self.interact(dt as f32);
        self.walking(&input, feet);
        self.survival.apply_lights(&mut self.streamer);
        self.play_sounds();
    }

    /// Footsteps and the hunger walking, sprinting and jumping cost.
    fn walking(&mut self, input: &PlayerInput, feet: DVec3) {
        let up = feet.normalize_or(DVec3::Y);
        let moved = feet - self.last_feet;
        let distance = (moved - up * moved.dot(up)).length();
        self.last_feet = feet;
        let on_ground = self.player.on_ground && self.player.mode != GameMode::Spectator;
        if on_ground && distance < 1.0 {
            let ground = self
                .planet
                .cell_at(feet - up * 0.2)
                .and_then(|c| self.streamer.block(c))
                .filter(|b| b.is_solid());
            let sprinting = input.sprint && input.walk != DVec3::ZERO;
            self.survival.walked(distance, sprinting, ground, feet);
        }
        // Jumping (more when sprinting).
        if self.was_on_ground && !self.player.on_ground && input.jump {
            self.survival.exert(if input.sprint { 0.2 } else { 0.05 });
        }
        self.was_on_ground = self.player.on_ground;
    }

    /// Plays the sounds of this frame where they happened.
    fn play_sounds(&mut self) {
        let listener = self.camera.position - self.planet.center;
        let right = self.camera.right().as_dvec3();
        for (sound, at) in self.survival.sounds.drain(..) {
            self.audio.play(sound, at - listener, right);
        }
    }

    /// Where the player stands and looks (planet-relative, body frame).
    fn pose(&self) -> Pose {
        let feet = self
            .player
            .body_position(&self.system.planet_frame(self.time));
        Pose {
            eye: self.camera.position - self.planet.center,
            look: self.camera.forward().as_dvec3(),
            feet,
        }
    }

    fn close_screen(&mut self) {
        let pose = self.pose();
        self.survival.close(&pose);
        self.input.clear();
        if self.capture.is_none() && !self.console.open {
            self.set_mouse_grab(true);
        }
    }

    /// Died: items drop where it happened, and the player is back at the spawn point.
    fn respawn(&mut self) {
        let pose = self.pose();
        self.survival.die(&pose);
        let frame = self.system.planet_frame(self.time);
        self.player.teleport(self.spawn_feet, &frame);
        self.message = Some(("You died!".to_string(), Instant::now()));
    }

    /// Turns the keys and the camera's heading into player controls.
    fn player_input(&self) -> PlayerInput {
        if self.survival.screen.is_some() {
            // Standing still while using a screen.
            return PlayerInput {
                spectator_speed: self.fly_speed,
                ..Default::default()
            };
        }
        let input = &self.input;
        let axis = |plus: KeyCode, minus: KeyCode| {
            input.held(plus) as i32 as f64 - input.held(minus) as i32 as f64
        };
        let (fwd, side) = (
            axis(KeyCode::KeyW, KeyCode::KeyS),
            axis(KeyCode::KeyD, KeyCode::KeyA),
        );
        let vertical = axis(KeyCode::Space, KeyCode::ShiftLeft);
        let up = self.camera.up().as_dvec3();
        let forward = self.camera.forward().as_dvec3();
        let right = self.camera.right().as_dvec3();
        let flat = |v: DVec3| (v - up * v.dot(up)).normalize_or_zero();
        let walk = (flat(forward) * fwd + flat(right) * side).normalize_or_zero();
        let mut fly_dir = (forward * fwd + right * side + up * vertical).normalize_or_zero();
        let boost = if input.held(KeyCode::ControlLeft) {
            10.0
        } else {
            1.0
        };
        let mut spectator_speed = self.fly_speed * boost;
        // Debug: ASTRO_AUTOFLY=<m/s> flies forward on its own (spectator; streaming test).
        if let Some(speed) = std::env::var("ASTRO_AUTOFLY")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            fly_dir = forward;
            spectator_speed = speed;
        }
        PlayerInput {
            walk,
            fly_dir,
            vertical,
            jump: input.held(KeyCode::Space),
            // Too hungry to sprint, as in Minecraft.
            sprint: input.held(KeyCode::ControlLeft)
                && (self.survival.food > 6.0 || self.player.mode != GameMode::Survival),
            spectator_speed,
        }
    }

    /// Looking at, breaking and placing blocks.
    fn interact(&mut self, dt: f32) {
        if self.player.mode == GameMode::Spectator {
            self.target = None;
            self.survival.mining = None;
            return;
        }
        let creative = self.player.mode == GameMode::Creative;
        let reach = if creative {
            CREATIVE_REACH
        } else {
            SURVIVAL_REACH
        };
        let origin = self.camera.position - self.planet.center;
        let dir = self.camera.forward().as_dvec3();
        self.target = interact::raycast(&self.planet, &self.streamer, origin, dir, reach);
        if self.survival.screen.is_some() || !self.mouse_grabbed {
            self.survival.mining = None;
            return;
        }
        let pose = self.pose();
        let mut world = survival::World {
            planet: &self.planet,
            streamer: &mut self.streamer,
        };
        let message = self.survival.interact(
            &mut world,
            &self.input,
            self.player.mode,
            &pose,
            self.target.as_ref(),
            reach,
            dt,
        );
        if let Some(message) = message {
            self.message = Some((message, Instant::now()));
        }
    }

    fn toggle_console(&mut self) {
        self.console.open = !self.console.open;
        self.input.clear();
        if self.console.open {
            self.set_mouse_grab(false);
        } else if self.capture.is_none() {
            self.set_mouse_grab(true);
        }
    }

    fn run_command(&mut self, line: &str) {
        let mut words = line.split_whitespace();
        let command = words.next().unwrap_or("").trim_start_matches('/');
        let args: Vec<&str> = words.collect();
        let number = |i: usize| args.get(i).and_then(|a| a.parse::<f64>().ok());
        let frame = self.system.planet_frame(self.time);
        let feet = self.player.body_position(&frame);
        let up = feet.normalize_or(DVec3::Y);
        match command {
            "help" => {
                for line in console::HELP {
                    self.console.print(*line);
                }
            }
            "clear" => self.console.lines.clear(),
            "gamemode" | "gm" => match args.first().and_then(|m| m.parse::<GameMode>().ok()) {
                Some(mode) => {
                    if self.player.mode == GameMode::Spectator && mode != GameMode::Spectator {
                        // Leave spectator at rest where you are.
                        self.player.teleport(feet, &frame);
                    }
                    self.player.set_mode(mode);
                    self.console.print(format!("Game mode: {}", mode.name()));
                }
                None => self
                    .console
                    .print("Usage: gamemode <survival|creative|spectator>"),
            },
            "tp" => match (number(0), number(1)) {
                (Some(lat), Some(lon)) => {
                    let dir = direction(lat, lon);
                    let ground = Self::ground_position(&self.planet, &self.terrain, dir);
                    let feet = ground + dir * number(2).unwrap_or(0.0);
                    self.player.teleport(feet, &frame);
                    self.console.print(format!("Teleported to {lat}, {lon}"));
                }
                _ => self
                    .console
                    .print("Usage: tp <lat> <lon> [altitude above ground]"),
            },
            "orbit" => {
                let altitude = number(0).unwrap_or(20.0) * 1000.0;
                let r = self.planet.sea_radius + altitude;
                let position = frame.to_inertial(up * r);
                // Prograde: eastwards, the way the planet turns (inclined at your latitude).
                let axis = frame.to_inertial(DVec3::Y);
                let east = axis
                    .cross(position)
                    .try_normalize()
                    .unwrap_or_else(|| position.any_orthonormal_vector());
                self.player.position = position;
                self.player.velocity = east * (self.gravity.mu / r).sqrt();
                self.player.on_ground = false;
                self.player.flying = false;
                self.console.print(format!(
                    "Circular orbit at {:.1} km, {:.0} m/s{}",
                    altitude / 1000.0,
                    (self.gravity.mu / r).sqrt(),
                    if self.player.mode == GameMode::Spectator {
                        " (spectators don't fall: switch to survival to orbit)"
                    } else {
                        ""
                    }
                ));
            }
            "launch" => match number(0) {
                Some(speed) => {
                    self.player.velocity += frame.to_inertial(up) * speed;
                    self.player.on_ground = false;
                    self.console.print(format!("Launched at {speed} m/s"));
                }
                None => self.console.print("Usage: launch <m/s>"),
            },
            "time" => match number(0) {
                Some(hours) => {
                    self.time = self
                        .system
                        .time_of_day(self.time, up, hours.rem_euclid(24.0));
                    self.console.print(format!("Time set to {hours:.2} h"));
                }
                None => self.console.print("Usage: time <hours>"),
            },
            "warp" => match number(0) {
                Some(w) => {
                    self.warp = WARP_LEVELS
                        .iter()
                        .rposition(|&l| l <= w.max(1.0))
                        .unwrap_or(0);
                    self.console
                        .print(format!("Time warp x{}", WARP_LEVELS[self.warp]));
                }
                None => self.console.print("Usage: warp <factor>"),
            },
            "drag" => match args.first().copied() {
                Some("on") => {
                    self.drag = true;
                    self.console.print("Air drag on");
                }
                Some("off") => {
                    self.drag = false;
                    self.console.print("Air drag off (vacuum physics)");
                }
                _ => self.console.print("Usage: drag <on|off>"),
            },
            "look" => match self.target.as_ref() {
                Some(hit) => {
                    let msg = format!("Looking at {:?} in {:?}", hit.block, hit.cell);
                    self.console.print(msg);
                }
                None => self.console.print("Not looking at a block"),
            },
            "break" => match self.target.as_ref().map(|h| (h.cell, h.block)) {
                Some((cell, block)) => {
                    let creative = self.player.mode == GameMode::Creative;
                    let mut world = survival::World {
                        planet: &self.planet,
                        streamer: &mut self.streamer,
                    };
                    self.survival.break_block(&mut world, cell, block, creative);
                    self.console.print(format!("Broke {}", block.name()));
                }
                None => self.console.print("Not looking at a block"),
            },
            "place" => {
                // Like a right click: uses a crafting table / furnace, or places the held
                // block.
                let mut input = Input::default();
                input.button_down(MouseButton::Right);
                let pose = self.pose();
                let mut world = survival::World {
                    planet: &self.planet,
                    streamer: &mut self.streamer,
                };
                self.survival.interact(
                    &mut world,
                    &input,
                    self.player.mode,
                    &pose,
                    self.target.as_ref(),
                    SURVIVAL_REACH,
                    0.0,
                );
                self.console.print("Used / placed");
            }
            "give" => {
                let count = number(1).map_or(1, |n| n.clamp(1.0, 64.0 * 36.0) as u32);
                match args.first().and_then(|id| Item::from_id(id)) {
                    Some(item) => {
                        let mut left = count;
                        while left > 0 {
                            let n = left.min(item.max_stack() as u32) as u8;
                            left -= n as u32;
                            let pose = self.pose();
                            if let Some(rest) = self.survival.inventory.add(ItemStack::new(item, n))
                            {
                                self.survival
                                    .spawn(rest, pose.feet + pose.up(), DVec3::ZERO, 0.0);
                            }
                        }
                        self.console.print(format!("Gave {count} {}", item.name()));
                    }
                    None => self.console.print(
                        "Usage: give <item> [count], e.g. give iron_pickaxe, give oak_log 16",
                    ),
                }
            }
            "aim" => match number(0) {
                Some(pitch) => {
                    self.camera.pitch = (pitch as f32).clamp(-89.9, 89.9).to_radians();
                    if let Some(yaw) = number(1) {
                        self.camera.yaw = (yaw as f32).to_radians();
                    }
                    // Look at the new target right away (commands run before the frame).
                    let origin = self.camera.position - self.planet.center;
                    let dir = self.camera.forward().as_dvec3();
                    self.target = interact::raycast(
                        &self.planet,
                        &self.streamer,
                        origin,
                        dir,
                        SURVIVAL_REACH,
                    );
                    self.console.print(format!("Aiming at pitch {pitch} deg"));
                }
                None => self.console.print("Usage: aim <pitch deg> [yaw deg]"),
            },
            "summon" => {
                let kind = match args.first().copied() {
                    Some("cow") => Some(MobKind::Cow),
                    Some("pig") => Some(MobKind::Pig),
                    Some("sheep") => Some(MobKind::Sheep),
                    _ => None,
                };
                match kind {
                    Some(kind) => {
                        // A few meters ahead, on the ground.
                        let pose = self.pose();
                        let ahead = pose.look - up * pose.look.dot(up);
                        let spot = (feet + ahead.normalize_or(up.any_orthonormal_vector()) * 4.0)
                            .normalize();
                        let ground = Self::ground_position(&self.planet, &self.terrain, spot);
                        self.survival.summon(kind, ground + spot * 0.5);
                        self.console.print(format!("Summoned a {}", kind.name()));
                    }
                    None => self.console.print("Usage: summon <cow|pig|sheep>"),
                }
            }
            "throw" => {
                // Like Q (`throw all`: Ctrl+Q).
                let pose = self.pose();
                self.survival
                    .throw_held(&pose, args.first().is_some_and(|a| *a == "all"));
            }
            "select" => match number(0) {
                Some(n) if (1.0..=9.0).contains(&n) => self.survival.select(n as usize - 1),
                _ => self.console.print("Usage: select <1-9>"),
            },
            "kill" => {
                self.respawn();
                self.console.print("Killed the player");
            }
            "heal" => {
                self.survival.health = survival::MAX_HEALTH;
                self.console.print("Healed");
            }
            "open" => {
                let screen = match args.first().copied() {
                    Some("creative") => Some(Screen::Creative),
                    Some("inventory") | None => Some(Screen::Inventory),
                    _ => None,
                };
                match screen {
                    Some(screen) => self.survival.open(screen),
                    None => self.console.print("Usage: open [inventory|creative]"),
                }
            }
            "speed" => match number(0) {
                Some(s) => {
                    self.fly_speed = s.clamp(0.5, 3.0e8);
                    self.console.print(format!("Spectator speed {s} m/s"));
                }
                None => self.console.print("Usage: speed <m/s>"),
            },
            "" => {}
            other => self
                .console
                .print(format!("Unknown command `{other}` (try `help`)")),
        }
    }

    /// The HUD, hotbar and console for this frame.
    fn overlay(&mut self) -> Ui {
        let (w, h) = self.renderer.screen_size();
        let mut ui = Ui::new(w, h);
        let frame = self.system.planet_frame(self.time);
        let feet = self.player.body_position(&frame);
        let altitude = feet.length() - self.planet.sea_radius;
        let physics = Physics {
            planet: &self.planet,
            gravity: &self.gravity,
            drag: self.drag,
        };
        let surface_speed = self.player.relative_velocity(&frame).length();
        let apsides = (!self.player.on_ground && (altitude > 1_000.0 || surface_speed > 100.0))
            .then(|| self.player.orbit(&physics, self.time))
            .flatten()
            .map(|o| {
                let (pe, ap) = o.apsides();
                (pe - self.planet.sea_radius, ap - self.planet.sea_radius)
            });
        if self
            .message
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs_f32() > 3.0)
        {
            self.message = None;
        }
        let up = feet.normalize_or(DVec3::Y);
        let info = HudInfo {
            mode: self.player.mode,
            flying: self.player.flying,
            time_of_day: self.system.local_time(self.time, up),
            warp: self.effective_warp,
            surface_speed,
            orbital_speed: self.player.velocity.length(),
            altitude,
            gravity: self.gravity.acceleration(self.player.position).length(),
            apsides,
            hotbar: &self.survival.inventory.slots[..inventory::HOTBAR],
            selected: self.survival.selected,
            selected_age: self.survival.selected_at.elapsed().as_secs_f32(),
            health: self.survival.health,
            attack_charge: 1.0 - self.survival.attack.0 / self.survival.attack.1.max(1e-3),
            mining: self.survival.progress(),
            food: self.survival.food,
            armor: self.survival.armor_points(),
            target: self.survival.target_name.clone(),
            message: self.message.as_ref().map(|(m, _)| m.as_str()),
            console: &self.console,
            screen_open: self.survival.screen.is_some(),
        };
        hud::draw(&mut ui, &info);
        self.survival.draw_screen(&mut ui, self.input.cursor);
        ui
    }

    fn altitude(&self) -> f64 {
        (self.camera.position - self.planet.center).length() - self.planet.sea_radius
    }

    /// Scene parameters for this frame, and whether the camera is underground.
    fn frame_params(&mut self, dt: f64) -> (FrameParams, bool) {
        let rel = self.camera.position - self.planet.center;
        let up = rel.try_normalize().unwrap_or(DVec3::Y);

        // The sun is wherever the orbit and the planet's spin put it.
        let sun = self.system.star_in_body_frame(self.time);
        let sun_dir = sun.normalize();
        let sun_angular_radius = (self.system.star.radius / sun.length()).asin() as f32;

        // Underground there is no sky: the background is dark and fog hides the edge of the
        // (smaller) loaded area. Inside solid rock it closes in further.
        let cave_color = Vec3::new(1.8e-4, 1.35e-4, 1.05e-4) * SUN_ILLUMINANCE;
        let (underground, cave_distance) =
            if self.streamer.block_at(rel).is_some_and(|b| b.is_opaque()) {
                (true, 96.0)
            } else if self.streamer.camera_underground(rel) {
                (true, UNDERGROUND_RADIUS * 0.9)
            } else {
                (false, 0.0)
            };

        // Auto exposure (eye adaptation) from the light around the camera: the sun through
        // the atmosphere (or the planet's shadow) and the sky. Above the atmosphere only
        // whether the camera itself is in sunlight matters (the lit planet fills the view).
        let a = &self.atmosphere;
        let r = rel.length() as f32;
        let mu_s = up.dot(sun_dir) as f32;
        let height = a.top_radius - a.bottom_radius;
        let space = smoothstep(0.4 * height, 1.2 * height, r - a.bottom_radius);
        let sun_t = a.sun_transmittance(&self.atmosphere_luts, r, mu_s);
        let sky = a.sky_irradiance(&self.atmosphere_luts, r, mu_s);
        let incidence = (mu_s * 0.8 + 0.2).clamp(0.0, 1.0) * (1.0 - space) + space;
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        // A quarter-albedo ground under sun and sky, plus the sky itself.
        let scene = (0.25 * luminance(sun_t * incidence) + 0.75 * luminance(sky)) / PI;
        let noon = (0.25 * 0.95 + 0.75 * 0.06) / PI;
        // Torchlight around the camera counts too (eye adaptation): a torch-lit spot at night
        // or in a cave isn't blown out.
        let level = self
            .planet
            .cell_at(rel)
            .map_or(0, |c| self.streamer.light(c)) as f32
            / 15.0;
        let torch = TORCH_IRRADIANCE / SUN_ILLUMINANCE * level / (4.0 - 3.0 * level);
        let torch_scene = 0.5 * torch / PI;
        // Adapt only partially (darker scenes stay darker), and never beyond a dim night.
        let relative = (noon / (scene + torch_scene).max(noon / 300.0)).powf(0.7);
        // Caves: a brighter-than-day exposure (the headlamp lights them), less near torches.
        let relative = if underground {
            (noon / torch_scene.max(1e-9)).powf(0.7).min(10.0)
        } else {
            relative
        };
        let target = 0.3 / (0.25 * 0.95 / PI) * relative / SUN_ILLUMINANCE;
        let exposure = match self.exposure {
            // Adapt in log space, over about a second.
            Some(e) => e * (target / e).powf((dt as f32 * 1.5).min(1.0)),
            None => target,
        };
        self.exposure = Some(exposure);

        let block_target = self.target.as_ref().map(|hit| BlockTarget {
            local_from_camera: interact::outline_matrix(&self.planet, hit.cell, rel),
            progress: self.survival.mining_progress(),
        });

        let params = FrameParams {
            camera_planet: rel,
            sun_dir: sun_dir.as_vec3(),
            sun_angular_radius,
            sun_illuminance: Vec3::splat(SUN_ILLUMINANCE),
            star_rotation: self.system.planet_rotation_matrix(self.time).as_mat3(),
            exposure,
            // Stars stand out more above the atmosphere (artistic, KSP-like).
            star_boost: 1.0 + 4.0 * space,
            sky: !underground,
            cave_fog: cave_color.extend(cave_distance as f32),
            night_ambient: NIGHT_AMBIENT * SUN_ILLUMINANCE,
            time: (self.time % 100_000.0) as f32,
            // Overlap a little with the loaded chunks so no sky shows through at the seam.
            far_discard_radius: (self.streamer.loaded_radius() - 32.0).max(0.0),
            target: block_target,
        };
        (params, underground)
    }

    fn title(&self, fps: f64) -> String {
        let rel = self.camera.position - self.planet.center;
        let up = rel.try_normalize().unwrap_or(DVec3::Y);
        let latitude = up.y.asin().to_degrees();
        let longitude = up.z.atan2(up.x).to_degrees();
        let biome = self.terrain.sample_column(up).biome.name();
        let hours = self.system.local_time(self.time, up);
        let s = self.streamer.stats();
        let r = self.render_stats;
        format!(
            "Astro Block | {fps:.0} FPS ({:.2} ms, cpu {:.2} stream {:.2} gpu-wait {:.2}) | chunks {}/{} (gen {}) drawn {} culled {} | lod {}/{} | mesh VRAM {} MB ({} MB reserved) | alt {:.0} m | {:.3}°, {:.3}° {} | {} | {:02}:{:02} x{}",
            1000.0 / fps.max(1e-3),
            self.cpu_ms.0,
            self.cpu_ms.1,
            r.gpu_wait_ms,
            s.loaded,
            s.desired,
            s.in_flight,
            r.chunks_drawn,
            r.chunks_culled,
            r.far_drawn,
            s.lod_desired,
            self.renderer.mesh_memory().0 >> 20,
            self.renderer.mesh_memory().1 >> 20,
            self.altitude(),
            latitude,
            longitude,
            biome,
            self.player.mode.name(),
            hours as u32,
            (hours.fract() * 60.0) as u32,
            WARP_LEVELS[self.warp],
        )
    }

    fn set_mouse_grab(&mut self, grab: bool) {
        if grab == self.mouse_grabbed {
            return;
        }
        if grab {
            // Windows doesn't support `Locked`; fall back to `Confined`.
            let ok = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            if !ok {
                log::warn!("could not grab the cursor");
            }
            self.window.set_cursor_visible(false);
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
        }
        self.mouse_grabbed = grab;
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Default)]
struct FrameStats {
    frames: u32,
    elapsed: f64,
    last_fps: f64,
}

impl FrameStats {
    /// Returns the average FPS twice per second.
    fn tick(&mut self, dt: f64) -> Option<f64> {
        self.frames += 1;
        self.elapsed += dt;
        if self.elapsed >= 0.5 {
            let fps = self.frames as f64 / self.elapsed;
            *self = Self {
                last_fps: fps,
                ..Self::default()
            };
            Some(fps)
        } else {
            None
        }
    }
}

struct Capture {
    path: PathBuf,
    after_secs: f64,
    start: Instant,
    requested: bool,
}

impl Capture {
    /// `ASTRO_CAPTURE="path.bmp;seconds"`
    fn from_env() -> Option<Self> {
        let spec = std::env::var("ASTRO_CAPTURE").ok()?;
        let (path, secs) = spec.split_once(';').unwrap_or((&spec, "8"));
        Some(Self {
            path: PathBuf::from(path),
            after_secs: secs.trim().parse().unwrap_or(8.0),
            start: Instant::now(),
            requested: false,
        })
    }
}
