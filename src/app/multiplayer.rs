//! Playing together: hosting, joining, and keeping everyone's world the same.
//!
//! **Who decides what.** The host owns the world and its saves: every change goes through it,
//! and it is what a new player downloads. But no machine has the whole planet loaded, so
//! each player simulates what is around them:
//!
//! | what | decided by |
//! |---|---|
//! | where you are, your inventory, health | you (the host keeps a copy with the world) |
//! | blocks, models, chests, furnaces | whoever changes them; the host passes it on |
//! | dropped items | anyone can drop; the host decides who picks up |
//! | animals | the player they spawned near, handed over when someone else is closer |
//! | a rocket in flight | its pilot (the host grants the seat) |
//! | an empty rocket | the host |
//! | your clock and time warp | you (see `timeline.rs`) |
//! | the weather, tornadoes, eruptions | one player for everyone at the same time: the host, or whoever has warped away |
//! | tornadoes tearing up ground | whoever has that ground loaded |
//! | lava rivers | worked out by each player from the same eruption (not sent) |
//!
//! Peers are trusted (you are playing with friends), but anything from the network is still
//! checked enough that a broken or hostile message can't crash the game or write a file.

use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use glam::{DVec3, Quat, Vec3};
use serde::{Deserialize, Serialize};

use super::{
    EYE_HEIGHT, Game, WARP_LEVELS,
    flight::Eva,
    session::{Opening, SavedGame},
    timeline::{self, Follow},
};
use crate::{
    audio::Sound,
    flight::{
        map_view::Planner,
        vessel::{FUEL_CAPACITY, Vessel},
    },
    mobs::Mob,
    net::{
        Event, Link, PeerId,
        direct::{self, DirectClient, DirectHost},
        protocol::{
            self, HOST, MAX_PLAYERS, Msg, NetTornado, NetVolcano, PROTOCOL, PlayerId, PlayerPose,
            VesselState, WorldBlob, WorldEvent,
        },
        steam::{self, Friend, Steam},
    },
    player::{
        GameMode, Physics, SavedPlayer,
        avatar::{self, AvatarPose, avatar_model},
    },
    render::{DrawItem, Fade, MeshHandle, models},
    space::bodies::Body,
    survival::{NetSide, Outgoing, SavedSurvival},
    ui::{
        Ui,
        anim::approach,
        menu::{Menu, Page},
    },
    util::FastMap,
    weather::{self, sim},
    world::{chunk::BlockCell, planet::Planet},
};

/// Seconds between position updates.
const TICK: f32 = 1.0 / 20.0;
const MOB_TICK: f32 = 1.0 / 8.0;
const LIVE_TICK: f32 = 0.33;
/// An idle rocket's position is corrected this often (s).
const VESSEL_TICK: f32 = 0.5;
/// One cube face of the weather goes out this often (s), so all six take 9 s.
const WEATHER_TICK: f32 = 1.5;
/// A client hands the host its state this often (s), for when it disappears.
const RECORD_TICK: f32 = 30.0;
/// How long a connection may stay silent (s): playing, and before the world has loaded.
const SILENCE: f64 = 20.0;
const SILENCE_LOADING: f64 = 150.0;
const CONNECT_TIMEOUT: f64 = 20.0;
/// How much is sent to one player per second at most (reliable traffic), and the size of
/// the pieces a world goes out in.
const RATE: f64 = 1_500_000.0;
const WORLD_PIECE: usize = 60_000;
/// Limits on what the other side may send.
const MAX_BLOB: usize = 512 * 1024 * 1024;
const MAX_WORLD: usize = 256 * 1024 * 1024;
const MAX_RECORD: usize = 4 * 1024 * 1024;
const MAX_BACKLOG: usize = 96 * 1024 * 1024;
const MAX_NAME: usize = 28;
const MAX_SYSTEMS_SENT: usize = 64;
/// Mobs per datagram.
const MOBS_PER_PACKET: usize = 36;
/// Others are drawn out to here (m); sounds and animals travel this far.
const AVATAR_VIEW: f64 = 900.0;
const NAME_VIEW: f64 = 96.0;
const SOUND_RANGE: f64 = 64.0;
const MOB_RANGE: f64 = 320.0;
/// An animal goes to another player once its owner is this far away, and they are close.
const HANDOFF_OWNER: f64 = 72.0;
const HANDOFF_OTHER: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Role {
    #[default]
    Offline,
    Connecting,
    Host,
    Client,
}

/// A connection, at the host.
struct Peer {
    link: PeerId,
    /// Known once it has said hello.
    player: Option<PlayerId>,
    name: String,
    stable: String,
    ready: bool,
    out: VecDeque<Vec<u8>>,
    /// Bytes in `out`.
    queued: usize,
    heard: Instant,
    pose: Option<PlayerPose>,
}

/// Another player, as drawn here.
pub(super) struct Remote {
    pub name: String,
    /// Whose weather they are watching (themselves: they run their own).
    pub sky: PlayerId,
    /// At another time from this player (they have been told).
    pub apart: bool,
    pose: Option<PlayerPose>,
    heard: Instant,
    /// Where the model is drawn (eases after the news).
    feet: DVec3,
    look: Vec3,
    walk: f32,
    swing: f32,
    arm: f32,
    swings: u32,
    uses: u32,
    mesh: Option<MeshHandle>,
    item: Option<MeshHandle>,
}

impl Remote {
    fn new(name: String) -> Self {
        Self {
            name,
            sky: HOST,
            apart: false,
            pose: None,
            heard: Instant::now(),
            feet: DVec3::ZERO,
            look: Vec3::Y,
            walk: 0.0,
            swing: 0.0,
            arm: 0.0,
            swings: 0,
            uses: 0,
            mesh: None,
            item: None,
        }
    }

    /// Their clock now and how fast it is running, going by what was last heard.
    pub(super) fn clock(&self) -> Option<(f64, f64)> {
        let pose = self.pose.as_ref()?;
        let since = self.heard.elapsed().as_secs_f64().min(0.25);
        let rate = pose.rate as f64;
        Some((pose.time + since * rate, rate))
    }
}

/// A player's side of a game, which the host keeps with the world for next time.
#[derive(Serialize, Deserialize)]
struct PlayerRecord {
    active: Body,
    player: SavedPlayer,
    camera: (Quat, f32, f32),
    survival: SavedSurvival,
    fly_speed: f64,
    eva: Option<Eva>,
}

/// A world on its way here.
struct Downloading {
    seed: u64,
    mode: GameMode,
    data: Vec<u8>,
}

/// A downloaded world, ready to be built.
pub(super) struct JoinedWorld {
    seed: u64,
    mode: GameMode,
    saved: SavedGame,
    home: Vec<u8>,
    moon: Vec<u8>,
    weather: Vec<Vec<u8>>,
    record: Option<Vec<u8>>,
    pilots: Vec<(u32, PlayerId)>,
}

pub(super) struct Multiplayer {
    pub steam: Option<Steam>,
    link: Link,
    pub role: Role,
    pub me: PlayerId,
    peers: Vec<Peer>,
    /// A client's reliable messages on their way to the host.
    out: VecDeque<Vec<u8>>,
    pub players: FastMap<PlayerId, Remote>,
    /// Each player's state, by who they are (host; saved with the world).
    pub records: BTreeMap<String, String>,
    next_player: PlayerId,
    downloading: Option<Downloading>,
    /// What arrived while the world was still loading.
    early: Vec<Msg>,
    /// Waiting for everything to arrive and be built.
    loaded: bool,
    started: Instant,
    heard: Instant,
    /// Send budget for this frame's reliable messages.
    budget: f64,
    tick: f32,
    mob_tick: f32,
    live_tick: f32,
    vessel_tick: f32,
    weather_tick: f32,
    record_tick: f32,
    /// Each weather face as last sent (our own weather) or received (someone else's).
    weather_faces: Vec<Vec<u8>>,
    weather_next: usize,
    /// Whose weather is showing (`me`: it is simulated here).
    pub sky: PlayerId,
    /// The faces received whole from `sky` so far (differences only apply on top of those).
    pub keyed: [bool; 6],
    /// Who watches this player's weather.
    pub listeners: Vec<PlayerId>,
    /// The other player whose clock and warp this one is attached to.
    pub follow: Option<Follow>,
    /// How far this clock is behind the one it keeps to (s).
    pub clock_error: f64,
    /// Sounds to tell the others about.
    sounds: Vec<(Sound, Vec3)>,
    /// Why the game ended, to show on the menu.
    pub notice: Option<String>,
    pub friends: Vec<Friend>,
    friends_at: Option<Instant>,
}

impl Multiplayer {
    pub(super) fn new(steam: Option<Steam>) -> Self {
        Self {
            steam,
            link: Link::None,
            role: Role::Offline,
            me: HOST,
            peers: Vec::new(),
            out: VecDeque::new(),
            players: FastMap::default(),
            records: BTreeMap::new(),
            next_player: 1,
            downloading: None,
            early: Vec::new(),
            loaded: false,
            started: Instant::now(),
            heard: Instant::now(),
            budget: 0.0,
            tick: 0.0,
            mob_tick: 0.0,
            live_tick: 0.0,
            vessel_tick: 0.0,
            weather_tick: 0.0,
            record_tick: RECORD_TICK,
            weather_faces: Vec::new(),
            weather_next: 0,
            sky: HOST,
            keyed: [true; 6],
            listeners: Vec::new(),
            follow: None,
            clock_error: 0.0,
            sounds: Vec::new(),
            notice: None,
            friends: Vec::new(),
            friends_at: None,
        }
    }

    /// In a game with others (or open to them).
    pub(super) fn in_session(&self) -> bool {
        matches!(self.role, Role::Host | Role::Client)
    }

    pub(super) fn is_client(&self) -> bool {
        self.role == Role::Client
    }

    pub(super) fn is_host(&self) -> bool {
        self.role == Role::Host
    }

    pub(super) fn connecting(&self) -> bool {
        self.role == Role::Connecting
    }

    pub(super) fn player_count(&self) -> usize {
        self.players.len() + 1
    }

    /// Whether lava rivers may be worked out here: only from the one weather the host sees,
    /// so that the blocks they turn to lava are the same for all who get them.
    pub(super) fn lava_shared(&self) -> bool {
        let host_sky = if self.is_host() {
            self.sky
        } else {
            self.players.get(&HOST).map_or(HOST, |r| r.sky)
        };
        !self.in_session() || self.sky == host_sky
    }

    fn send_reliable_to(&mut self, i: usize, msg: &Msg) {
        let data = protocol::encode(msg);
        let peer = &mut self.peers[i];
        peer.queued += data.len();
        peer.out.push_back(data);
    }

    fn send_host(&mut self, msg: &Msg, reliable: bool) {
        if reliable {
            self.out.push_back(protocol::encode(msg));
        } else {
            self.link
                .send(&mut self.steam, PeerId::HOST, &protocol::encode(msg), false);
        }
    }

    /// To every player (all but `except`). Reliable news also goes to those still loading:
    /// it queues up behind their world, and they apply it once that is built. Without it
    /// their copy would miss whatever changed in the meantime.
    fn broadcast(&mut self, msg: &Msg, reliable: bool, except: Option<PlayerId>) {
        let data = protocol::encode(msg);
        for peer in &mut self.peers {
            if peer.player.is_none() || peer.player == except {
                continue;
            }
            if reliable {
                peer.queued += data.len();
                peer.out.push_back(data.clone());
            } else if peer.ready {
                self.link.send(&mut self.steam, peer.link, &data, false);
            }
        }
    }

    /// (Host.) To everyone who watches the weather of `from`.
    fn send_watchers(&mut self, from: PlayerId, msg: &Msg, reliable: bool) {
        let data = protocol::encode(msg);
        for peer in &mut self.peers {
            let Some(id) = peer.player else { continue };
            let sky = self.players.get(&id).map_or(HOST, |r| r.sky);
            if id == from || sky != from {
                continue;
            }
            if reliable {
                peer.queued += data.len();
                peer.out.push_back(data.clone());
            } else if peer.ready {
                self.link.send(&mut self.steam, peer.link, &data, false);
            }
        }
    }

    /// To the players on `body` near `at`, unreliably (what only matters close by).
    fn nearby(&mut self, msg: &Msg, body: Body, at: DVec3, range: f64, except: Option<PlayerId>) {
        let data = protocol::encode(msg);
        for peer in &mut self.peers {
            if !peer.ready || peer.player == except {
                continue;
            }
            let near = peer
                .pose
                .as_ref()
                .is_some_and(|p| p.body == body && (p.feet - at).length() < range);
            if near {
                self.link.send(&mut self.steam, peer.link, &data, false);
            }
        }
    }

    fn flush(&mut self, dt: f64) {
        self.budget = (RATE * dt).max(WORLD_PIECE as f64 * 2.0);
        let budget = self.budget as usize;
        for peer in &mut self.peers {
            let mut left = budget;
            while left > 0 {
                let Some(data) = peer.out.front() else { break };
                if !self.link.send(&mut self.steam, peer.link, data, true) {
                    break;
                }
                left = left.saturating_sub(data.len());
                peer.queued -= data.len();
                peer.out.pop_front();
            }
        }
        let mut left = budget;
        while left > 0 {
            let Some(data) = self.out.front() else { break };
            if !self.link.send(&mut self.steam, PeerId::HOST, data, true) {
                break;
            }
            left = left.saturating_sub(data.len());
            self.out.pop_front();
        }
    }

    /// Whatever is under way is over; the connection closes. (The Steam client stays.)
    fn close(&mut self) {
        if let Some(steam) = &mut self.steam {
            steam.stop();
        }
        self.link = Link::None;
        self.role = Role::Offline;
        self.me = HOST;
        self.peers.clear();
        self.out.clear();
        self.players.clear();
        self.next_player = 1;
        self.downloading = None;
        self.early.clear();
        self.loaded = false;
        self.weather_faces.clear();
        self.sky = HOST;
        self.keyed = [true; 6];
        self.listeners.clear();
        self.follow = None;
        self.clock_error = 0.0;
        self.sounds.clear();
    }

    /// Keeps the friends list fresh while a menu shows it.
    pub(super) fn refresh_friends(&mut self) {
        if self
            .friends_at
            .is_some_and(|t| t.elapsed().as_secs_f32() < 2.0)
        {
            return;
        }
        self.friends_at = Some(Instant::now());
        self.friends = self.steam.as_ref().map(Steam::friends).unwrap_or_default();
    }
}

fn finite(v: DVec3) -> bool {
    v.is_finite() && v.length() < 1e13
}

fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| (' '..='~').contains(c))
        .take(MAX_NAME)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "player".to_string()
    } else {
        name.to_string()
    }
}

/// Only cells that exist on that grid (an out-of-range cell would index past a chunk).
fn valid_cell(planet: &Planet, c: BlockCell) -> bool {
    if c.face.0 >= 6 || !(0..planet.layers).contains(&c.k) {
        return false;
    }
    let n = planet.res_at(c.k);
    (0..n).contains(&c.u) && (0..n).contains(&c.v)
}

impl Game {
    // ---- Starting and ending a game ------------------------------------------------------

    fn player_name(&self) -> String {
        if let Ok(name) = std::env::var("ASTRO_NAME") {
            return clean_name(&name);
        }
        if let Some(steam) = &self.net.steam {
            return clean_name(&steam.name());
        }
        clean_name(&self.settings.player_name)
    }

    /// Opens the world being played to others. `bind`: where to listen for direct
    /// connections (by default every address, on the default port).
    pub(super) fn start_hosting(&mut self, bind: Option<SocketAddr>) {
        if self.backdrop || self.net.role != Role::Offline {
            return;
        }
        let bind = bind.unwrap_or(SocketAddr::from(([0, 0, 0, 0], direct::DEFAULT_PORT)));
        let direct = match DirectHost::bind(bind) {
            Ok(host) => Some(host),
            Err(e) => {
                log::error!("{e:#}");
                self.console.print(format!("{e:#}"));
                None
            }
        };
        let steam = self.net.steam.is_some();
        if direct.is_none() && !steam {
            self.message = Some(("could not open the game to others".into(), Instant::now()));
            return;
        }
        if let Some(steam) = &mut self.net.steam {
            steam.start_hosting();
        }
        let port = direct.as_ref().map(|d| d.port);
        self.net.link = Link::Host { direct, steam };
        self.net.role = Role::Host;
        self.net.me = HOST;
        self.net.started = Instant::now();
        self.net.tick = 0.0;
        self.survival.net = NetSide::Host;
        self.survival.me = HOST;
        self.survival.renumber(&mut self.other.state);
        for v in &mut self.flight.vessels {
            v.id = self.survival.new_id();
        }
        self.net.next_player = 1;
        self.capture_weather();
        let text = match port {
            Some(port) => format!("open to others on port {port}"),
            None => "open to steam friends".to_string(),
        };
        self.console.print(text.clone());
        self.message = Some((text, Instant::now()));
    }

    /// Starts joining a host by address.
    pub(super) fn join_direct(&mut self, address: &str) {
        let target = match direct::resolve(address) {
            Ok(target) => target,
            Err(e) => {
                self.join_failed(format!("{e:#}"));
                return;
            }
        };
        self.leave_for_joining();
        self.net.link = Link::Direct(DirectClient::connect(target));
        self.connecting_screen(format!("connecting to {address}"));
    }

    /// Starts joining a Steam friend.
    pub(super) fn join_steam(&mut self, host: u64) {
        if self.net.steam.is_none() {
            self.join_failed("steam isn't running".to_string());
            return;
        }
        self.leave_for_joining();
        if let Some(steam) = &mut self.net.steam {
            steam.join(host);
        }
        self.net.link = Link::Steam;
        self.connecting_screen("connecting through steam".to_string());
        self.say_hello();
    }

    fn connecting_screen(&mut self, text: String) {
        self.net.role = Role::Connecting;
        self.net.started = Instant::now();
        self.net.heard = Instant::now();
        self.net.loaded = false;
        self.net.notice = None;
        self.menu = Some(Menu::connecting(text));
        self.set_mouse_grab(false);
        self.input.clear();
    }

    /// Leaves whatever is going on (saved first) to go and join someone.
    fn leave_for_joining(&mut self) {
        if let Err(e) = self.save_world(true) {
            log::error!("{e:#}");
        }
        self.net_leave();
        self.session = None;
    }

    fn say_hello(&mut self) {
        let name = self.player_name();
        let stable = match &self.net.link {
            Link::Steam => self
                .net
                .steam
                .as_ref()
                .map_or_else(String::new, |s| format!("steam:{}", s.id())),
            _ => format!("id:{}", self.settings.client_id),
        };
        let msg = Msg::Hello {
            protocol: PROTOCOL,
            name,
            stable,
        };
        self.net.send_host(&msg, true);
    }

    fn join_failed(&mut self, why: String) {
        log::warn!("couldn't join: {why}");
        self.console.print(format!("couldn't join: {why}"));
        self.net.close();
        let mut menu = Menu::new(Page::Multiplayer);
        menu.error = Some(why);
        menu.prepare_join(&self.settings.player_name, &self.settings.last_address);
        // (Joining left the old world behind: back to the menu's view.)
        self.menu = Some(menu);
        if !self.backdrop {
            self.net.notice = self.menu.as_ref().and_then(|m| m.error.clone());
            self.begin_opening(Opening::Backdrop, "leaving");
        }
    }

    /// The connection to the host is gone.
    pub(super) fn host_lost(&mut self, why: String) {
        if self.net.role == Role::Connecting {
            self.join_failed(why);
            return;
        }
        log::warn!("disconnected: {why}");
        self.net_forget_players();
        self.net.close();
        self.net.notice = Some(why);
        self.session = None;
        self.begin_opening(Opening::Backdrop, "leaving");
    }

    fn net_forget_players(&mut self) {
        for remote in std::mem::take(&mut self.net.players).into_values() {
            for mesh in remote.mesh.into_iter().chain(remote.item) {
                self.renderer.destroy_mesh(mesh);
            }
        }
    }

    /// Ends the session: says goodbye and hangs up. Used when leaving a world or the game.
    pub(super) fn net_leave(&mut self) {
        if self.net.role == Role::Offline {
            return;
        }
        if self.net.is_client() && self.net.loaded {
            if let Some(record) = self.player_record() {
                self.net.send_host(&Msg::PlayerSave(record), true);
            }
            self.net.send_host(&Msg::Bye, true);
        }
        if self.net.is_host() {
            // (Steam doesn't tell anyone a session has ended.)
            self.net.broadcast(&Msg::Bye, true, None);
        }
        let mut link_events = Vec::new();
        self.net.flush(1.0);
        self.net.link.poll(&mut self.net.steam, &mut link_events);
        // Give the sockets a moment to get the last words out.
        std::thread::sleep(Duration::from_millis(120));
        self.net_forget_players();
        self.net.close();
        self.survival.net = NetSide::Offline;
        self.net.records.clear();
    }

    // ---- Each frame ----------------------------------------------------------------------

    /// Takes in everything that has arrived.
    pub(super) fn net_receive(&mut self) -> Result<()> {
        if let Some(steam) = &mut self.net.steam {
            steam.pump();
            if let Some(host) = steam.take_join_request() {
                self.join_steam(host);
            }
        }
        // A world that has finished loading takes what arrived in the meantime.
        if self.net.is_client() && !self.net.early.is_empty() && !self.loading() {
            for msg in std::mem::take(&mut self.net.early) {
                self.client_message(msg);
            }
        }
        let mut events = Vec::new();
        self.net.link.poll(&mut self.net.steam, &mut events);
        for event in events {
            match event {
                Event::Connected(peer) => {
                    if self.net.is_host() {
                        self.net.peers.push(Peer {
                            link: peer,
                            player: None,
                            name: String::new(),
                            stable: String::new(),
                            ready: false,
                            out: VecDeque::new(),
                            queued: 0,
                            heard: Instant::now(),
                            pose: None,
                        });
                    } else if self.net.connecting() {
                        self.net.heard = Instant::now();
                        self.say_hello();
                    }
                }
                Event::Received(peer, data) => {
                    let Some(msg) = protocol::decode(&data) else {
                        continue; // damaged, or not from this version
                    };
                    if self.net.is_host() {
                        if let Some(i) = self.net.peers.iter().position(|p| p.link == peer) {
                            self.net.peers[i].heard = Instant::now();
                            self.host_message(i, msg);
                        }
                    } else if matches!(self.net.role, Role::Client | Role::Connecting) {
                        self.net.heard = Instant::now();
                        self.client_message(msg);
                    }
                }
                Event::Lost(peer, why) => {
                    if self.net.is_host() {
                        if let Some(i) = self.net.peers.iter().position(|p| p.link == peer) {
                            self.drop_peer(i, "left");
                        }
                    } else if matches!(self.net.role, Role::Client | Role::Connecting) {
                        self.host_lost(why);
                        return Ok(());
                    }
                }
            }
        }
        self.net_timeouts();
        Ok(())
    }

    fn net_timeouts(&mut self) {
        match self.net.role {
            Role::Offline => {}
            Role::Connecting => {
                if self.net.heard.elapsed().as_secs_f64() > CONNECT_TIMEOUT {
                    self.join_failed("nobody answered".to_string());
                }
            }
            Role::Client => {
                if self.net.loaded && self.net.heard.elapsed().as_secs_f64() > SILENCE {
                    self.host_lost("the host stopped answering".to_string());
                }
            }
            Role::Host => {
                let mut i = 0;
                while i < self.net.peers.len() {
                    let peer = &self.net.peers[i];
                    let limit = if peer.ready { SILENCE } else { SILENCE_LOADING };
                    let silent = peer.heard.elapsed().as_secs_f64() > limit;
                    if silent || peer.queued > MAX_BACKLOG {
                        self.drop_peer(i, "lost connection");
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    /// Sends this frame's news. `dt` is real seconds.
    pub(super) fn net_send(&mut self, dt: f64) {
        if !self.net.in_session() {
            if self.net.connecting() {
                self.net.flush(dt);
            }
            return;
        }
        let dt32 = dt as f32;
        // Between building a world and showing it, nothing should go out yet.
        if self.net.is_client() && (self.loading() || !self.net.loaded) {
            self.net.flush(dt);
            return;
        }
        self.keep_time();
        self.keep_sky();
        self.notice_apart();
        self.net.tick -= dt32;
        if self.net.tick <= 0.0 {
            self.net.tick += TICK;
            if self.net.tick <= 0.0 {
                self.net.tick = TICK;
            }
            self.net_tick();
        }
        self.net.mob_tick -= dt32;
        if self.net.mob_tick <= 0.0 {
            self.net.mob_tick = MOB_TICK;
            self.send_mobs();
        }
        self.weather_ticks(dt32);
        if self.net.is_host() {
            self.host_ticks(dt32);
        } else {
            self.net.record_tick -= dt32;
            if self.net.record_tick <= 0.0 {
                self.net.record_tick = RECORD_TICK;
                if let Some(record) = self.player_record() {
                    self.net.send_host(&Msg::PlayerSave(record), true);
                }
            }
        }
        self.net.flush(dt);
    }

    /// Sends this player's weather, if it is theirs and someone is watching.
    fn weather_ticks(&mut self, dt: f32) {
        if self.net.sky != self.net.me || self.net.listeners.is_empty() {
            return;
        }
        self.net.live_tick -= dt;
        if self.net.live_tick <= 0.0 {
            self.net.live_tick = LIVE_TICK;
            self.send_weather_live();
        }
        self.net.weather_tick -= dt;
        if self.net.weather_tick <= 0.0 {
            self.net.weather_tick = WEATHER_TICK;
            self.send_weather_face();
            self.send_weather_systems();
        }
    }

    fn send_weather_systems(&mut self) {
        let msg = Msg::WeatherSystems {
            from: self.net.me,
            systems: self.weather.snapshot.systems.clone(),
        };
        self.send_my_watchers(&msg, true);
    }

    fn send_my_watchers(&mut self, msg: &Msg, reliable: bool) {
        if self.net.is_host() {
            let me = self.net.me;
            self.net.send_watchers(me, msg, reliable);
        } else {
            self.net.send_host(msg, reliable);
        }
    }

    fn host_ticks(&mut self, dt: f32) {
        self.net.vessel_tick -= dt;
        if self.net.vessel_tick <= 0.0 {
            self.net.vessel_tick = VESSEL_TICK;
            let t = self.time;
            let states: Vec<_> = self
                .flight
                .vessels
                .iter()
                .filter(|v| v.pilot.is_none() && v.landed.is_none())
                .map(|v| vessel_state(v, t))
                .collect();
            for state in states {
                self.net
                    .broadcast(&Msg::VesselState(Box::new(state)), false, None);
            }
        }
    }

    fn net_tick(&mut self) {
        let pose = self.my_pose();
        let msg = Msg::Pose(pose);
        if self.net.is_host() {
            self.net.broadcast(&msg, false, None);
        } else {
            self.net.send_host(&msg, false);
        }

        // What happened in the world since the last tick.
        let mut world_events = Vec::new();
        for item in std::mem::take(&mut self.survival.outbox) {
            match item {
                Outgoing::World(event) => world_events.push(event),
                Outgoing::Pickup(id) => {
                    self.net.send_host(&Msg::Pickup(self.active, id), true);
                }
                Outgoing::MobHit {
                    owner,
                    id,
                    damage,
                    from,
                    creative,
                } => {
                    let msg = Msg::MobHit {
                        owner,
                        id,
                        damage,
                        from,
                        creative,
                    };
                    if self.net.is_host() {
                        self.route_to_player(owner, &msg);
                    } else {
                        self.net.send_host(&msg, true);
                    }
                }
                Outgoing::MobGone(id) => {
                    let msg = Msg::MobGone(id);
                    self.send_everyone(&msg, true);
                }
            }
        }
        if !world_events.is_empty() {
            let msg = Msg::World(self.active, world_events);
            self.send_everyone(&msg, true);
        }
        let sounds = std::mem::take(&mut self.net.sounds);
        if !sounds.is_empty() {
            let at = sounds[0].1.as_dvec3();
            let msg = Msg::Sounds(self.active, sounds);
            if self.net.is_host() {
                self.net.nearby(&msg, self.active, at, SOUND_RANGE, None);
            } else {
                self.net.send_host(&msg, false);
            }
        }

        // The rocket I am flying.
        if let Some(i) = self.flight.boarded {
            let state = vessel_state(&self.flight.vessels[i], self.time);
            self.send_everyone(&Msg::VesselState(Box::new(state)), false);
        }
    }

    pub(super) fn send_everyone(&mut self, msg: &Msg, reliable: bool) {
        if self.net.is_host() {
            self.net.broadcast(msg, reliable, None);
        } else {
            self.net.send_host(msg, reliable);
        }
    }

    fn route_to_player(&mut self, player: PlayerId, msg: &Msg) {
        if let Some(i) = self.net.peers.iter().position(|p| p.player == Some(player)) {
            self.net.send_reliable_to(i, msg);
        }
    }

    fn my_pose(&self) -> PlayerPose {
        let frame = self.active_frame(self.time);
        let (feet, _) = self.feet_and_up();
        let velocity = self.player.relative_velocity(&frame);
        let physics = Physics {
            planet: &self.planet,
            gravity: &self.gravity,
            drag: self.drag,
        };
        let can_rail = match self.flight.boarded {
            Some(i) => self.flight.vessels[i].can_rail_warp(&self.bodies),
            None => {
                self.flight.eva.is_some() || self.player.can_rail_warp(&physics, &frame, self.time)
            }
        };
        PlayerPose {
            id: self.net.me,
            body: self.active,
            feet,
            velocity: velocity.as_vec3(),
            look: self.camera.forward(),
            held: self.survival.held().map(|s| s.item),
            swings: self.survival.swings,
            uses: self.survival.uses,
            mining: self.survival.mining.is_some(),
            eating: self.survival.eating > 0.0,
            on_ground: self.player.on_ground,
            spectator: self.player.mode == GameMode::Spectator,
            vessel: self.flight.boarded.map(|i| self.flight.vessels[i].id),
            can_rail,
            time: self.time,
            rate: self.effective_warp as f32,
        }
    }

    /// Hands the animals I simulate to the others, and away when someone else is nearer.
    fn send_mobs(&mut self) {
        let (feet, _) = self.feet_and_up();
        let others: Vec<(PlayerId, DVec3)> = self
            .net
            .players
            .iter()
            .filter_map(|(id, r)| {
                let pose = r.pose.as_ref()?;
                (pose.body == self.active).then_some((*id, pose.feet))
            })
            .collect();
        // Far from me, close to someone else: theirs now.
        let mut i = 0;
        while i < self.survival.mobs.len() {
            let mob = &self.survival.mobs[i];
            let mine = mob.remote.is_none();
            let far = (mob.position - feet).length() > HANDOFF_OWNER;
            let taker = others
                .iter()
                .find(|(_, at)| (*at - mob.position).length() < HANDOFF_OTHER);
            match taker {
                Some(&(to, _)) if mine && far => {
                    let mob = self.survival.take_mob(i);
                    let msg = Msg::MobHandoff {
                        to,
                        mob: Box::new(mob),
                    };
                    if self.net.is_host() {
                        self.route_to_player(to, &msg);
                    } else {
                        self.net.send_host(&msg, true);
                    }
                }
                _ => i += 1,
            }
        }
        let list: Vec<_> = self
            .survival
            .mobs
            .iter()
            .filter(|m| m.remote.is_none())
            .map(Mob::net_state)
            .collect();
        if list.is_empty() {
            return;
        }
        let owner = self.net.me;
        let body = self.active;
        for part in list.chunks(MOBS_PER_PACKET) {
            let msg = Msg::Mobs {
                owner,
                body,
                list: part.to_vec(),
            };
            if self.net.is_host() {
                self.net.nearby(&msg, body, feet, MOB_RANGE, None);
            } else {
                self.net.send_host(&msg, false);
            }
        }
    }

    fn send_weather_live(&mut self) {
        let snapshot = self.weather.snapshot.clone();
        let msg = Msg::WeatherLive {
            from: self.net.me,
            time: self.time,
            tornadoes: snapshot
                .tornadoes
                .iter()
                .map(|t| NetTornado {
                    dir: t.dir,
                    velocity: t.velocity,
                    radius: t.radius,
                    strength: t.strength,
                })
                .collect(),
            volcanoes: snapshot
                .volcanoes
                .iter()
                .map(|v| NetVolcano {
                    dir: v.dir,
                    summit: v.summit,
                    vent: v.vent,
                    started: v.started,
                    activity: v.activity,
                })
                .collect(),
        };
        self.send_my_watchers(&msg, false);
    }

    /// The maps as they are now, as the base the next differences are taken from.
    pub(super) fn capture_weather(&mut self) {
        let snapshot = self.weather.snapshot.clone();
        self.net.weather_faces = (0..6).map(|f| weather::pack_face(&snapshot, f)).collect();
    }

    /// Sends the next cube face: what changed since the last time, deflated.
    fn send_weather_face(&mut self) {
        let snapshot = self.weather.snapshot.clone();
        let face = self.net.weather_next;
        self.net.weather_next = (face + 1) % 6;
        let packed = weather::pack_face(&snapshot, face);
        let delta: Vec<u8> = packed
            .iter()
            .zip(&self.net.weather_faces[face])
            .map(|(new, old)| new.wrapping_sub(*old))
            .collect();
        self.net.weather_faces[face] = packed;
        let msg = Msg::WeatherFace {
            from: self.net.me,
            face: face as u8,
            data: protocol::compress(&delta),
        };
        self.send_my_watchers(&msg, true);
    }

    /// Someone says whose weather they watch now. Newly ours: they need all of it first.
    fn got_sky(&mut self, who: PlayerId, watches: PlayerId) {
        if let Some(remote) = self.net.players.get_mut(&who) {
            remote.sky = watches;
        }
        let me = self.net.me;
        if watches != me {
            self.net.listeners.retain(|l| *l != who);
            return;
        }
        if self.net.sky != me || self.net.listeners.contains(&who) {
            return;
        }
        // (With nobody relying on the last faces sent, start from the weather as it is.)
        if self.net.listeners.is_empty() {
            self.capture_weather();
        }
        self.net.listeners.push(who);
        self.send_weather_systems();
        for face in 0..6 {
            let msg = Msg::WeatherKey {
                from: me,
                to: who,
                face: face as u8,
                data: protocol::compress(&self.net.weather_faces[face]),
            };
            if self.net.is_host() {
                self.route_to_player(who, &msg);
            } else {
                self.net.send_host(&msg, true);
            }
        }
    }

    /// Sounds made here this frame, for the others to hear.
    pub(super) fn net_sound(&mut self, sound: Sound, at: DVec3) {
        if self.net.in_session() && self.net.sounds.len() < 24 {
            self.net.sounds.push((sound, at.as_vec3()));
        }
    }

    // ---- The host's side -----------------------------------------------------------------

    fn drop_peer(&mut self, i: usize, how: &str) {
        let peer = self.net.peers.remove(i);
        self.net.link.close(&mut self.net.steam, peer.link);
        let Some(id) = peer.player else { return };
        self.remove_player(id, how);
        self.net.broadcast(&Msg::PlayerLeft(id), true, None);
        // Their seats are free again.
        let seats: Vec<u32> = self
            .flight
            .vessels
            .iter_mut()
            .filter(|v| v.pilot == Some(id))
            .map(|v| {
                v.pilot = None;
                v.id
            })
            .collect();
        for vessel in seats {
            self.net.broadcast(&Msg::Pilot(vessel, None), true, None);
        }
    }

    fn remove_player(&mut self, id: PlayerId, how: &str) {
        self.net.listeners.retain(|l| *l != id);
        if let Some(remote) = self.net.players.remove(&id) {
            for mesh in remote.mesh.into_iter().chain(remote.item) {
                self.renderer.destroy_mesh(mesh);
            }
            let text = format!("{} {how}", remote.name);
            self.console.print(text.clone());
            self.message = Some((text, Instant::now()));
        }
    }

    /// Someone said hello: lets them in and sends them the world.
    fn welcome(&mut self, i: usize, protocol: u32, name: String, stable: String) {
        if self.net.peers[i].player.is_some() {
            return;
        }
        let refusal = if protocol != PROTOCOL {
            Some("the host is running a different version of the game")
        } else if self.net.player_count() >= MAX_PLAYERS {
            Some("the game is full")
        } else {
            None
        };
        if let Some(why) = refusal {
            self.net.send_reliable_to(i, &Msg::Refused(why.to_string()));
            return;
        }
        let name = clean_name(&name);
        let stable: String = stable.chars().take(64).collect();
        let id = loop {
            let id = self.net.next_player;
            self.net.next_player = if id >= 250 { 1 } else { id + 1 };
            if !self.net.players.contains_key(&id) {
                break id;
            }
        };
        // If nobody depends on the last faces sent, start from the weather as it is now.
        let our_weather = self.net.sky == HOST;
        if our_weather && self.net.listeners.is_empty() {
            self.capture_weather();
        }
        let state = match serde_json::to_vec(&self.saved_game()) {
            Ok(state) => state,
            Err(e) => {
                log::error!("{e:#}");
                return;
            }
        };
        let (home, moon) = if self.active == Body::Planet {
            (
                self.streamer.edits_bytes(),
                self.other.streamer.edits_bytes(),
            )
        } else {
            (
                self.other.streamer.edits_bytes(),
                self.streamer.edits_bytes(),
            )
        };
        let blob = WorldBlob {
            state,
            home,
            moon,
            weather: self.net.weather_faces.clone(),
            record: self
                .net
                .records
                .get(&stable)
                .map(|r| r.clone().into_bytes()),
            pilots: self
                .flight
                .vessels
                .iter()
                .filter_map(|v| Some((v.id, v.pilot?)))
                .collect(),
        };
        let bytes = match postcard::to_stdvec(&blob) {
            Ok(bytes) => protocol::compress(&bytes),
            Err(e) => {
                log::error!("{e:#}");
                return;
            }
        };
        log::info!("{name} joined: sending {} KB of world", bytes.len() / 1024);
        let mut players: Vec<(PlayerId, String, PlayerId)> = self
            .net
            .players
            .iter()
            .map(|(id, r)| (*id, r.name.clone(), r.sky))
            .collect();
        players.push((HOST, self.player_name(), self.net.sky));
        let seed = self.planet_seed();
        let mode = self
            .session
            .as_ref()
            .map_or(self.player.mode, |s| s.meta.mode);
        self.net.send_reliable_to(
            i,
            &Msg::Welcome {
                you: id,
                seed,
                mode,
                players,
            },
        );
        for piece in bytes.chunks(WORLD_PIECE) {
            self.net
                .send_reliable_to(i, &Msg::WorldPart(piece.to_vec()));
        }
        self.net.send_reliable_to(i, &Msg::WorldEnd);
        let peer = &mut self.net.peers[i];
        peer.player = Some(id);
        peer.name = name.clone();
        peer.stable = stable;
        self.net
            .broadcast(&Msg::PlayerJoined(id, name.clone()), true, Some(id));
        self.net.players.insert(id, Remote::new(name.clone()));
        if our_weather {
            self.net.listeners.push(id);
        }
        let text = format!("{name} joined");
        self.console.print(text.clone());
        self.message = Some((text, Instant::now()));
    }

    fn planet_seed(&self) -> u64 {
        if self.active == Body::Planet {
            self.planet.seed
        } else {
            self.other.planet.seed
        }
    }

    fn host_message(&mut self, i: usize, msg: Msg) {
        // Until they've said who they are, nothing else counts.
        let Some(id) = self.net.peers[i].player else {
            if let Msg::Hello {
                protocol,
                name,
                stable,
            } = msg
            {
                self.welcome(i, protocol, name, stable);
            }
            return;
        };
        match msg {
            Msg::Ready => self.net.peers[i].ready = true,
            Msg::Bye => {
                self.drop_peer(i, "left");
            }
            Msg::Pose(mut pose) => {
                if !pose_is_sound(&pose) {
                    return;
                }
                pose.id = id;
                self.net.peers[i].pose = Some(pose.clone());
                self.got_pose(pose.clone());
                self.net.broadcast(&Msg::Pose(pose), false, Some(id));
            }
            Msg::PlayerSave(data) => {
                let stable = self.net.peers[i].stable.clone();
                if data.len() < MAX_RECORD
                    && let Ok(text) = String::from_utf8(data)
                {
                    self.net.records.insert(stable, text);
                }
            }
            Msg::Sounds(body, sounds) => {
                let Some(at) = sounds.first().map(|s| s.1.as_dvec3()) else {
                    return;
                };
                self.play_remote_sounds(body, &sounds);
                self.net
                    .nearby(&Msg::Sounds(body, sounds), body, at, SOUND_RANGE, Some(id));
            }
            Msg::World(body, events) => {
                let events = self.apply_world_events(body, events);
                if !events.is_empty() {
                    self.net
                        .broadcast(&Msg::World(body, events), true, Some(id));
                }
            }
            Msg::Pickup(body, drop) => self.pickup_for(i, body, drop),
            Msg::Mobs { body, list, .. } => {
                let at = self.net.peers[i].pose.as_ref().map(|p| p.feet);
                self.got_mobs(id, body, &list);
                if let Some(at) = at {
                    let msg = Msg::Mobs {
                        owner: id,
                        body,
                        list,
                    };
                    self.net.nearby(&msg, body, at, MOB_RANGE, Some(id));
                }
            }
            Msg::MobHit {
                owner,
                id: mob,
                damage,
                from,
                creative,
            } => {
                if owner == HOST {
                    self.mob_hit_here(mob, damage, from, creative);
                } else {
                    let msg = Msg::MobHit {
                        owner,
                        id: mob,
                        damage,
                        from,
                        creative,
                    };
                    self.route_to_player(owner, &msg);
                }
            }
            Msg::MobGone(mob) => {
                self.survival.remove_replica(mob);
                self.net.broadcast(&Msg::MobGone(mob), true, Some(id));
            }
            Msg::MobHandoff { to, mob } => {
                if to == HOST {
                    self.adopt_mob(*mob);
                } else {
                    self.route_to_player(to, &Msg::MobHandoff { to, mob });
                }
            }
            Msg::VesselNew(mut vessel) => {
                if !finite(vessel.position) {
                    return;
                }
                vessel.pilot = None;
                self.net
                    .broadcast(&Msg::VesselNew(vessel.clone()), true, Some(id));
                self.add_remote_vessel(*vessel);
            }
            Msg::VesselGone { id: vessel, reason } => {
                self.remote_vessel_gone(vessel, reason.clone());
                self.net
                    .broadcast(&Msg::VesselGone { id: vessel, reason }, true, Some(id));
            }
            Msg::VesselState(state) => {
                let flying = self
                    .flight
                    .vessels
                    .iter()
                    .any(|v| v.id == state.id && v.pilot == Some(id));
                if flying && finite(state.position) && finite(state.velocity) {
                    self.apply_vessel_state(&state);
                    self.net
                        .broadcast(&Msg::VesselState(state), false, Some(id));
                }
            }
            Msg::Board(vessel) => {
                let free = self
                    .flight
                    .vessels
                    .iter_mut()
                    .find(|v| v.id == vessel && v.pilot.is_none());
                let answer = if let Some(v) = free {
                    v.pilot = Some(id);
                    Msg::Pilot(vessel, Some(id))
                } else {
                    // Not free: say who has it, which tells them no.
                    let pilot = self
                        .flight
                        .vessels
                        .iter()
                        .find(|v| v.id == vessel)
                        .and_then(|v| v.pilot);
                    Msg::Pilot(vessel, pilot)
                };
                self.net.broadcast(&answer, true, None);
            }
            Msg::Pilot(vessel, None) => {
                let t = self.time;
                let Some(v) = self
                    .flight
                    .vessels
                    .iter_mut()
                    .find(|v| v.id == vessel && v.pilot == Some(id))
                else {
                    return;
                };
                v.pilot = None;
                let state = vessel_state(v, t);
                // It stops being news-at-20-Hz now, so everyone must get where it ended up.
                self.net
                    .broadcast(&Msg::VesselState(Box::new(state)), true, None);
                self.net.broadcast(&Msg::Pilot(vessel, None), true, None);
            }
            Msg::Sky { watches, .. } => {
                self.got_sky(id, watches);
                self.net
                    .broadcast(&Msg::Sky { who: id, watches }, true, Some(id));
            }
            Msg::WeatherSystems { systems, .. } => {
                if systems.len() > MAX_SYSTEMS_SENT {
                    return;
                }
                if self.net.sky == id {
                    self.weather.set_systems(systems.clone());
                }
                let msg = Msg::WeatherSystems { from: id, systems };
                self.net.send_watchers(id, &msg, true);
            }
            Msg::WeatherKey { to, face, data, .. } => {
                if to == HOST {
                    self.got_weather_key(id, face as usize, &data);
                } else {
                    let msg = Msg::WeatherKey {
                        from: id,
                        to,
                        face,
                        data,
                    };
                    self.route_to_player(to, &msg);
                }
            }
            Msg::WeatherFace { face, data, .. } => {
                self.got_weather_face(id, face as usize, &data);
                let msg = Msg::WeatherFace {
                    from: id,
                    face,
                    data,
                };
                self.net.send_watchers(id, &msg, true);
            }
            Msg::WeatherLive {
                time,
                tornadoes,
                volcanoes,
                ..
            } => {
                if !time.is_finite() {
                    return;
                }
                let msg = Msg::WeatherLive {
                    from: id,
                    time,
                    tornadoes: tornadoes.clone(),
                    volcanoes: volcanoes.clone(),
                };
                self.net.send_watchers(id, &msg, false);
                self.got_weather_live(id, time, tornadoes, volcanoes);
            }
            _ => {} // only the host may say the rest
        }
    }

    /// `peers[i]` wants a drop: the first to ask gets it.
    fn pickup_for(&mut self, i: usize, body: Body, id: u32) {
        let stack = if body == self.active {
            self.survival.take_drop(id)
        } else if body == self.other.body {
            self.survival.swap_contents(&mut self.other.state);
            let stack = self.survival.take_drop(id);
            self.survival.swap_contents(&mut self.other.state);
            stack
        } else {
            None
        };
        let Some(stack) = stack else { return };
        self.net.send_reliable_to(i, &Msg::Give(stack));
        self.net.broadcast(
            &Msg::World(body, vec![WorldEvent::DropGone(id)]),
            true,
            None,
        );
    }

    // ---- The client's side ---------------------------------------------------------------

    fn client_message(&mut self, msg: Msg) {
        // Everything before the world is on screen waits for it.
        let joining = matches!(
            msg,
            Msg::Refused(_) | Msg::Welcome { .. } | Msg::WorldPart(_) | Msg::WorldEnd | Msg::Bye
        );
        if !joining && (self.net.connecting() || self.loading() || !self.net.loaded) {
            // (Positions and such are out of date by then: only keep what must arrive.)
            if !matches!(
                msg,
                Msg::Pose(_) | Msg::Sounds(..) | Msg::Mobs { .. } | Msg::WeatherLive { .. }
            ) {
                self.net.early.push(msg);
            }
            return;
        }
        match msg {
            Msg::Refused(why) => self.join_failed(why),
            Msg::Bye => self.host_lost("the host ended the game".to_string()),
            Msg::Welcome {
                you,
                seed,
                mode,
                players,
            } => {
                self.net.me = you;
                self.net.players = players
                    .into_iter()
                    .map(|(id, name, sky)| {
                        let mut remote = Remote::new(clean_name(&name));
                        remote.sky = sky;
                        (id, remote)
                    })
                    .collect();
                self.net.downloading = Some(Downloading {
                    seed,
                    mode,
                    data: Vec::new(),
                });
                if let Some(menu) = &mut self.menu {
                    menu.set_loading_text("downloading the world");
                }
            }
            Msg::WorldPart(part) => {
                if let Some(d) = &mut self.net.downloading {
                    if d.data.len() + part.len() > MAX_BLOB {
                        self.host_lost("the world is too big".to_string());
                        return;
                    }
                    d.data.extend_from_slice(&part);
                }
            }
            Msg::WorldEnd => self.world_arrived(),
            Msg::PlayerJoined(id, name) => {
                let name = clean_name(&name);
                let text = format!("{name} joined");
                self.console.print(text.clone());
                self.message = Some((text, Instant::now()));
                self.net.players.insert(id, Remote::new(name));
            }
            Msg::PlayerLeft(id) => self.remove_player(id, "left"),
            Msg::Pose(pose) => {
                if pose_is_sound(&pose) {
                    self.got_pose(pose);
                }
            }
            Msg::Sounds(body, sounds) => self.play_remote_sounds(body, &sounds),
            Msg::World(body, events) => {
                self.apply_world_events(body, events);
            }
            Msg::Give(stack) => self.got_item(stack),
            Msg::Mobs { owner, body, list } => self.got_mobs(owner, body, &list),
            Msg::MobHit {
                id,
                damage,
                from,
                creative,
                ..
            } => self.mob_hit_here(id, damage, from, creative),
            Msg::MobGone(id) => self.survival.remove_replica(id),
            Msg::MobHandoff { mob, .. } => self.adopt_mob(*mob),
            Msg::VesselNew(vessel) => {
                if finite(vessel.position) {
                    self.add_remote_vessel(*vessel);
                }
            }
            Msg::VesselGone { id, reason } => self.remote_vessel_gone(id, reason),
            Msg::VesselState(state) => {
                if finite(state.position) && finite(state.velocity) {
                    self.apply_vessel_state(&state);
                }
            }
            Msg::Pilot(vessel, pilot) => self.got_pilot(vessel, pilot),
            Msg::Sky { who, watches } => self.got_sky(who, watches),
            Msg::WeatherSystems { from, systems } => {
                if from == self.net.sky && systems.len() <= MAX_SYSTEMS_SENT {
                    self.weather.set_systems(systems);
                }
            }
            Msg::WeatherKey {
                from, face, data, ..
            } => self.got_weather_key(from, face as usize, &data),
            Msg::WeatherFace { from, face, data } => {
                self.got_weather_face(from, face as usize, &data);
            }
            Msg::WeatherLive {
                from,
                time,
                tornadoes,
                volcanoes,
            } => self.got_weather_live(from, time, tornadoes, volcanoes),
            _ => {}
        }
    }

    fn got_weather_live(
        &mut self,
        from: PlayerId,
        time: f64,
        tornadoes: Vec<NetTornado>,
        volcanoes: Vec<NetVolcano>,
    ) {
        if from != self.net.sky || !time.is_finite() {
            return;
        }
        self.weather.set_live(
            time,
            tornadoes
                .into_iter()
                .map(|t| sim::Tornado::seen(t.dir, t.velocity, t.radius, t.strength))
                .collect(),
            volcanoes
                .into_iter()
                .map(|v| sim::VolcanoState::seen(v.dir, v.summit, v.vent, v.started, v.activity))
                .collect(),
        );
    }

    /// The whole world has arrived: unpack it, and build it behind the loading screen.
    fn world_arrived(&mut self) {
        let Some(download) = self.net.downloading.take() else {
            return;
        };
        match unpack_world(download) {
            Ok(world) => {
                // (A world in play was already left when joining began.)
                self.begin_opening(Opening::Join(Box::new(world)), "building the world");
            }
            Err(e) => self.host_lost(format!("{e:#}")),
        }
    }

    /// Builds a world that came from the host (called from `perform_opening`).
    pub(super) fn install_joined(&mut self, joined: JoinedWorld) -> Result<()> {
        let JoinedWorld {
            seed,
            mode,
            saved,
            home,
            moon,
            weather,
            record,
            pilots,
        } = joined;
        self.install_remote_world(seed, mode, saved.time());
        self.backdrop = false;
        self.session = None;
        self.apply_saved(saved, &home, &moon)?;
        // That was the host's game: the world stays, its player doesn't.
        self.survival.clear_player();
        self.survival.clear_mobs();
        self.other.state.clear_mobs();
        self.flight.boarded = None;
        self.flight.eva = None;
        self.flight.map = false;
        self.flight.planner = Planner::default();
        // (Before their things come back: what doesn't fit lands on the ground, for all to see.)
        self.survival.net = NetSide::Client;
        self.survival.me = self.net.me;
        for (vessel, pilot) in pilots {
            if let Some(v) = self.flight.vessels.iter_mut().find(|v| v.id == vessel) {
                v.pilot = Some(pilot);
            }
        }
        let restored = record
            .and_then(|bytes| serde_json::from_slice::<PlayerRecord>(&bytes).ok())
            .map(|record| self.apply_record(record))
            .unwrap_or(false);
        if !restored {
            self.switch_world(Body::Planet);
            self.player.set_mode(mode);
            let frame = self.active_frame(self.time);
            self.player.teleport(self.spawn_feet, &frame);
            self.reset_camera();
        }
        self.message = None;

        // The weather as the host had it.
        self.net.weather_faces = weather;
        let time = self.time;
        for (face, packed) in self.net.weather_faces.iter().enumerate() {
            self.weather.set_face(face, packed, time);
        }
        self.net.role = Role::Client;
        self.net.sky = HOST;
        self.net.keyed = [true; 6];
        self.net.listeners.clear();
        // Loading took a while: catch up with the host once its clock is heard.
        self.net.follow = Some(Follow {
            player: HOST,
            once: true,
        });
        self.net.loaded = true;
        self.net.heard = Instant::now();
        self.net.tick = 0.0;
        self.net.record_tick = RECORD_TICK;
        Ok(())
    }

    /// Back to the world as you left it. False if that doesn't work out.
    fn apply_record(&mut self, record: PlayerRecord) -> bool {
        if !matches!(record.active, Body::Planet | Body::Moon) {
            return false;
        }
        self.switch_world(record.active);
        self.player.restore(&record.player);
        self.fly_speed = record.fly_speed;
        self.flight.eva = record.eva;
        let frame = self.active_frame(self.time);
        let feet = self.player.body_position(&frame);
        self.survival.restore_player(record.survival, feet);
        self.last_feet = feet;
        let up = feet.normalize_or(DVec3::Y);
        self.camera.position = self.planet.center + feet + up * EYE_HEIGHT;
        self.camera.set_view_state(record.camera);
        true
    }

    /// What the host should keep of this player.
    fn player_record(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(&PlayerRecord {
            active: self.active,
            player: self.player.saved(),
            camera: self.camera.view_state(),
            survival: self.survival.saved_player(),
            fly_speed: self.fly_speed,
            eva: self.flight.eva.clone(),
        })
        .ok()
    }

    /// Tells the host the world is built (once the loading screen has gone).
    pub(super) fn net_world_shown(&mut self) {
        if self.net.is_client() {
            self.net.send_host(&Msg::Ready, true);
        }
    }

    /// A whole face from the weather being watched: differences can follow it now.
    fn got_weather_key(&mut self, from: PlayerId, face: usize, data: &[u8]) {
        if from != self.net.sky || face >= 6 {
            return;
        }
        let Some(whole) = protocol::decompress(data, weather::PACKED_FACE) else {
            return;
        };
        if whole.len() != weather::PACKED_FACE {
            return;
        }
        if self.net.weather_faces.len() != 6 {
            self.net.weather_faces = vec![vec![0; weather::PACKED_FACE]; 6];
        }
        self.weather.set_face(face, &whole, self.time);
        self.net.weather_faces[face] = whole;
        self.net.keyed[face] = true;
    }

    fn got_weather_face(&mut self, from: PlayerId, face: usize, data: &[u8]) {
        if from != self.net.sky || !self.net.keyed.get(face).copied().unwrap_or(false) {
            return;
        }
        let Some(base) = self.net.weather_faces.get_mut(face) else {
            return;
        };
        let Some(delta) = protocol::decompress(data, weather::PACKED_FACE) else {
            return;
        };
        if delta.len() != base.len() {
            return;
        }
        for (b, d) in base.iter_mut().zip(&delta) {
            *b = b.wrapping_add(*d);
        }
        let packed = base.clone();
        self.weather.set_face(face, &packed, self.time);
    }

    fn got_item(&mut self, stack: crate::survival::items::ItemStack) {
        let pose = self.pose();
        self.survival.received(stack, &pose);
    }

    // ---- Both sides ----------------------------------------------------------------------

    fn got_pose(&mut self, pose: PlayerPose) {
        let id = pose.id;
        if id == self.net.me {
            return;
        }
        let Some(remote) = self.net.players.get_mut(&id) else {
            return;
        };
        if remote.pose.is_none() || (remote.feet - pose.feet).length() > 40.0 {
            remote.feet = pose.feet;
            remote.look = pose.look;
            remote.swings = pose.swings;
            remote.uses = pose.uses;
        }
        remote.heard = Instant::now();
        remote.pose = Some(pose);
    }

    fn play_remote_sounds(&mut self, body: Body, sounds: &[(Sound, Vec3)]) {
        if body != self.active {
            return;
        }
        let listener = self.camera.position - self.planet.center;
        let right = self.camera.right().as_dvec3();
        for (sound, at) in sounds.iter().take(24) {
            if at.is_finite() {
                self.audio.play(*sound, at.as_dvec3() - listener, right);
            }
        }
    }

    /// Applies changes to a world (whichever it is). Returns the ones that were valid.
    fn apply_world_events(&mut self, body: Body, events: Vec<WorldEvent>) -> Vec<WorldEvent> {
        let here = body == self.active;
        if !here && body != self.other.body {
            return Vec::new();
        }
        if !here {
            self.survival.swap_contents(&mut self.other.state);
        }
        let (planet, streamer) = if here {
            (&self.planet, &mut self.streamer)
        } else {
            (&self.other.planet, &mut self.other.streamer)
        };
        let mut world = crate::survival::World {
            planet: planet.as_ref(),
            streamer,
        };
        let valid: Vec<WorldEvent> = events
            .into_iter()
            .filter(|event| valid_event(planet, event))
            .collect();
        for event in &valid {
            self.survival.apply_event(&mut world, event, here);
        }
        if !here {
            self.survival.swap_contents(&mut self.other.state);
        }
        valid
    }

    fn got_mobs(&mut self, owner: PlayerId, body: Body, list: &[protocol::MobState]) {
        if body != self.active || owner == self.net.me {
            return;
        }
        for state in list {
            if state.position.is_finite() {
                self.survival.update_replica(owner, state);
            }
        }
    }

    fn mob_hit_here(&mut self, id: u32, damage: f32, from: DVec3, creative: bool) {
        if finite(from) && damage.is_finite() {
            self.survival
                .mob_hit(id, damage.clamp(0.0, 100.0), from, creative);
        }
    }

    fn adopt_mob(&mut self, mut mob: Mob) {
        if finite(mob.position) {
            mob.remote = None;
            mob.owner = self.net.me;
            mob.mesh = None;
            self.survival.adopt(mob);
        }
    }

    // ---- Rockets ------------------------------------------------------------------------

    fn add_remote_vessel(&mut self, mut vessel: Vessel) {
        if self.flight.vessels.iter().any(|v| v.id == vessel.id) {
            return;
        }
        vessel.mesh = None;
        self.flight.vessels.push(vessel);
    }

    fn remote_vessel_gone(&mut self, id: u32, reason: Option<String>) {
        let Some(i) = self.flight.vessels.iter().position(|v| v.id == id) else {
            return;
        };
        match reason {
            Some(reason) => self.explode_vessel(i, &reason),
            None => self.remove_vessel(i),
        }
    }

    fn apply_vessel_state(&mut self, state: &VesselState) {
        let boarded = self.flight.boarded.map(|i| self.flight.vessels[i].id);
        let known_body = matches!(state.body, Body::Planet | Body::Moon);
        if boarded == Some(state.id) || !known_body || !state.time.is_finite() {
            return;
        }
        let t = self.time;
        let Some(v) = self.flight.vessels.iter_mut().find(|v| v.id == state.id) else {
            return;
        };
        timeline::place_vessel(&self.bodies, v, state, t);
        v.spin = state.spin.as_dvec3();
        v.throttle = state.throttle as f64;
        v.fuel = (state.fuel as f64).clamp(0.0, FUEL_CAPACITY);
        v.temperature = state.temperature as f64;
        v.heating = state.heating as f64;
        v.landed = state.landed;
        if v.landed.is_some() {
            v.sync_landed(&self.bodies, t);
        }
    }

    fn got_pilot(&mut self, id: u32, pilot: Option<PlayerId>) {
        let Some(i) = self.flight.vessels.iter().position(|v| v.id == id) else {
            return;
        };
        let was = self.flight.vessels[i].pilot;
        self.flight.vessels[i].pilot = pilot;
        if pilot == Some(self.net.me) && was != pilot && self.flight.boarded != Some(i) {
            self.enter_vessel(i);
        }
    }

    /// A rocket placed here: everyone gets a copy.
    pub(super) fn net_vessel_added(&mut self) {
        if !self.net.in_session() {
            return;
        }
        let Some(v) = self.flight.vessels.last_mut() else {
            return;
        };
        v.id = self.survival.new_id();
        let vessel = Box::new(v.clone());
        self.send_everyone(&Msg::VesselNew(vessel), true);
    }

    /// A rocket is gone here: tell the others why.
    pub(super) fn net_vessel_gone(&mut self, id: u32, reason: Option<&str>) {
        if self.net.in_session() {
            let msg = Msg::VesselGone {
                id,
                reason: reason.map(str::to_string),
            };
            self.send_everyone(&msg, true);
        }
    }

    /// Asks for the seat of rocket `i`. True: get in now (nobody else is around to ask).
    pub(super) fn request_seat(&mut self, i: usize) -> bool {
        if !self.net.in_session() {
            return true;
        }
        let v = &self.flight.vessels[i];
        let (id, pilot) = (v.id, v.pilot);
        if pilot.is_some() {
            self.message = Some(("someone is flying this rocket".into(), Instant::now()));
            return false;
        }
        if self.net.is_host() {
            self.flight.vessels[i].pilot = Some(HOST);
            self.net.broadcast(&Msg::Pilot(id, Some(HOST)), true, None);
            true
        } else {
            self.net.send_host(&Msg::Board(id), true);
            false
        }
    }

    /// Getting out (or the rocket is gone): the seat is free.
    pub(super) fn release_seat(&mut self, i: usize) {
        if !self.net.in_session() {
            return;
        }
        let t = self.time;
        let v = &mut self.flight.vessels[i];
        v.pilot = None;
        let (id, state) = (v.id, vessel_state(v, t));
        if self.net.is_host() {
            self.net
                .broadcast(&Msg::VesselState(Box::new(state)), true, None);
            self.net.broadcast(&Msg::Pilot(id, None), true, None);
        } else {
            self.net.send_host(&Msg::VesselState(Box::new(state)), true);
            self.net.send_host(&Msg::Pilot(id, None), true);
        }
    }

    // ---- Seeing the others ---------------------------------------------------------------

    /// The others' figures and what they hold.
    pub(super) fn avatar_draw_items(&mut self, objects: &mut Vec<DrawItem>, dt: f32) -> Result<()> {
        if !self.net.in_session() {
            return Ok(());
        }
        let camera = self.camera.position - self.planet.center;
        let ids: Vec<PlayerId> = self.net.players.keys().copied().collect();
        for id in ids {
            let visible_info = {
                let remote = self.net.players.get_mut(&id).expect("listed");
                let Some(pose) = remote.pose.clone() else {
                    continue;
                };
                // Moves on with what we last heard, and eases to it.
                let ahead = remote.heard.elapsed().as_secs_f64().min(0.25);
                let target = pose.feet + pose.velocity.as_dvec3() * ahead;
                remote.feet += (target - remote.feet) * (1.0 - (-dt as f64 * 14.0).exp());
                remote.look = remote.look.lerp(pose.look, approach(0.0, 1.0, 16.0, dt));
                let up = remote.feet.normalize_or(DVec3::Y);
                let speed = (pose.velocity.as_dvec3() - up * pose.velocity.as_dvec3().dot(up))
                    .length() as f32;
                remote.walk += (speed * dt * 3.2).min(1.5);
                let target_swing = if pose.on_ground {
                    (speed / 3.0).min(1.0)
                } else {
                    0.3
                };
                remote.swing = approach(remote.swing, target_swing, 10.0, dt);
                if pose.swings != remote.swings || pose.uses != remote.uses {
                    remote.swings = pose.swings;
                    remote.uses = pose.uses;
                    remote.arm = 1.0;
                }
                let working = if pose.mining || pose.eating {
                    0.5 + 0.5
                        * (remote.walk + remote.heard.elapsed().as_secs_f32())
                            .sin()
                            .abs()
                } else {
                    0.0
                };
                remote.arm = (remote.arm - dt * 4.0).max(working);
                let near = pose.body == self.active
                    && !pose.spectator
                    && pose.vessel.is_none()
                    && (remote.feet - camera).length() < AVATAR_VIEW;
                (near, pose.held, pose.time, pose.on_ground, pose.can_rail)
            };
            let (visible, item_kind, time, on_ground, can_rail) = visible_info;
            let visible = visible && !self.floating_elsewhen(time, on_ground, can_rail);
            if !visible {
                continue;
            }
            let remote = self.net.players.get_mut(&id).expect("listed");
            let up = remote.feet.normalize_or(DVec3::Y);
            let look = remote.look.as_dvec3();
            let forward = (look - up * look.dot(up)).normalize_or(up.any_orthonormal_vector());
            let look_pitch = look.dot(up).clamp(-1.0, 1.0).asin() as f32;
            let right = forward.cross(up);
            let cell = self.planet.cell_at(remote.feet + up);
            let block = cell.map_or(0, |c| self.streamer.light(c)) as u32;
            let light = models::Light { sky: 15, block };
            let mesh = avatar_model(
                &AvatarPose {
                    walk: remote.walk,
                    swing: remote.swing,
                    arm: remote.arm,
                    look_pitch,
                    holding: item_kind.is_some(),
                },
                (forward, up),
                light,
            );
            if let Some(handle) = self.renderer.create_mesh(&mesh)?
                && let Some(old) = remote.mesh.replace(handle)
            {
                self.renderer.destroy_mesh(old);
            }
            if let Some(mesh) = remote.mesh {
                objects.push(DrawItem {
                    mesh,
                    origin: self.planet.center + remote.feet,
                    rotation: Quat::IDENTITY,
                    radius: 2.0,
                    fade: Fade::Visible,
                });
            }
            match item_kind {
                Some(item) => {
                    let mesh = models::drop_model(item, (right, forward, up), light);
                    if let Some(handle) = self.renderer.create_mesh(&mesh)?
                        && let Some(old) = remote.item.replace(handle)
                    {
                        self.renderer.destroy_mesh(old);
                    }
                    if let Some(mesh) = remote.item {
                        let [x, y, z] = avatar::HAND;
                        objects.push(DrawItem {
                            mesh,
                            origin: self.planet.center
                                + remote.feet
                                + right * x
                                + forward * y
                                + up * z,
                            rotation: Quat::IDENTITY,
                            radius: 1.0,
                            fade: Fade::Visible,
                        });
                    }
                }
                None => {
                    if let Some(old) = remote.item.take() {
                        self.renderer.destroy_mesh(old);
                    }
                }
            }
        }
        Ok(())
    }

    /// Names over the heads of people close by.
    pub(super) fn draw_name_tags(&self, ui: &mut Ui) {
        if !self.net.in_session() {
            return;
        }
        let (w, h) = self.renderer.screen_size();
        let view = self.camera.view_projection(w / h);
        let camera = self.camera.position - self.planet.center;
        for remote in self.net.players.values() {
            let Some(pose) = &remote.pose else { continue };
            if pose.body != self.active || pose.spectator || pose.vessel.is_some() {
                continue;
            }
            let up = remote.feet.normalize_or(DVec3::Y);
            let head = remote.feet + up * 2.05 - camera;
            let distance = head.length();
            if distance > NAME_VIEW {
                continue;
            }
            let clip = view * head.as_vec3().extend(1.0);
            if clip.w <= 0.05 {
                continue;
            }
            let x = (clip.x / clip.w * 0.5 + 0.5) * w;
            let y = (clip.y / clip.w * 0.5 + 0.5) * h;
            let alpha = (((NAME_VIEW - distance) / 16.0).clamp(0.0, 1.0) * 255.0) as u8;
            let label = match self.gap_label(remote) {
                Some(gap) => format!("{}  {gap}", remote.name),
                None => remote.name.clone(),
            };
            let width = ui.text_width(&label);
            ui.text(
                (x - width * 0.5).round(),
                y.round(),
                &label,
                [235, 240, 245, alpha],
            );
        }
    }

    // ---- Console -------------------------------------------------------------------------

    /// `net host|join|leave|status`. Returns whether the command was one of these.
    pub(super) fn net_command(&mut self, command: &str, args: &[&str]) -> bool {
        if command != "net" {
            return false;
        }
        match args {
            ["host"] => self.start_hosting(None),
            ["host", bind] => match direct::resolve(bind) {
                Ok(bind) => self.start_hosting(Some(bind)),
                Err(e) => self.console.print(format!("{e:#}")),
            },
            ["join", address] => match steam::parse_connect(address) {
                Some(host) => self.join_steam(host),
                None => self.join_direct(address),
            },
            ["leave"] => self.leave_world(),
            ["sync", who] | ["follow", who] => match self.player_named(who) {
                Some(id) => self.time_sync(id, args[0] == "follow"),
                None => self.console.print("Nobody by that name"),
            },
            ["unfollow"] => self.unfollow(),
            ["status"] | [] => {
                let line = format!(
                    "{:?}, {} players, steam {}",
                    self.net.role,
                    self.net.player_count(),
                    self.net
                        .steam
                        .as_ref()
                        .map_or_else(|| "off".to_string(), |s| s.name()),
                );
                self.console.print(line);
                let names: Vec<String> = self
                    .net
                    .players
                    .iter()
                    .map(|(id, r)| {
                        let at = r
                            .pose
                            .as_ref()
                            .map_or(0.0, |p| (p.feet - self.pose().feet).length());
                        format!("{id}:{} at {at:.0} m", r.name)
                    })
                    .collect();
                self.console.print(names.join(", "));
                let replicas = self.survival.mobs.iter().filter(|m| m.remote.is_some());
                let line = format!(
                    "drops {}, mobs {} ({} simulated elsewhere), rockets {}, pilots {:?}, inventory items {}, warp x{}, time {:.1}, sky {}, keyed {}, watchers {:?}, follow {:?}",
                    self.survival.drops.len(),
                    self.survival.mobs.len(),
                    replicas.count(),
                    self.flight.vessels.len(),
                    self.flight
                        .vessels
                        .iter()
                        .map(|v| v.pilot)
                        .collect::<Vec<_>>(),
                    self.survival
                        .inventory
                        .slots
                        .iter()
                        .flatten()
                        .map(|s| s.count as u32)
                        .sum::<u32>(),
                    WARP_LEVELS[self.warp],
                    self.time,
                    self.net.sky,
                    self.net.keyed.iter().filter(|k| **k).count(),
                    self.net.listeners,
                    self.net.follow.map(|f| (f.player, f.once)),
                );
                self.console.print(self.together_line().unwrap_or_default());
                self.console.print(line);
                let hit = self
                    .target
                    .as_ref()
                    .map(|h| format!("{:?} at {:?}", h.block, h.cell));
                self.console.print(format!(
                    "looking at {hit:?}, weather started {}, tornadoes {}, cover here {:.2}",
                    self.weather.started(),
                    self.weather.snapshot.tornadoes.len(),
                    self.weather_fx.local.cover
                ));
            }
            // For tests: hit the nearest animal someone else simulates.
            ["hit"] => {
                let pose = self.pose();
                let nearest = self
                    .survival
                    .mobs
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.remote.is_some())
                    .min_by(|a, b| {
                        let d = |m: &Mob| (m.position - pose.feet).length();
                        d(a.1).total_cmp(&d(b.1))
                    })
                    .map(|(i, _)| i);
                if let Some(i) = nearest {
                    self.survival.attack = (0.0, 1.0);
                    self.survival.attack_mob(i, &pose, false);
                    self.console.print("Hit it");
                }
            }
            // For tests: stand on the ground some metres ahead.
            ["nudge", metres] => {
                let metres: f64 = metres.parse().unwrap_or(8.0);
                let pose = self.pose();
                let up = pose.up();
                let ahead = pose.look - up * pose.look.dot(up);
                let ahead = ahead.normalize_or(up.any_orthonormal_vector());
                let dir = (pose.feet + ahead * metres).normalize();
                let feet = Self::ground_position(&self.planet, &self.terrain, dir);
                let frame = self.active_frame(self.time);
                self.player.teleport(feet, &frame);
                self.reset_camera();
            }
            _ => self
                .console
                .print("Usage: net host [address:port] | join <address:port> | leave | status | sync <who> | follow <who> | unfollow"),
        }
        true
    }

    /// The nearest other player's head, for `lookat player`.
    pub(super) fn nearest_player(&self) -> Option<DVec3> {
        let eye = self.camera.position - self.planet.center;
        self.net
            .players
            .values()
            .filter_map(|r| r.pose.as_ref().filter(|p| p.body == self.active))
            .map(|p| p.feet + p.feet.normalize_or(DVec3::Y) * 1.2)
            .min_by(|a, b| (*a - eye).length().total_cmp(&(*b - eye).length()))
    }

    /// The menus' view of the session.
    pub(super) fn join_error_shown(&mut self) {
        if let (Some(notice), Some(menu)) = (self.net.notice.take(), &mut self.menu) {
            menu.prepare_join(&self.settings.player_name, &self.settings.last_address);
            menu.error = Some(notice);
            menu.show(Page::Multiplayer);
        }
    }
}

/// Nothing in a pose that could break the clock or the drawing.
fn pose_is_sound(pose: &PlayerPose) -> bool {
    finite(pose.feet)
        && pose.time.is_finite()
        && pose.rate.is_finite()
        && (0.0..=2.0e6).contains(&pose.rate)
}

fn vessel_state(v: &Vessel, time: f64) -> VesselState {
    VesselState {
        id: v.id,
        time,
        body: v.body,
        position: v.position,
        velocity: v.velocity,
        attitude: v.attitude,
        spin: v.spin.as_vec3(),
        throttle: v.throttle as f32,
        fuel: v.fuel as f32,
        temperature: v.temperature as f32,
        heating: v.heating as f32,
        landed: v.landed,
    }
}

fn valid_event(planet: &Planet, event: &WorldEvent) -> bool {
    match event {
        WorldEvent::Broke(cell) => valid_cell(planet, *cell),
        WorldEvent::Placed { cell, support, .. } => {
            valid_cell(planet, *cell) && support.is_none_or(|s| valid_cell(planet, s))
        }
        WorldEvent::Chest { cell, .. } | WorldEvent::Furnace { cell, .. } => {
            valid_cell(planet, *cell)
        }
        WorldEvent::DropNew(drop) => finite(drop.position) && finite(drop.velocity),
        WorldEvent::DropGone(_) => true,
    }
}

fn unpack_world(download: Downloading) -> Result<JoinedWorld> {
    let raw = protocol::decompress(&download.data, MAX_WORLD)
        .ok_or_else(|| anyhow!("the world arrived damaged"))?;
    let blob: WorldBlob = postcard::from_bytes(&raw).context("the world arrived damaged")?;
    let saved: SavedGame = serde_json::from_slice(&blob.state).context("reading the world")?;
    if blob.weather.len() != 6 || blob.weather.iter().any(|f| f.len() != weather::PACKED_FACE) {
        return Err(anyhow!("the weather arrived damaged"));
    }
    Ok(JoinedWorld {
        seed: download.seed,
        mode: download.mode,
        saved,
        home: blob.home,
        moon: blob.moon,
        weather: blob.weather,
        record: blob.record,
        pilots: blob.pilots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_up() {
        assert_eq!(clean_name("  hello  "), "hello");
        assert_eq!(clean_name("\u{1f680}"), "player");
        assert_eq!(clean_name(""), "player");
        assert_eq!(clean_name(&"x".repeat(99)).len(), MAX_NAME);
    }

    #[test]
    fn cells_off_the_grid_are_refused() {
        let planet = Planet::new(DVec3::ZERO, 64, 128, 1);
        let good = planet
            .cell_at(DVec3::Y * (planet.sea_radius + 1.0))
            .unwrap();
        assert!(valid_cell(&planet, good));
        assert!(!valid_cell(&planet, BlockCell { k: -5, ..good }));
        assert!(!valid_cell(
            &planet,
            BlockCell {
                k: i32::MAX,
                ..good
            }
        ));
        assert!(!valid_cell(
            &planet,
            BlockCell {
                u: i32::MAX,
                ..good
            }
        ));
        assert!(!valid_cell(
            &planet,
            BlockCell {
                face: crate::world::planet::Face(99),
                ..good
            }
        ));
        assert!(!valid_event(
            &planet,
            &WorldEvent::Broke(BlockCell { v: -1, ..good })
        ));
    }

    #[test]
    fn a_world_too_damaged_to_read_is_refused() {
        let download = Downloading {
            seed: 1,
            mode: GameMode::Survival,
            data: vec![1, 2, 3, 4, 5],
        };
        assert!(unpack_world(download).is_err());
    }

    #[test]
    fn datagram_budget_holds_a_full_mob_packet() {
        use crate::{
            mobs::MobKind,
            net::protocol::{MAX_UNRELIABLE, MobState},
        };
        let list = vec![
            MobState {
                id: u32::MAX,
                kind: MobKind::Sheep,
                coat: 3,
                position: Vec3::splat(44_000.0),
                heading: [-127, 127, 0],
                graze: 255,
                hurt: true,
            };
            MOBS_PER_PACKET
        ];
        let msg = Msg::Mobs {
            owner: 7,
            body: Body::Planet,
            list,
        };
        assert!(protocol::encode(&msg).len() <= MAX_UNRELIABLE);
    }
}
