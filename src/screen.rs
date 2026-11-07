//! Inventory screens: the player inventory (suit slots and a 2x2 assembly grid), the
//! workbench, the furnace, storage chests and the creative catalog.
//!
//! The look is Astro Block's minimal flat style: no panel, the world just dims behind a grid
//! of faintly outlined cells; sections carry small lowercase labels; progress is a thin line.
//! Layout is in "GUI pixels" (`Ui::scale` screen pixels each) on a 20-pixel cell pitch. The
//! click rules live in `survival`.
//!
//! Screens move (see `ScreenAnim`): they fade and rise in when opened and sink out when
//! closed, hovered cells light up and fade out behind the mouse, the details card fades in,
//! slots pop when items land in them, and a ready result glows softly.

use crate::{
    anim::{Pop, approach, ease_out},
    furnace::Furnace,
    inventory::{HOTBAR, SLOTS, Slot},
    items::{Item, ItemStack, ToolKind},
    render::UiVertex,
    ui::Ui,
    util::FastMap,
    world::chunk::BlockCell,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Worn armor (head, body, legs, feet).
    Armor(usize),
    /// A slot of the open chest.
    Chest(usize),
}

/// Cell pitch and content width, in GUI pixels.
const CELL: f32 = 20.0;
const WIDTH: f32 = 9.0 * CELL;
const COLUMNS: usize = 9;

pub struct Layout {
    /// Content box in screen pixels (clicks outside it drop the held stack), and screen
    /// pixels per GUI pixel.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub s: f32,
    /// (slot, cell top-left in GUI pixels).
    pub slots: Vec<(SlotId, f32, f32)>,
    /// Section labels (GUI pixels).
    labels: Vec<(f32, f32, &'static str)>,
}

impl Layout {
    pub fn new(screen: Screen, width: f32, height: f32, s: f32, catalog: usize) -> Self {
        let mut slots = Vec::new();
        let mut labels = Vec::new();
        let row = |slots: &mut Vec<_>, ids: &mut dyn Iterator<Item = SlotId>, y: f32| {
            for (i, id) in ids.enumerate() {
                slots.push((
                    id,
                    (i % COLUMNS) as f32 * CELL,
                    y + (i / COLUMNS) as f32 * CELL,
                ));
            }
        };

        // The screen's own section on top.
        let top = match screen {
            Screen::Inventory => {
                labels.push((0.0, 0.0, "suit"));
                for i in 0..4 {
                    slots.push((SlotId::Armor(i), 0.0, 10.0 + i as f32 * CELL));
                }
                labels.push((80.0, 0.0, "assemble"));
                for i in 0..4 {
                    let (c, r) = ((i % 2) as f32, (i / 2) as f32);
                    slots.push((SlotId::Grid(i), 80.0 + c * CELL, 20.0 + r * CELL));
                }
                slots.push((SlotId::Output, 160.0, 30.0));
                92.0
            }
            Screen::Crafting(_) => {
                labels.push((20.0, 0.0, "workbench"));
                for i in 0..9 {
                    let (c, r) = ((i % 3) as f32, (i / 3) as f32);
                    slots.push((SlotId::Grid(i), 20.0 + c * CELL, 10.0 + r * CELL));
                }
                slots.push((SlotId::Output, 140.0, 30.0));
                72.0
            }
            Screen::Furnace(_) => {
                labels.push((40.0, 0.0, "furnace"));
                slots.push((SlotId::FurnaceIn, 40.0, 10.0));
                slots.push((SlotId::FurnaceFuel, 40.0, 50.0));
                slots.push((SlotId::FurnaceOut, 120.0, 30.0));
                72.0
            }
            Screen::Chest(_) => {
                labels.push((0.0, 0.0, "storage"));
                row(&mut slots, &mut (0..CHEST_SLOTS).map(SlotId::Chest), 10.0);
                72.0
            }
            Screen::Creative => {
                labels.push((0.0, 0.0, "catalog"));
                row(&mut slots, &mut (0..catalog).map(SlotId::Catalog), 10.0);
                10.0 + catalog.div_ceil(COLUMNS) as f32 * CELL + 2.0
            }
        };

        // The player's own items below: the backpack, then the hotbar a little apart.
        let hotbar_y = if screen == Screen::Creative {
            top + 6.0
        } else {
            labels.push((0.0, top + 4.0, "backpack"));
            row(
                &mut slots,
                &mut (HOTBAR..SLOTS).map(SlotId::Inv),
                top + 14.0,
            );
            top + 14.0 + 3.0 * CELL + 6.0
        };
        row(&mut slots, &mut (0..HOTBAR).map(SlotId::Inv), hotbar_y);
        let content_h = hotbar_y + CELL;

        let (w, h) = (WIDTH * s, content_h * s);
        Self {
            x: ((width - w) / 2.0).round(),
            y: ((height - h) / 2.0).round(),
            w,
            h,
            s,
            slots,
            labels,
        }
    }

    /// Screen position of a GUI-pixel position.
    fn at(&self, gx: f32, gy: f32) -> (f32, f32) {
        (self.x + gx * self.s, self.y + gy * self.s)
    }

    /// The slot under the mouse.
    pub fn slot_at(&self, mx: f32, my: f32) -> Option<SlotId> {
        self.slots.iter().find_map(|&(id, gx, gy)| {
            let (x, y) = self.at(gx, gy);
            let size = CELL * self.s;
            (mx >= x && mx < x + size && my >= y && my < y + size).then_some(id)
        })
    }

    /// Whether the mouse is over the screen's content (with a small margin).
    pub fn contains(&self, mx: f32, my: f32) -> bool {
        let m = 8.0 * self.s;
        mx >= self.x - m && mx < self.x + self.w + m && my >= self.y - m && my < self.y + self.h + m
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

const LABEL: [u8; 4] = [190, 200, 210, 170];
const OUTLINE: [u8; 4] = [255, 255, 255, 38];
const ACCENT: [u8; 4] = [240, 196, 120, 150];
const HOVER: [u8; 4] = [255, 255, 255, 28];
const LINE: [u8; 4] = [235, 240, 245, 235];
const FAINT: [u8; 4] = [255, 255, 255, 45];

/// Seconds a screen takes to open and to close.
const OPEN_TIME: f32 = 0.18;
const CLOSE_TIME: f32 = 0.12;
/// How far (GUI pixels) the content rises while opening and sinks while closing.
const RISE: f32 = 8.0;

/// The screens' motion state, carried from frame to frame.
#[derive(Default)]
pub struct ScreenAnim {
    time: f32,
    /// The screen drawn last frame, and how far it has opened (0..1, linear).
    shown: Option<Screen>,
    open: f32,
    /// Hover glow per cell (0..1).
    hover: FastMap<SlotId, f32>,
    pops: FastMap<SlotId, Pop>,
    held: Pop,
    /// The cell whose details card is up, and the card's fade (0..1).
    card: (Option<SlotId>, f32),
    /// Last frame's screen (dim first, then content), replayed while it closes.
    closing: Vec<UiVertex>,
}

impl ScreenAnim {
    /// Called on frames without an open screen: the last one fades and sinks out.
    pub fn draw_closing(&mut self, ui: &mut Ui, dt: f32) {
        if self.shown.take().is_some() {
            // Close from wherever opening had got to.
            self.open = ease_out(self.open);
        }
        if self.closing.is_empty() {
            return;
        }
        self.open -= dt / CLOSE_TIME;
        if self.open <= 0.0 {
            self.closing.clear();
            self.open = 0.0;
            return;
        }
        let e = self.open * self.open;
        let start = ui.vertices.len();
        ui.vertices.extend_from_slice(&self.closing);
        ui.fade_from(start, e, 0.0, 0.0);
        ui.fade_from(start + 4, 1.0, 0.0, (1.0 - e) * RISE * 0.5 * ui.scale);
    }
}

pub fn draw(
    ui: &mut Ui,
    screen: Screen,
    layout: &Layout,
    view: &View,
    anim: &mut ScreenAnim,
    dt: f32,
) {
    let s = layout.s;
    anim.time += dt;
    if anim.shown != Some(screen) {
        // Freshly opened: start from the bottom, and don't pop what's already there.
        if anim.shown.is_none() && anim.closing.is_empty() {
            anim.open = 0.0;
        }
        anim.shown = Some(screen);
        anim.pops.clear();
        anim.hover.clear();
        anim.card = (None, 0.0);
        anim.held = Pop::new(&view.cursor);
    }
    anim.open = (anim.open + dt / OPEN_TIME).min(1.0);

    // The world dims; there is no panel.
    let start = ui.vertices.len();
    ui.rect(0.0, 0.0, ui.width, ui.height, [4, 6, 10, 165]);
    for &(gx, gy, text) in &layout.labels {
        let (lx, ly) = layout.at(gx, gy);
        ui.text_plain(lx, ly, text, LABEL);
    }

    // Thin lines: assembly input -> result, and the furnace's fuel and progress.
    let line = |ui: &mut Ui, gx: f32, gy: f32, len: f32, fill: f32| {
        let (x, y) = layout.at(gx, gy);
        ui.rect(x, y, len * s, s, FAINT);
        if fill > 0.0 {
            ui.rect(x, y, len * s * fill.min(1.0), s, LINE);
        }
    };
    match screen {
        Screen::Inventory => line(ui, 124.0, 40.0, 32.0, 0.0),
        Screen::Crafting(_) => line(ui, 84.0, 40.0, 52.0, 0.0),
        Screen::Furnace(_) => {
            let f = view.furnace.copied().unwrap_or_default();
            line(ui, 64.0, 40.0, 52.0, f.cook / crate::furnace::COOK_TIME);
            let burn = if f.lit() && f.burn_total > 0.0 {
                f.burn_left / f.burn_total
            } else {
                0.0
            };
            line(ui, 42.0, 40.0, 16.0, burn);
        }
        Screen::Creative | Screen::Chest(_) => {}
    }

    // Hover glow: quick to light up, slower to fade, so it trails behind the mouse.
    let hovered = layout.slot_at(view.mouse.0, view.mouse.1);
    if let Some(id) = hovered {
        anim.hover.entry(id).or_insert(0.0);
    }
    anim.hover.retain(|&id, glow| {
        let on = hovered == Some(id);
        *glow = approach(
            *glow,
            if on { 1.0 } else { 0.0 },
            if on { 30.0 } else { 9.0 },
            dt,
        );
        on || *glow > 0.01
    });

    // Cells and their items.
    let pulse = 0.5 + 0.5 * (anim.time * 4.0).sin();
    for &(id, gx, gy) in &layout.slots {
        let (cx, cy) = layout.at(gx + 1.0, gy + 1.0);
        let size = (CELL - 2.0) * s;
        let stack = match id {
            SlotId::Catalog(i) => view.catalog.get(i).map(|&item| ItemStack::one(item)),
            _ => (view.slot)(id),
        };
        let glow = anim.hover.get(&id).copied().unwrap_or(0.0);
        let result = matches!(id, SlotId::Output | SlotId::FurnaceOut);
        let outline = if result {
            // A ready result breathes softly.
            let ready = if stack.is_some() { 60.0 * pulse } else { 0.0 };
            with_alpha(ACCENT, ACCENT[3] as f32 + ready + 60.0 * glow)
        } else {
            with_alpha(OUTLINE, OUTLINE[3] as f32 + 50.0 * glow)
        };
        ui.frame(cx, cy, size, size, s, outline);
        if glow > 0.0 {
            ui.rect(
                cx,
                cy,
                size,
                size,
                with_alpha(HOVER, HOVER[3] as f32 * glow),
            );
        }
        let (ix, iy) = layout.at(gx + 2.0, gy + 2.0);
        // Items pop when they land in a cell (not the catalog's, which never change).
        let scale = match id {
            SlotId::Catalog(_) => 1.0,
            _ => anim
                .pops
                .entry(id)
                .or_insert_with(|| Pop::new(&stack))
                .observe(&stack, dt),
        };
        if let Some(stack) = stack {
            let icon = (16.0 * s * scale).round();
            let off = ((icon - 16.0 * s) / 2.0).round();
            ui.item(ix - off, iy - off, icon, &stack);
        } else if let SlotId::Armor(i) = id {
            // An empty suit slot says what goes there.
            let text = ["hd", "bd", "lg", "ft"][i];
            let tw = ui.text_width(text);
            let (_, ch) = ui.char_size();
            ui.text_plain(
                cx + (size - tw) / 2.0,
                cy + (size - ch) / 2.0 + s,
                text,
                FAINT,
            );
        }
    }

    // The held stack follows the mouse (popping when picked up); otherwise the hovered
    // item's details, on a card that fades and slides in.
    let (mx, my) = view.mouse;
    let held = anim.held.observe(&view.cursor, dt);
    let card = if view.cursor.is_some() { None } else { hovered };
    if card != anim.card.0 {
        anim.card = (card, 0.0);
    }
    anim.card.1 = approach(anim.card.1, 1.0, 20.0, dt);
    if let Some(stack) = view.cursor {
        let icon = (16.0 * s * held).round();
        ui.item(mx - icon / 2.0, my - icon / 2.0, icon, &stack);
    } else if let Some(id) = hovered {
        let stack = match id {
            SlotId::Catalog(i) => view.catalog.get(i).map(|&item| ItemStack::one(item)),
            _ => (view.slot)(id),
        };
        if let Some(stack) = stack {
            let card_start = ui.vertices.len();
            tooltip(ui, mx + 12.0 * s, my + 8.0 * s, &stack);
            let a = ease_out(anim.card.1);
            ui.fade_from(card_start, a, 0.0, (1.0 - a) * 3.0 * s);
        }
    }

    // Kept for the closing fade, then faded and lifted in while opening (the dim only fades).
    anim.closing.clear();
    anim.closing.extend_from_slice(&ui.vertices[start..]);
    let e = ease_out(anim.open);
    ui.fade_from(start, e, 0.0, 0.0);
    ui.fade_from(start + 4, 1.0, 0.0, (1.0 - e) * RISE * s);
}

/// A colour with a new alpha (clamped to 0..255).
fn with_alpha(color: [u8; 4], alpha: f32) -> [u8; 4] {
    [color[0], color[1], color[2], alpha.clamp(0.0, 255.0) as u8]
}

/// An item's details: its name, then a few short facts, on a dark translucent card.
pub fn tooltip(ui: &mut Ui, x: f32, y: f32, stack: &ItemStack) {
    let s = ui.scale;
    let name = stack.item.name().to_lowercase();
    let mut facts: Vec<String> = Vec::new();
    if let Some((kind, tier)) = stack.item.tool() {
        let (damage, speed) = stack.item.attack();
        facts.push(format!("damage {damage}   {speed}/s"));
        if kind != ToolKind::Sword {
            facts.push(format!("dig speed x{}", tier.speed()));
        }
    }
    if let Some((piece, tier)) = stack.item.armor() {
        facts.push(format!("plating +{}", tier.points(piece)));
    }
    if let Some(max) = stack.item.max_durability() {
        facts.push(format!("wear {} / {}", max - stack.wear.min(max), max));
    }
    if let Some((food, _)) = stack.item.food() {
        facts.push(format!("food +{food}"));
    }
    if let Some(fuel) = stack.item.fuel() {
        facts.push(format!("burns {fuel} s"));
    }
    if let Some(result) = stack.item.smelted() {
        facts.push(format!("smelts to {}", result.name().to_lowercase()));
    }
    let (_, ch) = ui.char_size();
    let width = std::iter::once(&name)
        .chain(&facts)
        .map(|l| ui.text_width(l))
        .fold(0.0, f32::max)
        + 10.0 * s;
    let height = (1 + facts.len()) as f32 * (ch + 2.0 * s) + 8.0 * s;
    let x = x.min(ui.width - width - 2.0 * s);
    let y = y.max(2.0 * s).min(ui.height - height - 2.0 * s);
    ui.rect(x, y, width, height, [8, 10, 14, 230]);
    ui.rect(x, y, s, height, ACCENT);
    let mut ly = y + 4.0 * s;
    ui.text_plain(x + 5.0 * s, ly, &name, LINE);
    for fact in &facts {
        ly += ch + 2.0 * s;
        ui.text_plain(x + 5.0 * s, ly, fact, LABEL);
    }
}
