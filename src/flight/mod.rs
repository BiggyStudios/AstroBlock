//! Rockets: the craft's physics, its model, its HUD and the map view with its maneuver
//! planner. The game's side of flying (boarding, cameras, effects) is in `app/flight.rs`.

pub mod craft;
pub mod craft_model;
mod craft_physics;
pub mod hud;
pub mod map_view;
pub mod rocket_model;
pub mod vessel;
