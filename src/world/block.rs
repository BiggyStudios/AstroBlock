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
    pub const ALL: [Block; 49] = {
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
        ]
    };

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
        self.is_solid() && self != Block::Torch
    }

    /// Light level this block gives off by itself (its own faces glow; see `world::light`
    /// for light that spreads).
    pub fn glow(self) -> u32 {
        match self {
            Block::OuterCore | Block::Lava => 15,
            Block::InnerCore => 12,
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
        )
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
        }
    }

    pub fn material(self) -> Material {
        use Block::*;
        match self {
            Grass | SnowyGrass | Cactus => Material::Grass,
            Dirt => Material::Dirt,
            OakLog | SpruceLog | OakPlanks | SprucePlanks | CraftingTable | Chest | Torch => {
                Material::Wood
            }
            Sand | RedSand | IlmeniteSand | Regolith | Ash => Material::Sand,
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
        assert_eq!(*Block::ALL.last().unwrap(), Block::Lava);
    }
}
