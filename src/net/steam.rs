//! Playing through Steam: peer-to-peer messages (relayed by Steam when two machines can't
//! reach each other), friends and invites.
//!
//! The game runs as Spacewar (app id 480, Valve's test app, which every Steam account has),
//! so no store page or app id of our own is needed. Steam not running is normal: the game
//! then just has no Steam features, and direct connections still work.
//!
//! A host advertises itself with a *connect string* (`+astro_host <steam id>`) in its rich
//! presence, which is what makes "join game" work on the Steam friends list. Invites carry
//! the same string. Only friends can join a Steam game.

use std::sync::{Arc, Mutex};

use steamworks::{
    Client, FriendFlags, FriendState, GameRichPresenceJoinRequested, SteamId,
    networking_types::{NetworkingIdentity, SendFlags},
};

use super::{Event, PeerId};
use crate::util::FastMap;

/// Spacewar.
const APP_ID: u32 = 480;
const CONNECT_PREFIX: &str = "+astro_host ";
/// Messages taken from Steam per call.
const BATCH: usize = 256;

/// A friend, for the menus.
#[derive(Clone, Debug)]
pub struct Friend {
    pub id: u64,
    pub name: String,
    /// Someone hosting Astro Block right now: they can be joined.
    pub hosting: bool,
}

/// The Steam id in a connect string, if it is one of ours.
pub fn parse_connect(text: &str) -> Option<u64> {
    text.trim()
        .strip_prefix(CONNECT_PREFIX.trim())?
        .trim()
        .parse()
        .ok()
}

/// Which Steam users may open a session with us.
#[derive(Clone, Copy, PartialEq)]
enum Accepting {
    Nobody,
    /// Hosting: friends.
    Friends,
    /// Joined this one host.
    Only(u64),
}

pub struct Steam {
    client: Client,
    me: u64,
    accepting: Arc<Mutex<Accepting>>,
    failed: Arc<Mutex<Vec<u64>>>,
    /// Invites and "join game" clicks, as the host's Steam id.
    joins: Arc<Mutex<Vec<u64>>>,
    peers: FastMap<u64, PeerId>,
    users: FastMap<PeerId, u64>,
    next_peer: u32,
    /// The host we joined, when a client.
    host: Option<u64>,
}

impl Steam {
    /// Connects to the running Steam client, if there is one.
    pub fn start() -> Option<Self> {
        let client = match Client::init_app(APP_ID) {
            Ok(client) => client,
            Err(e) => {
                log::info!("steam isn't available ({e:?}): direct connections only");
                return None;
            }
        };
        let me = client.user().steam_id().raw();
        let accepting = Arc::new(Mutex::new(Accepting::Nobody));
        let failed: Arc<Mutex<Vec<u64>>> = Arc::default();
        let joins: Arc<Mutex<Vec<u64>>> = Arc::default();

        let messages = client.networking_messages();
        let (friends, allowed) = (client.clone(), accepting.clone());
        messages.session_request_callback(move |request| {
            let Some(id) = request.remote().steam_id() else {
                request.reject();
                return;
            };
            let accept = match *allowed.lock().expect("accepting") {
                Accepting::Nobody => false,
                Accepting::Friends => friends
                    .friends()
                    .get_friend(id)
                    .has_friend(FriendFlags::IMMEDIATE),
                Accepting::Only(host) => id.raw() == host,
            };
            if accept {
                request.accept();
            } else {
                request.reject();
            }
        });
        let lost = failed.clone();
        messages.session_failed_callback(move |info| {
            if let Some(id) = info.identity_remote().and_then(|i| i.steam_id()) {
                lost.lock().expect("failed").push(id.raw());
            }
        });
        let requested = joins.clone();
        // (Handles left to live as long as the game does.)
        std::mem::forget(
            client.register_callback(move |join: GameRichPresenceJoinRequested| {
                if let Some(host) = parse_connect(&join.connect) {
                    requested.lock().expect("joins").push(host);
                }
            }),
        );
        client.networking_utils().init_relay_network_access();
        log::info!("steam: signed in as {}", client.friends().name());
        Some(Self {
            client,
            me,
            accepting,
            failed,
            joins,
            peers: FastMap::default(),
            users: FastMap::default(),
            next_peer: 0,
            host: None,
        })
    }

    pub fn name(&self) -> String {
        self.client.friends().name()
    }

    pub fn id(&self) -> u64 {
        self.me
    }

    /// Lets friends in, and says so on the friends list.
    pub fn start_hosting(&mut self) {
        *self.accepting.lock().expect("accepting") = Accepting::Friends;
        let connect = self.connect_string();
        let friends = self.client.friends();
        friends.set_rich_presence("connect", Some(&connect));
        friends.set_rich_presence("status", Some("hosting Astro Block"));
    }

    /// Ends whatever session this was.
    pub fn stop(&mut self) {
        *self.accepting.lock().expect("accepting") = Accepting::Nobody;
        self.client.friends().clear_rich_presence();
        self.peers.clear();
        self.users.clear();
        self.host = None;
    }

    fn connect_string(&self) -> String {
        format!("{CONNECT_PREFIX}{}", self.me)
    }

    /// Starts talking to a host. (Steam sets the session up when the first message goes out.)
    pub fn join(&mut self, host: u64) {
        *self.accepting.lock().expect("accepting") = Accepting::Only(host);
        self.host = Some(host);
        self.peers.clear();
        self.users.clear();
        self.peers.insert(host, PeerId::HOST);
        self.users.insert(PeerId::HOST, host);
    }

    /// Runs Steam's callbacks (every frame, whatever is going on).
    pub fn pump(&mut self) {
        self.client.run_callbacks();
    }

    /// Collects what has arrived.
    pub fn poll(&mut self, out: &mut Vec<Event>) {
        for id in std::mem::take(&mut *self.failed.lock().expect("failed")) {
            if let Some(peer) = self.peers.remove(&id) {
                self.users.remove(&peer);
                out.push(Event::Lost(peer, "steam lost the connection".to_string()));
            }
        }
        let messages = self.client.networking_messages();
        loop {
            let batch = messages.receive_messages_on_channel(0, BATCH);
            let count = batch.len();
            for message in batch {
                let Some(id) = message.identity_peer().steam_id() else {
                    continue;
                };
                let id = id.raw();
                let peer = match self.peers.get(&id) {
                    Some(peer) => *peer,
                    // (A client only hears from its host: anything else isn't for us.)
                    None if self.host.is_some() => continue,
                    None => {
                        self.next_peer += 1;
                        let peer = PeerId::steam(self.next_peer);
                        self.peers.insert(id, peer);
                        self.users.insert(peer, id);
                        out.push(Event::Connected(peer));
                        peer
                    }
                };
                out.push(Event::Received(peer, message.data().to_vec()));
            }
            if count < BATCH {
                break;
            }
        }
    }

    /// False if a reliable message has to wait.
    pub fn send(&mut self, peer: PeerId, data: &[u8], reliable: bool) -> bool {
        let Some(id) = self.users.get(&peer) else {
            return true;
        };
        let flags = if reliable {
            SendFlags::RELIABLE_NO_NAGLE
        } else {
            SendFlags::UNRELIABLE_NO_DELAY
        };
        let identity = NetworkingIdentity::new_steam_id(SteamId::from_raw(*id));
        let sent = self
            .client
            .networking_messages()
            .send_message_to_user(identity, flags, data, 0);
        sent.is_ok() || !reliable
    }

    pub fn close(&mut self, peer: PeerId) {
        if let Some(id) = self.users.remove(&peer) {
            self.peers.remove(&id);
        }
    }

    /// Someone clicked an invite or "join game" since the last call.
    pub fn take_join_request(&mut self) -> Option<u64> {
        self.joins.lock().expect("joins").pop()
    }

    /// Online friends (hosting ones first).
    pub fn friends(&self) -> Vec<Friend> {
        let friends = self.client.friends();
        let mut list: Vec<Friend> = friends
            .get_friends(FriendFlags::IMMEDIATE)
            .into_iter()
            .filter(|f| !matches!(f.state(), FriendState::Offline))
            .map(|f| Friend {
                id: f.id().raw(),
                name: f.name(),
                hosting: f
                    .rich_presence("connect")
                    .is_some_and(|c| parse_connect(&c).is_some()),
            })
            .collect();
        list.sort_by_key(|f| (!f.hosting, f.name.to_lowercase()));
        list
    }

    pub fn invite(&self, friend: u64) {
        self.client
            .friends()
            .get_friend(SteamId::from_raw(friend))
            .invite_user_to_game(&self.connect_string());
    }

    /// Steam's own invite window (only works if its overlay is running).
    pub fn invite_overlay(&self) {
        self.client
            .friends()
            .activate_invite_dialog_connect_string(&self.connect_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_strings_name_a_host() {
        assert_eq!(
            parse_connect("+astro_host 76561198000000000"),
            Some(76561198000000000)
        );
        assert_eq!(parse_connect("  +astro_host   42 "), Some(42));
        assert_eq!(parse_connect("+connect 1.2.3.4"), None);
        assert_eq!(parse_connect("+astro_host nonsense"), None);
    }
}
