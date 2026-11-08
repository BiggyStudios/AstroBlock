//! Moving bytes between players, over Steam or a plain IP connection. What the bytes mean is
//! `protocol`; what the game does with them is `app::multiplayer`.
//!
//! A host can accept players over both at once; a client has exactly one connection.

pub mod direct;
pub mod protocol;
pub mod steam;

use direct::{DirectClient, DirectHost};
use steam::Steam;

/// One connection, as the transports number them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PeerId(u32);

impl PeerId {
    /// A client's only peer.
    pub const HOST: PeerId = PeerId(0);

    fn direct(n: u32) -> Self {
        Self(n << 1)
    }

    fn steam(n: u32) -> Self {
        Self(n << 1 | 1)
    }

    fn is_steam(self) -> bool {
        self.0 & 1 == 1
    }
}

#[derive(Debug)]
pub enum Event {
    Connected(PeerId),
    Received(PeerId, Vec<u8>),
    /// The connection is gone, and why.
    Lost(PeerId, String),
}

/// The open connections of one game.
#[derive(Default)]
pub enum Link {
    #[default]
    None,
    Host {
        direct: Option<DirectHost>,
        steam: bool,
    },
    Direct(DirectClient),
    Steam,
}

impl Link {
    pub fn poll(&mut self, steam: &mut Option<Steam>, out: &mut Vec<Event>) {
        match self {
            Link::None => {}
            Link::Host { direct, steam: on } => {
                if let Some(direct) = direct {
                    direct.poll(out);
                }
                if *on && let Some(steam) = steam {
                    steam.poll(out);
                }
            }
            Link::Direct(client) => client.poll(out),
            Link::Steam => {
                if let Some(steam) = steam {
                    steam.poll(out);
                }
            }
        }
    }

    /// Sends to one peer. False: a reliable message has to wait (try it again later).
    pub fn send(
        &mut self,
        steam: &mut Option<Steam>,
        peer: PeerId,
        data: &[u8],
        reliable: bool,
    ) -> bool {
        match self {
            Link::None => true,
            Link::Host { direct, .. } => {
                if peer.is_steam() {
                    steam
                        .as_mut()
                        .is_none_or(|steam| steam.send(peer, data, reliable))
                } else {
                    direct
                        .as_mut()
                        .is_none_or(|direct| direct.send(peer, data, reliable))
                }
            }
            Link::Direct(client) => client.send(data, reliable),
            Link::Steam => steam
                .as_mut()
                .is_none_or(|steam| steam.send(peer, data, reliable)),
        }
    }

    pub fn close(&mut self, steam: &mut Option<Steam>, peer: PeerId) {
        if let Link::Host { direct, .. } = self {
            if peer.is_steam() {
                if let Some(steam) = steam {
                    steam.close(peer);
                }
            } else if let Some(direct) = direct {
                direct.close(peer);
            }
        }
    }
}
