mod app;
mod audio;
mod effects;
mod flight;
mod mobs;
mod net;
mod player;
mod render;
mod space;
mod survival;
mod ui;
mod util;
mod weather;
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
