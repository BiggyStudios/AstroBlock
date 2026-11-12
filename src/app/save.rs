//! What lives on disk: the settings, and the saved worlds.
//!
//! ```text
//! <data folder>/settings.json
//! <data folder>/worlds/<folder>/world.json    name, seed, game mode, dates
//! <data folder>/worlds/<folder>/state.json    player, time, inventory, chests, rockets, ...
//! <data folder>/worlds/<folder>/home.edits    blocks changed at home (`WorldEdits::to_bytes`)
//! <data folder>/worlds/<folder>/moon.edits    and on the moon
//! <data folder>/worlds/<folder>/players.json  what those who joined this world had
//! <data folder>/worlds/<folder>/backups/<unix time>[-crash]/   copies of the above (`write_backup`)
//! ```
//!
//! The data folder is `%APPDATA%\AstroBlock` on Windows (`ASTRO_SAVE_DIR` overrides it).
//! Terrain isn't saved: it is regenerated from the seed, and only the edits are stored.
//! JSON names its fields and enum variants, so adding a block or an item doesn't invalidate
//! old saves. Files are written whole to a temporary name and then renamed, so a crash
//! mid-save leaves the previous save intact.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::player::GameMode;

/// Bumped when a change makes older saves unreadable.
pub const SAVE_VERSION: u32 = 1;

pub const MAX_NAME: usize = 28;
pub const MAX_SEED: usize = 24;

/// Where saves go. Unattended captures only ever use an explicit `ASTRO_SAVE_DIR`, so a test
/// run can't touch real worlds.
pub fn root(unattended: bool) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ASTRO_SAVE_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    if unattended {
        return None;
    }
    if let Ok(dir) = std::env::var("APPDATA") {
        return Some(PathBuf::from(dir).join("AstroBlock"));
    }
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        return Some(PathBuf::from(dir).join("astro_block"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".local/share/astro_block"))
}

/// `Settings::mouse_sensitivity` goes from one over this to this.
pub const SENSITIVITY_RANGE: f32 = 4.0;

pub fn clamp_sensitivity(value: f32) -> f32 {
    value.clamp(1.0 / SENSITIVITY_RANGE, SENSITIVITY_RANGE)
}

/// The options on the settings screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub fullscreen: bool,
    pub vsync: bool,
    /// Frames per second at most (0 = no limit).
    pub fps_cap: u32,
    /// 0..1
    pub volume: f32,
    /// How fast the mouse turns the view, as a multiple of the normal speed.
    pub mouse_sensitivity: f32,
    /// Spreads mouse movement over a few frames (`input::MouseSmoother`).
    pub mouse_smoothing: bool,
    /// What others see, when not signed in to Steam.
    pub player_name: String,
    /// The address last connected to.
    pub last_address: String,
    /// Who this player is to a host (so it can give them their things back).
    pub client_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fullscreen: false,
            vsync: true,
            fps_cap: 0,
            volume: 1.0,
            mouse_sensitivity: 1.0,
            mouse_smoothing: true,
            player_name: String::new(),
            last_address: String::new(),
            client_id: String::new(),
        }
    }
}

impl Settings {
    /// The saved settings, or the defaults if there are none (or they're unreadable).
    pub fn load(root: Option<&Path>) -> Self {
        let mut settings: Self = root
            .and_then(|r| fs::read(r.join("settings.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        settings.mouse_sensitivity = clamp_sensitivity(settings.mouse_sensitivity);
        settings
    }

    /// Picks a `client_id` the first time. Returns whether it did.
    pub fn ensure_client_id(&mut self) -> bool {
        if !self.client_id.is_empty() {
            return false;
        }
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(unix_now());
        self.client_id = format!("{:016x}", hasher.finish());
        true
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        write_files(
            root,
            vec![(
                "settings.json".to_string(),
                serde_json::to_vec_pretty(self)?,
            )],
        )
    }
}

/// What the world list shows about a world.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldMeta {
    pub version: u32,
    pub name: String,
    pub seed: u64,
    pub mode: GameMode,
    /// Seconds since 1970.
    pub created: u64,
    pub last_played: u64,
    /// Seconds spent in it.
    pub played: f64,
}

#[derive(Clone, Debug)]
pub struct WorldEntry {
    pub dir: PathBuf,
    pub meta: WorldMeta,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Every saved world, most recently played first. Folders that aren't readable worlds are
/// left out.
pub fn list_worlds(root: &Path) -> Vec<WorldEntry> {
    let Ok(folders) = fs::read_dir(root.join("worlds")) else {
        return Vec::new();
    };
    let mut worlds: Vec<WorldEntry> = folders
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let bytes = fs::read(dir.join("world.json")).ok()?;
            let meta = serde_json::from_slice(&bytes).ok()?;
            Some(WorldEntry { dir, meta })
        })
        .collect();
    worlds.sort_by_key(|w| std::cmp::Reverse(w.meta.last_played));
    worlds
}

/// A folder name from a world's name: only safe characters.
fn folder_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '_',
        })
        .collect();
    let clean = clean.trim_matches('_');
    if clean.is_empty() {
        "world".to_string()
    } else {
        clean.to_string()
    }
}

/// A new, unused folder for a world called `name`.
pub fn new_world_dir(root: &Path, name: &str) -> PathBuf {
    let worlds = root.join("worlds");
    let base = folder_name(name);
    let mut dir = worlds.join(&base);
    let mut n = 2;
    while dir.exists() {
        dir = worlds.join(format!("{base}_{n}"));
        n += 1;
    }
    dir
}

/// A name not used by any of `worlds`, starting from `name`.
pub fn unique_name(worlds: &[WorldEntry], name: &str) -> String {
    let taken = |candidate: &str| worlds.iter().any(|w| w.meta.name == candidate);
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|n| format!("{name} {n}"))
        .find(|candidate| !taken(candidate))
        .expect("some number is free")
}

/// Deletes a world. Refuses anything but a world folder directly inside `root/worlds`.
pub fn delete_world(root: &Path, dir: &Path) -> Result<()> {
    if dir.parent() != Some(root.join("worlds").as_path()) || !dir.join("world.json").is_file() {
        bail!("{} is not a saved world", dir.display());
    }
    fs::remove_dir_all(dir).with_context(|| format!("deleting {}", dir.display()))
}

/// Text typed as a seed: a number is used as it is, anything else is hashed, and nothing
/// at all picks one at random.
pub fn parse_seed(text: &str) -> u64 {
    let text = text.trim();
    if text.is_empty() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        return crate::world::noise::mix64(nanos);
    }
    if let Ok(number) = text.parse::<u64>() {
        return number;
    }
    // FNV-1a.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Writes files into `dir`: each in full under a temporary name first, and only when all of
/// them are complete are they moved into place.
pub fn write_files(dir: &Path, files: Vec<(String, Vec<u8>)>) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let temporary = |name: &str| dir.join(format!("{name}.tmp"));
    for (name, bytes) in &files {
        fs::write(temporary(name), bytes).with_context(|| format!("writing {name}"))?;
    }
    for (name, _) in &files {
        fs::rename(temporary(name), dir.join(name)).with_context(|| format!("replacing {name}"))?;
    }
    Ok(())
}

/// A world's backups live in this folder inside it, one folder each (named by when, with
/// `-crash` for what was saved as the game went down): the newest `KEEP_BACKUPS` regular ones
/// and `KEEP_CRASHES` crash ones (so a game crashing again and again can't push out the rest).
pub const BACKUPS: &str = "backups";
pub const KEEP_BACKUPS: usize = 6;
pub const KEEP_CRASHES: usize = 2;

/// One of a world's backups.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub dir: PathBuf,
    /// When it was written (Unix seconds).
    pub time: u64,
    /// Saved as the game crashed.
    pub crash: bool,
}

/// Writes a backup of a world (the same files as its save), and forgets the oldest.
pub fn write_backup(world: &Path, files: Vec<(String, Vec<u8>)>, crash: bool) -> Result<()> {
    let name = format!("{}{}", unix_now(), if crash { "-crash" } else { "" });
    write_files(&world.join(BACKUPS).join(name), files)?;
    let all = backups(world);
    let (crashes, regular): (Vec<_>, Vec<_>) = all.into_iter().partition(|b| b.crash);
    let old = crashes
        .into_iter()
        .skip(KEEP_CRASHES)
        .chain(regular.into_iter().skip(KEEP_BACKUPS));
    for old in old {
        fs::remove_dir_all(&old.dir)
            .with_context(|| format!("removing the old backup {}", old.dir.display()))?;
    }
    Ok(())
}

/// A world's backups, newest first.
pub fn backups(world: &Path) -> Vec<Backup> {
    let Ok(entries) = fs::read_dir(world.join(BACKUPS)) else {
        return Vec::new();
    };
    let mut found: Vec<Backup> = entries
        .flatten()
        .filter(|e| e.path().join("state.json").is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (time, crash) = match name.strip_suffix("-crash") {
                Some(time) => (time, true),
                None => (name.as_str(), false),
            };
            Some(Backup {
                dir: e.path(),
                time: time.parse().ok()?,
                crash,
            })
        })
        .collect();
    found.sort_by(|a, b| b.time.cmp(&a.time).then(b.crash.cmp(&a.crash)));
    found
}

/// When a file was last written (Unix seconds; 0 if that can't be told).
pub fn written_at(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// "2h 05m", "12m"
pub fn format_played(seconds: f64) -> String {
    let minutes = (seconds / 60.0) as u64;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    }
}

/// "just now", "3 hours ago", "5 days ago"
pub fn format_ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(seconds / 60, "minute"),
        3_600..=86_399 => plural(seconds / 3_600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astro_save_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta(name: &str, last_played: u64) -> WorldMeta {
        WorldMeta {
            version: SAVE_VERSION,
            name: name.to_string(),
            seed: 42,
            mode: GameMode::Creative,
            created: 1,
            last_played,
            played: 90.0,
        }
    }

    #[test]
    fn worlds_are_listed_newest_first_and_junk_is_ignored() {
        let root = scratch("list");
        for (name, t) in [("old", 10), ("new", 20)] {
            let dir = new_world_dir(&root, name);
            write_files(
                &dir,
                vec![(
                    "world.json".into(),
                    serde_json::to_vec(&meta(name, t)).unwrap(),
                )],
            )
            .unwrap();
        }
        fs::create_dir_all(root.join("worlds/not_a_world")).unwrap();
        fs::write(root.join("worlds/not_a_world/world.json"), "{nope").unwrap();
        let names: Vec<String> = list_worlds(&root)
            .into_iter()
            .map(|w| w.meta.name)
            .collect();
        assert_eq!(names, ["new", "old"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backups_are_kept_newest_first_crashes_apart() {
        let world = scratch("backups");
        let files = || vec![("state.json".to_string(), b"{}".to_vec())];
        for t in 100..108 {
            write_files(&world.join(BACKUPS).join(t.to_string()), files()).unwrap();
        }
        for t in [50, 60, 70] {
            write_files(&world.join(BACKUPS).join(format!("{t}-crash")), files()).unwrap();
        }
        // (Not a backup: no save in it.)
        fs::create_dir_all(world.join(BACKUPS).join("99")).unwrap();
        write_backup(&world, files(), false).unwrap();

        let kept = backups(&world);
        let times: Vec<(u64, bool)> = kept.iter().map(|b| (b.time, b.crash)).collect();
        let now = unix_now();
        assert_eq!(times[0], (now, false), "{times:?}");
        let regular: Vec<u64> = times.iter().filter(|t| !t.1).map(|t| t.0).collect();
        assert_eq!(regular, vec![now, 107, 106, 105, 104, 103]);
        let crashes: Vec<u64> = times.iter().filter(|t| t.1).map(|t| t.0).collect();
        assert_eq!(crashes, vec![70, 60]);
        assert!(!world.join(BACKUPS).join("100").exists());
        assert!(!world.join(BACKUPS).join("50-crash").exists());
        fs::remove_dir_all(world).unwrap();
    }

    #[test]
    fn only_saved_worlds_can_be_deleted() {
        let root = scratch("delete");
        let dir = new_world_dir(&root, "keep me");
        assert!(dir.ends_with("worlds/keep_me"));
        // Not a world yet.
        fs::create_dir_all(&dir).unwrap();
        assert!(delete_world(&root, &dir).is_err());
        write_files(&dir, vec![("world.json".into(), b"{}".to_vec())]).unwrap();
        // Outside the worlds folder.
        assert!(delete_world(&root.join("worlds"), &dir.join("..")).is_err());
        assert!(delete_world(&root, &root).is_err());
        assert!(dir.exists());
        delete_world(&root, &dir).unwrap();
        assert!(!dir.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_names_are_safe_and_unique() {
        let root = scratch("names");
        assert!(new_world_dir(&root, "..\\..//evil").ends_with("worlds/evil"));
        assert!(new_world_dir(&root, "   ").ends_with("worlds/world"));
        let first = new_world_dir(&root, "My World");
        fs::create_dir_all(&first).unwrap();
        assert!(new_world_dir(&root, "my world").ends_with("worlds/my_world_2"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seeds() {
        assert_eq!(parse_seed(" 1234 "), 1234);
        assert_eq!(parse_seed("banana"), parse_seed("banana"));
        assert_ne!(parse_seed("banana"), parse_seed("bananas"));
    }

    #[test]
    fn settings_survive_and_tolerate_missing_fields() {
        let root = scratch("settings");
        let settings = Settings {
            fullscreen: true,
            vsync: false,
            fps_cap: 144,
            volume: 0.35,
            ..Settings::default()
        };
        settings.save(&root).unwrap();
        assert_eq!(Settings::load(Some(&root)), settings);
        // An older file without some options still loads, with defaults for the rest.
        fs::write(root.join("settings.json"), r#"{"fps_cap": 60}"#).unwrap();
        let partial = Settings::load(Some(&root));
        assert_eq!(partial.fps_cap, 60);
        assert!(partial.vsync);
        assert_eq!(partial.mouse_sensitivity, 1.0);
        // A number no slider could have set still leaves the mouse usable.
        fs::write(root.join("settings.json"), r#"{"mouse_sensitivity": 0}"#).unwrap();
        assert_eq!(Settings::load(Some(&root)).mouse_sensitivity, 0.25);
        fs::remove_dir_all(root).unwrap();
    }

    /// Positions must come back exactly: the world is at 1 AU in f64.
    #[test]
    fn numbers_come_back_exactly() {
        for x in [
            1.496e11_f64 + 0.123456789,
            1.0 / 3.0,
            43_900.000_000_1,
            -2.5e-300,
        ] {
            let text = serde_json::to_string(&x).unwrap();
            assert_eq!(
                serde_json::from_str::<f64>(&text).unwrap().to_bits(),
                x.to_bits()
            );
        }
    }
}
