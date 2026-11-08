//! What lives on disk: the settings, and the saved worlds.
//!
//! ```text
//! <data folder>/settings.json
//! <data folder>/worlds/<folder>/world.json    name, seed, game mode, dates
//! <data folder>/worlds/<folder>/state.json    player, time, inventory, chests, rockets, ...
//! <data folder>/worlds/<folder>/home.edits    blocks changed at home (`WorldEdits::to_bytes`)
//! <data folder>/worlds/<folder>/moon.edits    and on the moon
//! <data folder>/worlds/<folder>/players.json  what those who joined this world had
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
            player_name: String::new(),
            last_address: String::new(),
            client_id: String::new(),
        }
    }
}

impl Settings {
    /// The saved settings, or the defaults if there are none (or they're unreadable).
    pub fn load(root: Option<&Path>) -> Self {
        root.and_then(|r| fs::read(r.join("settings.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
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
