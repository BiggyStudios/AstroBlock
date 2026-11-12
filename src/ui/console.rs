//! The debug console (toggled with the ` key): a line editor with history and a scrollback
//! log. Commands are executed by the game (`Game::run_command`).

const MAX_LINES: usize = 200;

#[derive(Default)]
pub struct Console {
    pub open: bool,
    pub input: String,
    /// Output, oldest first.
    pub lines: Vec<String>,
    history: Vec<String>,
    /// Position while browsing the history with the arrow keys.
    browsing: Option<usize>,
}

impl Console {
    pub fn print(&mut self, line: impl Into<String>) {
        let line = line.into();
        log::info!("console: {line}");
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            self.lines.remove(0);
        }
    }

    pub fn type_text(&mut self, text: &str) {
        self.input
            .extend(text.chars().filter(|c| !c.is_control() && *c != '`'));
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }

    /// Enter: returns the command line to run (and echoes it).
    pub fn submit(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.input).trim().to_string();
        self.browsing = None;
        if line.is_empty() {
            return None;
        }
        self.print(format!("> {line}"));
        if self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        Some(line)
    }

    /// Up (`older = true`) / down arrow: walk through earlier commands.
    pub fn browse(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        self.browsing = match (self.browsing, older) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i < last => Some(i + 1),
            (Some(_), false) => None,
        };
        self.input = self
            .browsing
            .map_or_else(String::new, |i| self.history[i].clone());
    }
}

/// Help text for the `help` command.
pub const HELP: &[&str] = &[
    "gamemode <survival|creative|spectator>   (or 0 / 1 / 3)",
    "tp <lat> <lon> [altitude]   teleport (degrees; altitude above the ground); tp <player>   go to them",
    "orbit [altitude km]   circular orbit above the current spot (default 20 km)",
    "launch <m/s>   push yourself straight up",
    "time <hours|sunrise|day|noon|sunset|night|midnight>   set the local time of day (you stay put)",
    "warp <factor>   your own time warp (also , and .)",
    "drag <on|off>   air drag",
    "speed <m/s>   spectator fly speed",
    "break / place   break the block you look at / use or place the held item",
    "give <item> [count]   e.g. give iron_pickaxe, give oak_log 16 (names: snake_case)",
    "kill / respawn / heal   die (items drop) / press the respawn button / restore health",
    "crash   crash the game on purpose (tests the crash save: the world opens where it was)",
    "fly   toggle creative flight (double-tap space)",
    "hurt <n>      take n damage (armor ignored)",
    "swing [secs]  swing the held item (optionally in slow motion, to inspect it)",
    "rain [n/s]    toggle a test rain shower (GPU particles; default 6000 drops/s)",
    "open [inventory|creative [tab]]   open an inventory screen (tab: terrain, building, factory, vehicles, materials, electronics, nuclear, gear, food)",
    "aim <pitch> [yaw]   point the camera (degrees)",
    "turn <deg>          turn the view to the right (left if negative)",
    "select <1-9> / throw [all]   pick a hotbar slot / drop the held item (like Q)",
    "summon <cow|pig|sheep>   bring an animal",
    "rocket [m] / board / exit   stand a test rocket ahead / get in the nearest / get out (anywhere)",
    "  fuel [percent]   in a rocket: what is in the tank, or set it; fuel infinite [on|off]   tanks stay full",
    "vehicle <plane|rover|rocket|digger|probe|gate> [m] / sit / aboard / cockpit   a ready-made vehicle / sit in the nearest / stand on it / view from the seat",
    "  vehicle take <x> <y> <z>   break that part off the nearest vehicle (grid meters from the core)",
    "  steer <pitch> <yaw> <roll> <drive>   holds those controls (a test without a keyboard)",
    "  power [percent] / drill [on|off]   in a vehicle: its batteries, or set them / its drills (G)",
    "power infinite [on|off]   vehicles' batteries stay full and factories never go short",
    "factory [status] / factory demo / factory turn   what the factories here do / a small ready-made one ahead / turn the conveyor you look at (R)",
    "  factory industry   oil, a miner, silicon, wire and power ahead; factory open <machine|power>; factory recipe <item>",
    "  factory power / chemistry / nuclear   steam, wind and an rtg / an electrolyzer and a blender making acid / a reactor and turbines",
    "  factory fill <fluid> [l] / drain / fluids   fill the tank you look at / empty every tank and pipe / what each pipe line holds",
    "net team <who> / net team leave   warp together with someone's team",
    "in a rocket: throttle <0-1>, sas [off|hold|pro|retro|norm|anti|rad+|rad-|node],",
    "  orbit <km> [moon], transfer (plan a moon transfer), node <pro> [norm] [rad] [in s],",
    "  dive <m/s> [deg] (test reentry), chase <yaw> <pitch> <m>",
    "lookat <moon|home|sun|rocket|tornado|volcano>   point the camera at it",
    "goto <moon|home> [lat lon]   stand on that world's ground (on foot)",
    "map   toggle the map view; mapcam <yaw> <pitch> <km> [craft|home|moon]",
    "weather [auto|clear|fair|cloudy|fog|drizzle|rain|storm|snow|blizzard|sandstorm|tornado]",
    "   what the sky is doing here, or hold a condition here; erupt   the nearest volcano",
    "world create <name> [seed] [mode], world load <name>, world leave, save",
    "settings [vsync on|off] [fps <n>] [volume <0-100>] [fullscreen on|off]",
    "   [sensitivity <25-400>] [smoothing on|off]",
    "net host [address:port] | join <address:port> | leave | status | vessels   play with others",
    "weather strike   a lightning bolt ahead; weather approach [m]   go near the tornado",
    "clear   clear the console",
];
