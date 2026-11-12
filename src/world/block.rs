use super::textures::Tex;

#[repr(u8)]
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Block {
    #[default]
    Air,
    Water,
    Stone,
    Deepstone,
    /// Rocky mantle (peridotite), below the crust.
    Mantle,
    /// Molten iron outer core.
    OuterCore,
    /// Solid iron-nickel inner core.
    InnerCore,
    Dirt,
    Grass,
    SnowyGrass,
    Snow,
    Ice,
    Sand,
    RedSand,
    Sandstone,
    Gravel,
    TerracottaOrange,
    TerracottaRed,
    TerracottaYellow,
    TerracottaWhite,
    TerracottaBrown,
    OakLog,
    OakLeaves,
    SpruceLog,
    SpruceLeaves,
    JungleLeaves,
    Cactus,
    CoalOre,
    IronOre,
    CopperOre,
    GoldOre,
    DiamondOre,
    BauxiteOre,
    UraniumOre,
    /// Heavy-mineral beach sand rich in ilmenite - the titanium source.
    IlmeniteSand,
    OakPlanks,
    SprucePlanks,
    Cobblestone,
    /// Drawn as a 3D model (see `models`), not a cube.
    CraftingTable,
    /// Drawn as a 3D model; its contents live in `Survival::furnaces`.
    Furnace,
    /// See-through (a cutout block).
    Glass,
    /// A model; gives off light.
    Torch,
    /// A model; its contents live in `Survival::chests`.
    Chest,
    WhiteWool,
    /// Loose grey dust over the moon's rock.
    Regolith,
    /// The moon's pale highland rock (anorthosite).
    MoonRock,
    /// The dark rock of the moon's maria and of volcanoes.
    Basalt,
    /// Volcanic ash, the loose grey soil on a volcano's slopes.
    Ash,
    /// Molten rock in a volcano's crater: glows, and burns whoever stands on it.
    Lava,
    // Vehicles are built from these (see `flight::craft`). Except for hull, they only exist
    // as parts of a vehicle.
    /// Starts a vehicle: everything else is built onto it.
    VehicleCore,
    /// Light metal plating, for building vehicles (and anything else).
    Hull,
    Seat,
    FuelTank,
    /// Pulls through the air: cheap on fuel, slow.
    Propeller,
    /// Needs air, faster than a propeller.
    JetEngine,
    /// Works anywhere, drinks fuel.
    RocketEngine,
    /// A small engine that sips fuel, weak in air.
    OrbitalEngine,
    /// A flat lifting surface.
    Wing,
    /// A wing that moves: pitch, yaw and roll in the air.
    Flap,
    /// Reaction wheels: turns a vehicle without air.
    Gyro,
    /// A driven wheel, for rovers and cars.
    Wheel,
    /// A free-rolling wheel on a strut, for landing.
    LandingGear,
    // Factories (see `factory`). Models; what they hold lives in `Factory`.
    /// A belt that carries items the way its arrow points: it takes them out of whatever is
    /// behind it and puts them into whatever is in front.
    Conveyor,
    /// Carries power: cables run from pole to pole and to machines.
    PowerPole,
    /// Burns fuel for power.
    CoalGenerator,
    /// Smelts on power, twice as fast as a furnace.
    ElectricFurnace,
    /// Dark rock soaked with crude oil, deep under dry plains, deserts and the cold flats. An
    /// oil pump over it brings the oil up.
    OilShale,
    /// Digs up whatever is under it, for ever, on power (see `factory`).
    Miner,
    /// Pumps crude oil out of oil shale below it.
    OilPump,
    /// Crude oil into fuel, plastic and rubber.
    Refinery,
    /// A very hot electric furnace: silicon from sand, steel from iron.
    ArcFurnace,
    /// Puts parts together into what is picked on its screen.
    Assembler,
    /// Makes wafers and chips out of silicon.
    ChipFab,
    /// Stores power when there is more than is used, and gives it back when there isn't enough.
    Accumulator,
    // More vehicle parts (only on a vehicle, like the ones above).
    /// A driven wheel half as big again: climbs more, grips more.
    LargeWheel,
    /// A driven wheel more than twice as big, for heavy rovers.
    GiantWheel,
    /// A rocket engine on a gimbal: its thrust steers the vehicle.
    VectorEngine,
    /// A rocket engine that works as well in air as in vacuum, and sips fuel.
    Aerospike,
    /// Runs on electricity, not fuel: very little thrust for its weight, but it never runs dry
    /// while there is power. Only in vacuum.
    IonEngine,
    /// Bores through whatever is in front of it, on power.
    VehicleDrill,
    /// Makes power from sunlight, in a factory or on a vehicle.
    SolarPanel,
    // Doors: each block is one cell of a door (a door is two cells tall, a big door three by
    // three); the open ones are the same doors standing open (walked through).
    /// Swings open on a hinge.
    Door,
    DoorOpen,
    /// Slides aside.
    SlidingDoor,
    SlidingDoorOpen,
    /// Three by three, rolls up on a motor: needs power.
    BigDoor,
    BigDoorOpen,
    // More ores (the chemistry and nuclear chains, see `factory::recipes`).
    /// Yellow crystals round volcanoes (and deep down anywhere): sulfuric acid.
    SulfurOre,
    /// Galena: lead, for shielding and RTGs.
    LeadOre,
    /// Purple crystals in mountains: hydrofluoric acid.
    FluoriteOre,
    /// A white nitrate crust just under deserts: nitric acid.
    SaltpeterOre,
    // Building blocks.
    /// Poured stone, out of a blender.
    Concrete,
    /// A block of steel plate.
    SteelPlating,
    /// Nuclear waste set in glass, in a steel can: safe to keep (it glows a little).
    VitrifiedWaste,
    // Fluids and power (factory parts, models).
    /// Carries fluids between machines and tanks: every pipe touching another joins it.
    Pipe,
    /// Holds a lot of one fluid.
    StorageTank,
    /// Pumps water from the water next to it.
    WaterPump,
    /// Splits water into hydrogen and oxygen; aluminium out of alumina.
    Electrolyzer,
    /// Spins uranium hexafluoride apart: enriched, and depleted uranium.
    Centrifuge,
    /// Makes fuel rods.
    FuelFabricator,
    /// Burns fuel to boil water into steam.
    Boiler,
    /// Boils water on the heat underground (lava near it, or deep down).
    GeothermalWell,
    /// Makes power from the wind: more on high ground and in storms.
    WindTurbine,
    /// A radioisotope generator: a little power for ever, in a factory or on a vehicle.
    Rtg,
    // Big machines: each fills a block of cells round the one it was put on (`structure`).
    /// Five by five: mixes acids and leaches ore.
    Blender,
    /// Three by three: steam into power.
    SteamTurbine,
    /// Five by five: fuel rods and water into steam. Melts down if it runs dry.
    NuclearReactor,
    /// Three by three: spent fuel cools under water.
    CoolingPool,
    /// Three by three: takes spent fuel apart into uranium, plutonium and waste.
    ReprocessingPlant,
    /// Seven by seven: a ring that makes plutonium and antimatter, on a lot of power.
    ParticleAccelerator,
    /// A vehicle engine: antimatter heats its propellant. Very fast exhaust.
    AntimatterDrive,
}

/// What kind of door a door block belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorKind {
    Hinged,
    Sliding,
    Big,
}

impl DoorKind {
    /// Cells across and up.
    pub fn size(self) -> (i32, i32) {
        match self {
            DoorKind::Big => (3, 3),
            _ => (1, 2),
        }
    }

    pub fn closed(self) -> Block {
        match self {
            DoorKind::Hinged => Block::Door,
            DoorKind::Sliding => Block::SlidingDoor,
            DoorKind::Big => Block::BigDoor,
        }
    }

    pub fn opened(self) -> Block {
        match self {
            DoorKind::Hinged => Block::DoorOpen,
            DoorKind::Sliding => Block::SlidingDoorOpen,
            DoorKind::Big => Block::BigDoorOpen,
        }
    }
}

/// What a block sounds like (footsteps, breaking, placing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Material {
    Grass,
    Dirt,
    Stone,
    Wood,
    Sand,
    Gravel,
    Snow,
    Glass,
    Wool,
    Leaves,
}

/// Which biome colour a block face is tinted with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    None,
    Grass,
    Foliage,
    Spruce,
    Water,
}

impl Block {
    /// Every block, in declaration order.
    pub const ALL: [Block; 111] = {
        use Block::*;
        [
            Air,
            Water,
            Stone,
            Deepstone,
            Mantle,
            OuterCore,
            InnerCore,
            Dirt,
            Grass,
            SnowyGrass,
            Snow,
            Ice,
            Sand,
            RedSand,
            Sandstone,
            Gravel,
            TerracottaOrange,
            TerracottaRed,
            TerracottaYellow,
            TerracottaWhite,
            TerracottaBrown,
            OakLog,
            OakLeaves,
            SpruceLog,
            SpruceLeaves,
            JungleLeaves,
            Cactus,
            CoalOre,
            IronOre,
            CopperOre,
            GoldOre,
            DiamondOre,
            BauxiteOre,
            UraniumOre,
            IlmeniteSand,
            OakPlanks,
            SprucePlanks,
            Cobblestone,
            CraftingTable,
            Furnace,
            Glass,
            Torch,
            Chest,
            WhiteWool,
            Regolith,
            MoonRock,
            Basalt,
            Ash,
            Lava,
            VehicleCore,
            Hull,
            Seat,
            FuelTank,
            Propeller,
            JetEngine,
            RocketEngine,
            OrbitalEngine,
            Wing,
            Flap,
            Gyro,
            Wheel,
            LandingGear,
            Conveyor,
            PowerPole,
            CoalGenerator,
            ElectricFurnace,
            OilShale,
            Miner,
            OilPump,
            Refinery,
            ArcFurnace,
            Assembler,
            ChipFab,
            Accumulator,
            LargeWheel,
            GiantWheel,
            VectorEngine,
            Aerospike,
            IonEngine,
            VehicleDrill,
            SolarPanel,
            Door,
            DoorOpen,
            SlidingDoor,
            SlidingDoorOpen,
            BigDoor,
            BigDoorOpen,
            SulfurOre,
            LeadOre,
            FluoriteOre,
            SaltpeterOre,
            Concrete,
            SteelPlating,
            VitrifiedWaste,
            Pipe,
            StorageTank,
            WaterPump,
            Electrolyzer,
            Centrifuge,
            FuelFabricator,
            Boiler,
            GeothermalWell,
            WindTurbine,
            Rtg,
            Blender,
            SteamTurbine,
            NuclearReactor,
            CoolingPool,
            ReprocessingPlant,
            ParticleAccelerator,
            AntimatterDrive,
        ]
    };

    /// Only exists on a vehicle: can't be placed in the world.
    pub fn is_vehicle_part(self) -> bool {
        ((Block::VehicleCore as u8..=Block::LandingGear as u8).contains(&(self as u8))
            && self != Block::Hull)
            || (Block::LargeWheel as u8..=Block::VehicleDrill as u8).contains(&(self as u8))
            || self == Block::AntimatterDrive
    }

    /// Part of a factory (see `factory`): conveyors, pipes, poles and machines.
    pub fn is_factory(self) -> bool {
        matches!(self, Block::Conveyor | Block::Pipe) || self.is_electric()
    }

    /// The cells a model block fills (see `factory::footprint`): this many cells out from the
    /// one it was put on, each way across, and this many up. (0, 1) for one cell.
    pub fn structure(self) -> (i32, i32) {
        match self {
            Block::Blender => (2, 4),
            Block::Miner => (1, 5),
            Block::NuclearReactor => (2, 5),
            Block::SteamTurbine => (1, 2),
            Block::CoolingPool => (1, 1),
            Block::ReprocessingPlant => (1, 3),
            Block::ParticleAccelerator => (3, 2),
            Block::Centrifuge => (0, 3),
            Block::Electrolyzer | Block::Boiler | Block::StorageTank => (0, 2),
            Block::WindTurbine => (0, 8),
            _ => (0, 1),
        }
    }

    /// Fills more than its own cell (`structure`).
    pub fn is_structure(self) -> bool {
        self.structure() != (0, 1)
    }

    /// A cell of a door, and which kind; whether it stands open.
    pub fn door(self) -> Option<(DoorKind, bool)> {
        Some(match self {
            Block::Door => (DoorKind::Hinged, false),
            Block::DoorOpen => (DoorKind::Hinged, true),
            Block::SlidingDoor => (DoorKind::Sliding, false),
            Block::SlidingDoorOpen => (DoorKind::Sliding, true),
            Block::BigDoor => (DoorKind::Big, false),
            Block::BigDoorOpen => (DoorKind::Big, true),
            _ => return None,
        })
    }

    /// Exists only as the state of another block (an open door): never an item of its own.
    pub fn is_state(self) -> bool {
        self.door().is_some_and(|(_, open)| open)
    }

    /// The item it is as (an open door is the door).
    pub fn as_item(self) -> Block {
        match self.door() {
            Some((kind, _)) => kind.closed(),
            None => self,
        }
    }

    /// Cables can run to it: poles and machines.
    pub fn is_electric(self) -> bool {
        self == Block::PowerPole || self.is_machine()
    }

    /// A machine of a factory (`factory::MachineKind`).
    pub fn is_machine(self) -> bool {
        matches!(
            self,
            Block::CoalGenerator
                | Block::ElectricFurnace
                | Block::Miner
                | Block::OilPump
                | Block::Refinery
                | Block::ArcFurnace
                | Block::Assembler
                | Block::ChipFab
                | Block::Accumulator
                | Block::SolarPanel
                | Block::StorageTank
                | Block::WaterPump
                | Block::Electrolyzer
                | Block::Centrifuge
                | Block::FuelFabricator
                | Block::Boiler
                | Block::GeothermalWell
                | Block::WindTurbine
                | Block::Rtg
                | Block::Blender
                | Block::SteamTurbine
                | Block::NuclearReactor
                | Block::CoolingPool
                | Block::ReprocessingPlant
                | Block::ParticleAccelerator
        )
    }

    /// Solid and fully hides what's behind it (a full cube).
    pub fn is_opaque(self) -> bool {
        !matches!(self, Block::Air | Block::Water | Block::Glass) && !self.is_model()
    }

    /// Drawn with see-through pixels (in the chunk mesh's cutout section).
    pub fn is_cutout(self) -> bool {
        self == Block::Glass
    }

    /// Things walk into it (torches don't stop you).
    pub fn collides(self) -> bool {
        // (A conveyor is a low belt you walk over, as you walk through a torch; an open door
        // you walk through.)
        self.is_solid() && !matches!(self, Block::Torch | Block::Conveyor) && !self.is_state()
    }

    /// Trees and cacti: a tornado takes several at a time, and vehicles knock down the ones
    /// the world grew (`Voxels::grown`).
    pub fn is_plant(self) -> bool {
        matches!(
            self,
            Block::OakLeaves
                | Block::SpruceLeaves
                | Block::JungleLeaves
                | Block::OakLog
                | Block::SpruceLog
                | Block::Cactus
        )
    }

    /// Light level this block gives off by itself (its own faces glow; see `world::light`
    /// for light that spreads).
    pub fn glow(self) -> u32 {
        match self {
            Block::OuterCore | Block::Lava => 15,
            Block::InnerCore => 12,
            Block::VitrifiedWaste => 5,
            _ => 0,
        }
    }

    /// Collides and can be targeted: cubes and models.
    pub fn is_solid(self) -> bool {
        !matches!(self, Block::Air | Block::Water)
    }

    /// Occupies its cell but is drawn as a 3D model instead of a cube (the mesher skips it).
    pub fn is_model(self) -> bool {
        matches!(
            self,
            Block::CraftingTable | Block::Furnace | Block::Torch | Block::Chest
        ) || self.is_factory()
            || self.door().is_some()
    }

    /// Textures for (top, side, bottom) faces. "Top" is radially outward.
    pub fn textures(self) -> [Tex; 3] {
        use Block::*;
        let all = |t| [t, t, t];
        match self {
            Air | Water => all(Tex::Water),
            Stone => all(Tex::Stone),
            Deepstone => all(Tex::Deepstone),
            Mantle => all(Tex::Mantle),
            OuterCore => all(Tex::OuterCore),
            InnerCore => all(Tex::InnerCore),
            Dirt => all(Tex::Dirt),
            Grass => [Tex::GrassTop, Tex::GrassSide, Tex::Dirt],
            SnowyGrass => [Tex::Snow, Tex::SnowyGrassSide, Tex::Dirt],
            Snow => all(Tex::Snow),
            Ice => all(Tex::Ice),
            Sand => all(Tex::Sand),
            RedSand => all(Tex::RedSand),
            Sandstone => [Tex::SandstoneTop, Tex::Sandstone, Tex::SandstoneTop],
            Gravel => all(Tex::Gravel),
            TerracottaOrange => all(Tex::TerracottaOrange),
            TerracottaRed => all(Tex::TerracottaRed),
            TerracottaYellow => all(Tex::TerracottaYellow),
            TerracottaWhite => all(Tex::TerracottaWhite),
            TerracottaBrown => all(Tex::TerracottaBrown),
            OakLog => [Tex::LogTop, Tex::OakLogSide, Tex::LogTop],
            OakLeaves => all(Tex::OakLeaves),
            SpruceLog => [Tex::LogTop, Tex::SpruceLogSide, Tex::LogTop],
            SpruceLeaves => all(Tex::SpruceLeaves),
            JungleLeaves => all(Tex::JungleLeaves),
            Cactus => [Tex::CactusTop, Tex::CactusSide, Tex::CactusTop],
            CoalOre => all(Tex::CoalOre),
            IronOre => all(Tex::IronOre),
            CopperOre => all(Tex::CopperOre),
            GoldOre => all(Tex::GoldOre),
            DiamondOre => all(Tex::DiamondOre),
            BauxiteOre => all(Tex::BauxiteOre),
            UraniumOre => all(Tex::UraniumOre),
            IlmeniteSand => all(Tex::IlmeniteSand),
            OakPlanks => all(Tex::OakPlanks),
            SprucePlanks => all(Tex::SprucePlanks),
            Cobblestone => all(Tex::Cobblestone),
            CraftingTable => [Tex::CraftingTop, Tex::CraftingSide, Tex::OakPlanks],
            Furnace => [Tex::FurnaceTop, Tex::Cobblestone, Tex::Cobblestone],
            Glass => all(Tex::Glass),
            Torch => [Tex::TorchTop, Tex::TorchSide, Tex::TorchSide],
            Chest => [Tex::ChestTop, Tex::ChestSide, Tex::ChestSide],
            WhiteWool => all(Tex::WhiteWool),
            Regolith => all(Tex::Regolith),
            MoonRock => all(Tex::MoonRock),
            Basalt => all(Tex::Basalt),
            Ash => all(Tex::Ash),
            Lava => all(Tex::Lava),
            VehicleCore => all(Tex::VehicleCore),
            Hull => all(Tex::HullPlate),
            Seat => all(Tex::SeatCloth),
            FuelTank => [Tex::HullPlate, Tex::TankSide, Tex::HullPlate],
            Propeller => all(Tex::PropellerFace),
            JetEngine => all(Tex::JetFace),
            RocketEngine => all(Tex::RocketFace),
            OrbitalEngine => all(Tex::OrbitalFace),
            Wing => all(Tex::WingSkin),
            Flap => all(Tex::FlapSkin),
            Gyro => all(Tex::GyroFace),
            Wheel => all(Tex::Tire),
            LandingGear => all(Tex::GearFace),
            Conveyor => [Tex::ConveyorBelt, Tex::MachineSide, Tex::MachineSide],
            PowerPole => [Tex::LogTop, Tex::OakLogSide, Tex::LogTop],
            OilShale => all(Tex::OilShale),
            CoalGenerator | ElectricFurnace | Miner | OilPump | Refinery | ArcFurnace
            | Assembler | Accumulator => [Tex::MachineTop, Tex::MachineSide, Tex::MachineSide],
            ChipFab => [Tex::CleanPanel, Tex::CleanPanel, Tex::CleanPanel],
            LargeWheel | GiantWheel => all(Tex::Tire),
            VectorEngine => all(Tex::VectorFace),
            Aerospike => all(Tex::AerospikeFace),
            IonEngine => all(Tex::IonFace),
            VehicleDrill => all(Tex::DrillFace),
            SolarPanel => [Tex::SolarCell, Tex::MachineSide, Tex::MachineSide],
            Door | DoorOpen => all(Tex::DoorPlanks),
            SlidingDoor | SlidingDoorOpen => all(Tex::SlidingPanel),
            BigDoor | BigDoorOpen => all(Tex::ShutterPanel),
            SulfurOre => all(Tex::SulfurOre),
            LeadOre => all(Tex::LeadOre),
            FluoriteOre => all(Tex::FluoriteOre),
            SaltpeterOre => all(Tex::SaltpeterOre),
            Concrete => all(Tex::Concrete),
            SteelPlating => all(Tex::SteelPlate),
            VitrifiedWaste => [Tex::VitrifiedTop, Tex::VitrifiedSide, Tex::VitrifiedTop],
            Pipe => all(Tex::MachineTop),
            StorageTank | Centrifuge | Electrolyzer => {
                [Tex::TankPaint, Tex::TankPaint, Tex::MachineTop]
            }
            NuclearReactor | CoolingPool | ReprocessingPlant => {
                [Tex::Concrete, Tex::Concrete, Tex::Concrete]
            }
            WaterPump | FuelFabricator | Boiler | GeothermalWell | WindTurbine | Rtg | Blender
            | SteamTurbine | ParticleAccelerator => {
                [Tex::MachineTop, Tex::MachineSide, Tex::MachineSide]
            }
            AntimatterDrive => all(Tex::AntimatterFace),
        }
    }

    pub fn material(self) -> Material {
        use Block::*;
        match self {
            Grass | SnowyGrass | Cactus => Material::Grass,
            Dirt => Material::Dirt,
            OakLog | SpruceLog | OakPlanks | SprucePlanks | CraftingTable | Chest | Torch
            | PowerPole | Door | DoorOpen => Material::Wood,
            Sand | RedSand | IlmeniteSand | Regolith | Ash | SaltpeterOre => Material::Sand,
            Gravel => Material::Gravel,
            Snow => Material::Snow,
            Glass | Ice => Material::Glass,
            WhiteWool => Material::Wool,
            OakLeaves | SpruceLeaves | JungleLeaves => Material::Leaves,
            _ => Material::Stone,
        }
    }

    /// Inventory icon faces (top, left, right) of the isometric cube.
    pub fn icon_faces(self) -> [Tex; 3] {
        match self {
            Block::Furnace => [Tex::FurnaceTop, Tex::FurnaceFront, Tex::Cobblestone],
            Block::Chest => [Tex::ChestTop, Tex::ChestFront, Tex::ChestSide],
            Block::Conveyor => [Tex::ConveyorBelt, Tex::ConveyorBelt, Tex::MachineSide],
            Block::PowerPole => [Tex::LogTop, Tex::PoleIcon, Tex::OakLogSide],
            Block::CoalGenerator => [Tex::MachineTop, Tex::GeneratorFront, Tex::MachineSide],
            Block::ElectricFurnace => [Tex::MachineTop, Tex::ElectricFront, Tex::MachineSide],
            Block::Miner => [Tex::MachineTop, Tex::MinerFront, Tex::MachineSide],
            Block::OilPump => [Tex::MachineTop, Tex::PumpFront, Tex::MachineSide],
            Block::Refinery => [Tex::MachineTop, Tex::RefineryFront, Tex::MachineSide],
            Block::ArcFurnace => [Tex::MachineTop, Tex::ArcFront, Tex::MachineSide],
            Block::Assembler => [Tex::MachineTop, Tex::AssemblerFront, Tex::MachineSide],
            Block::ChipFab => [Tex::CleanPanel, Tex::FabFront, Tex::CleanPanel],
            Block::Accumulator => [Tex::MachineTop, Tex::AccumulatorFront, Tex::MachineSide],
            Block::SolarPanel => [Tex::SolarCell, Tex::SolarCell, Tex::MachineSide],
            Block::NuclearReactor => [Tex::Concrete, Tex::ControlPanelLit, Tex::Hazard],
            Block::ReprocessingPlant => [Tex::Concrete, Tex::HotCellLit, Tex::Concrete],
            Block::CoolingPool => [Tex::Cherenkov, Tex::Concrete, Tex::Concrete],
            Block::ParticleAccelerator => [Tex::MachineTop, Tex::ControlPanelLit, Tex::Hazard],
            Block::FuelFabricator => [Tex::MachineTop, Tex::ControlPanel, Tex::MachineSide],
            Block::Boiler => [Tex::MachineTop, Tex::GeneratorFrontLit, Tex::MachineSide],
            Block::Blender => [Tex::TankPaint, Tex::Hazard, Tex::MachineSide],
            Block::SteamTurbine | Block::WindTurbine => {
                [Tex::TankPaint, Tex::MachineSide, Tex::TankPaint]
            }
            Block::Rtg => [Tex::CableSkin, Tex::Hazard, Tex::CableSkin],
            _ => {
                let [top, side, _] = self.textures();
                [top, side, side]
            }
        }
    }

    pub fn tint(self) -> Tint {
        match self {
            Block::Grass => Tint::Grass,
            Block::OakLeaves | Block::JungleLeaves => Tint::Foliage,
            Block::SpruceLeaves => Tint::Spruce,
            Block::Water => Tint::Water,
            _ => Tint::None,
        }
    }

    /// A readable name ("OakLog" -> "Oak Log").
    pub fn name(self) -> String {
        let raw = format!("{self:?}");
        let mut out = String::new();
        for (i, c) in raw.chars().enumerate() {
            if i > 0 && c.is_uppercase() {
                out.push(' ');
            }
            out.push(c);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_blocks_in_order() {
        for (i, b) in Block::ALL.iter().enumerate() {
            assert_eq!(*b as usize, i);
        }
        assert_eq!(*Block::ALL.last().unwrap(), Block::AntimatterDrive);
        assert!(Block::Pipe.is_factory() && !Block::Pipe.is_electric() && Block::Pipe.is_model());
        assert!(Block::NuclearReactor.is_machine() && Block::NuclearReactor.is_structure());
        assert!(Block::AntimatterDrive.is_vehicle_part() && !Block::Rtg.is_vehicle_part());
        assert!(!Block::Concrete.is_model() && !Block::SulfurOre.is_factory());
        assert!(Block::Assembler.is_machine() && Block::Assembler.is_model());
        assert!(!Block::OilShale.is_model() && !Block::OilShale.is_factory());
        assert!(Block::Wheel.is_vehicle_part() && !Block::Hull.is_vehicle_part());
        assert!(!Block::Conveyor.is_vehicle_part() && Block::Conveyor.is_model());
        assert!(Block::IonEngine.is_vehicle_part() && Block::VehicleDrill.is_vehicle_part());
        assert!(!Block::SolarPanel.is_vehicle_part() && Block::SolarPanel.is_machine());
        // Doors: closed ones stop you, open ones don't; an open door is the door as an item.
        assert!(Block::Door.collides() && !Block::DoorOpen.collides());
        assert!(Block::BigDoorOpen.is_solid() && Block::BigDoorOpen.is_model());
        assert_eq!(Block::SlidingDoorOpen.as_item(), Block::SlidingDoor);
        assert!(!Block::BigDoor.is_factory() && !Block::Door.is_vehicle_part());
    }
}
