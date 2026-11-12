//! Items: blocks you carry, materials, tools; stacks; and the Minecraft rules for mining
//! (break times, which tool harvests what) and what blocks drop.

use crate::world::{
    block::Block,
    textures::{IconTex, Tex},
};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Sword,
}

/// Tool material, weakest first (gold is fast but weak, as in Minecraft).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Tier {
    Wood,
    Stone,
    Iron,
    Gold,
    Diamond,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::Wood,
        Tier::Stone,
        Tier::Iron,
        Tier::Gold,
        Tier::Diamond,
    ];

    /// Mining speed multiplier with the right tool.
    pub fn speed(self) -> f32 {
        match self {
            Tier::Wood => 2.0,
            Tier::Stone => 4.0,
            Tier::Iron => 6.0,
            Tier::Gold => 12.0,
            Tier::Diamond => 8.0,
        }
    }

    /// Harvest level: which ores it can mine (see `Mining::level`).
    pub fn level(self) -> u8 {
        match self {
            Tier::Wood | Tier::Gold => 0,
            Tier::Stone => 1,
            Tier::Iron => 2,
            Tier::Diamond => 3,
        }
    }

    pub fn durability(self) -> u16 {
        match self {
            Tier::Wood => 59,
            Tier::Stone => 131,
            Tier::Iron => 250,
            Tier::Gold => 32,
            Tier::Diamond => 1561,
        }
    }

    fn adjective(self) -> &'static str {
        match self {
            Tier::Wood => "Wooden",
            Tier::Stone => "Stone",
            Tier::Iron => "Iron",
            Tier::Gold => "Golden",
            Tier::Diamond => "Diamond",
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ArmorPiece {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
}

impl ArmorPiece {
    pub const ALL: [ArmorPiece; 4] = [
        ArmorPiece::Helmet,
        ArmorPiece::Chestplate,
        ArmorPiece::Leggings,
        ArmorPiece::Boots,
    ];
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ArmorTier {
    Leather,
    Iron,
    Gold,
    Diamond,
}

impl ArmorTier {
    pub const ALL: [ArmorTier; 4] = [
        ArmorTier::Leather,
        ArmorTier::Iron,
        ArmorTier::Gold,
        ArmorTier::Diamond,
    ];

    /// Armor points of a piece (Minecraft's values; 20 = a full row of the armor bar).
    pub fn points(self, piece: ArmorPiece) -> u32 {
        let table = match self {
            ArmorTier::Leather => [1, 3, 2, 1],
            ArmorTier::Iron => [2, 6, 5, 2],
            ArmorTier::Gold => [2, 5, 3, 1],
            ArmorTier::Diamond => [3, 8, 6, 3],
        };
        table[piece as usize]
    }

    pub fn toughness(self) -> f32 {
        if self == ArmorTier::Diamond { 2.0 } else { 0.0 }
    }

    pub fn durability(self, piece: ArmorPiece) -> u16 {
        let base = [11, 16, 15, 13][piece as usize];
        let factor = match self {
            ArmorTier::Leather => 5,
            ArmorTier::Iron => 15,
            ArmorTier::Gold => 7,
            ArmorTier::Diamond => 33,
        };
        base * factor
    }

    fn adjective(self) -> &'static str {
        match self {
            ArmorTier::Leather => "Leather",
            ArmorTier::Iron => "Iron",
            ArmorTier::Gold => "Golden",
            ArmorTier::Diamond => "Diamond",
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Item {
    Block(Block),
    Stick,
    Coal,
    Charcoal,
    RawIron,
    RawCopper,
    RawGold,
    IronIngot,
    CopperIngot,
    GoldIngot,
    Diamond,
    Tool(ToolKind, Tier),
    RawBeef,
    Steak,
    RawPorkchop,
    CookedPorkchop,
    RawMutton,
    CookedMutton,
    Leather,
    Armor(ArmorPiece, ArmorTier),
    /// A test rocket: placed on the ground, then boarded.
    Rocket,
    /// Insulated copper wire: runs power between poles and machines.
    Cable,
    // Industry (see `factory::recipes` for how each is made).
    /// A barrel of it, from an oil pump.
    CrudeOil,
    /// A can of it: burns five times as long as coal.
    Fuel,
    Plastic,
    Rubber,
    Silicon,
    SiliconWafer,
    SteelIngot,
    CopperWire,
    InsulatedWire,
    CircuitBoard,
    Battery,
    Led,
    MemoryChip,
    Cpu,
    AiChip,
    Screen,
    Motherboard,
    PowerSupply,
    ComputerCase,
    /// (Saves from when there were AI servers load them as computers.)
    #[serde(alias = "AiServer")]
    Computer,
    /// A fluid: only ever what a recipe makes (it is piped, never carried: see `fluids`).
    Fluid(crate::factory::fluids::Fluid),
    // Chemistry and metals.
    Sulfur,
    Fluorite,
    Saltpeter,
    RawLead,
    LeadIngot,
    Alumina,
    AluminiumIngot,
    TitaniumIngot,
    Zircon,
    Zirconium,
    ElectricMotor,
    TurbineBlade,
    SuperconductingMagnet,
    // Nuclear.
    Yellowcake,
    DepletedUranium,
    UraniumFuelRod,
    MoxFuelRod,
    /// Hot out of a reactor: it must cool in a cooling pool before it can be reprocessed.
    SpentFuelRod,
    CooledSpentFuel,
    /// The chopped cladding of spent fuel.
    FuelHulls,
    RecoveredUranium,
    Plutonium,
    /// Held in a magnetic bottle.
    Antimatter,
}

/// What the chemistry and metal chains make, in catalog order.
pub const CHEMISTRY: [Item; 13] = [
    Item::Sulfur,
    Item::Fluorite,
    Item::Saltpeter,
    Item::RawLead,
    Item::LeadIngot,
    Item::Alumina,
    Item::AluminiumIngot,
    Item::TitaniumIngot,
    Item::Zircon,
    Item::Zirconium,
    Item::ElectricMotor,
    Item::TurbineBlade,
    Item::SuperconductingMagnet,
];

/// What the nuclear chains make, in catalog order.
pub const NUCLEAR: [Item; 10] = [
    Item::Yellowcake,
    Item::DepletedUranium,
    Item::UraniumFuelRod,
    Item::MoxFuelRod,
    Item::SpentFuelRod,
    Item::CooledSpentFuel,
    Item::FuelHulls,
    Item::RecoveredUranium,
    Item::Plutonium,
    Item::Antimatter,
];

/// What industry makes, in catalog order.
pub const INDUSTRY: [Item; 20] = [
    Item::CrudeOil,
    Item::Fuel,
    Item::Plastic,
    Item::Rubber,
    Item::Silicon,
    Item::SiliconWafer,
    Item::SteelIngot,
    Item::CopperWire,
    Item::InsulatedWire,
    Item::CircuitBoard,
    Item::Battery,
    Item::Led,
    Item::MemoryChip,
    Item::Cpu,
    Item::AiChip,
    Item::Screen,
    Item::Motherboard,
    Item::PowerSupply,
    Item::ComputerCase,
    Item::Computer,
];

/// The creative catalog's sections (the tabs down its left side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Category {
    /// What the worlds are made of: rock, soil, ores, trees, ice, lava.
    #[default]
    Terrain,
    /// Made blocks: planks, glass, wool, and the workbench, furnace, chest and torch.
    Building,
    /// Conveyors, poles, machines, cable.
    Factory,
    /// Vehicle parts, hull plating and the test rocket.
    Vehicles,
    /// Ores raw and smelted, coal, sticks, leather; oil and what is refined from it.
    Materials,
    /// Wire, boards, chips, screens and computers.
    Electronics,
    /// Uranium, fuel rods, plutonium, antimatter, waste.
    Nuclear,
    /// Tools, weapons and armor.
    Gear,
    Food,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Terrain,
        Category::Building,
        Category::Factory,
        Category::Vehicles,
        Category::Materials,
        Category::Electronics,
        Category::Nuclear,
        Category::Gear,
        Category::Food,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Terrain => "terrain",
            Category::Building => "building",
            Category::Factory => "factory",
            Category::Vehicles => "vehicles",
            Category::Materials => "materials",
            Category::Electronics => "electronics",
            Category::Nuclear => "nuclear",
            Category::Gear => "gear",
            Category::Food => "food",
        }
    }

    /// Its items, in catalog order.
    pub fn items(self) -> Vec<Item> {
        Item::all()
            .into_iter()
            .filter(|i| i.category() == self)
            .collect()
    }
}

/// How an item looks in the inventory (and, but for pictures, on the ground and in hand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A flat tile of the block's side (`Block::icon_faces`).
    Block(Block),
    /// A flat sprite (alpha = transparency).
    Sprite(Tex),
    /// A machine's picture, only in the interface (`IconTex`); in the world the machine
    /// shows its model.
    Picture(IconTex),
}

impl Item {
    /// Everything that can exist as an item, in catalog order.
    pub fn all() -> Vec<Item> {
        let mut items: Vec<Item> = Block::ALL
            .iter()
            .filter(|b| !matches!(b, Block::Air | Block::Water) && !b.is_state())
            .map(|&b| Item::Block(b))
            .collect();
        items.extend([
            Item::Stick,
            Item::Coal,
            Item::Charcoal,
            Item::RawIron,
            Item::RawCopper,
            Item::RawGold,
            Item::IronIngot,
            Item::CopperIngot,
            Item::GoldIngot,
            Item::Diamond,
        ]);
        for kind in [
            ToolKind::Pickaxe,
            ToolKind::Axe,
            ToolKind::Shovel,
            ToolKind::Sword,
        ] {
            items.extend(Tier::ALL.iter().map(|&t| Item::Tool(kind, t)));
        }
        items.extend([
            Item::RawBeef,
            Item::Steak,
            Item::RawPorkchop,
            Item::CookedPorkchop,
            Item::RawMutton,
            Item::CookedMutton,
            Item::Leather,
        ]);
        for piece in ArmorPiece::ALL {
            items.extend(ArmorTier::ALL.iter().map(|&t| Item::Armor(piece, t)));
        }
        items.push(Item::Rocket);
        items.push(Item::Cable);
        items.extend(INDUSTRY);
        items.extend(CHEMISTRY);
        items.extend(NUCLEAR);
        items
    }

    /// Which tab of the creative catalog it is under. New blocks land in terrain unless they
    /// are factory or vehicle parts, or listed under building here.
    pub fn category(self) -> Category {
        match self {
            Item::Block(Block::VitrifiedWaste) => Category::Nuclear,
            Item::Block(b) if b.is_factory() => Category::Factory,
            Item::Cable => Category::Factory,
            Item::Block(b) if b.is_vehicle_part() => Category::Vehicles,
            Item::Block(Block::Hull) | Item::Rocket => Category::Vehicles,
            Item::Block(
                Block::OakPlanks
                | Block::SprucePlanks
                | Block::Cobblestone
                | Block::Glass
                | Block::WhiteWool
                | Block::CraftingTable
                | Block::Furnace
                | Block::Chest
                | Block::Torch
                | Block::Door
                | Block::SlidingDoor
                | Block::BigDoor
                | Block::Concrete
                | Block::SteelPlating,
            ) => Category::Building,
            Item::Block(_) => Category::Terrain,
            Item::Tool(..) | Item::Armor(..) => Category::Gear,
            Item::RawBeef
            | Item::Steak
            | Item::RawPorkchop
            | Item::CookedPorkchop
            | Item::RawMutton
            | Item::CookedMutton => Category::Food,
            Item::Stick
            | Item::Coal
            | Item::Charcoal
            | Item::RawIron
            | Item::RawCopper
            | Item::RawGold
            | Item::IronIngot
            | Item::CopperIngot
            | Item::GoldIngot
            | Item::Diamond
            | Item::Leather
            | Item::CrudeOil
            | Item::Fuel
            | Item::Plastic
            | Item::Rubber
            | Item::Silicon
            | Item::SteelIngot
            | Item::Fluid(_)
            | Item::Sulfur
            | Item::Fluorite
            | Item::Saltpeter
            | Item::RawLead
            | Item::LeadIngot
            | Item::Alumina
            | Item::AluminiumIngot
            | Item::TitaniumIngot
            | Item::Zircon
            | Item::Zirconium => Category::Materials,
            Item::Yellowcake
            | Item::DepletedUranium
            | Item::UraniumFuelRod
            | Item::MoxFuelRod
            | Item::SpentFuelRod
            | Item::CooledSpentFuel
            | Item::FuelHulls
            | Item::RecoveredUranium
            | Item::Plutonium
            | Item::Antimatter => Category::Nuclear,
            Item::ElectricMotor | Item::TurbineBlade | Item::SuperconductingMagnet => {
                Category::Electronics
            }
            Item::SiliconWafer
            | Item::CopperWire
            | Item::InsulatedWire
            | Item::CircuitBoard
            | Item::Battery
            | Item::Led
            | Item::MemoryChip
            | Item::Cpu
            | Item::AiChip
            | Item::Screen
            | Item::Motherboard
            | Item::PowerSupply
            | Item::ComputerCase
            | Item::Computer => Category::Electronics,
        }
    }

    pub fn name(self) -> String {
        match self {
            Item::Block(b) => b.name(),
            Item::Tool(kind, tier) => format!("{} {:?}", tier.adjective(), kind),
            Item::RawIron => "Raw Iron".into(),
            Item::RawCopper => "Raw Copper".into(),
            Item::RawGold => "Raw Gold".into(),
            Item::IronIngot => "Iron Ingot".into(),
            Item::CopperIngot => "Copper Ingot".into(),
            Item::GoldIngot => "Gold Ingot".into(),
            Item::RawBeef => "Raw Beef".into(),
            Item::RawPorkchop => "Raw Porkchop".into(),
            Item::CookedPorkchop => "Cooked Porkchop".into(),
            Item::RawMutton => "Raw Mutton".into(),
            Item::CookedMutton => "Cooked Mutton".into(),
            Item::Armor(piece, tier) => format!("{} {:?}", tier.adjective(), piece),
            Item::Cpu => "CPU".into(),
            Item::Led => "LED".into(),
            Item::AiChip => "AI Chip".into(),
            Item::MoxFuelRod => "MOX Fuel Rod".into(),
            Item::Fluid(f) => {
                let mut name = f.name().to_string();
                if let Some(first) = name.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                name
            }
            other => {
                // "SiliconWafer" -> "Silicon Wafer"
                let raw = format!("{other:?}");
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
    }

    /// `snake_case` id for the console (`give iron_pickaxe`).
    pub fn id(self) -> String {
        self.name().to_lowercase().replace(' ', "_")
    }

    pub fn from_id(id: &str) -> Option<Item> {
        let id = id.to_lowercase();
        Item::all().into_iter().find(|i| i.id() == id)
    }

    pub fn max_stack(self) -> u8 {
        match self {
            Item::Tool(..) | Item::Armor(..) | Item::Rocket => 1,
            _ => 64,
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Item::Block(Block::Torch) => Icon::Sprite(Tex::TorchSide),
            Item::Block(Block::PowerPole) => Icon::Sprite(Tex::PoleIcon),
            Item::Block(b) => IconTex::of(b).map_or(Icon::Block(b), Icon::Picture),
            Item::Stick => Icon::Sprite(Tex::Stick),
            Item::Coal => Icon::Sprite(Tex::Coal),
            Item::Charcoal => Icon::Sprite(Tex::Charcoal),
            Item::RawIron => Icon::Sprite(Tex::RawIron),
            Item::RawCopper => Icon::Sprite(Tex::RawCopper),
            Item::RawGold => Icon::Sprite(Tex::RawGold),
            Item::IronIngot => Icon::Sprite(Tex::IronIngot),
            Item::CopperIngot => Icon::Sprite(Tex::CopperIngot),
            Item::GoldIngot => Icon::Sprite(Tex::GoldIngot),
            Item::Diamond => Icon::Sprite(Tex::Diamond),
            Item::Tool(kind, tier) => Icon::Sprite(Tex::tool(kind as usize, tier as usize)),
            Item::RawBeef => Icon::Sprite(Tex::RawBeef),
            Item::Steak => Icon::Sprite(Tex::Steak),
            Item::RawPorkchop => Icon::Sprite(Tex::RawPorkchop),
            Item::CookedPorkchop => Icon::Sprite(Tex::CookedPorkchop),
            Item::RawMutton => Icon::Sprite(Tex::RawMutton),
            Item::CookedMutton => Icon::Sprite(Tex::CookedMutton),
            Item::Leather => Icon::Sprite(Tex::Leather),
            Item::Armor(piece, tier) => Icon::Sprite(Tex::armor(piece as usize, tier as usize)), // Torches show as their sprite, not a cube.
            Item::Rocket => Icon::Sprite(Tex::Rocket),
            Item::Cable => Icon::Sprite(Tex::Cable),
            Item::CrudeOil => Icon::Sprite(Tex::CrudeOil),
            Item::Fuel => Icon::Sprite(Tex::FuelCan),
            Item::Plastic => Icon::Sprite(Tex::Plastic),
            Item::Rubber => Icon::Sprite(Tex::Rubber),
            Item::Silicon => Icon::Sprite(Tex::Silicon),
            Item::SiliconWafer => Icon::Sprite(Tex::Wafer),
            Item::SteelIngot => Icon::Sprite(Tex::SteelIngot),
            Item::CopperWire => Icon::Sprite(Tex::CopperWire),
            Item::InsulatedWire => Icon::Sprite(Tex::InsulatedWire),
            Item::CircuitBoard => Icon::Sprite(Tex::CircuitBoard),
            Item::Battery => Icon::Sprite(Tex::Battery),
            Item::Led => Icon::Sprite(Tex::Led),
            Item::MemoryChip => Icon::Sprite(Tex::MemoryChip),
            Item::Cpu => Icon::Sprite(Tex::Cpu),
            Item::AiChip => Icon::Sprite(Tex::AiChip),
            Item::Screen => Icon::Sprite(Tex::Monitor),
            Item::Motherboard => Icon::Sprite(Tex::Motherboard),
            Item::PowerSupply => Icon::Sprite(Tex::PowerSupply),
            Item::ComputerCase => Icon::Sprite(Tex::ComputerCase),
            Item::Computer => Icon::Sprite(Tex::Computer),
            Item::Fluid(f) => Icon::Sprite(f.tex()),
            Item::Sulfur => Icon::Sprite(Tex::Sulfur),
            Item::Fluorite => Icon::Sprite(Tex::Fluorite),
            Item::Saltpeter => Icon::Sprite(Tex::Saltpeter),
            Item::RawLead => Icon::Sprite(Tex::RawLead),
            Item::LeadIngot => Icon::Sprite(Tex::LeadIngot),
            Item::Alumina => Icon::Sprite(Tex::Alumina),
            Item::AluminiumIngot => Icon::Sprite(Tex::AluminiumIngot),
            Item::TitaniumIngot => Icon::Sprite(Tex::TitaniumIngot),
            Item::Zircon => Icon::Sprite(Tex::Zircon),
            Item::Zirconium => Icon::Sprite(Tex::Zirconium),
            Item::ElectricMotor => Icon::Sprite(Tex::ElectricMotor),
            Item::TurbineBlade => Icon::Sprite(Tex::TurbineBlade),
            Item::SuperconductingMagnet => Icon::Sprite(Tex::Magnet),
            Item::Yellowcake => Icon::Sprite(Tex::Yellowcake),
            Item::DepletedUranium => Icon::Sprite(Tex::DepletedUranium),
            Item::UraniumFuelRod => Icon::Sprite(Tex::UraniumFuelRod),
            Item::MoxFuelRod => Icon::Sprite(Tex::MoxFuelRod),
            Item::SpentFuelRod => Icon::Sprite(Tex::SpentFuelRod),
            Item::CooledSpentFuel => Icon::Sprite(Tex::CooledSpentFuel),
            Item::FuelHulls => Icon::Sprite(Tex::FuelHulls),
            Item::RecoveredUranium => Icon::Sprite(Tex::RecoveredUranium),
            Item::Plutonium => Icon::Sprite(Tex::Plutonium),
            Item::Antimatter => Icon::Sprite(Tex::Antimatter),
        }
    }

    /// The block this item places, if any.
    pub fn block(self) -> Option<Block> {
        match self {
            Item::Block(b) => Some(b),
            _ => None,
        }
    }

    pub fn tool(self) -> Option<(ToolKind, Tier)> {
        match self {
            Item::Tool(kind, tier) => Some((kind, tier)),
            _ => None,
        }
    }

    /// Seconds this item burns in a furnace.
    pub fn fuel(self) -> Option<f32> {
        match self {
            Item::Fuel => Some(400.0),
            Item::Coal | Item::Charcoal => Some(80.0),
            Item::Block(Block::OilShale) => Some(30.0),
            Item::Block(Block::OakLog | Block::SpruceLog) => Some(15.0),
            Item::Block(
                Block::OakPlanks | Block::SprucePlanks | Block::CraftingTable | Block::Chest,
            ) => Some(15.0),
            Item::Tool(_, Tier::Wood) => Some(10.0),
            Item::Stick => Some(5.0),
            _ => None,
        }
    }

    /// What smelting this item gives.
    pub fn smelted(self) -> Option<Item> {
        match self {
            Item::RawIron | Item::Block(Block::IronOre) => Some(Item::IronIngot),
            Item::RawCopper | Item::Block(Block::CopperOre) => Some(Item::CopperIngot),
            Item::RawGold | Item::Block(Block::GoldOre) => Some(Item::GoldIngot),
            Item::RawLead | Item::Block(Block::LeadOre) => Some(Item::LeadIngot),
            Item::Block(Block::Cobblestone) => Some(Item::Block(Block::Stone)),
            Item::Block(Block::OakLog | Block::SpruceLog) => Some(Item::Charcoal),
            Item::Block(Block::Sand) => Some(Item::Block(Block::Glass)),
            Item::RawBeef => Some(Item::Steak),
            Item::RawPorkchop => Some(Item::CookedPorkchop),
            Item::RawMutton => Some(Item::CookedMutton),
            _ => None,
        }
    }

    /// Hunger points restored and saturation added by eating it (Minecraft's values).
    pub fn food(self) -> Option<(f32, f32)> {
        match self {
            Item::RawBeef | Item::RawPorkchop => Some((3.0, 1.8)),
            Item::Steak | Item::CookedPorkchop => Some((8.0, 12.8)),
            Item::RawMutton => Some((2.0, 1.2)),
            Item::CookedMutton => Some((6.0, 9.6)),
            _ => None,
        }
    }

    pub fn armor(self) -> Option<(ArmorPiece, ArmorTier)> {
        match self {
            Item::Armor(piece, tier) => Some((piece, tier)),
            _ => None,
        }
    }

    /// Uses before it breaks (tools and armor).
    pub fn max_durability(self) -> Option<u16> {
        match self {
            Item::Tool(_, tier) => Some(tier.durability()),
            Item::Armor(piece, tier) => Some(tier.durability(piece)),
            _ => None,
        }
    }

    /// Melee damage (half-hearts) and attacks per second, as in Minecraft 1.9+.
    pub fn attack(self) -> (f32, f32) {
        use {Tier::*, ToolKind::*};
        match self {
            Item::Tool(Sword, tier) => {
                let damage = match tier {
                    Wood | Gold => 4.0,
                    Stone => 5.0,
                    Iron => 6.0,
                    Diamond => 7.0,
                };
                (damage, 1.6)
            }
            Item::Tool(Axe, tier) => match tier {
                Wood | Gold => (7.0, if tier == Gold { 1.0 } else { 0.8 }),
                Stone => (9.0, 0.8),
                Iron => (9.0, 0.9),
                Diamond => (9.0, 1.0),
            },
            Item::Tool(Pickaxe, tier) => (2.0 + tier.level() as f32, 1.2),
            Item::Tool(Shovel, tier) => (2.5 + tier.level() as f32, 1.0),
            _ => (1.0, 4.0),
        }
    }
}

/// A stack of items in a slot. Tools never stack and carry their wear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemStack {
    pub item: Item,
    pub count: u8,
    /// Uses so far (tools).
    pub wear: u16,
}

impl ItemStack {
    pub fn new(item: Item, count: u8) -> Self {
        Self {
            item,
            count,
            wear: 0,
        }
    }

    pub fn one(item: Item) -> Self {
        Self::new(item, 1)
    }

    /// Whether `other` can merge into this stack.
    pub fn stacks_with(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.item.max_stack() > 1
    }

    /// Remaining durability as a fraction (tools only).
    pub fn durability(&self) -> Option<f32> {
        let max = self.item.max_durability()?;
        Some(1.0 - self.wear as f32 / max as f32)
    }
}

/// How a block is mined: Minecraft hardness, the tool that speeds it up, and the harvest
/// level that tool needs for the block to drop anything (None: anything works, even hands).
pub struct Mining {
    pub hardness: f32,
    pub tool: Option<ToolKind>,
    pub level: Option<u8>,
}

pub fn mining(block: Block) -> Mining {
    use {Block::*, ToolKind::*};
    let m = |hardness, tool, level| Mining {
        hardness,
        tool,
        level,
    };
    match block {
        Air | Water => m(0.0, None, None),
        OakLeaves | SpruceLeaves | JungleLeaves => m(0.2, None, None),
        Snow => m(0.2, Some(Shovel), None),
        Dirt | Sand | RedSand | IlmeniteSand | Regolith | Ash => m(0.5, Some(Shovel), None),
        Lava => m(3.0, Some(Pickaxe), Some(1)),
        MoonRock => m(1.5, Some(Pickaxe), Some(0)),
        Basalt => m(1.8, Some(Pickaxe), Some(0)),
        Grass | SnowyGrass | Gravel => m(0.6, Some(Shovel), None),
        Ice => m(0.5, Some(Pickaxe), None),
        Cactus => m(0.4, None, None),
        OakLog | SpruceLog | OakPlanks | SprucePlanks => m(2.0, Some(Axe), None),
        CraftingTable | Chest => m(2.5, Some(Axe), None),
        Glass => m(0.3, None, None),
        Torch => m(0.0, None, None),
        WhiteWool => m(0.8, None, None),
        Sandstone => m(0.8, Some(Pickaxe), Some(0)),
        TerracottaOrange | TerracottaRed | TerracottaYellow | TerracottaWhite | TerracottaBrown => {
            m(1.25, Some(Pickaxe), Some(0))
        }
        Stone => m(1.5, Some(Pickaxe), Some(0)),
        Cobblestone => m(2.0, Some(Pickaxe), Some(0)),
        Deepstone => m(3.0, Some(Pickaxe), Some(0)),
        Furnace => m(3.5, Some(Pickaxe), Some(0)),
        CoalOre => m(3.0, Some(Pickaxe), Some(0)),
        IronOre | CopperOre | BauxiteOre => m(3.0, Some(Pickaxe), Some(1)),
        GoldOre | DiamondOre | UraniumOre => m(3.0, Some(Pickaxe), Some(2)),
        Mantle => m(4.5, Some(Pickaxe), Some(2)),
        OuterCore | InnerCore => m(10.0, Some(Pickaxe), Some(3)),
        Wing | Flap | Seat => m(0.5, None, None),
        Hull | FuelTank | Wheel | LandingGear | Propeller => m(0.8, None, None),
        VehicleCore | JetEngine | RocketEngine | OrbitalEngine | Gyro => m(1.2, None, None),
        Conveyor => m(1.0, Some(Pickaxe), None),
        PowerPole => m(1.5, Some(Axe), None),
        CoalGenerator | ElectricFurnace | Miner | OilPump | Refinery | ArcFurnace | Assembler
        | ChipFab | Accumulator => m(3.5, Some(Pickaxe), Some(0)),
        OilShale => m(2.0, Some(Pickaxe), Some(0)),
        LargeWheel | GiantWheel => m(0.8, None, None),
        VectorEngine | Aerospike | IonEngine | VehicleDrill => m(1.2, None, None),
        SolarPanel => m(1.5, Some(Pickaxe), None),
        Door | DoorOpen => m(3.0, Some(Axe), None),
        SlidingDoor | SlidingDoorOpen => m(2.0, Some(Pickaxe), None),
        BigDoor | BigDoorOpen => m(3.5, Some(Pickaxe), None),
        SulfurOre | SaltpeterOre => m(2.0, Some(Pickaxe), Some(0)),
        LeadOre | FluoriteOre => m(3.0, Some(Pickaxe), Some(1)),
        Concrete => m(2.5, Some(Pickaxe), Some(0)),
        SteelPlating | VitrifiedWaste => m(5.0, Some(Pickaxe), Some(1)),
        Pipe => m(1.0, Some(Pickaxe), None),
        StorageTank | WaterPump | Electrolyzer | Centrifuge | FuelFabricator | Boiler
        | GeothermalWell | WindTurbine | Rtg | Blender | SteamTurbine | NuclearReactor
        | CoolingPool | ReprocessingPlant | ParticleAccelerator => m(3.5, Some(Pickaxe), Some(0)),
        AntimatterDrive => m(1.2, None, None),
    }
}

/// Whether `tool` (None = bare hands) gets drops from `block`.
pub fn can_harvest(block: Block, tool: Option<(ToolKind, Tier)>) -> bool {
    let m = mining(block);
    match m.level {
        None => true,
        Some(level) => {
            tool.is_some_and(|(kind, tier)| Some(kind) == m.tool && tier.level() >= level)
        }
    }
}

/// Seconds to break `block` with `tool`, by Minecraft's formula.
pub fn break_time(block: Block, tool: Option<(ToolKind, Tier)>) -> f32 {
    let m = mining(block);
    let speed = match tool {
        Some((kind, tier)) if Some(kind) == m.tool => tier.speed(),
        Some((ToolKind::Sword, _)) => 1.5,
        _ => 1.0,
    };
    let factor = if can_harvest(block, tool) { 1.5 } else { 5.0 };
    // Minecraft counts in 1/20 s ticks; very soft blocks break in one.
    (m.hardness * factor / speed).max(if m.hardness > 0.0 { 0.05 } else { 0.0 })
}

/// What breaking `block` with `tool` drops. `roll` in [0, 1) decides chance drops.
pub fn drops(block: Block, tool: Option<(ToolKind, Tier)>, roll: f32) -> Option<ItemStack> {
    use Block::*;
    if !can_harvest(block, tool) {
        return None;
    }
    let item = match block {
        // (Glass and ice need silk touch in Minecraft.)
        Air | Water | Ice | Glass => return None,
        Grass | SnowyGrass => Item::Block(Dirt),
        Stone => Item::Block(Cobblestone),
        CoalOre => Item::Coal,
        IronOre => Item::RawIron,
        CopperOre => Item::RawCopper,
        GoldOre => Item::RawGold,
        DiamondOre => Item::Diamond,
        LeadOre => Item::RawLead,
        FluoriteOre => Item::Fluorite,
        SulfurOre => return Some(ItemStack::new(Item::Sulfur, 2)),
        SaltpeterOre => return Some(ItemStack::new(Item::Saltpeter, 2)),
        // No saplings yet: leaves sometimes drop a stick.
        OakLeaves | SpruceLeaves | JungleLeaves => {
            return (roll < 0.05).then(|| ItemStack::one(Item::Stick));
        }
        other => Item::Block(other.as_item()),
    };
    Some(ItemStack::one(item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mining_follows_minecraft() {
        let hand = None;
        let wood_pick = Some((ToolKind::Pickaxe, Tier::Wood));
        let stone_pick = Some((ToolKind::Pickaxe, Tier::Stone));
        let iron_pick = Some((ToolKind::Pickaxe, Tier::Iron));
        // Stone: 7.5 s by hand and drops nothing; 1.15 s with a wooden pickaxe.
        assert!((break_time(Block::Stone, hand) - 7.5).abs() < 1e-4);
        assert_eq!(drops(Block::Stone, hand, 0.0), None);
        assert!((break_time(Block::Stone, wood_pick) - 1.125).abs() < 1e-4);
        assert_eq!(
            drops(Block::Stone, wood_pick, 0.0).map(|s| s.item),
            Some(Item::Block(Block::Cobblestone))
        );
        // Iron needs stone, diamonds need iron.
        assert_eq!(drops(Block::IronOre, wood_pick, 0.0), None);
        assert_eq!(
            drops(Block::IronOre, stone_pick, 0.0).map(|s| s.item),
            Some(Item::RawIron)
        );
        assert_eq!(drops(Block::DiamondOre, stone_pick, 0.0), None);
        assert_eq!(
            drops(Block::DiamondOre, iron_pick, 0.0).map(|s| s.item),
            Some(Item::Diamond)
        );
        // An axe chops logs faster; hands still get the log.
        let axe = Some((ToolKind::Axe, Tier::Stone));
        assert!(break_time(Block::OakLog, axe) < break_time(Block::OakLog, hand) / 3.0);
        assert!(drops(Block::OakLog, hand, 0.0).is_some());
        // Grass gives dirt.
        assert_eq!(
            drops(Block::Grass, hand, 0.0).map(|s| s.item),
            Some(Item::Block(Block::Dirt))
        );
    }

    #[test]
    fn every_catalog_tab_has_items_and_every_item_one_tab() {
        let tabs: Vec<Vec<Item>> = Category::ALL.iter().map(|c| c.items()).collect();
        for (c, items) in Category::ALL.iter().zip(&tabs) {
            assert!(!items.is_empty(), "{c:?} is empty");
        }
        assert_eq!(tabs.iter().map(Vec::len).sum::<usize>(), Item::all().len());
        let factory = &tabs[Category::Factory as usize];
        assert!(factory.contains(&Item::Cable) && factory.contains(&Item::Block(Block::Conveyor)));
        assert_eq!(Item::Block(Block::Wheel).category(), Category::Vehicles);
        assert_eq!(Item::Block(Block::Basalt).category(), Category::Terrain);
    }

    #[test]
    fn ids_round_trip() {
        for item in Item::all() {
            assert_eq!(Item::from_id(&item.id()), Some(item), "{}", item.id());
        }
        assert_eq!(
            Item::from_id("iron_pickaxe"),
            Some(Item::Tool(ToolKind::Pickaxe, Tier::Iron))
        );
    }
}
