//! Inventory screens: the player inventory (with its 2x2 crafting grid), the crafting table,
//! the furnace, chests and the creative catalog. Layout and drawing follow Minecraft's (176 GUI
//! pixels wide, 18-pixel slots); the click rules live in `survival`.

use crate::{
    furnace::Furnace,
    inventory::{HOTBAR, SLOTS, Slot},
    items::{Item, ItemStack, Tier, ToolKind},
    ui::Ui,
    world::{chunk::BlockCell, textures::Tex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Inventory,
    /// Creative mode's item catalog.
    Creative,
    Crafting(BlockCell),
    Furnace(BlockCell),
    Chest(BlockCell),
}

/// Slots in a chest (three rows).
pub const CHEST_SLOTS: usize = 27;

impl Screen {
    /// Crafting grid width (0: none).
    pub fn grid_width(self) -> usize {
        match self {
            Screen::Inventory => 2,
            Screen::Crafting(_) => 3,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotId {
    /// Player inventory slot (0..9 hotbar, 9..36 main).
    Inv(usize),
    /// Crafting grid cell (row-major, `grid_width` wide).
    Grid(usize),
    /// Crafting result.
    Output,
    FurnaceIn,
    FurnaceFuel,
    FurnaceOut,
    /// Creative catalog entry.
    Catalog(usize),
    /// Worn armor (helmet, chestplate, leggings, boots).
    Armor(usize),
    /// A slot of the open chest.
    Chest(usize),
}

/// GUI size in GUI pixels.
const GUI_W: f32 = 176.0;
const GUI_H: f32 = 166.0;
const CATALOG_COLUMNS: usize = 9;

pub struct Layout {
    /// Panel rectangle in screen pixels, and screen pixels per GUI pixel.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub s: f32,
    /// (slot, item position in GUI pixels, big result slot).
    pub slots: Vec<(SlotId, f32, f32, bool)>,
}

impl Layout {
    pub fn new(screen: Screen, width: f32, height: f32, s: f32, catalog: usize) -> Self {
        let mut slots = Vec::new();
        let catalog_rows = catalog.div_ceil(CATALOG_COLUMNS);
        let h = match screen {
            Screen::Creative => 18.0 + catalog_rows as f32 * 18.0 + 32.0,
            _ => GUI_H,
        };
        // Player inventory: three rows plus the hotbar (creative: the hotbar only).
        if screen == Screen::Creative {
            for i in 0..catalog {
                let (col, row) = (i % CATALOG_COLUMNS, i / CATALOG_COLUMNS);
                slots.push((
                    SlotId::Catalog(i),
                    8.0 + col as f32 * 18.0,
                    18.0 + row as f32 * 18.0,
                    false,
                ));
            }
            for i in 0..HOTBAR {
                slots.push((SlotId::Inv(i), 8.0 + i as f32 * 18.0, h - 24.0, false));
            }
        } else {
            for i in HOTBAR..SLOTS {
                let (col, row) = ((i - HOTBAR) % 9, (i - HOTBAR) / 9);
                slots.push((
                    SlotId::Inv(i),
                    8.0 + col as f32 * 18.0,
                    84.0 + row as f32 * 18.0,
                    false,
                ));
            }
            for i in 0..HOTBAR {
                slots.push((SlotId::Inv(i), 8.0 + i as f32 * 18.0, 142.0, false));
            }
        }
        match screen {
            Screen::Inventory => {
                for i in 0..4 {
                    let (col, row) = (i % 2, i / 2);
                    slots.push((
                        SlotId::Grid(i),
                        88.0 + col as f32 * 18.0,
                        26.0 + row as f32 * 18.0,
                        false,
                    ));
                }
                slots.push((SlotId::Output, 148.0, 36.0, true));
                for i in 0..4 {
                    slots.push((SlotId::Armor(i), 8.0, 8.0 + i as f32 * 18.0, false));
                }
            }
            Screen::Crafting(_) => {
                for i in 0..9 {
                    let (col, row) = (i % 3, i / 3);
                    slots.push((
                        SlotId::Grid(i),
                        30.0 + col as f32 * 18.0,
                        17.0 + row as f32 * 18.0,
                        false,
                    ));
                }
                slots.push((SlotId::Output, 124.0, 35.0, true));
            }
            Screen::Furnace(_) => {
                slots.push((SlotId::FurnaceIn, 56.0, 17.0, false));
                slots.push((SlotId::FurnaceFuel, 56.0, 53.0, false));
                slots.push((SlotId::FurnaceOut, 116.0, 35.0, true));
            }
            Screen::Chest(_) => {
                for i in 0..CHEST_SLOTS {
                    let (col, row) = (i % 9, i / 9);
                    slots.push((
                        SlotId::Chest(i),
                        8.0 + col as f32 * 18.0,
                        18.0 + row as f32 * 18.0,
                        false,
                    ));
                }
            }
            Screen::Creative => {}
        }
        let (w, h) = (GUI_W * s, h * s);
        Self {
            x: ((width - w) / 2.0).round(),
            y: ((height - h) / 2.0).round(),
            w,
            h,
            s,
            slots,
        }
    }

    /// Screen position of a GUI-pixel position.
    fn at(&self, gx: f32, gy: f32) -> (f32, f32) {
        (self.x + gx * self.s, self.y + gy * self.s)
    }

    /// The slot under the mouse.
    pub fn slot_at(&self, mx: f32, my: f32) -> Option<SlotId> {
        self.slots.iter().find_map(|&(id, gx, gy, big)| {
            let pad = if big { 5.0 } else { 1.0 };
            let (x, y) = self.at(gx - pad, gy - pad);
            let size = (16.0 + 2.0 * pad) * self.s;
            (mx >= x && mx < x + size && my >= y && my < y + size).then_some(id)
        })
    }

    pub fn contains(&self, mx: f32, my: f32) -> bool {
        mx >= self.x && mx < self.x + self.w && my >= self.y && my < self.y + self.h
    }
}

/// What the screen shows.
pub struct View<'a> {
    pub slot: &'a dyn Fn(SlotId) -> Slot,
    pub cursor: Slot,
    pub mouse: (f32, f32),
    pub furnace: Option<&'a Furnace>,
    pub catalog: &'a [Item],
}

const PANEL: [u8; 4] = [198, 198, 198, 255];
const LIGHT: [u8; 4] = [255, 255, 255, 255];
const SHADOW: [u8; 4] = [85, 85, 85, 255];
const SLOT: [u8; 4] = [139, 139, 139, 255];
const SLOT_DARK: [u8; 4] = [55, 55, 55, 255];
const LABEL: [u8; 4] = [64, 64, 64, 255];

pub fn draw(ui: &mut Ui, screen: Screen, layout: &Layout, view: &View) {
    let s = layout.s;
    // Dim the world behind.
    ui.rect(0.0, 0.0, ui.width, ui.height, [0, 0, 0, 110]);
    // A raised grey panel.
    let (x, y, w, h) = (layout.x, layout.y, layout.w, layout.h);
    ui.rect(x, y, w, h, [0, 0, 0, 255]);
    ui.rect(x + s, y + s, w - 2.0 * s, h - 2.0 * s, LIGHT);
    ui.rect(x + 3.0 * s, y + 3.0 * s, w - 4.0 * s, h - 4.0 * s, SHADOW);
    ui.rect(x + 3.0 * s, y + 3.0 * s, w - 6.0 * s, h - 6.0 * s, PANEL);

    let title = match screen {
        Screen::Inventory => "Crafting",
        Screen::Creative => "Creative Inventory",
        Screen::Crafting(_) => "Crafting",
        Screen::Furnace(_) => "Furnace",
        Screen::Chest(_) => "Chest",
    };
    let label = |ui: &mut Ui, gx: f32, gy: f32, text: &str| {
        let (lx, ly) = layout.at(gx, gy);
        // Labels are drawn without the shadow, Minecraft-style.
        ui.text_plain(lx, ly, text, LABEL);
    };
    let title_x = if screen == Screen::Inventory {
        88.0
    } else {
        8.0
    };
    label(ui, title_x, 6.0, title);
    // (The player inventory has its armor column there instead, as in Minecraft.)
    if !matches!(screen, Screen::Creative | Screen::Inventory) {
        label(ui, 8.0, 73.0, "Inventory");
    }

    // Arrows and the furnace flame.
    let arrow = |ui: &mut Ui, gx: f32, gy: f32, fill: f32| {
        let (ax, ay) = layout.at(gx, gy);
        let size = 16.0 * s;
        ui.sprite(ax, ay, size, Tex::Arrow, [139, 139, 139, 255]);
        if fill > 0.0 {
            ui.sprite_part(
                (ax, ay, size * fill, size),
                Tex::Arrow,
                ([0.0, 0.0], [fill, 1.0]),
                [255, 255, 255, 255],
            );
        }
    };
    match screen {
        Screen::Inventory => arrow(ui, 126.0, 36.0, 0.0),
        Screen::Crafting(_) => arrow(ui, 92.0, 35.0, 0.0),
        Screen::Furnace(_) => {
            let f = view.furnace.copied().unwrap_or_default();
            let cook = f.cook / crate::furnace::COOK_TIME;
            arrow(ui, 82.0, 35.0, cook);
            let (fx, fy) = layout.at(56.0, 36.0);
            let size = 16.0 * s;
            ui.sprite(fx, fy, size, Tex::FlameOff, [255, 255, 255, 255]);
            if f.lit() && f.burn_total > 0.0 {
                // The flame shrinks from the top as the fuel burns.
                let left = (f.burn_left / f.burn_total).clamp(0.0, 1.0);
                ui.sprite_part(
                    (fx, fy + size * (1.0 - left), size, size * left),
                    Tex::Flame,
                    ([0.0, 1.0 - left], [1.0, 1.0]),
                    [255, 255, 255, 255],
                );
            }
        }
        Screen::Creative | Screen::Chest(_) => {}
    }

    // Slots and their items.
    let hovered = layout.slot_at(view.mouse.0, view.mouse.1);
    for &(id, gx, gy, big) in &layout.slots {
        let pad = if big { 5.0 } else { 1.0 };
        let (bx, by) = layout.at(gx - pad, gy - pad);
        let size = (16.0 + 2.0 * pad) * s;
        ui.rect(bx, by, size, size, LIGHT);
        ui.rect(bx, by, size - s, size - s, SLOT_DARK);
        ui.rect(bx + s, by + s, size - 2.0 * s, size - 2.0 * s, SLOT);
        let (ix, iy) = layout.at(gx, gy);
        let stack = match id {
            SlotId::Catalog(i) => view.catalog.get(i).map(|&item| ItemStack::one(item)),
            _ => (view.slot)(id),
        };
        if let Some(stack) = stack {
            ui.item(ix, iy, 16.0 * s, &stack);
        } else if let SlotId::Armor(i) = id {
            // An empty armor slot shows a faint outline of what goes there.
            ui.sprite(ix, iy, 16.0 * s, Tex::armor(i, 1), [70, 70, 70, 160]);
        }
        if hovered == Some(id) {
            ui.rect(ix, iy, 16.0 * s, 16.0 * s, [255, 255, 255, 110]);
        }
    }

    // The held stack follows the mouse; otherwise the hovered item's tooltip.
    let (mx, my) = view.mouse;
    if let Some(stack) = view.cursor {
        ui.item(mx - 8.0 * s, my - 8.0 * s, 16.0 * s, &stack);
    } else if let Some(id) = hovered {
        let stack = match id {
            SlotId::Catalog(i) => view.catalog.get(i).map(|&item| ItemStack::one(item)),
            _ => (view.slot)(id),
        };
        if let Some(stack) = stack {
            tooltip(ui, mx + 12.0 * s, my - 12.0 * s, &stack);
        }
    }
}

/// Minecraft-style tooltip: the name, then what the item does.
pub fn tooltip(ui: &mut Ui, x: f32, y: f32, stack: &ItemStack) {
    let s = ui.scale;
    let mut lines: Vec<(String, [u8; 4])> = vec![(stack.item.name(), [255, 255, 255, 255])];
    let grey = [170, 170, 170, 255];
    let blue = [85, 85, 255, 255];
    if let Some((kind, tier)) = stack.item.tool() {
        let (damage, speed) = stack.item.attack();
        lines.push((String::new(), grey));
        lines.push(("When in main hand:".into(), grey));
        lines.push((format!(" {damage} Attack Damage"), [0, 168, 0, 255]));
        lines.push((format!(" {speed} Attack Speed"), [0, 168, 0, 255]));
        if kind != ToolKind::Sword {
            lines.push((format!(" {}x Mining Speed", tier.speed()), blue));
            let level = match tier {
                Tier::Wood | Tier::Gold => "stone, coal",
                Tier::Stone => "iron, copper",
                Tier::Iron => "gold, diamonds",
                Tier::Diamond => "everything",
            };
            if kind == ToolKind::Pickaxe {
                lines.push((format!(" Mines {level}"), blue));
            }
        }
        lines.push((
            format!(
                "Durability: {} / {}",
                tier.durability() - stack.wear,
                tier.durability()
            ),
            grey,
        ));
    }
    if let Some((piece, tier)) = stack.item.armor() {
        lines.push((String::new(), grey));
        lines.push(("When worn:".into(), grey));
        lines.push((format!(" +{} Armor", tier.points(piece)), blue));
        let max = tier.durability(piece);
        lines.push((format!("Durability: {} / {}", max - stack.wear, max), grey));
    }
    if let Some((food, _)) = stack.item.food() {
        lines.push((format!("Restores {food} hunger"), grey));
    }
    if let Some(fuel) = stack.item.fuel() {
        lines.push((format!("Fuel: {} s", fuel), grey));
    }
    if let Some(result) = stack.item.smelted() {
        lines.push((format!("Smelts into {}", result.name()), grey));
    }
    let (_, ch) = ui.char_size();
    let width = lines
        .iter()
        .map(|(l, _)| ui.text_width(l))
        .fold(0.0, f32::max)
        + 8.0 * s;
    let height = lines.len() as f32 * (ch + 2.0 * s) + 6.0 * s;
    let x = x.min(ui.width - width - 2.0 * s);
    let y = y.max(2.0 * s).min(ui.height - height - 2.0 * s);
    ui.rect(x, y, width, height, [16, 0, 16, 240]);
    ui.frame(
        x + s,
        y + s,
        width - 2.0 * s,
        height - 2.0 * s,
        s,
        [80, 0, 255, 120],
    );
    let mut ly = y + 4.0 * s;
    for (line, color) in &lines {
        ui.text(x + 4.0 * s, ly, line, *color);
        ly += ch + 2.0 * s;
    }
}
