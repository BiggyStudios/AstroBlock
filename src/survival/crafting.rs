//! Crafting recipes (Minecraft's shaped recipes, mirrored shapes allowed).

use std::sync::OnceLock;

use crate::{
    survival::items::{ArmorPiece, ArmorTier, Item, ItemStack, Tier, ToolKind},
    world::block::Block,
};

/// One cell of a recipe pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ingredient {
    Exactly(Item),
    /// Any kind of planks.
    Planks,
    /// Coal or charcoal.
    Coal,
}

impl Ingredient {
    fn matches(self, item: Item) -> bool {
        match self {
            Ingredient::Exactly(i) => i == item,
            Ingredient::Planks => {
                matches!(item, Item::Block(Block::OakPlanks | Block::SprucePlanks))
            }
            Ingredient::Coal => matches!(item, Item::Coal | Item::Charcoal),
        }
    }
}

struct Recipe {
    /// Rows of the pattern (None = must be empty).
    pattern: Vec<Vec<Option<Ingredient>>>,
    result: ItemStack,
}

fn recipe(rows: &[&str], keys: &[(char, Ingredient)], result: Item, count: u8) -> Recipe {
    let pattern = rows
        .iter()
        .map(|row| {
            row.chars()
                .map(|c| keys.iter().find(|(k, _)| *k == c).map(|(_, i)| *i))
                .collect()
        })
        .collect();
    Recipe {
        pattern,
        result: ItemStack::new(result, count),
    }
}

fn recipes() -> &'static [Recipe] {
    static RECIPES: OnceLock<Vec<Recipe>> = OnceLock::new();
    RECIPES.get_or_init(|| {
        use Ingredient::*;
        let block = |b| Exactly(Item::Block(b));
        let stick = ('S', Exactly(Item::Stick));
        let mut all = vec![
            recipe(
                &["L"],
                &[('L', block(Block::OakLog))],
                Item::Block(Block::OakPlanks),
                4,
            ),
            recipe(
                &["L"],
                &[('L', block(Block::SpruceLog))],
                Item::Block(Block::SprucePlanks),
                4,
            ),
            recipe(&["P", "P"], &[('P', Planks)], Item::Stick, 4),
            recipe(
                &["PP", "PP"],
                &[('P', Planks)],
                Item::Block(Block::CraftingTable),
                1,
            ),
            recipe(
                &["CCC", "C C", "CCC"],
                &[('C', block(Block::Cobblestone))],
                Item::Block(Block::Furnace),
                1,
            ),
            recipe(
                &["##", "##"],
                &[('#', block(Block::Sand))],
                Item::Block(Block::Sandstone),
                1,
            ),
            recipe(
                &["C", "S"],
                &[('C', Coal), stick],
                Item::Block(Block::Torch),
                4,
            ),
            recipe(
                &["PPP", "P P", "PPP"],
                &[('P', Planks)],
                Item::Block(Block::Chest),
                1,
            ),
        ];
        all.extend(vehicle_recipes());
        all.extend(factory_recipes());
        for tier in Tier::ALL {
            let material = match tier {
                Tier::Wood => Planks,
                Tier::Stone => block(Block::Cobblestone),
                Tier::Iron => Exactly(Item::IronIngot),
                Tier::Gold => Exactly(Item::GoldIngot),
                Tier::Diamond => Exactly(Item::Diamond),
            };
            let keys = [('M', material), stick];
            let tool = |kind| Item::Tool(kind, tier);
            all.push(recipe(
                &["MMM", " S ", " S "],
                &keys,
                tool(ToolKind::Pickaxe),
                1,
            ));
            all.push(recipe(&["MM", "MS", " S"], &keys, tool(ToolKind::Axe), 1));
            all.push(recipe(&["M", "S", "S"], &keys, tool(ToolKind::Shovel), 1));
            all.push(recipe(&["M", "M", "S"], &keys, tool(ToolKind::Sword), 1));
        }
        for tier in ArmorTier::ALL {
            let material = Exactly(match tier {
                ArmorTier::Leather => Item::Leather,
                ArmorTier::Iron => Item::IronIngot,
                ArmorTier::Gold => Item::GoldIngot,
                ArmorTier::Diamond => Item::Diamond,
            });
            let keys = [('M', material)];
            let piece = |p| Item::Armor(p, tier);
            all.push(recipe(&["MMM", "M M"], &keys, piece(ArmorPiece::Helmet), 1));
            all.push(recipe(
                &["M M", "MMM", "MMM"],
                &keys,
                piece(ArmorPiece::Chestplate),
                1,
            ));
            all.push(recipe(
                &["MMM", "M M", "M M"],
                &keys,
                piece(ArmorPiece::Leggings),
                1,
            ));
            all.push(recipe(&["M M", "M M"], &keys, piece(ArmorPiece::Boots), 1));
        }
        all
    })
}

/// What vehicles are made of.
fn vehicle_recipes() -> Vec<Recipe> {
    use Ingredient::*;
    let iron = ('I', Exactly(Item::IronIngot));
    let copper = ('C', Exactly(Item::CopperIngot));
    let gold = ('G', Exactly(Item::GoldIngot));
    let leather = ('L', Exactly(Item::Leather));
    let coal = ('O', Coal);
    let planks = ('P', Planks);
    let furnace = ('F', Exactly(Item::Block(Block::Furnace)));
    let wheel = ('W', Exactly(Item::Block(Block::Wheel)));
    let made = |b| Item::Block(b);
    vec![
        recipe(
            &["IGI", "GCG", "IGI"],
            &[iron, gold, copper],
            made(Block::VehicleCore),
            1,
        ),
        recipe(&["II", "II"], &[iron], made(Block::Hull), 8),
        recipe(
            &["L  ", "LLL", "I I"],
            &[leather, iron],
            made(Block::Seat),
            1,
        ),
        recipe(
            &["III", "IOI", "III"],
            &[iron, coal],
            made(Block::FuelTank),
            1,
        ),
        recipe(
            &["P P", " I ", "P P"],
            &[planks, iron],
            made(Block::Propeller),
            1,
        ),
        recipe(
            &["III", "CFC", "III"],
            &[iron, copper, furnace],
            made(Block::JetEngine),
            1,
        ),
        recipe(
            &["I I", "IFI", "C C"],
            &[iron, copper, furnace],
            made(Block::RocketEngine),
            1,
        ),
        recipe(
            &["G G", "GFG", " I "],
            &[gold, furnace, iron],
            made(Block::OrbitalEngine),
            1,
        ),
        recipe(&["PPP", "III"], &[planks, iron], made(Block::Wing), 2),
        recipe(
            &["PPP", "ICI"],
            &[planks, iron, copper],
            made(Block::Flap),
            2,
        ),
        recipe(
            &["ICI", "CGC", "ICI"],
            &[iron, copper, gold],
            made(Block::Gyro),
            1,
        ),
        recipe(
            &[" L ", "LIL", " L "],
            &[leather, iron],
            made(Block::Wheel),
            2,
        ),
        recipe(&["I", "W"], &[iron, wheel], made(Block::LandingGear), 1),
        recipe(&["WIW"], &[wheel, iron], made(Block::LargeWheel), 1),
        recipe(
            &[" L ", "LXL", " L "],
            &[leather, ('X', Exactly(Item::Block(Block::LargeWheel)))],
            made(Block::GiantWheel),
            1,
        ),
        recipe(
            &[" G ", "CRC"],
            &[
                gold,
                copper,
                ('R', Exactly(Item::Block(Block::RocketEngine))),
            ],
            made(Block::VectorEngine),
            1,
        ),
        recipe(
            &["GIG", " R "],
            &[gold, iron, ('R', Exactly(Item::Block(Block::RocketEngine)))],
            made(Block::Aerospike),
            1,
        ),
        recipe(
            &["IBI", "G G"],
            &[iron, ('B', Exactly(Item::CircuitBoard)), gold],
            made(Block::IonEngine),
            1,
        ),
        recipe(
            &["III", "IPI", " C "],
            &[
                iron,
                ('P', Exactly(Item::Tool(ToolKind::Pickaxe, Tier::Iron))),
                copper,
            ],
            made(Block::VehicleDrill),
            1,
        ),
        // Doors, for buildings and vehicles alike.
        recipe(&["PP", "PI", "PP"], &[planks, iron], made(Block::Door), 2),
        recipe(
            &["IG", "IG", "IG"],
            &[iron, ('G', Exactly(Item::Block(Block::Glass)))],
            made(Block::SlidingDoor),
            2,
        ),
        recipe(
            &["III", "CBC", "III"],
            &[iron, copper, ('B', Exactly(Item::CircuitBoard))],
            made(Block::BigDoor),
            1,
        ),
    ]
}

/// What factories are made of: iron, copper and furnaces.
fn factory_recipes() -> Vec<Recipe> {
    use Ingredient::*;
    let iron = ('I', Exactly(Item::IronIngot));
    let copper = ('C', Exactly(Item::CopperIngot));
    let stick = ('S', Exactly(Item::Stick));
    let furnace = ('F', Exactly(Item::Block(Block::Furnace)));
    let glass = ('G', Exactly(Item::Block(Block::Glass)));
    let made = |b| Item::Block(b);
    vec![
        recipe(&["III", "C C"], &[iron, copper], made(Block::Conveyor), 4),
        recipe(
            &["CSC", " S ", " S "],
            &[copper, stick],
            made(Block::PowerPole),
            2,
        ),
        recipe(&["CCC"], &[copper], Item::Cable, 8),
        recipe(
            &["ICI", "IFI", "III"],
            &[iron, copper, furnace],
            made(Block::CoalGenerator),
            1,
        ),
        recipe(
            &["III", "CFC", "CCC"],
            &[iron, copper, furnace],
            made(Block::ElectricFurnace),
            1,
        ),
        // Industry. The first machines need only metal, glass and furnaces; the chip fab
        // needs circuit boards out of an assembler, the accumulator batteries.
        recipe(
            &["ICI", "IPI", "I I"],
            &[
                iron,
                copper,
                ('P', Exactly(Item::Tool(ToolKind::Pickaxe, Tier::Iron))),
            ],
            made(Block::Miner),
            1,
        ),
        recipe(
            &["III", "CIC", "I I"],
            &[iron, copper],
            made(Block::OilPump),
            1,
        ),
        recipe(
            &["IGI", "IFI", "CCC"],
            &[iron, glass, furnace, copper],
            made(Block::Refinery),
            1,
        ),
        recipe(
            &["O O", "IFI", "III"],
            &[('O', Coal), iron, furnace],
            made(Block::ArcFurnace),
            1,
        ),
        recipe(
            &["ICI", "CTC", "ICI"],
            &[
                iron,
                copper,
                ('T', Exactly(Item::Block(Block::CraftingTable))),
            ],
            made(Block::Assembler),
            1,
        ),
        recipe(
            &["GBG", "IAI", "III"],
            &[
                glass,
                ('B', Exactly(Item::CircuitBoard)),
                ('A', Exactly(Item::Block(Block::Assembler))),
                iron,
            ],
            made(Block::ChipFab),
            1,
        ),
        recipe(
            &["IBI", "BCB", "IBI"],
            &[iron, ('B', Exactly(Item::Battery)), copper],
            made(Block::Accumulator),
            1,
        ),
        recipe(
            &["GGG", "SSS", "ICI"],
            &[glass, ('S', Exactly(Item::Silicon)), iron, copper],
            made(Block::SolarPanel),
            2,
        ),
    ]
}

/// The result of the items in a crafting grid `width` cells wide (row-major), if any.
pub fn craft(grid: &[Option<Item>], width: usize) -> Option<ItemStack> {
    let height = grid.len() / width;
    let filled = |x: usize, y: usize| grid[y * width + x].is_some();
    let xs: Vec<usize> = (0..width)
        .filter(|&x| (0..height).any(|y| filled(x, y)))
        .collect();
    let ys: Vec<usize> = (0..height)
        .filter(|&y| (0..width).any(|x| filled(x, y)))
        .collect();
    let (&x0, &x1, &y0, &y1) = (xs.first()?, xs.last()?, ys.first()?, ys.last()?);
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let item = |x: usize, y: usize| grid[(y0 + y) * width + x0 + x];

    recipes().iter().find_map(|r| {
        if r.pattern.len() != h || r.pattern[0].len() != w {
            return None;
        }
        let fits = |mirror: bool| {
            (0..h).all(|y| {
                (0..w).all(|x| {
                    let want = r.pattern[y][if mirror { w - 1 - x } else { x }];
                    match (want, item(x, y)) {
                        (None, None) => true,
                        (Some(ing), Some(have)) => ing.matches(have),
                        _ => false,
                    }
                })
            })
        };
        (fits(false) || fits(true)).then_some(r.result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cells: &[Option<Item>]) -> Vec<Option<Item>> {
        cells.to_vec()
    }

    const LOG: Option<Item> = Some(Item::Block(Block::OakLog));
    const PLANK: Option<Item> = Some(Item::Block(Block::OakPlanks));
    const SPRUCE: Option<Item> = Some(Item::Block(Block::SprucePlanks));
    const STICK: Option<Item> = Some(Item::Stick);
    const COBBLE: Option<Item> = Some(Item::Block(Block::Cobblestone));

    #[test]
    fn basic_recipes_anywhere_in_the_grid() {
        // A log anywhere in a 2x2 grid gives 4 planks.
        let r = craft(&grid(&[None, None, None, LOG]), 2).unwrap();
        assert_eq!((r.item, r.count), (Item::Block(Block::OakPlanks), 4));
        // Two planks (mixed kinds) stacked give sticks.
        let r = craft(&grid(&[None, PLANK, None, SPRUCE]), 2).unwrap();
        assert_eq!((r.item, r.count), (Item::Stick, 4));
        // Side by side they don't.
        assert_eq!(craft(&grid(&[PLANK, SPRUCE, None, None]), 2), None);
        // A full 2x2 of planks is a crafting table.
        let r = craft(&grid(&[PLANK, PLANK, PLANK, PLANK]), 2).unwrap();
        assert_eq!(r.item, Item::Block(Block::CraftingTable));
    }

    #[test]
    fn tools_and_furnace_need_the_3x3_grid() {
        let pickaxe = [COBBLE, COBBLE, COBBLE, None, STICK, None, None, STICK, None];
        let r = craft(&pickaxe, 3).unwrap();
        assert_eq!(r.item, Item::Tool(ToolKind::Pickaxe, Tier::Stone));
        // The axe works mirrored too.
        let axe = [None, PLANK, PLANK, None, STICK, PLANK, None, STICK, None];
        let r = craft(&axe, 3).unwrap();
        assert_eq!(r.item, Item::Tool(ToolKind::Axe, Tier::Wood));
        let furnace = [
            COBBLE, COBBLE, COBBLE, COBBLE, None, COBBLE, COBBLE, COBBLE, COBBLE,
        ];
        assert_eq!(
            craft(&furnace, 3).unwrap().item,
            Item::Block(Block::Furnace)
        );
        // A sword in the left column.
        let sword = [PLANK, None, None, PLANK, None, None, STICK, None, None];
        let r = craft(&sword, 3).unwrap();
        assert_eq!(r.item, Item::Tool(ToolKind::Sword, Tier::Wood));
        // Junk doesn't craft.
        assert_eq!(
            craft(&[STICK, COBBLE, None, None, None, None, None, None, LOG], 3),
            None
        );
    }
}
