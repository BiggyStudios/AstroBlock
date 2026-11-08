//! In a game with others everyone has their own clock and their own time warp. Players can
//! be hours apart, as in KSP's multiplayer "subspaces":
//!
//! - **Sync** (`time_sync`) jumps you to where another player is in time, and to their weather.
//! - **Follow** does that and stays attached: when they warp, you warp.
//! - Blocks, items and animals aren't tied to the clock, so they are simply shared. What is:
//!   the sun, the weather and anything in orbit. A rocket someone else flies at another time
//!   is shown where it would be at *your* time (`vessel_at`).
//! - **Weather** can't be run per player for nothing, so each group of players at the same
//!   time watches one simulation (`Multiplayer::sky`). Warp more than `SAME_TIME` away from
//!   the player you were watching and your own copy carries on from what you could see.
//!   Whoever then syncs to you watches yours.
//!
//! Clocks of players sharing a sky are kept together: rounding and hitches would otherwise
//! slowly split them.

use std::time::Instant;

use glam::{DQuat, DVec3};

use super::{Game, WARP_LEVELS, multiplayer::Remote};
use crate::{
    flight::vessel::{self, Vessel},
    net::protocol::{Msg, PlayerId, VesselState},
    space::{
        bodies::{Bodies, Body},
        conic::propagate,
        trajectory,
    },
    ui::menu::TimeRow,
};

/// Two clocks this close (s) are the same time: their owners share weather.
pub(super) const SAME_TIME: f64 = 60.0;
/// A clock this far off (s, times the warp) is simply set.
const CLOCK_SNAP: f64 = 8.0;
/// Others further apart than this (s) and floating aren't drawn: where they are depends on when.
const SEEN_APART: f64 = 10.0;

/// Whose clock this player is attached to.
#[derive(Clone, Copy, Debug)]
pub(super) struct Follow {
    pub player: PlayerId,
    /// Only until they stop warping (a sync, or just having joined).
    pub once: bool,
}

/// Fits the time step to reach another clock over a few seconds, by running a little fast or
/// slow (so nothing jumps).
fn time_scale(error: f64, warp: f64) -> f64 {
    1.0 + (error / (warp * 2.0)).clamp(-0.2, 0.2)
}

/// The warp level that runs at `rate`.
fn level_for(rate: f64) -> usize {
    WARP_LEVELS
        .iter()
        .rposition(|&l| l <= rate.max(1.0) * 1.000_001)
        .unwrap_or(0)
}

/// `seconds` between two clocks, in the units of the game's own day.
pub(super) fn time_gap(seconds: f64, solar_day: f64) -> String {
    let sign = if seconds < 0.0 { "-" } else { "+" };
    let hours = seconds.abs() / solar_day * 24.0;
    if hours >= 24.0 {
        format!("{sign}{:.1} days", hours / 24.0)
    } else if hours >= 1.0 {
        format!(
            "{sign}{} h {:02} m",
            hours as u32,
            (hours.fract() * 60.0) as u32
        )
    } else {
        format!("{sign}{} m", (hours * 60.0) as u32)
    }
}

/// Where a rocket described by `state` is at this player's time `t`: further along (or back)
/// its conic if it is coasting, otherwise over the same ground.
pub(super) fn vessel_at(
    bodies: &Bodies,
    state: &VesselState,
    t: f64,
) -> (Body, DVec3, DVec3, DQuat) {
    let dt = t - state.time;
    let raw = (state.body, state.position, state.velocity, state.attitude);
    if dt.abs() < 0.5 || state.landed.is_some() {
        return raw;
    }
    if vessel::coasting(bodies, state.body, state.position, state.throttle as f64) {
        if dt > 0.0 {
            let (body, r, v, _, _) = trajectory::advance(
                bodies,
                state.body,
                state.position,
                state.velocity,
                (state.time, t),
                vessel::RAILS_CLEARANCE,
            );
            return (body, r, v, state.attitude);
        }
        let (r, v) = propagate(bodies.mu(state.body), state.position, state.velocity, dt);
        return (state.body, r, v, state.attitude);
    }
    let (from, to) = (
        bodies.frame(state.body, state.time),
        bodies.frame(state.body, t),
    );
    let ground = from.to_body(state.position);
    let relative = from.relative_velocity(state.position, state.velocity);
    (
        state.body,
        to.to_inertial(ground),
        to.inertial_velocity(ground, relative),
        to.rotation * from.rotation.inverse() * state.attitude,
    )
}

impl Game {
    /// How fast this frame's world time should run (slightly off, to stay with another clock).
    pub(super) fn net_time_scale(&self) -> f64 {
        time_scale(self.net.clock_error, WARP_LEVELS[self.warp])
    }

    /// Stops following anyone's warp. Your clock simply goes on from where it is.
    pub(super) fn stop_following(&mut self) {
        let Some(follow) = self.net.follow.take() else {
            return;
        };
        if !follow.once {
            let name = self.player_called(follow.player);
            self.message = Some((format!("no longer following {name}"), Instant::now()));
        }
    }

    fn player_called(&self, id: PlayerId) -> String {
        self.net
            .players
            .get(&id)
            .map_or_else(|| "them".to_string(), |r| r.name.clone())
    }

    /// The stop button: no longer attached, and back to normal speed.
    pub(super) fn unfollow(&mut self) {
        if self.net.follow.is_some() {
            self.stop_following();
            self.warp = 0;
        }
    }

    /// Goes to another player's time and weather. `stay`: and stays attached to their warp.
    pub(super) fn time_sync(&mut self, id: PlayerId, stay: bool) {
        let Some(remote) = self.net.players.get(&id) else {
            return;
        };
        let (name, watches) = (remote.name.clone(), remote.sky);
        let Some((time, rate)) = remote.clock() else {
            self.message = Some((format!("waiting to hear from {name}"), Instant::now()));
            return;
        };
        if let Err(why) = self.clock_jump(time) {
            self.console.print(why.clone());
            self.message = Some((why, Instant::now()));
            return;
        }
        // Someone still warping is only caught by going along with them.
        self.net.follow = (stay || rate > 1.0).then_some(Follow {
            player: id,
            once: !stay,
        });
        self.warp = if self.net.follow.is_some() {
            level_for(rate)
        } else {
            0
        };
        self.flight.planner.warp_to = None;
        self.watch_sky(watches);
        let text = if stay {
            format!("following {name}")
        } else {
            format!("synced with {name}")
        };
        self.console.print(text.clone());
        self.message = Some((text, Instant::now()));
    }

    /// Sets this player's clock to `to` at once. If whatever they are flying can't get there
    /// (the reason comes back), nothing has changed.
    pub(super) fn clock_jump(&mut self, to: f64) -> Result<(), String> {
        let from = self.time;
        if to == from {
            return Ok(());
        }
        let back = to < from;
        let boarded = self.flight.boarded;
        if let Some(i) = boarded {
            let v = &self.flight.vessels[i];
            if !v.can_rail_warp(&self.bodies) {
                return Err("not while the engine burns or in the air".to_string());
            }
            if v.landed.is_none() {
                if back {
                    return Err("in flight you can only go forward in time".to_string());
                }
                let (body, r, vel, t, stopped) = trajectory::advance(
                    &self.bodies,
                    v.body,
                    v.position,
                    v.velocity,
                    (from, to),
                    vessel::RAILS_CLEARANCE,
                );
                if stopped || t < to {
                    return Err("your path comes down before then".to_string());
                }
                let v = &mut self.flight.vessels[i];
                (v.body, v.position, v.velocity) = (body, r, vel);
            }
        } else if let Some(eva) = &self.flight.eva {
            if back {
                return Err("in flight you can only go forward in time".to_string());
            }
            let (body, r, vel, t, stopped) = trajectory::advance(
                &self.bodies,
                eva.body,
                eva.position,
                eva.velocity,
                (from, to),
                vessel::RAILS_CLEARANCE,
            );
            if stopped || t < to {
                return Err("your path comes down before then".to_string());
            }
            if let Some(eva) = &mut self.flight.eva {
                (eva.body, eva.position, eva.velocity) = (body, r, vel);
            }
        }

        let old_frame = self.active_frame(from);
        self.time = to;
        let new_frame = self.active_frame(to);
        match boarded {
            Some(i) => {
                let v = &mut self.flight.vessels[i];
                v.sync_landed(&self.bodies, to);
                let (body, r, vel) = (v.body, v.position, v.velocity);
                (self.player.position, self.player.velocity) =
                    self.bodies.rebase(body, self.active, r, vel, to);
            }
            None => match &self.flight.eva {
                Some(eva) => {
                    (self.player.position, self.player.velocity) =
                        self.bodies
                            .rebase(eva.body, self.active, eva.position, eva.velocity, to);
                }
                None => self.player.carried_through_time(&old_frame, &new_frame),
            },
        }

        // Everything else that follows the clock.
        if back {
            for (i, v) in self.flight.vessels.iter_mut().enumerate() {
                if Some(i) != boarded && v.landed.is_none() {
                    (v.position, v.velocity) =
                        propagate(self.bodies.mu(v.body), v.position, v.velocity, to - from);
                }
            }
        } else {
            self.survival.run_furnaces(to - from);
        }
        self.update_vessels(from);
        self.weather.clock_jumped(to);
        self.net.clock_error = 0.0;
        Ok(())
    }

    /// Each frame: stays with the clock this player is attached to.
    pub(super) fn keep_time(&mut self) {
        self.net.clock_error = 0.0;
        let following = self.net.follow;
        if let Some(f) = following
            && !self.net.players.contains_key(&f.player)
        {
            self.net.follow = None;
            self.warp = 0;
            return;
        }
        let target = following.map(|f| f.player).or_else(|| {
            let sky = self.net.sky;
            (sky != self.net.me && self.warp == 0).then_some(sky)
        });
        let Some(target) = target else { return };
        let Some((time, rate)) = self.net.players.get(&target).and_then(Remote::clock) else {
            return;
        };
        let error = time - self.time;
        if following.is_some() {
            self.warp = level_for(rate);
        } else if rate > 1.0 || error.abs() > SAME_TIME {
            // They are warping, or have warped away: not a matter of keeping in step.
            return;
        }
        // Walking while they warp hard, physics can't go that fast but the clock still can.
        let capped = following.is_some() && self.effective_warp * 1.5 < rate;
        let limit = if capped {
            1.0
        } else {
            CLOCK_SNAP * WARP_LEVELS[self.warp]
        };
        if error.abs() <= limit || self.clock_jump(time).is_err() {
            self.net.clock_error = error;
        }
        if let Some(f) = following
            && f.once
            && rate <= 1.0
            && (time - self.time).abs() < 1.0
        {
            self.net.follow = None;
        }
    }

    /// Each frame: keeps watching weather from someone at this player's time.
    pub(super) fn keep_sky(&mut self) {
        let (me, sky) = (self.net.me, self.net.sky);
        if let Some(follow) = self.net.follow {
            let want = self
                .net
                .players
                .get(&follow.player)
                .map_or(me, |remote| remote.sky);
            self.watch_sky(want);
            return;
        }
        if sky == me {
            return;
        }
        let Some(remote) = self.net.players.get(&sky) else {
            self.own_sky();
            return;
        };
        if remote.sky != sky {
            let next = remote.sky;
            self.watch_sky(next);
            return;
        }
        if remote
            .clock()
            .is_some_and(|(time, _)| (time - self.time).abs() > SAME_TIME)
        {
            self.own_sky();
        }
    }

    /// From now on the weather shown here comes from `who`.
    pub(super) fn watch_sky(&mut self, who: PlayerId) {
        if who == self.net.me {
            self.own_sky();
            return;
        }
        if self.net.sky == who {
            return;
        }
        self.net.sky = who;
        self.net.keyed = [false; 6];
        self.net.listeners.clear();
        self.weather.stop_local();
        self.announce_sky();
    }

    /// From now on the weather is simulated here, starting from how it looks now.
    pub(super) fn own_sky(&mut self) {
        if self.net.sky == self.net.me {
            return;
        }
        self.net.sky = self.net.me;
        self.net.listeners.clear();
        let terrain = if self.active == Body::Planet {
            self.terrain.clone()
        } else {
            self.other.terrain.clone()
        };
        let sun = self
            .system
            .star_in_body_frame(self.time)
            .normalize()
            .as_vec3();
        self.weather.go_local(&terrain, self.time, sun);
        self.capture_weather();
        self.announce_sky();
    }

    fn announce_sky(&mut self) {
        let msg = Msg::Sky {
            who: self.net.me,
            watches: self.net.sky,
        };
        self.send_everyone(&msg, true);
    }

    /// Tells the player once when someone else has moved on ahead in time.
    pub(super) fn notice_apart(&mut self) {
        let settling = self.net.follow.is_some();
        let time = self.time;
        let day = self.system.solar_day();
        let mut news = None;
        for remote in self.net.players.values_mut() {
            let Some((theirs, _)) = remote.clock() else {
                continue;
            };
            let gap = theirs - time;
            let apart = gap.abs() > SAME_TIME;
            if apart && !remote.apart && gap > 0.0 && !settling {
                news = Some(format!(
                    "{} is {} ahead: esc, then time, to sync",
                    remote.name,
                    time_gap(gap, day).trim_start_matches('+')
                ));
            }
            remote.apart = apart;
        }
        if let Some(text) = news {
            self.console.print(text.clone());
            self.message = Some((text, Instant::now()));
        }
    }

    /// Whether a figure should be left out: floating, somewhere that depends on the clock.
    pub(super) fn floating_elsewhen(&self, time: f64, on_ground: bool, can_rail: bool) -> bool {
        (time - self.time).abs() > SEEN_APART && !on_ground && can_rail
    }

    /// What goes after a name over someone's head, when they aren't at this player's time.
    pub(super) fn gap_label(&self, remote: &Remote) -> Option<String> {
        let (theirs, _) = remote.clock()?;
        let gap = theirs - self.time;
        (gap.abs() > SAME_TIME).then(|| time_gap(gap, self.system.solar_day()))
    }

    /// The line on the HUD: who you follow, or who is at another time.
    pub(super) fn together_line(&self) -> Option<String> {
        if !self.net.in_session() {
            return None;
        }
        if let Some(follow) = self.net.follow
            && !follow.once
        {
            return Some(format!("following {}", self.player_called(follow.player)));
        }
        let mut apart: Vec<(PlayerId, String)> = self
            .net
            .players
            .iter()
            .filter_map(|(id, remote)| {
                let gap = self.gap_label(remote)?;
                Some((*id, format!("{} {gap}", remote.name)))
            })
            .collect();
        apart.sort();
        let list: Vec<String> = apart.into_iter().take(3).map(|(_, text)| text).collect();
        (!list.is_empty()).then(|| list.join("   "))
    }

    /// Everyone else, as the time page lists them.
    pub(super) fn time_rows(&self) -> Vec<TimeRow> {
        let mut rows: Vec<TimeRow> = self
            .net
            .players
            .iter()
            .map(|(id, remote)| {
                let clock = remote.clock();
                let gap = clock.map_or(0.0, |(time, _)| time - self.time);
                TimeRow {
                    id: *id,
                    name: remote.name.clone(),
                    gap: if gap.abs() > SAME_TIME {
                        time_gap(gap, self.system.solar_day())
                    } else {
                        "same time".to_string()
                    },
                    rate: format!("x{}", clock.map_or(1.0, |(_, rate)| rate).round()),
                    following: self.net.follow.is_some_and(|f| f.player == *id && !f.once),
                    apart: gap.abs() > SAME_TIME,
                }
            })
            .collect();
        rows.sort_by_key(|r| r.id);
        rows
    }

    /// Finds someone by number or name (console).
    pub(super) fn player_named(&self, who: &str) -> Option<PlayerId> {
        if let Ok(id) = who.parse::<PlayerId>()
            && self.net.players.contains_key(&id)
        {
            return Some(id);
        }
        self.net
            .players
            .iter()
            .find(|(_, r)| r.name.replace(' ', "_") == who)
            .map(|(id, _)| *id)
    }
}

/// What the network says about a rocket, as of this player's time.
pub(super) fn place_vessel(bodies: &Bodies, v: &mut Vessel, state: &VesselState, t: f64) {
    (v.body, v.position, v.velocity, v.attitude) = vessel_at(bodies, state, t);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{SolarSystem, gravity::PlanetGravity};

    fn bodies() -> Bodies {
        let gravity = PlanetGravity::new(43_900.0, 1_200.0, 9_000.0);
        Bodies::new(SolarSystem::new(), gravity, 10_982.0)
    }

    fn state(position: DVec3, velocity: DVec3, throttle: f32, time: f64) -> VesselState {
        VesselState {
            id: 1,
            time,
            body: Body::Planet,
            position,
            velocity,
            attitude: DQuat::IDENTITY,
            spin: glam::Vec3::ZERO,
            throttle,
            fuel: 0.0,
            temperature: 0.0,
            heating: 0.0,
            landed: None,
        }
    }

    #[test]
    fn the_clock_catches_up_gently() {
        assert_eq!(time_scale(0.0, 1.0), 1.0);
        assert!((time_scale(0.2, 1.0) - 1.1).abs() < 1e-9);
        // Never more than 20 % off.
        assert_eq!(time_scale(5.0, 1.0), 1.2);
        assert_eq!(time_scale(-5.0, 1.0), 0.8);
        // At x10 the same error is a tenth as urgent.
        assert!((time_scale(0.2, 10.0) - 1.01).abs() < 1e-9);
    }

    #[test]
    fn warp_levels_are_found_from_a_rate() {
        for (i, level) in WARP_LEVELS.iter().enumerate() {
            assert_eq!(level_for(*level), i);
            assert_eq!(level_for(*level as f32 as f64), i);
        }
        assert_eq!(level_for(0.0), 0);
    }

    #[test]
    fn gaps_read_in_the_games_own_hours() {
        let day = 1_800.0;
        assert_eq!(time_gap(150.0, day), "+2 h 00 m");
        assert_eq!(time_gap(-75.0, day), "-1 h 00 m");
        assert_eq!(time_gap(60.0, day), "+48 m");
        assert_eq!(time_gap(3_600.0, day), "+2.0 days");
    }

    #[test]
    fn a_coasting_rocket_is_shown_along_its_orbit_either_way() {
        let bodies = bodies();
        let r = 43_900.0 + 30_000.0;
        let speed = (bodies.mu(Body::Planet) / r).sqrt();
        let (position, velocity) = (DVec3::X * r, DVec3::Y * speed);
        let start = state(position, velocity, 0.0, 1_000.0);

        let (_, ahead, ahead_v, _) = vessel_at(&bodies, &start, 1_400.0);
        let (expected, _) = propagate(bodies.mu(Body::Planet), position, velocity, 400.0);
        assert!((ahead - expected).length() < 1e-3);
        assert!((ahead.length() - r).abs() < 1.0, "stays on the circle");

        // From the future, back to where it was.
        let later = state(ahead, ahead_v, 0.0, 1_400.0);
        let (_, back, _, _) = vessel_at(&bodies, &later, 1_000.0);
        assert!((back - position).length() < 1e-2);
    }

    #[test]
    fn a_rocket_under_power_stays_over_the_same_ground() {
        let bodies = bodies();
        let frame = bodies.frame(Body::Planet, 1_000.0);
        let over = DVec3::X * 45_000.0;
        let position = frame.to_inertial(over);
        let velocity = frame.inertial_velocity(over, DVec3::Y * 20.0);
        let burning = state(position, velocity, 1.0, 1_000.0);

        let later = 1_450.0;
        let (_, p, v, _) = vessel_at(&bodies, &burning, later);
        let frame = bodies.frame(Body::Planet, later);
        assert!((frame.to_body(p) - over).length() < 1e-6);
        assert!((frame.relative_velocity(p, v) - DVec3::Y * 20.0).length() < 1e-6);
    }

    #[test]
    fn close_clocks_change_nothing() {
        let bodies = bodies();
        let s = state(DVec3::X * 90_000.0, DVec3::Y * 700.0, 0.0, 50.0);
        let (_, p, v, _) = vessel_at(&bodies, &s, 50.2);
        assert_eq!((p, v), (s.position, s.velocity));
    }
}
