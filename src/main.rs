mod anim;
mod app;
mod audio;
mod camera;
mod console;
mod crafting;
mod drops;
mod effects;
mod flight_hud;
mod furnace;
mod hud;
mod input;
mod interact;
mod inventory;
mod items;
mod map_view;
mod mob_models;
mod mobs;
mod models;
mod player;
mod render;
mod rocket_model;
mod screen;
mod space;
mod survival;
mod ui;
mod util;
mod vessel;
mod viewmodel;
mod world;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let event_loop = EventLoop::new()?;
    // Render continuously (a game, not a desktop app that sleeps until input).
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::default();
    event_loop.run_app(&mut app)?;

    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
