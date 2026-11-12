//! What players say to each other: every message that crosses the network.
//!
//! The host owns the world. Clients tell it what they did and it passes that on to the
//! others (see the top of `app/multiplayer.rs` for who decides what). Messages are postcard
//! (compact, not self-describing), so both ends must be the same build of the protocol:
//! `PROTOCOL` is checked when joining.

use glam::{DQuat, DVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::{
    audio::Sound,
    factory::{Machine, NetBelts},
    flight::{
        craft::{At, Part},
        vessel::{Landed, Vessel},
    },
    mobs::{Mob, MobKind},
    player::GameMode,
    render::models::ModelState,
    space::bodies::Body,
    survival::{
        furnace::Furnace,
        inventory::Slot,
        items::{Item, ItemStack},
    },
    weather::sim::System,
    world::{block::Block, chunk::BlockCell},
};

/// Bumped whenever a message changes.
pub const PROTOCOL: u32 = 14;
/// The host is always player 0.
pub const HOST: PlayerId = 0;
/// Most people in one game, the host included.
pub const MAX_PLAYERS: usize = 8;
/// A datagram this size gets through without being split up on the way.
pub const MAX_UNRELIABLE: usize = 1100;

pub type PlayerId = u32;

/// Where someone is and what they are doing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerPose {
    pub id: PlayerId,
    /// The world they are on; `feet` is in its body frame, planet-relative.
    pub body: Body,
    pub feet: DVec3,
    pub velocity: Vec3,
    /// Where they look (unit, same frame).
    pub look: Vec3,
    pub held: Option<Item>,
    pub swings: u32,
    pub uses: u32,
    pub mining: bool,
    pub eating: bool,
    pub on_ground: bool,
    pub spectator: bool,
    /// Just been hit (the figure flashes).
    pub hurt: bool,
    /// In survival: only then can they hit or be hit by other players.
    pub survival: bool,
    /// The rocket they are in (their model isn't drawn then).
    pub vessel: Option<u32>,
    /// On the ground or on an orbit (not somewhere only physics can say).
    pub can_rail: bool,
    /// Their own clock (everyone warps on their own), and how fast it is running right now.
    pub time: f64,
    pub rate: f32,
    /// On a vehicle: where, in its own grid.
    pub aboard: Option<AboardPose>,
}

/// Someone standing or sitting on a vehicle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AboardPose {
    pub vessel: u32,
    /// Grid coordinates.
    pub feet: Vec3,
    pub seat: bool,
}

/// A part put on or taken off a vehicle.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CraftChange {
    Put(Part),
    Take(At),
    /// Knocked off in a crash: taken off, with a bang where it was.
    Crash(At),
    /// The door with a cell at `at` opened or closed.
    Door {
        at: At,
        open: bool,
    },
}

/// A change to one world, passed on to everyone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorldEvent {
    Broke(BlockCell),
    Placed {
        cell: BlockCell,
        block: Block,
        state: ModelState,
        support: Option<BlockCell>,
    },
    Chest {
        cell: BlockCell,
        slots: Vec<Slot>,
    },
    Furnace {
        cell: BlockCell,
        furnace: Furnace,
    },
    /// A generator or electric furnace (its slots and how it is doing).
    Machine {
        cell: BlockCell,
        machine: Machine,
    },
    /// A cable run (`on`) or taken down between two poles or machines.
    Cable {
        a: BlockCell,
        b: BlockCell,
        on: bool,
    },
    /// A conveyor turned to face another way.
    Turned {
        cell: BlockCell,
        facing: u8,
    },
    /// The door with a cell at `cell` opened or closed (all its cells).
    Door {
        cell: BlockCell,
        open: bool,
    },
    DropNew(NetDrop),
    DropGone(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetDrop {
    pub id: u32,
    pub stack: ItemStack,
    pub position: DVec3,
    pub velocity: DVec3,
    pub pickup_delay: f32,
}

/// An animal as seen by those who don't simulate it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MobState {
    pub id: u32,
    pub kind: MobKind,
    pub coat: u8,
    pub position: Vec3,
    pub heading: [i8; 3],
    /// How far the head is down (0..255).
    pub graze: u8,
    pub hurt: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VesselState {
    pub id: u32,
    /// The clock of whoever sent this: what is said here was true then.
    pub time: f64,
    pub body: Body,
    pub position: DVec3,
    pub velocity: DVec3,
    pub attitude: DQuat,
    pub spin: Vec3,
    pub throttle: f32,
    pub fuel: f32,
    pub temperature: f32,
    pub heating: f32,
    pub landed: Option<Landed>,
    /// For the eye: wheels on the ground, the front wheels' angle (rad), each surface's flap
    /// deflection (x 127).
    pub touching: bool,
    pub steer: f32,
    pub flaps: Vec<i8>,
    /// Electricity in its batteries (J), its drills running, how its gimbals steer (x 127).
    pub charge: f32,
    pub drilling: bool,
    pub gimbal: [i8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetTornado {
    pub dir: Vec3,
    pub velocity: Vec3,
    pub radius: f32,
    pub strength: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetVolcano {
    pub dir: Vec3,
    pub summit: f32,
    pub vent: f32,
    pub started: f64,
    pub activity: f32,
}

/// Everything a new player needs, sent once in pieces (compressed).
#[derive(Serialize, Deserialize)]
pub struct WorldBlob {
    /// The host's `SavedGame`, as JSON.
    pub state: Vec<u8>,
    pub home: Vec<u8>,
    pub moon: Vec<u8>,
    /// The weather maps, one entry per cube face.
    pub weather: Vec<Vec<u8>>,
    /// What this player had when they last left (a `PlayerRecord`, as JSON).
    pub record: Option<Vec<u8>>,
    /// Rockets someone is flying right now.
    pub pilots: Vec<(u32, PlayerId)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Msg {
    // ---- Joining ---------------------------------------------------------------------------
    Hello {
        protocol: u32,
        name: String,
        /// Who this is from one session to the next (a Steam id, or a random number).
        stable: String,
    },
    Refused(String),
    Welcome {
        you: PlayerId,
        seed: u64,
        mode: GameMode,
        /// Everyone here: who they are, their name, whose weather they are watching.
        players: Vec<(PlayerId, String, PlayerId)>,
    },
    /// The next piece of the compressed `WorldBlob`.
    WorldPart(Vec<u8>),
    WorldEnd,
    /// The client has built the world: it can take live traffic now.
    Ready,
    PlayerJoined(PlayerId, String),
    PlayerLeft(PlayerId),
    /// Leaving on purpose.
    Bye,

    // ---- Players ---------------------------------------------------------------------------
    Pose(PlayerPose),
    /// A client's own state (JSON `PlayerRecord`), for the host to keep with the world.
    PlayerSave(Vec<u8>),
    Sounds(Body, Vec<(Sound, Vec3)>),
    /// `by` has struck `to`, standing at `from` (`to` decides what that does).
    PlayerHit {
        to: PlayerId,
        by: PlayerId,
        body: Body,
        damage: f32,
        from: DVec3,
    },

    // ---- The world ---------------------------------------------------------------------------
    World(Body, Vec<WorldEvent>),
    /// A client wants this drop (the host decides who gets it).
    Pickup(Body, u32),
    /// You picked this up.
    Give(ItemStack),
    /// (From the host.) What is on the conveyors near you, by group (see `factory::NetBelts`).
    Belts(Body, Vec<NetBelts>),

    // ---- Animals -----------------------------------------------------------------------------
    /// The animals one player simulates.
    Mobs {
        owner: PlayerId,
        body: Body,
        list: Vec<MobState>,
    },
    MobHit {
        owner: PlayerId,
        id: u32,
        damage: f32,
        from: DVec3,
        creative: bool,
    },
    MobGone(u32),
    /// You are nearer to this animal than its owner: it is yours now.
    MobHandoff {
        to: PlayerId,
        mob: Box<Mob>,
    },

    // ---- Rockets -----------------------------------------------------------------------------
    VesselNew(Box<Vessel>),
    VesselGone {
        id: u32,
        /// How it was destroyed; none if it was simply picked up.
        reason: Option<String>,
    },
    VesselState(Box<VesselState>),
    /// A client asks to get in.
    Board(u32),
    /// Who is flying this rocket now.
    Pilot(u32, Option<PlayerId>),
    /// Someone has changed a vehicle.
    CraftEdit {
        vessel: u32,
        change: CraftChange,
    },

    // ---- Teams -------------------------------------------------------------------------------
    /// `who` is now on the team of `team` (the id of whoever started it), or on none.
    Team {
        who: PlayerId,
        team: Option<PlayerId>,
    },
    /// `from` has changed the warp, for the whole team.
    TeamWarp {
        from: PlayerId,
        level: u8,
    },

    // ---- Weather -----------------------------------------------------------------------------
    /// The cyclones behind that weather.
    WeatherSystems {
        from: PlayerId,
        systems: Vec<System>,
    },
    /// Now watching `watches`'s weather (their own id: running their own).
    Sky {
        who: PlayerId,
        watches: PlayerId,
    },
    /// A whole cube face, for someone who has just started watching.
    WeatherKey {
        from: PlayerId,
        to: PlayerId,
        face: u8,
        data: Vec<u8>,
    },
    /// One cube face of the weather, deflated difference from the previous one.
    WeatherFace {
        from: PlayerId,
        face: u8,
        data: Vec<u8>,
    },
    WeatherLive {
        from: PlayerId,
        time: f64,
        tornadoes: Vec<NetTornado>,
        volcanoes: Vec<NetVolcano>,
    },
}

pub fn encode(msg: &Msg) -> Vec<u8> {
    // Plain data into a growing buffer: serializing can't fail.
    postcard::to_stdvec(msg).expect("messages serialize")
}

/// None if it doesn't parse (a damaged or hostile packet is simply ignored).
pub fn decode(data: &[u8]) -> Option<Msg> {
    postcard::from_bytes(data).ok()
}

/// Deflates (fast: this happens on the main thread).
pub fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 2)
}

/// Inflates at most `limit` bytes; None if it is broken or bigger than that.
pub fn decompress(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::planet::Face;

    #[test]
    fn messages_survive_the_trip() {
        let cell = BlockCell {
            face: Face(2),
            u: 12,
            v: -3,
            k: 44_000,
        };
        let msg = Msg::World(
            Body::Moon,
            vec![
                WorldEvent::Broke(cell),
                WorldEvent::Placed {
                    cell,
                    block: Block::Torch,
                    state: ModelState::default(),
                    support: Some(cell),
                },
                WorldEvent::DropGone(7),
            ],
        );
        let Some(Msg::World(body, events)) = decode(&encode(&msg)) else {
            panic!("didn't parse");
        };
        assert_eq!(body, Body::Moon);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], WorldEvent::Broke(c) if c == cell));
        assert!(matches!(
            events[1],
            WorldEvent::Placed {
                block: Block::Torch,
                support: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_pose_fits_in_one_datagram_and_garbage_is_ignored() {
        let pose = PlayerPose {
            id: 3,
            body: Body::Planet,
            feet: DVec3::splat(43_000.0),
            velocity: Vec3::ONE,
            look: Vec3::Y,
            held: Some(Item::Block(Block::Stone)),
            swings: 4_000_000,
            uses: 9,
            mining: true,
            eating: false,
            on_ground: true,
            spectator: false,
            hurt: false,
            survival: true,
            vessel: Some(u32::MAX),
            can_rail: true,
            time: 1.0e9,
            rate: 1_000_000.0,
            aboard: Some(AboardPose {
                vessel: u32::MAX,
                feet: Vec3::splat(40.0),
                seat: true,
            }),
        };
        assert!(encode(&Msg::Pose(pose)).len() < 160);
        assert!(decode(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn a_full_group_of_conveyors_fits_in_one_datagram() {
        use crate::{
            factory::BELT_ITEMS,
            survival::items::{Tier, ToolKind},
        };
        // 4 x 4 x 2 conveyors, each carrying the biggest items there are.
        let item = Item::Tool(ToolKind::Pickaxe, Tier::Diamond);
        let group = NetBelts {
            key: (5, -40_000, 40_000, 44_000),
            belts: (0..32)
                .map(|i| (i, vec![(item, 255); BELT_ITEMS]))
                .collect(),
        };
        let msg = Msg::Belts(Body::Planet, vec![group]);
        assert!(
            encode(&msg).len() <= MAX_UNRELIABLE,
            "{}",
            encode(&msg).len()
        );
    }

    #[test]
    fn compression_round_trips_and_respects_its_limit() {
        let data = vec![7u8; 100_000];
        let small = compress(&data);
        assert!(small.len() < 2_000);
        assert_eq!(decompress(&small, 100_000).as_deref(), Some(&data[..]));
        assert!(decompress(&small, 1_000).is_none(), "over the limit");
    }
}
