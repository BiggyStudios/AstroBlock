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
    factory::{self, Machine, MachineKind, PowerView, recipes},
    render::UiVertex,
    survival::{
        furnace::Furnace,
        inventory::{HOTBAR, SLOTS, Slot},
        items::{Category, Item, ItemStack, ToolKind},
    },
    ui::{
        Ui,
        anim::{Pop, approach, ease_out},
    },
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
    /// A factory machine: a generator's fuel, what a furnace smelts, what a recipe machine
    /// makes (picked here), what a miner digs up.
    Machine(BlockCell, MachineKind),
    /// The power network a pole or accumulator is on.
    Power(BlockCell),
}

/// Slots in a chest (three rows).
pub const CHEST_SLOTS: usize = 27;

impl Screen {
    /// The block it belongs to (closed when that goes, or when you walk away).
    pub fn cell(self) -> Option<BlockCell> {
        match self {
            Screen::Crafting(c)
            | Screen::Furnace(c)
            | Screen::Chest(c)
            | Screen::Machine(c, _)
            | Screen::Power(c) => Some(c),
            Screen::Inventory | Screen::Creative => None,
        }
    }

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
    /// A recipe machine's ingredient slot (in recipe order).
    MachineIn(usize),
    /// One of what a recipe machine can make (click to pick it).
    Recipe(usize),
    /// A machine's second output (what a recipe also makes).
    Extra,
}

/// GUI pixels per gauge line on a machine's screen (its tanks).
const GAUGE_H: f32 = 12.0;

/// Cell pitch and content width, in GUI pixels.
const CELL: f32 = 20.0;
const WIDTH: f32 = 9.0 * CELL;
const COLUMNS: usize = 9;

/// Rows of the catalog shown at once (the wheel scrolls the rest).
const CATALOG_ROWS: usize = 6;
/// The catalog's tabs, down its left side (GUI pixels, x relative to the grid).
const TAB_X: f32 = -74.0;
const TAB_W: f32 = 64.0;
const TAB_H: f32 = 14.0;

/// What the creative catalog shows: its tab, how many items that has, the rows scrolled past.
#[derive(Clone, Copy, Debug, Default)]
pub struct Catalog {
    pub tab: Category,
    pub len: usize,
    pub scroll: usize,
}

/// How many rows a catalog of `len` items can be scrolled down.
pub fn catalog_scroll_limit(len: usize) -> usize {
    len.div_ceil(COLUMNS).saturating_sub(CATALOG_ROWS)
}

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
    /// The catalog's tabs (top-left in GUI pixels; none on other screens).
    tabs: Vec<(Category, f32, f32)>,
    catalog: Catalog,
    /// A machine screen's working row (ingredients, progress, output), GUI pixels down.
    work_y: f32,
    /// Where a machine screen's gauges start (GUI pixels down).
    gauges_y: f32,
}

/// Recipes in a row of a machine's screen.
const RECIPE_COLUMNS: usize = 9;

impl Layout {
    /// `recipe`: what the open recipe machine is set to make.
    pub fn new(
        screen: Screen,
        width: f32,
        height: f32,
        s: f32,
        catalog: Catalog,
        recipe: Option<Item>,
    ) -> Self {
        let mut work_y = 30.0;
        let mut gauges_y = 0.0;
        let mut slots = Vec::new();
        let mut labels = Vec::new();
        let mut tabs = Vec::new();
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
            Screen::Machine(_, kind) if kind.has_recipes() => {
                // What it can make (pick one), then its ingredients, progress and output.
                labels.push((0.0, 0.0, kind.name()));
                let list: Vec<_> = recipes::for_machine(kind).collect();
                for i in 0..list.len() {
                    let (c, r) = (i % RECIPE_COLUMNS, i / RECIPE_COLUMNS);
                    slots.push((SlotId::Recipe(i), c as f32 * CELL, 10.0 + r as f32 * CELL));
                }
                let rows = list.len().div_ceil(RECIPE_COLUMNS) as f32;
                work_y = 10.0 + rows * CELL + 14.0;
                labels.push((0.0, work_y - 10.0, "needs"));
                let picked = recipe.and_then(|r| recipes::find(kind, r));
                let needs = picked.map_or(0, |r| r.inputs.len());
                for i in 0..needs {
                    slots.push((SlotId::MachineIn(i), i as f32 * CELL, work_y));
                }
                // (A fluid product has no slot: it is piped away.)
                if !picked.is_some_and(|r| matches!(r.output, Item::Fluid(_))) {
                    slots.push((SlotId::FurnaceOut, 140.0, work_y));
                }
                if picked.is_some_and(|r| r.extra.is_some()) {
                    slots.push((SlotId::Extra, 160.0, work_y));
                }
                gauges_y = work_y + CELL + 16.0;
                gauges_y + factory::gauge_count(kind, recipe) as f32 * GAUGE_H
            }
            Screen::Machine(_, kind) => {
                labels.push((40.0, 0.0, kind.name()));
                match kind {
                    MachineKind::Generator | MachineKind::Boiler => {
                        slots.push((SlotId::FurnaceFuel, 40.0, 30.0))
                    }
                    MachineKind::Reactor => {
                        slots.push((SlotId::FurnaceFuel, 40.0, 30.0));
                        slots.push((SlotId::FurnaceOut, 120.0, 30.0));
                    }
                    MachineKind::ElectricFurnace => {
                        slots.push((SlotId::FurnaceIn, 40.0, 30.0));
                        slots.push((SlotId::FurnaceOut, 120.0, 30.0));
                    }
                    k if k.has_output() => slots.push((SlotId::FurnaceOut, 120.0, 30.0)),
                    _ => {}
                }
                gauges_y = 72.0;
                gauges_y + factory::gauge_count(kind, recipe) as f32 * GAUGE_H
            }
            Screen::Power(_) => {
                labels.push((0.0, 0.0, "power"));
                POWER_HEIGHT
            }
            Screen::Chest(_) => {
                labels.push((0.0, 0.0, "storage"));
                row(&mut slots, &mut (0..CHEST_SLOTS).map(SlotId::Chest), 10.0);
                72.0
            }
            Screen::Creative => {
                // Always as tall as `CATALOG_ROWS`, so the screen doesn't jump between tabs.
                labels.push((0.0, 0.0, catalog.tab.name()));
                let first = catalog.scroll * COLUMNS;
                let last = (first + CATALOG_ROWS * COLUMNS).min(catalog.len);
                row(&mut slots, &mut (first..last).map(SlotId::Catalog), 10.0);
                for (i, &tab) in Category::ALL.iter().enumerate() {
                    tabs.push((tab, TAB_X, 10.0 + i as f32 * TAB_H));
                }
                10.0 + CATALOG_ROWS as f32 * CELL + 2.0
            }
        };

        // The player's own items below: the backpack, then the hotbar a little apart. (The
        // power screen is only something to read.)
        let content_h = if matches!(screen, Screen::Power(_)) {
            top
        } else {
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
            hotbar_y + CELL
        };

        // Centred with the tabs, if any, as part of it.
        let (w, h) = (WIDTH * s, content_h * s);
        let left = if tabs.is_empty() { 0.0 } else { -TAB_X * s };
        Self {
            x: ((width - w - left) / 2.0 + left).round(),
            y: ((height - h) / 2.0).round(),
            w,
            h,
            s,
            slots,
            labels,
            tabs,
            catalog,
            work_y,
            gauges_y,
        }
    }

    /// The catalog tab under the mouse.
    pub fn tab_at(&self, mx: f32, my: f32) -> Option<Category> {
        self.tabs.iter().find_map(|&(tab, gx, gy)| {
            let (x, y) = self.at(gx, gy);
            (mx >= x && mx < x + TAB_W * self.s && my >= y && my < y + TAB_H * self.s)
                .then_some(tab)
        })
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

    /// Whether the mouse is over the screen's content, tabs included (with a small margin).
    pub fn contains(&self, mx: f32, my: f32) -> bool {
        let m = 8.0 * self.s;
        let left = if self.tabs.is_empty() {
            0.0
        } else {
            -TAB_X * self.s
        };
        mx >= self.x - left - m
            && mx < self.x + self.w + m
            && my >= self.y - m
            && my < self.y + self.h + m
    }
}

/// What the screen shows.
pub struct View<'a> {
    pub slot: &'a dyn Fn(SlotId) -> Slot,
    pub cursor: Slot,
    pub mouse: (f32, f32),
    pub furnace: Option<&'a Furnace>,
    /// The machine of the open screen.
    pub machine: Option<&'a Machine>,
    /// The power screen's network.
    pub power: Option<&'a PowerView>,
    pub catalog: &'a [Item],
}

/// How tall the power screen is (GUI pixels).
const POWER_HEIGHT: f32 = 150.0;

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
    /// The catalog tabs' hover glow, and where the open tab's marker has slid to (GUI y).
    tab_glow: [f32; Category::ALL.len()],
    tab_marker: Option<f32>,
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
            line(
                ui,
                64.0,
                40.0,
                52.0,
                f.cook / crate::survival::furnace::COOK_TIME,
            );
            let burn = if f.lit() && f.burn_total > 0.0 {
                f.burn_left / f.burn_total
            } else {
                0.0
            };
            line(ui, 42.0, 40.0, 16.0, burn);
        }
        Screen::Machine(_, kind) => {
            // The fuel burning (generator) or the work going on, and a word on how it goes.
            let m = view.machine;
            let fill = m.map_or(0.0, |m| {
                let burns = matches!(
                    kind,
                    MachineKind::Generator | MachineKind::Boiler | MachineKind::Reactor
                );
                if burns {
                    if m.energy_total > 0.0 {
                        m.energy / m.energy_total
                    } else {
                        0.0
                    }
                } else {
                    m.progress_share()
                }
            });
            let (lx, ly, text_y) = if kind.has_recipes() {
                (84.0, layout.work_y + 10.0, layout.work_y + CELL + 4.0)
            } else {
                (64.0, 40.0, 46.0)
            };
            line(ui, lx, ly, 52.0, fill);
            let mut words = m.map_or_else(String::new, Machine::status);
            if let Some(resource) = m.and_then(|m| m.resource) {
                words = format!("{}: {words}", resource.name().to_lowercase());
            }
            let (x, y) = layout.at(if kind.has_recipes() { 0.0 } else { 64.0 }, text_y);
            ui.text_plain(x, y, &words, LABEL);
            // What it is making, across from its name.
            if let Some(r) = m.and_then(Machine::current_recipe) {
                let (x, y) = layout.at(WIDTH - 2.0, 0.0);
                let text = recipe_line(r);
                let w = ui.text_width(&text);
                ui.text_plain(x - w, y, &text, ACCENT);
            }
            // Its tanks: what is in them, as words and a line in the fluid's colour.
            if let Some(m) = m {
                for (i, g) in m.gauges().iter().enumerate() {
                    let gy = layout.gauges_y + i as f32 * GAUGE_H;
                    let (x, y) = layout.at(0.0, gy);
                    ui.text_plain(x, y, &g.text, LABEL);
                    let (bx, by) = layout.at(0.0, gy + 9.0);
                    ui.rect(bx, by, WIDTH * s, s, FAINT);
                    let [r, gr, b] = g.color;
                    let fill = (WIDTH * s * g.fill.clamp(0.0, 1.0)).round();
                    ui.rect(bx, by, fill, s, [r, gr, b, 230]);
                }
            }
        }
        Screen::Power(_) => draw_power(ui, layout, view.power),
        Screen::Creative => draw_tabs(ui, layout, view.mouse, anim, dt),
        Screen::Chest(_) => {}
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
        let result = matches!(id, SlotId::Output | SlotId::FurnaceOut | SlotId::Extra);
        // The recipe it is set to stands out.
        let picked = matches!(id, SlotId::Recipe(_))
            && stack.is_some_and(|s| view.machine.and_then(|m| m.recipe) == Some(s.item));
        let outline = if picked {
            with_alpha(ACCENT, 230.0)
        } else if result {
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
        } else if let SlotId::MachineIn(i) = id
            && let Some(&(item, n)) = view
                .machine
                .and_then(Machine::current_recipe)
                .and_then(|r| r.inputs.get(i))
        {
            // An empty ingredient slot shows, faintly, what goes there.
            let start = ui.vertices.len();
            ui.item(ix, iy, (16.0 * s).round(), &ItemStack::new(item, n));
            ui.fade_from(start, 0.3, 0.0, 0.0);
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
        let recipe = match (id, view.machine) {
            (SlotId::Recipe(i), Some(m)) => recipes::for_machine(m.kind).nth(i),
            _ => None,
        };
        if let Some(recipe) = recipe {
            let card_start = ui.vertices.len();
            recipe_card(ui, mx + 12.0 * s, my + 8.0 * s, recipe);
            let a = ease_out(anim.card.1);
            ui.fade_from(card_start, a, 0.0, (1.0 - a) * 3.0 * s);
        } else if let Some(stack) = stack {
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

/// The catalog's tabs down its left side (words, the open one bright with an amber mark that
/// slides between them, a soft glow under the mouse) and, when its tab has more rows than fit,
/// a thin scroll line down its right side.
fn draw_tabs(ui: &mut Ui, layout: &Layout, mouse: (f32, f32), anim: &mut ScreenAnim, dt: f32) {
    let s = layout.s;
    let hovered = layout.tab_at(mouse.0, mouse.1);
    let (_, ch) = ui.char_size();
    for (i, &(tab, gx, gy)) in layout.tabs.iter().enumerate() {
        let glow = &mut anim.tab_glow[i];
        let on = hovered == Some(tab);
        *glow = approach(
            *glow,
            if on { 1.0 } else { 0.0 },
            if on { 30.0 } else { 9.0 },
            dt,
        );
        let (x, y) = layout.at(gx, gy);
        if *glow > 0.01 {
            let a = HOVER[3] as f32 * *glow;
            ui.rect(x, y, TAB_W * s, (TAB_H - 1.0) * s, with_alpha(HOVER, a));
        }
        let open = tab == layout.catalog.tab;
        let color = if open {
            LINE
        } else {
            with_alpha(LABEL, 120.0 + 80.0 * *glow)
        };
        let ty = y + ((TAB_H - 1.0) * s - ch) / 2.0 + s;
        ui.text_plain(x + 6.0 * s, ty, tab.name(), color);
        if open {
            let marker = anim.tab_marker.get_or_insert(gy);
            *marker = approach(*marker, gy, 25.0, dt);
        }
    }
    if let Some(gy) = anim.tab_marker {
        let (x, y) = layout.at(TAB_X, gy + 2.0);
        ui.rect(
            x,
            y.round(),
            s,
            (TAB_H - 5.0) * s,
            with_alpha(ACCENT, 255.0),
        );
    }

    // The scroll line: how much of the tab is in view, and where.
    let rows = layout.catalog.len.div_ceil(COLUMNS);
    if rows > CATALOG_ROWS {
        let full = CATALOG_ROWS as f32 * CELL - 2.0;
        let (x, y) = layout.at(WIDTH + 3.0, 11.0);
        ui.rect(x, y, s, full * s, FAINT);
        let part = CATALOG_ROWS as f32 / rows as f32;
        let from = layout.catalog.scroll as f32 / rows as f32;
        ui.rect(
            x,
            y + (from * full * s).round(),
            s,
            (part * full * s).round(),
            LINE,
        );
    }
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

/// "making plastic x2" (short, across from a machine's name; the amounts it takes show in its
/// slots and on the recipe's card).
fn recipe_line(r: &recipes::Recipe) -> String {
    let name = r.output.name().to_lowercase();
    if r.count > 1 {
        format!("making {name} x{}", r.count)
    } else {
        format!("making {name}")
    }
}

/// What a recipe takes, makes and how long it takes, on a card like an item's.
fn recipe_card(ui: &mut Ui, x: f32, y: f32, r: &recipes::Recipe) {
    let s = ui.scale;
    let name = if r.count > 1 {
        format!("{} x{}", r.output.name().to_lowercase(), r.count)
    } else {
        r.output.name().to_lowercase()
    };
    let mut facts: Vec<String> = r
        .inputs
        .iter()
        .map(|(item, n)| format!("{n} {}", item.name().to_lowercase()))
        .collect();
    facts.extend(
        r.fluids
            .iter()
            .map(|(fluid, n)| format!("{n} l {} (piped)", fluid.name())),
    );
    let makes = r.makes();
    if makes.len() > 1 || matches!(r.output, Item::Fluid(_)) {
        facts.push(format!("makes {}", makes.join(", ")));
    }
    let kw = r.machine.power() / 1000.0;
    facts.push(if kw >= 1000.0 {
        format!("{} s, {:.1} mw", r.time, kw / 1000.0)
    } else {
        format!("{} s, {kw:.0} kw", r.time)
    });
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

/// "1 pole", "3 poles".
fn count(n: u32, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The power screen: what the network makes and uses, what its accumulators hold, and the
/// last minute as a graph (made faint behind, used bright in front).
fn draw_power(ui: &mut Ui, layout: &Layout, power: Option<&PowerView>) {
    let s = layout.s;
    let Some(view) = power else {
        let (x, y) = layout.at(0.0, 14.0);
        ui.text_plain(x, y, "not on a network", LABEL);
        return;
    };
    let net = &view.network;
    let kw = |w: f32| {
        if w >= 1.0e6 {
            format!("{:.2} mw", w / 1.0e6)
        } else {
            format!("{:.0} kw", w / 1000.0)
        }
    };
    let mut lines: Vec<(String, [u8; 4])> = vec![
        (
            format!(
                "made {}   {} of {} generators burning",
                kw(net.capacity),
                net.generators.0,
                net.generators.1
            ),
            LINE,
        ),
        (
            format!(
                "used {}   {} of {} machines working",
                kw(net.used),
                net.consumers.0,
                net.consumers.1
            ),
            LINE,
        ),
    ];
    if net.demand > net.used + 1.0 {
        lines.push((
            format!(
                "short of power: machines at {:.0}%",
                net.used / net.demand * 100.0
            ),
            [255, 120, 100, 230],
        ));
    } else if net.capacity > net.used + net.charging + 1.0 {
        lines.push((
            format!(
                "{} going to waste",
                kw(net.capacity - net.used - net.charging)
            ),
            LABEL,
        ));
    }
    if net.accumulators > 0 {
        let flow = if net.charging > 0.0 {
            format!("charging {}", kw(net.charging))
        } else if net.discharging > 0.0 {
            format!("giving {}", kw(net.discharging))
        } else {
            "idle".to_string()
        };
        lines.push((
            format!(
                "{}: {:.1} of {:.0} mj, {flow}",
                count(net.accumulators, "accumulator"),
                net.stored / 1.0e6,
                net.storage / 1.0e6
            ),
            LINE,
        ));
    }
    lines.push((count(net.poles, "pole"), LABEL));
    let (_, ch) = ui.char_size();
    let mut gy = 14.0;
    for (text, color) in &lines {
        let (x, y) = layout.at(0.0, gy);
        ui.text_plain(x, y, text, *color);
        gy += (ch / s).ceil() + 3.0;
    }
    // The charge as a line.
    if net.storage > 0.0 {
        let (x, y) = layout.at(0.0, gy);
        ui.rect(x, y, WIDTH * s, s, FAINT);
        ui.rect(x, y, WIDTH * s * (net.stored / net.storage), s, ACCENT);
        gy += 5.0;
    }

    // The last minute.
    let (graph_h, top) = (POWER_HEIGHT - gy - 14.0, gy + 4.0);
    let samples = &view.history.samples;
    let most = samples
        .iter()
        .map(|&(made, used)| made.max(used))
        .fold(1.0, f32::max);
    let bar = WIDTH / factory::HISTORY as f32;
    let (x0, y0) = layout.at(0.0, top + graph_h);
    ui.rect(x0, y0, WIDTH * s, s, FAINT);
    let first = factory::HISTORY - samples.len();
    for (i, &(made, used)) in samples.iter().enumerate() {
        let x = x0 + (first + i) as f32 * bar * s;
        let w = (bar * s - s).max(s);
        let h = |p: f32| (p / most * graph_h * s).round();
        ui.rect(x, y0 - h(made), w, h(made), FAINT);
        ui.rect(x, y0 - h(used), w, h(used), with_alpha(LINE, 200.0));
    }
    let (x, y) = layout.at(0.0, top + graph_h + 3.0);
    ui.text_plain(x, y, "last minute: made, used", LABEL);
}
