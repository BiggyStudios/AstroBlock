//! Procedurally generated 16x16 block textures (no asset files needed yet).
//!
//! Alpha channel meaning: for opaque blocks it's the *tint mask* (255 = multiply by the biome
//! colour, used for grass and leaves); for water it's the opacity; for item sprites (from
//! `Tex::Stick` on) it's transparency (drawn as cutouts, see `models` and `ui`).

use crate::render::TextureArrayData;

mod chemistry;
mod icons;
mod industry;

pub use icons::IconTex;

pub const TEX_SIZE: usize = 16;

/// Declares the texture enum and `Tex::ALL` (every variant, in declaration order) together.
macro_rules! textures {
    ($($name:ident,)*) => {
        #[repr(u8)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Tex {
            $($name,)*
        }

        impl Tex {
            pub const ALL: &'static [Tex] = &[$(Tex::$name,)*];
        }
    };
}

textures! {
    Stone,
    Deepstone,
    Mantle,
    OuterCore,
    InnerCore,
    Dirt,
    GrassTop,
    GrassSide,
    SnowyGrassSide,
    Snow,
    Ice,
    Water,
    Sand,
    RedSand,
    Sandstone,
    SandstoneTop,
    Gravel,
    TerracottaOrange,
    TerracottaRed,
    TerracottaYellow,
    TerracottaWhite,
    TerracottaBrown,
    OakLogSide,
    SpruceLogSide,
    LogTop,
    OakLeaves,
    SpruceLeaves,
    JungleLeaves,
    CactusSide,
    CactusTop,
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
    CraftingTop,
    CraftingSide,
    FurnaceFront,
    FurnaceFrontLit,
    FurnaceTop,
    // Item sprites (alpha = transparency) from here on.
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
    // Tools: kind-major (pickaxe, axe, shovel, sword), then tier (wood, stone, iron, gold,
    // diamond). `Tex::tool` relies on this order.
    WoodenPickaxe,
    StonePickaxe,
    IronPickaxe,
    GoldenPickaxe,
    DiamondPickaxe,
    WoodenAxe,
    StoneAxe,
    IronAxe,
    GoldenAxe,
    DiamondAxe,
    WoodenShovel,
    StoneShovel,
    IronShovel,
    GoldenShovel,
    DiamondShovel,
    WoodenSword,
    StoneSword,
    IronSword,
    GoldenSword,
    DiamondSword,
    // Added with the light / chest / mob update.
    Glass,
    TorchSide,
    TorchTop,
    ChestSide,
    ChestTop,
    ChestFront,
    WhiteWool,
    RawBeef,
    Steak,
    RawPorkchop,
    CookedPorkchop,
    RawMutton,
    CookedMutton,
    Leather,
    // Armor: piece-major (helmet, chestplate, leggings, boots), then tier (leather, iron,
    // gold, diamond). `Tex::armor` relies on this order.
    LeatherHelmet,
    IronHelmet,
    GoldenHelmet,
    DiamondHelmet,
    LeatherChestplate,
    IronChestplate,
    GoldenChestplate,
    DiamondChestplate,
    LeatherLeggings,
    IronLeggings,
    GoldenLeggings,
    DiamondLeggings,
    LeatherBoots,
    IronBoots,
    GoldenBoots,
    DiamondBoots,
    // Animal coats: near-white and seamless, tiled at one texel per model unit and coloured
    // by each part's tint (alpha 255 = tinted; `Patches` leaves its light patches untinted).
    Fur,
    Patches,
    Wool,
    Hide,
    // The player's arm, seen in first person (alpha 0: never tinted).
    Skin,
    Sleeve,
    // Craft.
    Rocket,
    // The moon.
    Regolith,
    MoonRock,
    Basalt,
    Ash,
    Lava,
    // Vehicle parts.
    VehicleCore,
    HullPlate,
    SeatCloth,
    TankSide,
    PropellerFace,
    JetFace,
    RocketFace,
    OrbitalFace,
    WingSkin,
    FlapSkin,
    GyroFace,
    Tire,
    GearFace,
    // Factories.
    ConveyorBelt,
    MachineSide,
    MachineTop,
    GeneratorFront,
    GeneratorFrontLit,
    ElectricFront,
    ElectricFrontLit,
    // Item sprites (transparent around them).
    PoleIcon,
    Cable,
    Insulator,
    CableSkin,
    // Industry: oil, machines (idle and working), and what they make.
    OilShale,
    CleanPanel,
    MinerFront,
    MinerFrontLit,
    PumpFront,
    PumpFrontLit,
    RefineryFront,
    RefineryFrontLit,
    ArcFront,
    ArcFrontLit,
    AssemblerFront,
    AssemblerFrontLit,
    FabFront,
    FabFrontLit,
    AccumulatorFront,
    AccumulatorFrontLit,
    // (Lamp: white, its alpha all tint mask: lamps and glowing parts take their colour from a tint.)
    Lamp,
    RigSteel,
    Hazard,
    CrudeOil,
    FuelCan,
    Plastic,
    Rubber,
    Silicon,
    Wafer,
    SteelIngot,
    CopperWire,
    InsulatedWire,
    CircuitBoard,
    Battery,
    Led,
    MemoryChip,
    Cpu,
    AiChip,
    Monitor,
    Motherboard,
    PowerSupply,
    ComputerCase,
    Computer,
    // More vehicle parts, solar panels and doors (block faces: alpha 0, never tinted).
    VectorFace,
    AerospikeFace,
    IonFace,
    DrillFace,
    SolarCell,
    DoorPlanks,
    SlidingPanel,
    ShutterPanel,
    // Chemistry and nuclear power (`textures/chemistry.rs`). Block faces first (alpha 0).
    SulfurOre,
    LeadOre,
    FluoriteOre,
    SaltpeterOre,
    Concrete,
    SteelPlate,
    VitrifiedSide,
    VitrifiedTop,
    ControlPanel,
    ControlPanelLit,
    Cherenkov,
    HotCell,
    HotCellLit,
    TankPaint,
    AntimatterFace,
    // Pipes: the run, and the collars, flanges and fittings.
    PipeSteel,
    PipeFitting,
    // Fluids, as flasks (`Fluid::tex`, in `Fluid::ALL`'s order).
    FluidWater,
    FluidSteam,
    FluidSulfuricAcid,
    FluidNitricAcid,
    FluidHydrofluoricAcid,
    FluidHydrogen,
    FluidOxygen,
    FluidHexafluoride,
    FluidEnrichedHexafluoride,
    FluidDissolvedFuel,
    FluidLoadedSolvent,
    FluidLiquidWaste,
    FluidSolvent,
    // What the new chains make (item sprites).
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
    Magnet,
    Yellowcake,
    DepletedUranium,
    UraniumFuelRod,
    MoxFuelRod,
    SpentFuelRod,
    CooledSpentFuel,
    FuelHulls,
    RecoveredUranium,
    Plutonium,
    Antimatter,
}

impl Tex {
    /// The sprite of tool `kind` (0 pickaxe, 1 axe, 2 shovel, 3 sword) of `tier` (0 wood,
    /// 1 stone, 2 iron, 3 gold, 4 diamond).
    pub fn tool(kind: usize, tier: usize) -> Tex {
        Tex::ALL[Tex::WoodenPickaxe as usize + kind * 5 + tier]
    }

    /// The sprite of armor `piece` (0 helmet, 1 chestplate, 2 leggings, 3 boots) of `tier`
    /// (0 leather, 1 iron, 2 gold, 3 diamond).
    pub fn armor(piece: usize, tier: usize) -> Tex {
        Tex::ALL[Tex::LeatherHelmet as usize + piece * 4 + tier]
    }
}

/// The RGBA pixels of one texture (row-major, 16x16), e.g. to extrude a sprite into 3D.
pub fn pixels(t: Tex) -> Vec<[u8; 4]> {
    make(t).px
}

pub fn generate() -> TextureArrayData {
    for (i, t) in Tex::ALL.iter().enumerate() {
        debug_assert_eq!(*t as usize, i, "Tex::ALL must be in declaration order");
    }
    // (The interface's pictures come after every `Tex`: `IconTex::layer`.)
    let layers = Tex::ALL
        .iter()
        .map(|&t| make(t).into_bytes())
        .chain(IconTex::ALL.iter().map(|&i| icons::make(i).into_bytes()))
        .collect();
    TextureArrayData {
        size: TEX_SIZE as u32,
        layers,
    }
}

type Rgb = [u8; 3];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        super::noise::mix64(self.0)
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    fn signed(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone)]
struct Img {
    px: Vec<[u8; 4]>,
}

impl Img {
    fn get(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y * TEX_SIZE + x]
    }
    fn set(&mut self, x: usize, y: usize, rgb: Rgb, a: u8) {
        self.px[y * TEX_SIZE + x] = [rgb[0], rgb[1], rgb[2], a];
    }
    fn shade(&mut self, x: usize, y: usize, f: f32) {
        let p = self.get(x, y);
        self.set(x, y, scale([p[0], p[1], p[2]], f), p[3]);
    }
    fn into_bytes(self) -> Vec<u8> {
        self.px.into_iter().flatten().collect()
    }
}

fn scale(c: Rgb, f: f32) -> Rgb {
    c.map(|v| (v as f32 * f).clamp(0.0, 255.0) as u8)
}

/// Base colour with per-pixel brightness noise.
fn noisy(r: &mut Rng, base: Rgb, amount: f32, alpha: u8) -> Img {
    let mut img = Img {
        px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
    };
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            img.set(x, y, scale(base, 1.0 + r.signed() * amount), alpha);
        }
    }
    img
}

/// A metal plate with a dark seam and rivets at the corners.
fn panel(r: &mut Rng, base: Rgb) -> Img {
    let mut img = noisy(r, base, 0.025, 0);
    for i in 0..TEX_SIZE {
        for edge in [0, 15] {
            img.shade(i, edge, 0.72);
            img.shade(edge, i, 0.72);
        }
    }
    for (x, y) in [(2, 2), (13, 2), (2, 13), (13, 13)] {
        img.shade(x, y, 1.3);
    }
    img
}

/// A round face on a square plate: `rim` outside, `hole` inside, `hub` at the very centre.
fn disc(r: &mut Rng, rim: Rgb, hole: Rgb, hub: Rgb) -> Img {
    let mut img = noisy(r, rim, 0.04, 0);
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 2.5 {
                img.set(x, y, hub, 0);
            } else if d < 6.2 {
                img.set(x, y, hole, 0);
            }
        }
    }
    img
}

/// Scatter `count` little mineral clusters over `img`.
fn speckle(r: &mut Rng, img: &mut Img, count: usize, colors: &[Rgb], alpha: u8) {
    for _ in 0..count {
        let (cx, cy) = (2 + r.below(12), 2 + r.below(12));
        for _ in 0..3 + r.below(3) {
            let x = (cx as i32 + r.below(3) as i32 - 1).clamp(0, 15) as usize;
            let y = (cy as i32 + r.below(3) as i32 - 1).clamp(0, 15) as usize;
            let c = colors[r.below(colors.len())];
            img.set(x, y, scale(c, 1.0 + r.signed() * 0.08), alpha);
        }
    }
}

fn stone(r: &mut Rng) -> Img {
    let mut img = noisy(r, [128, 128, 130], 0.12, 0);
    for _ in 0..7 {
        let (x, y) = (r.below(15), r.below(16));
        img.shade(x, y, 0.82);
        img.shade(x + 1, y, 0.86);
    }
    img
}

fn dirt(r: &mut Rng) -> Img {
    let mut img = noisy(r, [121, 85, 58], 0.14, 0);
    for _ in 0..14 {
        let (x, y) = (r.below(16), r.below(16));
        img.shade(x, y, if r.f() < 0.5 { 0.75 } else { 1.18 });
    }
    img
}

fn sand(r: &mut Rng, base: Rgb) -> Img {
    let mut img = noisy(r, base, 0.06, 0);
    for _ in 0..10 {
        let (x, y) = (r.below(16), r.below(16));
        img.shade(x, y, 0.9);
    }
    img
}

fn terracotta(r: &mut Rng, base: Rgb) -> Img {
    noisy(r, base, 0.04, 0)
}

fn leaves(r: &mut Rng, brightness: f32) -> Img {
    let mut img = Img {
        px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
    };
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let g = if r.f() < 0.18 {
                70.0
            } else {
                150.0 + r.signed() * 45.0
            };
            let g = (g * brightness).clamp(0.0, 255.0) as u8;
            img.set(x, y, [g, g, g], 255);
        }
    }
    img
}

fn log_side(r: &mut Rng, base: Rgb) -> Img {
    let mut img = noisy(r, base, 0.06, 0);
    let stripes: Vec<f32> = (0..TEX_SIZE).map(|_| 0.8 + r.f() * 0.35).collect();
    for y in 0..TEX_SIZE {
        for (x, &s) in stripes.iter().enumerate() {
            img.shade(x, y, s);
        }
    }
    img
}

fn make(t: Tex) -> Img {
    let mut r = Rng((t as u64)
        .wrapping_mul(0x2545_f491_4f6c_dd1d)
        .wrapping_add(17));
    let r = &mut r;
    match t {
        Tex::Stone => stone(r),
        Tex::Deepstone => {
            let mut img = noisy(r, [74, 74, 86], 0.1, 0);
            for y in (1..TEX_SIZE).step_by(4) {
                for x in 0..TEX_SIZE {
                    if r.f() < 0.7 {
                        img.shade(x, y, 0.8);
                    }
                }
            }
            img
        }
        Tex::Mantle => {
            // Peridotite: dark green-grey rock with olivine crystals.
            let mut img = noisy(r, [66, 74, 58], 0.1, 0);
            speckle(r, &mut img, 7, &[[112, 138, 64], [92, 120, 52]], 0);
            speckle(r, &mut img, 3, &[[34, 36, 32]], 0);
            img
        }
        Tex::OuterCore => {
            // Molten iron: glowing orange with bright streaks and darker crust.
            let mut img = noisy(r, [226, 110, 28], 0.08, 0);
            for y in 0..TEX_SIZE {
                let wave = ((y as f32 * 0.9).sin() * 3.0) as i32;
                for x in 0..TEX_SIZE {
                    if (x as i32 + wave).rem_euclid(7) == 0 {
                        img.set(x, y, [255, 214, 90], 0);
                    }
                }
            }
            speckle(r, &mut img, 4, &[[120, 40, 16], [150, 56, 20]], 0);
            img
        }
        Tex::InnerCore => {
            // Iron-nickel: brushed metallic grey.
            let mut img = noisy(r, [148, 150, 158], 0.04, 0);
            for y in 0..TEX_SIZE {
                let f = 0.9 + r.f() * 0.2;
                for x in 0..TEX_SIZE {
                    img.shade(x, y, f);
                }
            }
            speckle(r, &mut img, 3, &[[214, 216, 226]], 0);
            img
        }
        Tex::Dirt => dirt(r),
        Tex::GrassTop => {
            let mut img = Img {
                px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
            };
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let g = (195.0 + r.signed() * 30.0) as u8;
                    img.set(x, y, [g, g, g], 255);
                }
            }
            img
        }
        Tex::GrassSide | Tex::SnowyGrassSide => {
            let mut img = dirt(r);
            for x in 0..TEX_SIZE {
                let depth = 3 + r.below(3);
                for y in 0..depth {
                    if t == Tex::GrassSide {
                        let g = (195.0 + r.signed() * 30.0) as u8;
                        img.set(x, y, [g, g, g], 255);
                    } else {
                        img.set(x, y, scale([240, 245, 250], 1.0 + r.signed() * 0.03), 0);
                    }
                }
            }
            img
        }
        Tex::Snow => noisy(r, [240, 244, 250], 0.03, 0),
        Tex::Ice => {
            let mut img = noisy(r, [150, 190, 245], 0.04, 0);
            for _ in 0..3 {
                let (mut x, mut y) = (r.below(16) as i32, r.below(16) as i32);
                for _ in 0..6 {
                    img.set(
                        x.clamp(0, 15) as usize,
                        y.clamp(0, 15) as usize,
                        [215, 235, 255],
                        0,
                    );
                    x += r.below(3) as i32 - 1;
                    y += 1;
                }
            }
            img
        }
        Tex::Water => {
            let mut img = noisy(r, [215, 225, 240], 0.03, 215);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    if (x + y * 3) % 11 == 0 {
                        img.set(x, y, [245, 250, 255], 200);
                    }
                }
            }
            img
        }
        Tex::Sand => sand(r, [219, 207, 163]),
        Tex::RedSand => sand(r, [190, 102, 48]),
        Tex::Sandstone => {
            let mut img = noisy(r, [214, 200, 150], 0.04, 0);
            for x in 0..TEX_SIZE {
                for y in 0..3 {
                    img.shade(x, y, 1.07);
                }
                img.shade(x, 11, 0.85);
                img.shade(x, 12, 0.9);
            }
            img
        }
        Tex::SandstoneTop => noisy(r, [218, 205, 156], 0.03, 0),
        Tex::Gravel => {
            let mut img = noisy(r, [120, 116, 112], 0.1, 0);
            for _ in 0..22 {
                let (x, y) = (r.below(15), r.below(15));
                let g = 80 + r.below(100) as u8;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    img.set(
                        x + dx,
                        y + dy,
                        [g, g.saturating_sub(4), g.saturating_sub(8)],
                        0,
                    );
                }
            }
            img
        }
        Tex::TerracottaOrange => terracotta(r, [162, 84, 38]),
        Tex::TerracottaRed => terracotta(r, [143, 61, 47]),
        Tex::TerracottaYellow => terracotta(r, [186, 133, 35]),
        Tex::TerracottaWhite => terracotta(r, [209, 178, 161]),
        Tex::TerracottaBrown => terracotta(r, [96, 62, 42]),
        Tex::OakLogSide => log_side(r, [104, 80, 50]),
        Tex::SpruceLogSide => log_side(r, [62, 44, 27]),
        Tex::LogTop => {
            let mut img = Img {
                px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
            };
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
                    let d = (dx * dx + dy * dy).sqrt();
                    let c = if d > 7.0 {
                        [92, 70, 44]
                    } else if (d as usize).is_multiple_of(2) {
                        [176, 143, 92]
                    } else {
                        [150, 118, 74]
                    };
                    img.set(x, y, scale(c, 1.0 + r.signed() * 0.04), 0);
                }
            }
            img
        }
        Tex::OakLeaves => leaves(r, 1.0),
        Tex::SpruceLeaves => leaves(r, 0.85),
        Tex::JungleLeaves => leaves(r, 1.1),
        Tex::CactusSide => {
            let mut img = noisy(r, [82, 132, 46], 0.06, 0);
            for y in 0..TEX_SIZE {
                for x in [0, 4, 8, 12] {
                    img.shade(x, y, 0.75);
                }
            }
            for _ in 0..10 {
                let (x, y) = (r.below(16), r.below(16));
                img.set(x, y, [225, 225, 190], 0);
            }
            img
        }
        Tex::CactusTop => {
            let mut img = noisy(r, [98, 150, 56], 0.05, 0);
            for i in 0..TEX_SIZE {
                for (x, y) in [(i, 0), (i, 15), (0, i), (15, i)] {
                    img.shade(x, y, 0.75);
                }
            }
            img
        }
        Tex::CoalOre => {
            let mut img = stone(r);
            speckle(r, &mut img, 6, &[[28, 28, 30], [45, 45, 48]], 0);
            img
        }
        Tex::IronOre => {
            let mut img = stone(r);
            speckle(r, &mut img, 5, &[[216, 175, 147], [190, 140, 110]], 0);
            img
        }
        Tex::CopperOre => {
            let mut img = stone(r);
            speckle(
                r,
                &mut img,
                5,
                &[[224, 120, 70], [190, 95, 55], [80, 170, 130]],
                0,
            );
            img
        }
        Tex::GoldOre => {
            let mut img = stone(r);
            speckle(r, &mut img, 4, &[[250, 220, 60], [230, 180, 30]], 0);
            img
        }
        Tex::DiamondOre => {
            let mut img = stone(r);
            speckle(r, &mut img, 4, &[[90, 230, 225], [180, 250, 245]], 0);
            img
        }
        Tex::BauxiteOre => {
            let mut img = noisy(r, [150, 82, 56], 0.08, 0);
            speckle(
                r,
                &mut img,
                9,
                &[[196, 116, 72], [110, 56, 40], [214, 160, 120]],
                0,
            );
            img
        }
        Tex::UraniumOre => {
            let mut img = stone(r);
            speckle(r, &mut img, 4, &[[170, 230, 40], [210, 250, 90]], 0);
            speckle(r, &mut img, 2, &[[40, 42, 34]], 0);
            img
        }
        Tex::IlmeniteSand => {
            let mut img = noisy(r, [150, 140, 122], 0.06, 0);
            for _ in 0..40 {
                let (x, y) = (r.below(16), r.below(16));
                let c = if r.f() < 0.7 {
                    [32, 32, 36]
                } else {
                    [130, 130, 145]
                };
                img.set(x, y, c, 0);
            }
            img
        }
        Tex::OakPlanks => planks(r, [162, 130, 78]),
        Tex::SprucePlanks => planks(r, [114, 84, 48]),
        Tex::Cobblestone => cobblestone(r),
        Tex::CraftingTop => {
            // Planks with a dark frame and a 3x3 grid burned in.
            let mut img = planks(r, [162, 130, 78]);
            for i in 0..TEX_SIZE {
                for (x, y) in [(i, 0), (i, 15), (0, i), (15, i)] {
                    img.set(x, y, [74, 52, 30], 0);
                }
                for g in [5, 10] {
                    if (2..14).contains(&i) {
                        img.set(g, i, [96, 70, 40], 0);
                        img.set(i, g, [96, 70, 40], 0);
                    }
                }
            }
            img
        }
        Tex::CraftingSide => {
            // Planks with a dark apron band under the table top, and a saw hanging on it.
            let mut img = planks(r, [150, 118, 70]);
            for x in 0..TEX_SIZE {
                for y in 0..3 {
                    img.set(x, y, scale([96, 70, 40], 1.0 + r.signed() * 0.06), 0);
                }
            }
            for x in 4..12 {
                img.set(x, 6, [190, 190, 196], 0);
                if x % 2 == 0 {
                    img.set(x, 7, [150, 150, 158], 0);
                }
            }
            for y in 5..8 {
                img.set(11, y, [92, 60, 30], 0);
                img.set(12, y, [92, 60, 30], 0);
            }
            img
        }
        Tex::FurnaceFront | Tex::FurnaceFrontLit => {
            let mut img = cobblestone(r);
            let lit = t == Tex::FurnaceFrontLit;
            for y in 7..14 {
                for x in 4..12 {
                    // An arched opening.
                    if y == 7 && !(5..11).contains(&x) {
                        continue;
                    }
                    let c = if !lit {
                        scale([28, 24, 22], 1.0 + r.signed() * 0.1)
                    } else if y >= 11 || r.f() < 0.3 {
                        [255, 200, 60]
                    } else {
                        [240, 110, 24]
                    };
                    img.set(x, y, c, 0);
                }
            }
            // Lintel and sill.
            for x in 3..13 {
                img.set(x, 6, [96, 96, 98], 0);
                img.set(x, 14, [84, 84, 86], 0);
            }
            img
        }
        Tex::FurnaceTop => {
            let mut img = noisy(r, [138, 138, 140], 0.06, 0);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let d = ((x as f32 - 7.5).powi(2) + (y as f32 - 7.5).powi(2)).sqrt();
                    if d < 3.2 {
                        img.set(x, y, [36, 32, 30], 0);
                    } else if d < 4.4 {
                        img.shade(x, y, 0.7);
                    }
                }
            }
            img
        }
        Tex::Stick => tool_sprite(None, |_, _| false),
        Tex::Coal => lump(r, [42, 42, 46], [80, 80, 88], 5.5),
        Tex::Charcoal => lump(r, [52, 40, 30], [86, 68, 50], 5.0),
        Tex::RawIron => lump(r, [196, 150, 120], [232, 198, 170], 5.0),
        Tex::RawCopper => lump(r, [196, 104, 60], [236, 150, 96], 5.0),
        Tex::RawGold => lump(r, [222, 176, 40], [255, 226, 100], 5.0),
        Tex::IronIngot => ingot([216, 216, 220]),
        Tex::CopperIngot => ingot([222, 128, 76]),
        Tex::GoldIngot => ingot([250, 214, 64]),
        Tex::Diamond => sprite(|x, y| {
            let (dx, dy) = (x - 7.5, y - 8.0);
            if dx.abs() + (dy * 0.85).abs() > 6.2 || dy < -4.5 {
                return None;
            }
            Some(if dy < -2.0 {
                [190, 255, 250]
            } else if dx + dy < 0.0 {
                [90, 230, 220]
            } else {
                [40, 170, 170]
            })
        }),
        Tex::Glass => {
            // Clear (alpha 0) with a pale frame and a few glints.
            let mut img = Img {
                px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
            };
            for i in 0..TEX_SIZE {
                for (x, y) in [(i, 0), (i, 15), (0, i), (15, i)] {
                    img.set(x, y, [222, 238, 244], 255);
                }
            }
            for (x, y) in [(3, 4), (4, 3), (5, 2), (10, 11), (11, 10), (12, 9), (4, 5)] {
                img.set(x, y, [245, 252, 255], 255);
            }
            img
        }
        Tex::TorchSide => {
            // Only the middle two columns are used: a stick with a flame on top. Doubles as
            // the torch's item sprite.
            let mut img = Img {
                px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
            };
            for y in 6..TEX_SIZE {
                for x in 7..9 {
                    let c = match y {
                        6 => [255, 236, 130],
                        7 => [255, 170, 40],
                        _ if x == 7 => [137, 103, 39],
                        _ => [104, 78, 29],
                    };
                    img.set(x, y, c, 255);
                }
            }
            img
        }
        Tex::TorchTop => noisy(r, [255, 214, 90], 0.08, 0),
        Tex::ChestSide | Tex::ChestTop | Tex::ChestFront => {
            let mut img = planks(r, [150, 108, 56]);
            for i in 0..TEX_SIZE {
                for (x, y) in [(i, 0), (i, 15), (0, i), (15, i)] {
                    img.set(x, y, [70, 46, 22], 0);
                }
            }
            if t == Tex::ChestFront {
                for y in 3..8 {
                    for x in 7..9 {
                        img.set(
                            x,
                            y,
                            if y == 3 {
                                [90, 90, 96]
                            } else {
                                [196, 196, 204]
                            },
                            0,
                        );
                    }
                }
            }
            img
        }
        Tex::WhiteWool => {
            let mut img = noisy(r, [232, 232, 228], 0.05, 0);
            for _ in 0..30 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 0.88);
            }
            img
        }
        Tex::RawBeef => meat(r, [196, 52, 48], [240, 214, 206]),
        Tex::Steak => meat(r, [120, 70, 40], [170, 120, 80]),
        Tex::RawPorkchop => meat(r, [238, 150, 150], [250, 232, 228]),
        Tex::CookedPorkchop => meat(r, [196, 140, 90], [232, 206, 160]),
        Tex::RawMutton => meat(r, [208, 78, 78], [236, 200, 196]),
        Tex::CookedMutton => meat(r, [140, 82, 52], [196, 150, 110]),
        Tex::Leather => sprite(|x, y| {
            let (dx, dy) = (x - 8.0, y - 8.0);
            let edge = 5.5 + ((x * 1.7).sin() + (y * 2.3).cos()) * 0.6;
            (dx.abs().max(dy.abs() * 1.1) < edge).then(|| {
                if (x as i32 + y as i32) % 5 == 0 {
                    [128, 76, 40]
                } else {
                    [156, 96, 52]
                }
            })
        }),
        Tex::Fur => fur(r),
        Tex::Patches => {
            // Fur with a few cream patches (untinted), wrapping around the tile edges.
            let mut img = fur(r);
            for _ in 0..3 {
                let (cx, cy, rad) = (r.f() * 16.0, r.f() * 16.0, 2.5 + r.f() * 2.0);
                let wobble = [r.f() * 6.3, r.f() * 6.3];
                for y in 0..TEX_SIZE {
                    for x in 0..TEX_SIZE {
                        let wrap = |d: f32| (d + 8.0).rem_euclid(16.0) - 8.0;
                        let (dx, dy) = (wrap(x as f32 - cx), wrap(y as f32 - cy));
                        let edge = rad
                            * (1.0
                                + 0.25 * (dy.atan2(dx) * 3.0 + wobble[0]).sin()
                                + 0.12 * (dy.atan2(dx) * 5.0 + wobble[1]).sin());
                        if dx * dx + dy * dy < edge * edge {
                            img.set(x, y, scale([238, 232, 222], 1.0 + r.signed() * 0.03), 0);
                        }
                    }
                }
            }
            img
        }
        Tex::Wool => {
            // Soft lumps: rounded tufts, slightly shaded on their lower side.
            let mut img = noisy(r, [236, 234, 228], 0.02, 255);
            for _ in 0..22 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 1.04);
                img.shade((x + 1) % 16, y, 1.03);
                img.shade(x, (y + 1) % 16, 0.93);
                img.shade((x + 1) % 16, (y + 1) % 16, 0.95);
            }
            img
        }
        Tex::Hide => noisy(r, [240, 236, 232], 0.025, 255),
        Tex::Skin => noisy(r, [204, 154, 118], 0.03, 0),
        Tex::Sleeve => {
            // A plain work shirt: dark slate cotton with a faint weave.
            let mut img = noisy(r, [74, 88, 100], 0.05, 0);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    if (x + y) % 4 == 0 {
                        img.shade(x, y, 0.94);
                    }
                }
            }
            img
        }
        Tex::Regolith => {
            // Fine grey dust with a scatter of small pebbles.
            let mut img = noisy(r, [138, 136, 132], 0.06, 0);
            for _ in 0..14 {
                let (x, y) = (r.below(16), r.below(16));
                let g = 100 + r.below(70) as u8;
                img.set(x, y, [g, g, g.saturating_sub(3)], 0);
            }
            img
        }
        Tex::MoonRock => {
            // Pale anorthosite with bright feldspar specks.
            let mut img = noisy(r, [176, 174, 170], 0.08, 0);
            for _ in 0..18 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 1.16);
            }
            for _ in 0..6 {
                let (x, y) = (r.below(15), r.below(16));
                img.shade(x, y, 0.82);
                img.shade(x + 1, y, 0.86);
            }
            img
        }
        Tex::Basalt => {
            // Dark mare basalt with small gas bubbles.
            let mut img = noisy(r, [72, 72, 76], 0.08, 0);
            for _ in 0..12 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 0.62);
            }
            img
        }
        Tex::Ash => {
            // Soft dark grey ash, with a few pale cinders.
            let mut img = noisy(r, [84, 82, 80], 0.1, 0);
            for _ in 0..10 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 1.35);
            }
            img
        }
        Tex::Lava => {
            // Bright molten rock broken by a few darker, cooling plates.
            let mut img = noisy(r, [255, 128, 28], 0.14, 0);
            for _ in 0..7 {
                let (x, y) = (r.below(14), r.below(14));
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    img.shade(x + dx, y + dy, 0.45);
                }
            }
            for _ in 0..12 {
                let (x, y) = (r.below(16), r.below(16));
                img.shade(x, y, 1.25);
            }
            img
        }
        Tex::VehicleCore => {
            // A dark housing with an amber arrow showing which way is forward.
            let mut img = panel(r, [58, 62, 72]);
            for (x, y) in [
                (7, 3),
                (8, 3),
                (6, 4),
                (7, 4),
                (8, 4),
                (9, 4),
                (5, 5),
                (6, 5),
                (7, 5),
                (8, 5),
                (9, 5),
                (10, 5),
            ] {
                img.set(x, y, [240, 170, 50], 0);
            }
            for y in 6..=12 {
                for x in 7..=8 {
                    img.set(x, y, [240, 170, 50], 0);
                }
            }
            img
        }
        Tex::HullPlate => panel(r, [186, 192, 200]),
        Tex::SeatCloth => {
            let mut img = noisy(r, [58, 84, 128], 0.06, 0);
            for i in 0..TEX_SIZE {
                img.shade(i, 5, 0.75);
                img.shade(i, 10, 0.75);
                img.shade(5, i, 0.8);
                img.shade(10, i, 0.8);
            }
            img
        }
        Tex::TankSide => {
            let mut img = noisy(r, [226, 122, 44], 0.05, 0);
            for x in 0..TEX_SIZE {
                for y in [0, 15] {
                    img.set(x, y, [150, 154, 162], 0);
                }
                for y in 3..=4 {
                    img.set(x, y, [240, 236, 226], 0);
                }
            }
            img
        }
        Tex::PropellerFace => disc(r, [108, 76, 46], [70, 72, 80], [50, 50, 56]),
        Tex::JetFace => disc(r, [150, 154, 162], [28, 30, 34], [96, 100, 110]),
        Tex::RocketFace => disc(r, [92, 94, 102], [24, 22, 22], [200, 96, 40]),
        Tex::OrbitalFace => disc(r, [206, 168, 72], [38, 36, 34], [236, 206, 120]),
        Tex::WingSkin => {
            let mut img = noisy(r, [232, 234, 236], 0.02, 0);
            for x in 0..TEX_SIZE {
                img.set(x, 13, [200, 58, 46], 0);
                img.set(x, 14, [200, 58, 46], 0);
            }
            img
        }
        Tex::FlapSkin => {
            let mut img = noisy(r, [232, 234, 236], 0.02, 0);
            for x in 0..TEX_SIZE {
                img.set(x, 2, [70, 74, 84], 0);
                for y in 11..16 {
                    if (x + y) % 6 < 3 {
                        img.set(x, y, [240, 170, 50], 0);
                    }
                }
            }
            img
        }
        Tex::GyroFace => disc(r, [60, 112, 92], [30, 44, 40], [150, 226, 190]),
        Tex::Tire => {
            let mut img = noisy(r, [34, 34, 38], 0.08, 0);
            for y in (0..TEX_SIZE).step_by(4) {
                for x in 0..TEX_SIZE {
                    img.shade(x, y, 0.55);
                }
            }
            img
        }
        Tex::GearFace => {
            let mut img = panel(r, [120, 124, 132]);
            for x in 0..TEX_SIZE {
                for y in 0..3 {
                    if (x + y) % 4 < 2 {
                        img.set(x, y, [226, 180, 40], 0);
                    }
                }
            }
            img
        }
        Tex::Rocket => sprite(|x, y| {
            let (x, y) = (x as i32, y as i32);
            let body = (6..=9).contains(&x) && (3..=11).contains(&y);
            let nose = (y == 1 && (7..=8).contains(&x)) || (y == 2 && (6..=9).contains(&x));
            let fin = ((x == 5 || x == 10) && (8..=12).contains(&y))
                || ((x == 4 || x == 11) && (10..=12).contains(&y));
            let flame = (7..=8).contains(&x) && (12..=14).contains(&y);
            if nose {
                Some([235, 120, 40])
            } else if body {
                Some(if (7..=8).contains(&x) && y == 5 {
                    [60, 100, 150]
                } else if y == 8 {
                    [235, 120, 40]
                } else if x == 9 {
                    [196, 198, 204]
                } else {
                    [240, 240, 236]
                })
            } else if fin {
                Some([200, 52, 40])
            } else if flame {
                Some(if y == 12 {
                    [255, 230, 140]
                } else {
                    [250, 150, 50]
                })
            } else {
                None
            }
        }),
        Tex::ConveyorBelt => {
            // Dark rubber between two metal edges, amber chevrons pointing the way it runs
            // (towards the top of the image, the model's front).
            let mut img = noisy(r, [46, 48, 54], 0.06, 0);
            for y in 0..TEX_SIZE {
                for x in [0, 1, 14, 15] {
                    img.set(x, y, scale([140, 146, 156], 1.0 + r.signed() * 0.04), 0);
                }
                if y % 4 == 0 {
                    for x in 2..14 {
                        img.shade(x, y, 0.7);
                    }
                }
            }
            for tip in [2, 9] {
                for d in 0..4usize {
                    for thick in 0..2 {
                        let y = tip + d + thick;
                        img.set(7 - d, y, [240, 170, 50], 0);
                        img.set(8 + d, y, [240, 170, 50], 0);
                    }
                }
            }
            img
        }
        Tex::MachineSide => {
            // Steel housing with vents.
            let mut img = panel(r, [96, 102, 112]);
            for y in [5, 7, 9, 11] {
                for x in 4..12 {
                    img.set(x, y, [44, 46, 52], 0);
                }
            }
            img
        }
        Tex::MachineTop => panel(r, [112, 118, 128]),
        Tex::GeneratorFront | Tex::GeneratorFrontLit => {
            // A round firebox hatch behind a grille; it glows while burning.
            let lit = t == Tex::GeneratorFrontLit;
            let mut img = panel(r, [78, 82, 92]);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let (dx, dy) = (x as f32 - 7.5, y as f32 - 8.5);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 5.2 {
                        let glow = if lit {
                            scale([255, 150, 50], 1.0 - d / 9.0 + r.signed() * 0.08)
                        } else {
                            scale([34, 30, 30], 1.0 + r.signed() * 0.1)
                        };
                        let bar = y % 3 == 0;
                        img.set(x, y, if bar { [70, 72, 80] } else { glow }, 0);
                    } else if d < 6.2 {
                        img.set(x, y, [150, 154, 162], 0);
                    }
                }
            }
            img
        }
        Tex::ElectricFront | Tex::ElectricFrontLit => {
            // A window onto the heating coil, red hot while it works.
            let lit = t == Tex::ElectricFrontLit;
            let mut img = panel(r, [96, 102, 112]);
            for y in 4..12 {
                for x in 3..13 {
                    let coil = y % 2 == 0 && (4..12).contains(&x);
                    let c = match (lit, coil) {
                        (true, true) => [255, 196, 90],
                        (true, false) => [230, 96, 36],
                        (false, true) => [168, 98, 60],
                        (false, false) => [30, 34, 40],
                    };
                    img.set(x, y, scale(c, 1.0 + r.signed() * 0.05), 0);
                }
            }
            for x in 3..13 {
                img.set(x, 3, [150, 154, 162], 0);
                img.set(x, 12, [150, 154, 162], 0);
            }
            img
        }
        Tex::PoleIcon => sprite(|x, y| {
            let (x, y) = (x as i32, y as i32);
            let post = (7..=8).contains(&x) && (3..=15).contains(&y);
            let arm = (2..=13).contains(&x) && (4..=5).contains(&y);
            let insulator = (x == 3 || x == 12) && (1..=3).contains(&y);
            if insulator {
                Some([206, 226, 210])
            } else if post || arm {
                Some(if x == 8 || y == 5 {
                    [104, 78, 46]
                } else {
                    [136, 104, 62]
                })
            } else {
                None
            }
        }),
        Tex::Cable => sprite(|x, y| {
            // A coil of insulated copper wire, its copper end showing.
            let (dx, dy) = ((x - 8.0) / 1.15, y - 8.0);
            let d = (dx * dx + dy * dy).sqrt();
            if (3.0..6.0).contains(&d) {
                let ring = (d * 2.0) as i32 % 2 == 0;
                Some(if ring { [52, 50, 48] } else { [34, 32, 30] })
            } else if (12.0..15.0).contains(&x) && (9.0..11.0).contains(&y) {
                Some([222, 128, 76])
            } else {
                None
            }
        }),
        Tex::Insulator => noisy(r, [204, 222, 208], 0.04, 0),
        Tex::CableSkin => noisy(r, [40, 38, 36], 0.06, 0),
        _ if t as usize >= Tex::OilShale as usize => industry::make(t, r),
        _ if t as usize >= Tex::LeatherHelmet as usize => {
            let i = t as usize - Tex::LeatherHelmet as usize;
            let (piece, tier) = (i / 4, i % 4);
            let color: Rgb = [
                [160, 100, 58],
                [216, 216, 222],
                [250, 214, 64],
                [86, 224, 212],
            ][tier];
            sprite(|x, y| {
                armor_shape(piece, x, y).then(|| {
                    if x + y < 14.0 {
                        scale(color, 1.15)
                    } else {
                        color
                    }
                })
            })
        }
        _ => {
            let tool = t as usize - Tex::WoodenPickaxe as usize;
            let (kind, tier) = (tool / 5, tool % 5);
            let head: [Rgb; 5] = [
                [150, 116, 65],
                [128, 128, 130],
                [222, 222, 226],
                [250, 222, 70],
                [80, 226, 210],
            ];
            let shape: fn(f32, f32) -> bool = match kind {
                // Pickaxe: a curved head across the top of the handle.
                0 => |s, t| (s - 11.4 + 0.08 * t * t).abs() < 1.1 && t.abs() < 6.6,
                // Axe: a rounded blade on one side.
                1 => |s, t| {
                    ((s - 11.6) / 3.1).powi(2) + ((t + 2.2) / 2.9).powi(2) < 1.0
                        || ((9.2..14.0).contains(&s) && (-1.2..0.9).contains(&t))
                },
                // Shovel: an oval blade on the end.
                2 => |s, t| ((s - 12.6) / 3.3).powi(2) + (t / 2.3).powi(2) < 1.0,
                // Sword: a long tapering blade with a cross guard.
                _ => |s, t| {
                    ((4.6..16.6).contains(&s) && t.abs() < 1.2f32.min((16.8 - s) * 0.9))
                        || ((3.4..4.6).contains(&s) && t.abs() < 3.0)
                },
            };
            tool_sprite(Some((head[tier], kind == 3)), shape)
        }
    }
}

/// A cut of meat (a lump with a streak of fat).
fn meat(r: &mut Rng, base: Rgb, fat: Rgb) -> Img {
    let mut img = lump(r, base, scale(base, 1.15), 5.0);
    for i in 0..6 {
        let (x, y) = (5 + i, 10 - i / 2);
        if img.get(x, y)[3] != 0 {
            img.set(x, y, fat, 255);
        }
    }
    img
}

/// Armor piece silhouettes: 0 helmet, 1 chestplate, 2 leggings, 3 boots.
fn armor_shape(piece: usize, x: f32, y: f32) -> bool {
    let (x, y) = (x as i32, y as i32);
    match piece {
        0 => {
            let dome = (3..11).contains(&y) && (x - 7).abs() + (x - 8).abs() <= 11;
            let face = (7..11).contains(&y) && (4..12).contains(&x);
            dome && (2..14).contains(&x) && !face
        }
        1 => {
            let shoulders = (2..6).contains(&y) && (1..15).contains(&x);
            let body = (5..14).contains(&y) && (3..13).contains(&x);
            let neck = (2..4).contains(&y) && (6..10).contains(&x);
            (shoulders || body) && !neck
        }
        2 => {
            let waist = (2..5).contains(&y) && (3..13).contains(&x);
            let legs = (5..14).contains(&y) && ((3..7).contains(&x) || (9..13).contains(&x));
            waist || legs
        }
        _ => {
            let shafts = (6..12).contains(&y) && ((2..6).contains(&x) || (10..14).contains(&x));
            let toes = (11..14).contains(&y) && ((1..7).contains(&x) || (9..15).contains(&x));
            shafts || toes
        }
    }
}

fn planks(r: &mut Rng, base: Rgb) -> Img {
    let mut img = noisy(r, base, 0.05, 0);
    for board in 0..4 {
        let joint = r.below(16);
        for x in 0..TEX_SIZE {
            for dy in 0..4 {
                let y = board * 4 + dy;
                if dy == 3 {
                    img.shade(x, y, 0.62);
                } else if x == joint {
                    img.shade(x, y, 0.7);
                } else if r.f() < 0.12 {
                    img.shade(x, y, 0.9);
                }
            }
        }
    }
    img
}

/// Rounded stones with dark mortar between them (Voronoi cells on a wrapping 16x16 tile).
fn cobblestone(r: &mut Rng) -> Img {
    let seeds: Vec<(f32, f32, f32)> = (0..10)
        .map(|_| (r.f() * 16.0, r.f() * 16.0, 0.8 + r.f() * 0.4))
        .collect();
    let mut img = noisy(r, [124, 124, 126], 0.06, 0);
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let mut d = [(f32::MAX, 1.0f32); 2];
            for &(sx, sy, shade) in &seeds {
                let dx = (x as f32 + 0.5 - sx).abs();
                let dy = (y as f32 + 0.5 - sy).abs();
                let (dx, dy) = (dx.min(16.0 - dx), dy.min(16.0 - dy));
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < d[0].0 {
                    d[1] = d[0];
                    d[0] = (dist, shade);
                } else if dist < d[1].0 {
                    d[1] = (dist, shade);
                }
            }
            if d[1].0 - d[0].0 < 0.9 {
                img.set(x, y, [70, 70, 72], 0);
            } else {
                img.shade(x, y, d[0].1);
            }
        }
    }
    img
}

/// A sprite from a per-pixel colour function (pixel centres), with a dark outline.
fn sprite(color: impl Fn(f32, f32) -> Option<Rgb>) -> Img {
    let mut img = Img {
        px: vec![[0; 4]; TEX_SIZE * TEX_SIZE],
    };
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            if let Some(c) = color(x as f32 + 0.5, y as f32 + 0.5) {
                img.set(x, y, c, 255);
            }
        }
    }
    outline(&mut img);
    img
}

/// Gives every transparent pixel next to an opaque one a dark version of its colour.
fn outline(img: &mut Img) {
    let src = img.clone();
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            if src.get(x, y)[3] != 0 {
                continue;
            }
            let neighbours = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                ((0..16).contains(&nx) && (0..16).contains(&ny))
                    .then(|| src.get(nx as usize, ny as usize))
                    .filter(|p| p[3] != 0)
            });
            if let Some(p) = neighbours.iter().flatten().next() {
                img.set(x, y, scale([p[0], p[1], p[2]], 0.3), 255);
            }
        }
    }
}

/// A stick from the bottom left towards the top right, plus an optional head:
/// `shape(s, t)` with s along the handle and t across it (positive towards bottom right).
fn tool_sprite(head: Option<(Rgb, bool)>, shape: fn(f32, f32) -> bool) -> Img {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    sprite(|x, y| {
        let (px, py) = (x - 2.0, y - 14.0);
        let (s, t) = ((px - py) * h, (px + py) * h);
        let shade = |c: Rgb| {
            if t < -0.35 {
                scale(c, 1.18)
            } else if t > 0.6 {
                scale(c, 0.78)
            } else {
                c
            }
        };
        if let Some((color, _)) = head
            && shape(s, t)
        {
            return Some(shade(color));
        }
        let length = match head {
            None => 15.2,
            Some((_, true)) => 3.4,
            Some(_) => 12.5,
        };
        ((0.3..length).contains(&s) && t.abs() < 0.8).then(|| shade([137, 103, 39]))
    })
}

/// A rough lump (coal, raw ore) with lighter flecks.
fn lump(r: &mut Rng, base: Rgb, fleck: Rgb, radius: f32) -> Img {
    let bumps: Vec<f32> = (0..8).map(|_| r.f() * 1.4).collect();
    let flecks: Vec<(usize, usize)> = (0..9).map(|_| (3 + r.below(10), 3 + r.below(10))).collect();
    let noise: Vec<f32> = (0..256).map(|_| r.signed()).collect();
    let mut img = sprite(|x, y| {
        let (dx, dy) = (x - 8.0, y - 8.0);
        let a = dy.atan2(dx) / std::f32::consts::TAU * 8.0 + 4.0;
        let bump = bumps[(a as usize) % 8];
        if (dx * dx + dy * dy).sqrt() > radius + bump {
            return None;
        }
        let i = (y as usize) * 16 + x as usize;
        Some(scale(base, 1.0 + noise[i] * 0.08 - dy * 0.02))
    });
    for (x, y) in flecks {
        if img.get(x, y)[3] != 0 && img.get(x, y)[0] != scale(base, 0.3)[0] {
            img.set(x, y, fleck, 255);
        }
    }
    img
}

/// A metal bar seen from slightly above.
fn ingot(base: Rgb) -> Img {
    sprite(|x, y| {
        let (x, y) = (x as i32, y as i32);
        let top = (6..8).contains(&y) && (4 + (7 - y)..=12 + (7 - y)).contains(&x);
        let front = (8..12).contains(&y) && (2..=11).contains(&x);
        let side = (6..11).contains(&y) && (12..=13).contains(&x) && x - 12 <= y - 6;
        if top {
            Some(scale(base, 1.15))
        } else if front {
            Some(if y == 11 { scale(base, 0.8) } else { base })
        } else if side {
            Some(scale(base, 0.7))
        } else {
            None
        }
    })
}

/// Short animal hair, near white (the tint gives the colour): faint strands along the tile.
fn fur(r: &mut Rng) -> Img {
    let mut img = noisy(r, [238, 234, 228], 0.035, 255);
    for _ in 0..26 {
        let (x, y) = (r.below(16), r.below(16));
        for i in 0..2 + r.below(2) {
            img.shade(x, (y + i) % 16, 0.9);
        }
    }
    for _ in 0..10 {
        let (x, y) = (r.below(16), r.below(16));
        img.shade(x, y, 1.05);
    }
    img
}

/// Writes sprites eight times their size on a grey ground, seven to a row, as a BMP (to look
/// at them: see the ignored `write_*` tests).
#[cfg(test)]
fn write_sheet(path: &str, sprites: &[Vec<[u8; 4]>]) {
    const ZOOM: usize = 8;
    let columns = 7;
    let rows = sprites.len().div_ceil(columns);
    let cell = TEX_SIZE * ZOOM + 8;
    let (w, h) = (columns * cell, rows * cell);
    let mut rgb = vec![[96u8, 102, 116]; w * h];
    for (i, px) in sprites.iter().enumerate() {
        let (ox, oy) = ((i % columns) * cell + 4, (i / columns) * cell + 4);
        for y in 0..TEX_SIZE * ZOOM {
            for x in 0..TEX_SIZE * ZOOM {
                let p = px[(y / ZOOM) * TEX_SIZE + x / ZOOM];
                if p[3] >= 128 {
                    rgb[(oy + y) * w + ox + x] = [p[0], p[1], p[2]];
                }
            }
        }
    }
    let row = (w * 3).div_ceil(4) * 4;
    let mut file = Vec::new();
    let size = 54 + row * h;
    file.extend(b"BM");
    file.extend((size as u32).to_le_bytes());
    file.extend([0u8; 4]);
    file.extend(54u32.to_le_bytes());
    file.extend(40u32.to_le_bytes());
    file.extend((w as i32).to_le_bytes());
    file.extend((h as i32).to_le_bytes());
    file.extend(1u16.to_le_bytes());
    file.extend(24u16.to_le_bytes());
    file.extend([0u8; 24]);
    for y in (0..h).rev() {
        let start = file.len();
        for x in 0..w {
            let [r, g, b] = rgb[y * w + x];
            file.extend([b, g, r]);
        }
        file.resize(start + row, 0);
    }
    std::fs::create_dir_all("target").unwrap();
    std::fs::write(path, file).unwrap();
}
