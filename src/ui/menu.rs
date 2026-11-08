//! The menus: the main menu over the planet seen from orbit, the world list, creating a
//! world, settings, and the pause menu. All in the game's own minimal flat style: no panels
//! or buttons with bevels, just lowercase text, thin lines and an amber accent.
//!
//! Like the rest of the UI it is immediate-mode: `Menu::draw` draws the page and reports what
//! was clicked as `Action`s, which the game carries out between frames (`app::session`).

use winit::{event::MouseButton, keyboard::KeyCode};

use crate::{
    app::{
        input::Input,
        save::{self, MAX_NAME, MAX_SEED, Settings, WorldEntry},
    },
    flight::hud::{Pointer, button},
    net::steam::Friend,
    player::GameMode,
    ui::{Ui, anim::approach},
};

const TEXT: [u8; 4] = [235, 240, 245, 255];
const DIM: [u8; 4] = [160, 175, 190, 255];
const FAINT: [u8; 4] = [150, 165, 180, 130];
const AMBER: [u8; 4] = [240, 180, 90, 255];
const RED: [u8; 4] = [240, 110, 90, 255];

/// The frame rates offered as a cap (0 = none).
pub const FPS_CAPS: [u32; 7] = [0, 30, 60, 90, 120, 144, 240];
/// Longest address the join page takes.
const MAX_ADDRESS: usize = 60;
const MODES: [GameMode; 3] = [GameMode::Survival, GameMode::Creative, GameMode::Spectator];
/// Two clicks on a world within this time play it (s).
const DOUBLE_CLICK: f32 = 0.35;
const PAGE_FADE: f32 = 0.18;
const LOADING_FADE: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Worlds,
    Create,
    Settings,
    Pause,
    Loading,
    /// Joining someone's game.
    Multiplayer,
    /// Asking friends to join this one.
    Invite,
    /// Who is at what time; syncing to them.
    Time,
}

/// What the player asked for.
#[derive(Clone, Debug)]
pub enum Action {
    ShowWorlds,
    Play(usize),
    Create {
        name: String,
        seed: String,
        mode: GameMode,
    },
    Delete(usize),
    SettingsChanged,
    Resume,
    /// Save and go back to the main menu.
    LeaveWorld,
    Quit,
    /// The loading screen has faded away.
    LoadingDone,
    ShowMultiplayer,
    ShowInvite,
    /// Open this world to others.
    Host,
    Join {
        name: String,
        address: String,
    },
    JoinSteam(u64),
    Invite(u64),
    InviteOverlay,
    /// Go to this player's time and weather.
    SyncTime(u32),
    /// And stay attached to their time warp.
    FollowTime(u32),
    StopFollowing,
    CancelJoin,
}

/// Where this player stands, for the pause menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSession {
    Offline,
    Host,
    Client,
}

/// Another player, on the time page.
#[derive(Clone, Debug)]
pub struct TimeRow {
    pub id: u32,
    pub name: String,
    /// How far they are from this player (or that they are at the same time).
    pub gap: String,
    /// How fast their clock runs.
    pub rate: String,
    pub following: bool,
    pub apart: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Seed,
    Address,
    Player,
}

pub struct Menu {
    pub page: Page,
    /// The page the settings go back to.
    settings_from: Page,
    /// Seconds since this page opened.
    age: f32,
    clock: f32,
    hover: [f32; 16],
    pub error: Option<String>,

    selected: Option<usize>,
    scroll: f32,
    confirm_delete: bool,
    last_click: Option<(usize, f32)>,

    name: String,
    seed: String,
    mode: GameMode,
    field: Field,

    address: String,
    player: String,
    /// Connecting: Esc gives up.
    cancellable: bool,
    invited: Vec<u64>,

    dragging_volume: bool,

    loading: String,
    /// The world is ready: fading out, then this page opens (None closes the menu).
    loaded: Option<(Option<Page>, f32)>,
}

/// What the pages need to know.
pub struct MenuContext<'a> {
    pub settings: &'a mut Settings,
    pub worlds: &'a [WorldEntry],
    pub now: u64,
    /// Whether the menu is over a running world (it is dimmed then).
    pub in_world: bool,
    pub session: MenuSession,
    pub players: usize,
    /// Signed in to Steam.
    pub steam: bool,
    pub friends: &'a [Friend],
    pub time_rows: &'a [TimeRow],
}

impl Menu {
    pub fn new(page: Page) -> Self {
        Self {
            page,
            settings_from: Page::Main,
            age: 0.0,
            clock: 0.0,
            hover: [0.0; 16],
            error: None,
            selected: None,
            scroll: 0.0,
            confirm_delete: false,
            last_click: None,
            name: String::new(),
            seed: String::new(),
            mode: GameMode::Survival,
            field: Field::Name,
            address: String::new(),
            player: String::new(),
            cancellable: false,
            invited: Vec::new(),
            dragging_volume: false,
            loading: String::new(),
            loaded: None,
        }
    }

    pub fn loading(text: impl Into<String>) -> Self {
        let mut menu = Self::new(Page::Loading);
        menu.loading = text.into();
        menu
    }

    /// The waiting screen while reaching a host (Esc gives up).
    pub fn connecting(text: impl Into<String>) -> Self {
        let mut menu = Self::loading(text);
        menu.cancellable = true;
        menu
    }

    pub fn set_loading_text(&mut self, text: impl Into<String>) {
        self.loading = text.into();
        self.cancellable = false;
    }

    /// What the join page starts out with.
    pub fn prepare_join(&mut self, player: &str, address: &str) {
        self.player = player.to_string();
        self.address = address.to_string();
        self.field = Field::Address;
    }

    pub fn show(&mut self, page: Page) {
        if page == Page::Settings && self.page != Page::Settings {
            self.settings_from = self.page;
        }
        if page == Page::Create {
            self.name.clear();
            self.seed.clear();
            self.mode = GameMode::Survival;
            self.field = Field::Name;
        }
        if page != Page::Worlds {
            self.confirm_delete = false;
        }
        self.page = page;
        self.age = 0.0;
        self.hover = [0.0; 16];
    }

    /// While this is showing nothing in the world moves.
    pub fn is_loading(&self) -> bool {
        self.page == Page::Loading
    }

    /// The world behind the loading screen is ready; fade away into `next`.
    pub fn finish_loading(&mut self, next: Option<Page>) {
        if self.loaded.is_none() {
            self.loaded = Some((next, 0.0));
        }
    }

    pub fn waiting_for_world(&self) -> bool {
        self.page == Page::Loading && self.loaded.is_none()
    }

    /// A key went down (or repeats) while the menu is open.
    pub fn key(&mut self, code: KeyCode, text: Option<&str>) -> Option<Action> {
        match code {
            KeyCode::Escape => return self.back(),
            KeyCode::Tab if self.page == Page::Create => {
                self.field = match self.field {
                    Field::Name => Field::Seed,
                    _ => Field::Name,
                };
            }
            KeyCode::Tab if self.page == Page::Multiplayer => {
                self.field = match self.field {
                    Field::Address => Field::Player,
                    _ => Field::Address,
                };
            }
            KeyCode::Enter | KeyCode::NumpadEnter => match self.page {
                Page::Create => return Some(self.create_action()),
                Page::Multiplayer => return Some(self.join_action()),
                Page::Worlds if !self.confirm_delete => {
                    return self.selected.map(Action::Play);
                }
                _ => {}
            },
            KeyCode::Backspace if matches!(self.page, Page::Create | Page::Multiplayer) => {
                self.active_field().pop();
            }
            _ if matches!(self.page, Page::Create | Page::Multiplayer) => {
                if let Some(text) = text {
                    let limit = match self.field {
                        Field::Name | Field::Player => MAX_NAME,
                        Field::Seed => MAX_SEED,
                        Field::Address => MAX_ADDRESS,
                    };
                    let field = self.active_field();
                    for c in text.chars().filter(|c| (' '..='~').contains(c)) {
                        if field.chars().count() < limit {
                            field.push(c);
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn active_field(&mut self) -> &mut String {
        match self.field {
            Field::Name => &mut self.name,
            Field::Seed => &mut self.seed,
            Field::Address => &mut self.address,
            Field::Player => &mut self.player,
        }
    }

    fn create_action(&self) -> Action {
        Action::Create {
            name: self.name.trim().to_string(),
            seed: self.seed.clone(),
            mode: self.mode,
        }
    }

    fn join_action(&self) -> Action {
        Action::Join {
            name: self.player.trim().to_string(),
            address: self.address.trim().to_string(),
        }
    }

    fn back(&mut self) -> Option<Action> {
        match self.page {
            Page::Worlds if self.confirm_delete => self.confirm_delete = false,
            Page::Worlds => self.show(Page::Main),
            Page::Create => return Some(Action::ShowWorlds),
            Page::Settings => self.show(self.settings_from),
            Page::Pause => return Some(Action::Resume),
            Page::Multiplayer => self.show(Page::Main),
            Page::Invite | Page::Time => self.show(Page::Pause),
            Page::Loading if self.cancellable => return Some(Action::CancelJoin),
            Page::Main | Page::Loading => {}
        }
        None
    }

    /// Draws the open page. Clicks come out as actions.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        input: &Input,
        ctx: &mut MenuContext,
        dt: f32,
    ) -> Vec<Action> {
        self.age += dt;
        self.clock += dt;
        let mut pointer = Pointer {
            pos: input.cursor,
            pressed: input.button_pressed(MouseButton::Left),
            held: input.button_held(MouseButton::Left),
            scroll: input.scroll,
            over_ui: false,
        };
        let mut actions = Vec::new();
        if self.page == Page::Loading {
            self.draw_loading(ui, dt, &mut actions);
            return actions;
        }
        self.backdrop(ui, ctx.in_world);
        let start = ui.vertices.len();
        match self.page {
            Page::Main => self.draw_main(ui, &mut pointer, dt, &mut actions),
            Page::Worlds => self.draw_worlds(ui, &mut pointer, ctx, &mut actions),
            Page::Create => self.draw_create(ui, &mut pointer, &mut actions),
            Page::Settings => self.draw_settings(ui, &mut pointer, ctx, &mut actions),
            Page::Pause => self.draw_pause(ui, &mut pointer, ctx, dt, &mut actions),
            Page::Multiplayer => self.draw_multiplayer(ui, &mut pointer, ctx, &mut actions),
            Page::Invite => self.draw_invite(ui, &mut pointer, ctx, &mut actions),
            Page::Time => self.draw_time(ui, &mut pointer, ctx, &mut actions),
            Page::Loading => {}
        }
        // Each page fades in, rising a little into place.
        let t = (self.age / PAGE_FADE).min(1.0);
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        ui.fade_from(start, eased, 0.0, (1.0 - eased) * 8.0 * ui.scale);
        actions
    }

    /// Darkens the left side, where the text is, and all of it over a running world.
    fn backdrop(&self, ui: &mut Ui, in_world: bool) {
        let (w, h) = (ui.width, ui.height);
        if in_world {
            ui.rect(0.0, 0.0, w, h, [6, 9, 14, 120]);
        }
        let dark = [6, 9, 14, 190];
        let clear = [6, 9, 14, 0];
        ui.gradient(0.0, 0.0, w * 0.62, h, [dark, clear, clear, dark]);
    }

    fn left(ui: &Ui) -> f32 {
        (ui.width * 0.075).round().max(16.0 * ui.scale)
    }

    /// Big text.
    fn title(ui: &mut Ui, x: f32, y: f32, text: &str, factor: f32) {
        let s = ui.scale;
        ui.scale = s * factor;
        ui.text(x, y, text, TEXT);
        ui.scale = s;
    }

    fn heading(ui: &mut Ui, x: f32, y: f32, text: &str) {
        Self::title(ui, x, y, text, 2.0);
    }

    /// A row of the main and pause menus: slides right and lights up under the mouse.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        id: usize,
        x: f32,
        y: f32,
        label: &str,
        dt: f32,
    ) -> bool {
        let s = ui.scale;
        let (_, ch) = ui.char_size();
        let width = ui.text_width(label) + 12.0 * s;
        let height = ch + 4.0 * s;
        let over = pointer.inside(x - 2.0 * s, y - s, width + 2.0 * s, height);
        let t = &mut self.hover[id];
        *t = approach(*t, over as u8 as f32, 18.0, dt);
        let t = *t;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        let color = [
            mix(DIM[0], TEXT[0]),
            mix(DIM[1], TEXT[1]),
            mix(DIM[2], TEXT[2]),
            255,
        ];
        ui.rect(
            x,
            y + s,
            s,
            ch - 3.0 * s,
            [AMBER[0], AMBER[1], AMBER[2], (t * 255.0) as u8],
        );
        ui.text(x + 3.0 * s + (t * 3.0 * s).round(), y, label, color);
        over && pointer.pressed
    }

    fn draw_main(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        dt: f32,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.26).round();
        Self::title(ui, x, top, "astro block", 4.0);
        ui.rect(x, top + ch * 4.0 + 2.0 * s, 90.0 * s, s, AMBER);
        let rows = ch + 7.0 * s;
        let y = top + ch * 4.0 + 16.0 * s;
        if self.entry(ui, pointer, 0, x, y, "play", dt) {
            actions.push(Action::ShowWorlds);
        }
        if self.entry(ui, pointer, 1, x, y + rows, "multiplayer", dt) {
            actions.push(Action::ShowMultiplayer);
        }
        if self.entry(ui, pointer, 2, x, y + 2.0 * rows, "settings", dt) {
            self.show(Page::Settings);
        }
        if self.entry(ui, pointer, 3, x, y + 3.0 * rows, "quit", dt) {
            actions.push(Action::Quit);
        }
    }

    fn draw_pause(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &MenuContext,
        dt: f32,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.3).round();
        let (heading, leave) = match ctx.session {
            MenuSession::Offline => ("paused", "save and leave world"),
            MenuSession::Host => ("hosting", "save and end the game"),
            MenuSession::Client => ("playing together", "leave the game"),
        };
        Self::heading(ui, x, top, heading);
        ui.rect(x, top + ch * 2.0 + 2.0 * s, 60.0 * s, s, AMBER);
        let rows = ch + 7.0 * s;
        let y = top + ch * 2.0 + 14.0 * s;
        if self.entry(ui, pointer, 0, x, y, "resume", dt) {
            actions.push(Action::Resume);
        }
        let mut row = 1.0;
        match ctx.session {
            MenuSession::Offline => {
                if self.entry(ui, pointer, 1, x, y + rows * row, "open to others", dt) {
                    actions.push(Action::Host);
                }
                row += 1.0;
            }
            MenuSession::Host => {
                if self.entry(ui, pointer, 1, x, y + rows * row, "invite friends", dt) {
                    actions.push(Action::ShowInvite);
                }
                row += 1.0;
            }
            MenuSession::Client => {}
        }
        if !ctx.time_rows.is_empty() {
            if self.entry(ui, pointer, 4, x, y + rows * row, "time", dt) {
                self.show(Page::Time);
            }
            row += 1.0;
        }
        if self.entry(ui, pointer, 2, x, y + rows * row, "settings", dt) {
            self.show(Page::Settings);
        }
        row += 1.0;
        if self.entry(ui, pointer, 3, x, y + rows * row, leave, dt) {
            actions.push(Action::LeaveWorld);
        }
        row += 1.5;
        if ctx.session != MenuSession::Offline {
            let text = format!(
                "{} {} in this game",
                ctx.players,
                if ctx.players == 1 {
                    "player"
                } else {
                    "players"
                }
            );
            ui.text(x, y + rows * row, &text, DIM);
        }
    }

    /// Join by address, or a friend who is hosting.
    fn draw_multiplayer(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &MenuContext,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.14).round();
        Self::heading(ui, x, top, "multiplayer");
        let width = (MAX_ADDRESS as f32 * 0.7) * ui.char_size().0;
        let spacing = 2.0 * ch + 12.0 * s;
        let mut y = top + ch * 2.0 + 14.0 * s;

        ui.text(x, y, "address", DIM);
        let address = self.address.clone();
        if self.text_field(
            ui,
            pointer,
            x,
            y + ch + s,
            width,
            &address,
            "ip or name, and :port if it isn't the usual one",
            self.field == Field::Address,
        ) {
            self.field = Field::Address;
        }
        y += spacing;
        if !ctx.steam {
            ui.text(x, y, "your name", DIM);
            let player = self.player.clone();
            if self.text_field(
                ui,
                pointer,
                x,
                y + ch + s,
                width,
                &player,
                "player",
                self.field == Field::Player,
            ) {
                self.field = Field::Player;
            }
            y += spacing;
        }
        let (join, w) = button(
            ui,
            pointer,
            x,
            y,
            "join",
            true,
            !self.address.trim().is_empty(),
        );
        let (back, _) = button(ui, pointer, x + w + 8.0 * s, y, "back", false, true);
        if join {
            actions.push(self.join_action());
        }
        if back {
            self.show(Page::Main);
            return;
        }
        y += ch + 14.0 * s;
        if let Some(error) = &self.error {
            ui.text(x, y, error, RED);
            y += ch + 6.0 * s;
        }

        if !ctx.steam {
            ui.text(x, y, "steam isn't running: joining by address only", DIM);
            return;
        }
        let hosting: Vec<_> = ctx.friends.iter().filter(|f| f.hosting).collect();
        ui.text(x, y, "friends hosting a game", DIM);
        y += ch + 4.0 * s;
        if hosting.is_empty() {
            ui.text(x, y, "none right now", FAINT);
        }
        let bottom = ui.height - 12.0 * s;
        for friend in hosting {
            if y + ch > bottom {
                break;
            }
            let (clicked, w) = button(ui, pointer, x, y, "join", false, true);
            ui.text(x + w + 6.0 * s, y + s, &friend.name, TEXT);
            if clicked {
                actions.push(Action::JoinSteam(friend.id));
            }
            y += ch + 7.0 * s;
        }
    }

    /// Everyone else and where they are in time.
    fn draw_time(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &MenuContext,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.14).round();
        Self::heading(ui, x, top, "time");
        let mut y = top + ch * 2.0 + 14.0 * s;
        ui.text(x, y, "everyone warps on their own, with , and .", DIM);
        y += ch + 2.0 * s;
        ui.text(x, y, "sync jumps you to their time and weather", DIM);
        y += ch + 2.0 * s;
        ui.text(x, y, "follow keeps you with them whenever they warp", DIM);
        y += ch + 14.0 * s;
        let bottom_row = ui.height - 30.0 * s;
        let row = ch + 7.0 * s;
        for other in ctx.time_rows {
            if y + ch > bottom_row - s {
                break;
            }
            ui.text(x, y + s, &other.name, TEXT);
            let at = if other.apart { AMBER } else { DIM };
            ui.text(x + 130.0 * s, y + s, &other.gap, at);
            ui.text(x + 190.0 * s, y + s, &other.rate, DIM);
            let buttons = x + 226.0 * s;
            let (sync, w) = button(ui, pointer, buttons, y, "sync", false, other.apart);
            if sync {
                actions.push(Action::SyncTime(other.id));
            }
            let label = if other.following {
                "stop following"
            } else {
                "follow"
            };
            let (follow, _) = button(
                ui,
                pointer,
                buttons + w + 6.0 * s,
                y,
                label,
                other.following,
                true,
            );
            if follow {
                actions.push(if other.following {
                    Action::StopFollowing
                } else {
                    Action::FollowTime(other.id)
                });
            }
            y += row;
        }
        let (back, _) = button(ui, pointer, x, bottom_row, "back", false, true);
        if back {
            self.show(Page::Pause);
        }
    }

    /// Online Steam friends, to ask along.
    fn draw_invite(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &MenuContext,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.12).round();
        Self::heading(ui, x, top, "invite friends");
        let bottom_row = ui.height - 30.0 * s;
        let list_top = top + ch * 2.0 + 12.0 * s;
        let (back, w) = button(ui, pointer, x, bottom_row, "back", false, true);
        if back {
            self.show(Page::Pause);
            return;
        }
        if ctx.steam {
            let (overlay, _) = button(
                ui,
                pointer,
                x + w + 8.0 * s,
                bottom_row,
                "steam invite window",
                false,
                true,
            );
            if overlay {
                actions.push(Action::InviteOverlay);
            }
        } else {
            ui.text(x, list_top, "steam isn't running", DIM);
        }
        ui.text(
            x,
            bottom_row - ch - 4.0 * s,
            "others can also join at your ip address, port 47777",
            FAINT,
        );
        let row = ch + 7.0 * s;
        let visible = (((bottom_row - ch - 10.0 * s - list_top) / row) as usize).max(1);
        let max_scroll = ctx.friends.len().saturating_sub(visible) as f32;
        self.scroll = (self.scroll - pointer.scroll).clamp(0.0, max_scroll);
        let first = self.scroll as usize;
        for (n, friend) in ctx.friends.iter().skip(first).take(visible).enumerate() {
            let y = list_top + n as f32 * row;
            let done = self.invited.contains(&friend.id);
            let label = if done { "invited" } else { "invite" };
            let (clicked, _) = button(ui, pointer, x, y, label, done, !done);
            ui.text(x + 44.0 * s, y + s, &friend.name, TEXT);
            if clicked {
                self.invited.push(friend.id);
                actions.push(Action::Invite(friend.id));
            }
        }
    }

    fn draw_worlds(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &MenuContext,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.12).round();
        Self::heading(ui, x, top, "worlds");
        let width = (ui.width * 0.42).max(190.0 * s);
        let list_top = top + ch * 2.0 + 10.0 * s;
        let bottom_row = ui.height - 30.0 * s;
        let row = 2.0 * ch + 6.0 * s;
        let visible = (((bottom_row - 8.0 * s - list_top) / row) as usize).max(1);
        let worlds = ctx.worlds;
        if self.selected.is_some_and(|i| i >= worlds.len()) {
            self.selected = None;
        }
        // The wheel scrolls the list.
        let max_scroll = worlds.len().saturating_sub(visible) as f32;
        self.scroll = (self.scroll - pointer.scroll).clamp(0.0, max_scroll);
        let first = self.scroll as usize;
        if worlds.is_empty() {
            ui.text(x, list_top, "no worlds yet", DIM);
        }
        for (i, world) in worlds.iter().enumerate().skip(first).take(visible) {
            let y = list_top + (i - first) as f32 * row;
            let over = !self.confirm_delete && pointer.inside(x, y, width, row - 2.0 * s);
            let selected = self.selected == Some(i);
            if over {
                ui.rect(x, y, width, row - 2.0 * s, [255, 255, 255, 18]);
            }
            if selected {
                ui.rect(x, y, s, row - 2.0 * s, AMBER);
            }
            let name_color = if selected { AMBER } else { TEXT };
            ui.text(x + 5.0 * s, y + s, &world.meta.name, name_color);
            let details = format!(
                "{}   {}   {}",
                world.meta.mode.name(),
                save::format_played(world.meta.played),
                save::format_ago(world.meta.last_played, ctx.now),
            );
            ui.text(x + 5.0 * s, y + s + ch + s, &details, DIM);
            if over && pointer.pressed {
                let again = self
                    .last_click
                    .is_some_and(|(j, t)| j == i && self.clock - t < DOUBLE_CLICK);
                self.selected = Some(i);
                self.last_click = Some((i, self.clock));
                if again {
                    actions.push(Action::Play(i));
                }
            }
        }
        if first + visible < worlds.len() {
            let text = format!("{} more", worlds.len() - first - visible);
            ui.text(
                x + width - ui.text_width(&text),
                bottom_row - 8.0 * s,
                &text,
                FAINT,
            );
        }
        if let Some(error) = &self.error {
            ui.text(x, bottom_row - ch - 3.0 * s, error, RED);
        }

        let gap = 8.0 * s;
        let mut cx = x;
        if self.confirm_delete
            && let Some(i) = self.selected
        {
            let question = format!("delete {} for good?", worlds[i].meta.name);
            cx += ui.text(cx, bottom_row + s, &question, RED) + gap;
            let (yes, w) = button(ui, pointer, cx, bottom_row, "delete", false, true);
            cx += w + gap;
            let (no, _) = button(ui, pointer, cx, bottom_row, "keep it", false, true);
            if yes {
                self.confirm_delete = false;
                actions.push(Action::Delete(i));
                self.selected = None;
            } else if no {
                self.confirm_delete = false;
            }
            return;
        }
        let chosen = self.selected;
        let (play, w) = button(ui, pointer, cx, bottom_row, "play", false, chosen.is_some());
        cx += w + gap;
        let (new, w) = button(
            ui,
            pointer,
            cx,
            bottom_row,
            "new world",
            worlds.is_empty(),
            true,
        );
        cx += w + gap;
        let (delete, w) = button(
            ui,
            pointer,
            cx,
            bottom_row,
            "delete",
            false,
            chosen.is_some(),
        );
        cx += w + gap;
        let (back, _) = button(ui, pointer, cx, bottom_row, "back", false, true);
        if play && let Some(i) = chosen {
            actions.push(Action::Play(i));
        }
        if new {
            self.error = None;
            self.show(Page::Create);
        }
        if delete {
            self.confirm_delete = true;
        }
        if back {
            self.show(Page::Main);
        }
    }

    /// A one-line text field: returns whether it was clicked.
    #[allow(clippy::too_many_arguments)]
    fn text_field(
        &self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        x: f32,
        y: f32,
        width: f32,
        text: &str,
        hint: &str,
        focused: bool,
    ) -> bool {
        let s = ui.scale;
        let (cw, ch) = ui.char_size();
        let height = ch + 4.0 * s;
        let over = pointer.inside(x, y, width, height);
        if over {
            ui.rect(x, y, width, height, [255, 255, 255, 18]);
        }
        if text.is_empty() && !focused {
            ui.text(x + 2.0 * s, y + s, hint, FAINT);
        } else {
            ui.text(x + 2.0 * s, y + s, text, TEXT);
        }
        let line = if focused { AMBER } else { FAINT };
        ui.rect(x, y + height - s, width, s, line);
        if focused && ((self.clock * 2.0) as u32).is_multiple_of(2) {
            let caret = x + 2.0 * s + text.chars().count() as f32 * cw;
            ui.rect(caret, y + s, s, ch - s, AMBER);
        }
        over && pointer.pressed
    }

    fn draw_create(&mut self, ui: &mut Ui, pointer: &mut Pointer, actions: &mut Vec<Action>) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.16).round();
        Self::heading(ui, x, top, "new world");
        let width = (MAX_NAME as f32 + 3.0) * ui.char_size().0;
        let mut y = top + ch * 2.0 + 14.0 * s;
        let spacing = 2.0 * ch + 12.0 * s;

        ui.text(x, y, "name", DIM);
        let name = self.name.clone();
        if self.text_field(
            ui,
            pointer,
            x,
            y + ch + s,
            width,
            &name,
            "new world",
            self.field == Field::Name,
        ) {
            self.field = Field::Name;
        }
        y += spacing;

        ui.text(x, y, "seed", DIM);
        let seed = self.seed.clone();
        if self.text_field(
            ui,
            pointer,
            x,
            y + ch + s,
            width,
            &seed,
            "blank for a random planet",
            self.field == Field::Seed,
        ) {
            self.field = Field::Seed;
        }
        y += spacing;

        ui.text(x, y, "mode", DIM);
        let mut cx = x;
        for mode in MODES {
            let (clicked, w) = button(
                ui,
                pointer,
                cx,
                y + ch + s,
                mode.name(),
                self.mode == mode,
                true,
            );
            if clicked {
                self.mode = mode;
            }
            cx += w + 6.0 * s;
        }
        y += spacing + 6.0 * s;

        let (create, w) = button(ui, pointer, x, y, "create", true, true);
        let (back, _) = button(ui, pointer, x + w + 8.0 * s, y, "back", false, true);
        if create {
            actions.push(self.create_action());
        }
        if back {
            actions.push(Action::ShowWorlds);
        }
    }

    fn draw_settings(
        &mut self,
        ui: &mut Ui,
        pointer: &mut Pointer,
        ctx: &mut MenuContext,
        actions: &mut Vec<Action>,
    ) {
        let s = ui.scale;
        let x = Self::left(ui);
        let (_, ch) = ui.char_size();
        let top = (ui.height * 0.16).round();
        Self::heading(ui, x, top, "settings");
        let values = x + 66.0 * s;
        let spacing = ch + 12.0 * s;
        let mut y = top + ch * 2.0 + 16.0 * s;
        let settings = &mut *ctx.settings;
        let before = settings.clone();

        // A row of choices: the current one in amber.
        let row = |ui: &mut Ui,
                   pointer: &mut Pointer,
                   y: f32,
                   label: &str,
                   options: &[(&str, bool)]|
         -> Option<usize> {
            ui.text(x, y + s, label, DIM);
            let mut cx = values;
            let mut chosen = None;
            for (i, (name, active)) in options.iter().enumerate() {
                let (clicked, w) = button(ui, pointer, cx, y, name, *active, true);
                if clicked {
                    chosen = Some(i);
                }
                cx += w + 4.0 * s;
            }
            chosen
        };

        let display = [
            ("window", !settings.fullscreen),
            ("fullscreen", settings.fullscreen),
        ];
        if let Some(i) = row(ui, pointer, y, "display", &display) {
            settings.fullscreen = i == 1;
        }
        y += spacing;
        let vsync = [("on", settings.vsync), ("off", !settings.vsync)];
        if let Some(i) = row(ui, pointer, y, "vsync", &vsync) {
            settings.vsync = i == 0;
        }
        y += spacing;
        let labels = FPS_CAPS.map(|fps| {
            if fps == 0 {
                "none".to_string()
            } else {
                fps.to_string()
            }
        });
        let caps: Vec<(&str, bool)> = labels
            .iter()
            .zip(FPS_CAPS)
            .map(|(label, fps)| (label.as_str(), settings.fps_cap == fps))
            .collect();
        if let Some(i) = row(ui, pointer, y, "fps cap", &caps) {
            settings.fps_cap = FPS_CAPS[i];
        }
        y += spacing;

        // Volume: a thin line with a handle.
        ui.text(x, y + s, "volume", DIM);
        let width = 110.0 * s;
        let height = ch + 3.0 * s;
        if pointer.pressed && pointer.inside(values, y, width, height) {
            self.dragging_volume = true;
        }
        if !pointer.held {
            self.dragging_volume = false;
        }
        if self.dragging_volume {
            let level = (pointer.pos.0 - values) / width;
            // In whole percents, so the file and the number agree.
            settings.volume = (level.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        }
        let middle = (y + height * 0.5).round();
        let filled = width * settings.volume;
        ui.rect(values, middle, width, s, FAINT);
        ui.rect(values, middle, filled, s, AMBER);
        ui.rect(values + filled - s, y + s, 2.0 * s, height - 2.0 * s, TEXT);
        let percent = format!("{}", (settings.volume * 100.0).round() as u32);
        ui.text(values + width + 6.0 * s, y + s, &percent, TEXT);
        y += spacing + 8.0 * s;

        if *settings != before {
            actions.push(Action::SettingsChanged);
        }
        let (back, _) = button(ui, pointer, x, y, "back", false, true);
        if back {
            self.show(self.settings_from);
        }
    }

    fn draw_loading(&mut self, ui: &mut Ui, dt: f32, actions: &mut Vec<Action>) {
        let (w, h) = (ui.width, ui.height);
        let s = ui.scale;
        let mut alpha = 1.0;
        if let Some((next, t)) = &mut self.loaded {
            *t += dt;
            let next = *next;
            alpha = 1.0 - (*t / LOADING_FADE).min(1.0);
            if alpha <= 0.0 {
                self.loaded = None;
                if let Some(page) = next {
                    self.show(page);
                }
                actions.push(Action::LoadingDone);
                return;
            }
        }
        let start = ui.vertices.len();
        ui.rect(0.0, 0.0, w, h, [6, 9, 14, 255]);
        let x = Self::left(ui);
        let y = (h * 0.5).round();
        let text = self.loading.clone();
        ui.text(x, y, &text, DIM);
        // A short line running along under it.
        let width = 90.0 * s;
        let (_, ch) = ui.char_size();
        let line = y + ch + 2.0 * s;
        ui.rect(x, line, width, s, [150, 165, 180, 50]);
        let run = (self.clock * 0.8).fract();
        let pos = run * (width - 18.0 * s);
        ui.rect(x + pos, line, 18.0 * s, s, AMBER);
        ui.fade_from(start, alpha, 0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::save::WorldMeta;
    use std::path::PathBuf;

    /// Draws a frame of `menu` with the mouse at (x, y), the left button going down.
    fn click(
        menu: &mut Menu,
        settings: &mut Settings,
        worlds: &[WorldEntry],
        (x, y): (f32, f32),
    ) -> Vec<Action> {
        let mut input = Input::default();
        input.cursor = (x, y);
        input.button_down(MouseButton::Left);
        let mut ui = Ui::new(1920.0, 1080.0);
        let mut ctx = MenuContext {
            settings,
            worlds,
            now: 1000,
            in_world: false,
            session: MenuSession::Offline,
            players: 1,
            steam: false,
            friends: &[],
            time_rows: &[],
        };
        menu.draw(&mut ui, &input, &mut ctx, 0.016)
    }

    fn world(name: &str) -> WorldEntry {
        WorldEntry {
            dir: PathBuf::from(name),
            meta: WorldMeta {
                version: 1,
                name: name.to_string(),
                seed: 1,
                mode: GameMode::Survival,
                created: 1,
                last_played: 1,
                played: 0.0,
            },
        }
    }

    // At 1920x1080 a GUI pixel is 3 pixels and text is 27 tall; these follow from the layout.
    #[test]
    fn the_main_menu_responds_to_clicks() {
        let mut settings = Settings::default();
        let mut menu = Menu::new(Page::Main);
        let actions = click(&mut menu, &mut settings, &[], (170.0, 445.0));
        assert!(matches!(actions.as_slice(), [Action::ShowWorlds]));
        let actions = click(&mut menu, &mut settings, &[], (170.0, 493.0));
        assert!(matches!(actions.as_slice(), [Action::ShowMultiplayer]));
        let actions = click(&mut menu, &mut settings, &[], (170.0, 589.0));
        assert!(matches!(actions.as_slice(), [Action::Quit]));
        assert!(click(&mut menu, &mut settings, &[], (900.0, 445.0)).is_empty());
        click(&mut menu, &mut settings, &[], (170.0, 541.0));
        assert_eq!(menu.page, Page::Settings);
    }

    #[test]
    fn settings_change_when_clicked() {
        let mut settings = Settings::default();
        let mut menu = Menu::new(Page::Settings);
        // Vsync: off.
        let actions = click(&mut menu, &mut settings, &[], (410.0, 345.0));
        assert!(matches!(actions.as_slice(), [Action::SettingsChanged]));
        assert!(!settings.vsync);
        // Display: fullscreen.
        click(&mut menu, &mut settings, &[], (480.0, 283.0));
        assert!(settings.fullscreen);
        // Volume: half way along the line.
        click(&mut menu, &mut settings, &[], (342.0 + 165.0, 475.0));
        assert_eq!(settings.volume, 0.5);
        // Fps cap: the third choice (none, 30, 60).
        let x = 342.0 + (60.0 + 12.0 + 12.0) + (30.0 + 12.0 + 12.0) + 10.0;
        click(&mut menu, &mut settings, &[], (x, 408.0));
        assert_eq!(settings.fps_cap, 60);
    }

    #[test]
    fn a_world_is_chosen_with_one_click_and_played_with_two() {
        let mut settings = Settings::default();
        let worlds = [world("a"), world("b")];
        let mut menu = Menu::new(Page::Worlds);
        let spot = (240.0, 225.0 + 2.0 * 27.0 + 18.0);
        assert!(click(&mut menu, &mut settings, &worlds, spot).is_empty());
        assert_eq!(menu.selected, Some(1));
        let actions = click(&mut menu, &mut settings, &worlds, spot);
        assert!(matches!(actions.as_slice(), [Action::Play(1)]));
    }

    #[test]
    fn deleting_a_world_asks_first() {
        let mut settings = Settings::default();
        let worlds = [world("doomed")];
        let mut menu = Menu::new(Page::Worlds);
        click(&mut menu, &mut settings, &worlds, (240.0, 225.0));
        // play (4 letters), new world (9), delete (6): each 12 wider, 24 apart.
        let row = 1080.0 - 90.0 + 10.0;
        let delete = 144.0 + (60.0 + 12.0 + 24.0) + (135.0 + 12.0 + 24.0) + 20.0;
        assert!(click(&mut menu, &mut settings, &worlds, (delete, row)).is_empty());
        assert!(menu.confirm_delete);
        // Nothing is deleted without saying yes: the same spot is now the question's text.
        assert!(click(&mut menu, &mut settings, &worlds, (delete, row)).is_empty());
        assert!(menu.key(KeyCode::Escape, None).is_none());
        assert!(!menu.confirm_delete);
    }

    #[test]
    fn an_address_can_be_typed_and_joined() {
        let mut menu = Menu::new(Page::Multiplayer);
        menu.prepare_join("me", "");
        for c in "10.0.0.2:5000".chars() {
            menu.key(KeyCode::KeyA, Some(&c.to_string()));
        }
        menu.key(KeyCode::Tab, None);
        menu.key(KeyCode::KeyA, Some("x"));
        let action = menu.key(KeyCode::Enter, None);
        assert!(matches!(
            action,
            Some(Action::Join { name, address }) if name == "mex" && address == "10.0.0.2:5000"
        ));
        menu.key(KeyCode::Escape, None);
        assert_eq!(menu.page, Page::Main);
    }

    #[test]
    fn connecting_can_be_given_up_but_loading_cant() {
        let mut menu = Menu::connecting("connecting");
        assert!(matches!(
            menu.key(KeyCode::Escape, None),
            Some(Action::CancelJoin)
        ));
        menu.set_loading_text("downloading");
        assert!(menu.key(KeyCode::Escape, None).is_none());
    }

    #[test]
    fn typing_is_limited_to_what_the_font_can_show() {
        let mut menu = Menu::new(Page::Create);
        menu.key(KeyCode::KeyA, Some("héllo\n"));
        assert_eq!(menu.name, "hllo");
        for _ in 0..100 {
            menu.key(KeyCode::KeyA, Some("x"));
        }
        assert_eq!(menu.name.len(), MAX_NAME);
        menu.key(KeyCode::Tab, None);
        menu.key(KeyCode::Digit1, Some("1"));
        assert_eq!(menu.seed, "1");
        menu.key(KeyCode::Backspace, None);
        assert_eq!(menu.seed, "");
    }

    #[test]
    fn escape_goes_back_a_page() {
        let mut menu = Menu::new(Page::Main);
        menu.show(Page::Settings);
        assert!(menu.key(KeyCode::Escape, None).is_none());
        assert_eq!(menu.page, Page::Main);
        let mut menu = Menu::new(Page::Pause);
        menu.show(Page::Settings);
        menu.key(KeyCode::Escape, None);
        assert_eq!(menu.page, Page::Pause);
        assert!(matches!(
            menu.key(KeyCode::Escape, None),
            Some(Action::Resume)
        ));
    }
}
