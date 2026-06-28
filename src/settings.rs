//! Settings load/save and the persistence-format boundary.
//!
//! This module owns persisted playback state (previous station, volume,
//! favorites, theme) and the JSON-on-disk store. It is the only place that
//! knows the settings file format and location; the rest of the app consumes
//! always-valid typed values.
//!
//! Parse, don't validate: raw JSON is parsed once into typed domain values
//! ([`crate::model`] primitives and [`crate::theme::ThemeName`]) whose
//! constructors already reject impossible values. Invalid or corrupt files
//! surface as an `Err` from [`load`]/[`load_from`] so the caller can fall back
//! to [`Settings::default`] at the app boundary.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::{Station, VisualizerMode, VolumePercent};
use crate::theme::ThemeName;

/// Default startup volume used on first launch.
const DEFAULT_VOLUME: u8 = 60;

/// File name for the JSON settings document inside the config directory.
const SETTINGS_FILE: &str = "settings.json";

/// A collection of favorite stations that stays free of duplicates.
///
/// Two favorites are considered the same station when they share a
/// [`crate::model::StationId`] or a stream URL; display name is never used for
/// identity. Deduplication is the collection's responsibility so callers never
/// scatter favorite-equality checks. The on-disk form is a plain JSON array of
/// stations; deserialization re-runs deduplication so a hand-edited file still
/// yields an always-valid collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Vec<Station>", from = "Vec<Station>")]
pub struct Favorites(Vec<Station>);

impl Favorites {
    /// An empty favorites collection.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Build favorites from an iterator, dropping later duplicates.
    pub fn from_stations(stations: impl IntoIterator<Item = Station>) -> Self {
        let mut favorites = Self::new();
        for station in stations {
            favorites.add(station);
        }
        favorites
    }

    /// Add a station unless an equivalent one (same id or URL) already exists.
    ///
    /// Returns `true` when the station was added.
    pub fn add(&mut self, station: Station) -> bool {
        if self.contains(&station) {
            return false;
        }
        self.0.push(station);
        true
    }

    /// Remove any favorite equivalent to `station` (same id or URL).
    ///
    /// Returns `true` when something was removed.
    pub fn remove(&mut self, station: &Station) -> bool {
        let before = self.0.len();
        self.0.retain(|existing| !same_station(existing, station));
        self.0.len() != before
    }

    /// Whether an equivalent station (same id or URL) is already a favorite.
    pub fn contains(&self, station: &Station) -> bool {
        self.0
            .iter()
            .any(|existing| same_station(existing, station))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Station> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[Station] {
        &self.0
    }
}

/// Identity rule for favorites: same station id or same stream URL.
fn same_station(a: &Station, b: &Station) -> bool {
    a.id == b.id || a.url == b.url
}

impl From<Vec<Station>> for Favorites {
    fn from(stations: Vec<Station>) -> Self {
        Self::from_stations(stations)
    }
}

impl From<Favorites> for Vec<Station> {
    fn from(favorites: Favorites) -> Self {
        favorites.0
    }
}

/// Persisted playback state restored on the next launch.
///
/// All fields are typed domain values, so a successfully loaded `Settings` is
/// always valid by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub volume: VolumePercent,
    #[serde(default, deserialize_with = "deserialize_theme_or_default")]
    pub theme: ThemeName,
    #[serde(default, deserialize_with = "deserialize_visualizer_or_default")]
    pub visualizer: VisualizerMode,
    #[serde(default)]
    pub previous_station: Option<Station>,
    #[serde(default)]
    pub favorites: Favorites,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: VolumePercent::clamped(DEFAULT_VOLUME as i32),
            theme: ThemeName::Minimal,
            visualizer: VisualizerMode::SpectrumStack,
            previous_station: None,
            favorites: Favorites::new(),
        }
    }
}

fn deserialize_theme_or_default<'de, D>(deserializer: D) -> Result<ThemeName, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(ThemeName::parse_or_default(&raw))
}

/// Lenient visualizer-mode boundary: an unknown persisted mode falls back to the
/// default ([`VisualizerMode::SpectrumStack`]) instead of failing the whole load,
/// so one stale value never drops the rest of a user's settings.
fn deserialize_visualizer_or_default<'de, D>(deserializer: D) -> Result<VisualizerMode, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(VisualizerMode::parse_or_default(&raw))
}

/// Resolve the platform config path for the settings file.
///
/// Uses `directories::ProjectDirs` so the location follows OS conventions.
/// Returns an error only when no valid home/config directory is available.
pub fn config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("works", "takemo101", "radio")
        .context("could not determine a config directory for settings")?;
    Ok(dirs.config_dir().join(SETTINGS_FILE))
}

/// Load settings from the platform config path.
///
/// A missing file is not an error: it returns [`Settings::default`], which is
/// the safe first-launch state.
pub fn load() -> Result<Settings> {
    load_from(&config_path()?)
}

/// Save settings to the platform config path, creating directories as needed.
pub fn save(settings: &Settings) -> Result<()> {
    save_to(&config_path()?, settings)
}

/// Load settings from an explicit path (used for tests and custom locations).
///
/// A missing file yields safe defaults. A present-but-unreadable or invalid
/// file is an `Err`; callers that want a graceful boundary fall back with
/// `load_from(path).unwrap_or_default()`.
pub fn load_from(path: &Path) -> Result<Settings> {
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading settings file {}", path.display()))?;
    let settings = serde_json::from_str(&raw)
        .with_context(|| format!("parsing settings file {}", path.display()))?;
    Ok(settings)
}

/// Save settings to an explicit path, creating parent directories as needed.
pub fn save_to(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating settings directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).context("serializing settings to JSON")?;
    std::fs::write(path, json)
        .with_context(|| format!("writing settings file {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BitrateKbps, CodecKind, StationId, StationName, StationSource, StreamUrl, VisualizerMode,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Unique temp directory per test; never touches the real home directory.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("wave-tui-settings-{}-{}", std::process::id(), n));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn station(id: &str, url: &str, name: &str) -> Station {
        Station {
            id: StationId::new(id).unwrap(),
            name: StationName::new(name).unwrap(),
            url: StreamUrl::parse(url).unwrap(),
            homepage: None,
            country: Some("Japan".to_string()),
            language: Some("Japanese".to_string()),
            tags: vec!["news".to_string()],
            codec: CodecKind::Mp3,
            bitrate: Some(BitrateKbps::new(128).unwrap()),
            votes: Some(10),
            click_count: Some(20),
            source: StationSource::BuiltIn,
        }
    }

    #[test]
    fn default_settings_are_safe_for_first_launch() {
        let settings = Settings::default();
        assert_eq!(settings.volume.get(), 60);
        assert_eq!(settings.theme, ThemeName::Minimal);
        assert_eq!(settings.visualizer, VisualizerMode::SpectrumStack);
        assert!(settings.previous_station.is_none());
        assert!(settings.favorites.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips_selected_visualizer_mode() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            visualizer: VisualizerMode::WaveScope,
            ..Settings::default()
        };
        save_to(&path, &settings).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.visualizer, VisualizerMode::WaveScope);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn save_then_load_roundtrips_every_visualizer_mode() {
        // Every selectable mode (including SkylinePeaks) must survive a save/load
        // cycle unchanged, so the persisted `v` selection is stable.
        for mode in VisualizerMode::ALL {
            let dir = TempDir::new();
            let path = dir.path().join("settings.json");
            let settings = Settings {
                visualizer: mode,
                ..Settings::default()
            };
            save_to(&path, &settings).unwrap();
            let loaded = load_from(&path).unwrap();
            assert_eq!(loaded.visualizer, mode);
            assert_eq!(loaded, settings);
        }
    }

    #[test]
    fn load_falls_back_unknown_visualizer_without_dropping_other_settings() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let raw = r#"{
            "volume": 75,
            "theme": "neon",
            "visualizer": "hologram",
            "previous_station": null,
            "favorites": []
        }"#;
        std::fs::write(&path, raw).unwrap();

        let settings = load_from(&path).unwrap();

        // The unknown mode falls back, but every other setting is preserved.
        assert_eq!(settings.visualizer, VisualizerMode::SpectrumStack);
        assert_eq!(settings.volume, VolumePercent::new(75).unwrap());
        assert_eq!(settings.theme, ThemeName::Neon);
    }

    #[test]
    fn load_defaults_visualizer_when_absent_without_dropping_other_settings() {
        // A settings file written before this field existed must still load.
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let raw = r#"{
            "volume": 75,
            "theme": "neon",
            "previous_station": null,
            "favorites": []
        }"#;
        std::fs::write(&path, raw).unwrap();

        let settings = load_from(&path).unwrap();

        assert_eq!(settings.visualizer, VisualizerMode::SpectrumStack);
        assert_eq!(settings.volume, VolumePercent::new(75).unwrap());
        assert_eq!(settings.theme, ThemeName::Neon);
    }

    #[test]
    fn load_from_missing_file_returns_defaults() {
        let dir = TempDir::new();
        let path = dir.path().join("does-not-exist.json");
        let settings = load_from(&path).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn save_then_load_roundtrips_all_fields() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            volume: VolumePercent::new(42).unwrap(),
            theme: ThemeName::Crt,
            visualizer: VisualizerMode::MirrorWave,
            previous_station: Some(station("demo", "https://example.com/a.mp3", "Demo")),
            favorites: Favorites::from_stations([station(
                "fav",
                "https://example.com/fav.mp3",
                "Fav",
            )]),
        };
        save_to(&path, &settings).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = TempDir::new();
        let path = dir.path().join("nested/deeper/settings.json");
        assert!(!path.parent().unwrap().exists());
        save_to(&path, &Settings::default()).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn corrupt_settings_file_fails_but_can_fall_back_to_defaults() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ this is not valid json").unwrap();
        assert!(load_from(&path).is_err());
        // The app boundary recovers by choosing defaults.
        let recovered = load_from(&path).unwrap_or_default();
        assert_eq!(recovered, Settings::default());
    }

    #[test]
    fn load_parses_raw_json_into_typed_values() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let raw = r#"{
            "volume": 75,
            "theme": "neon",
            "previous_station": null,
            "favorites": []
        }"#;
        std::fs::write(&path, raw).unwrap();
        let settings = load_from(&path).unwrap();
        assert_eq!(settings.volume, VolumePercent::new(75).unwrap());
        assert_eq!(settings.theme, ThemeName::Neon);
    }

    #[test]
    fn load_rejects_out_of_range_volume_at_the_boundary() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"volume": 150, "theme": "minimal"}"#).unwrap();
        assert!(load_from(&path).is_err());
    }

    #[test]
    fn load_falls_back_unknown_theme_without_dropping_other_settings() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        let raw = r#"{
            "volume": 75,
            "theme": "aurora",
            "previous_station": null,
            "favorites": []
        }"#;
        std::fs::write(&path, raw).unwrap();

        let settings = load_from(&path).unwrap();

        assert_eq!(settings.volume, VolumePercent::new(75).unwrap());
        assert_eq!(settings.theme, ThemeName::Minimal);
    }

    #[test]
    fn save_then_load_roundtrips_every_theme() {
        // All six themes must survive a save/load cycle unchanged.
        for theme in [
            ThemeName::Minimal,
            ThemeName::Neon,
            ThemeName::Crt,
            ThemeName::Solarized,
            ThemeName::Midnight,
            ThemeName::Sakura,
        ] {
            let dir = TempDir::new();
            let path = dir.path().join("settings.json");
            let settings = Settings {
                theme,
                ..Settings::default()
            };
            save_to(&path, &settings).unwrap();
            let loaded = load_from(&path).unwrap();
            assert_eq!(loaded.theme, theme);
            assert_eq!(loaded, settings);
        }
    }

    #[test]
    fn favorites_deduplicate_by_station_id() {
        let mut favorites = Favorites::new();
        assert!(favorites.add(station("same-id", "https://a.example/1.mp3", "First")));
        // Same id, different URL and display name: still a duplicate.
        assert!(!favorites.add(station("same-id", "https://b.example/2.mp3", "Second")));
        assert_eq!(favorites.len(), 1);
    }

    #[test]
    fn favorites_deduplicate_by_stream_url() {
        let mut favorites = Favorites::new();
        assert!(favorites.add(station("id-one", "https://shared.example/s.mp3", "One")));
        // Different id, same URL: treated as the same station.
        assert!(!favorites.add(station("id-two", "https://shared.example/s.mp3", "Two")));
        assert_eq!(favorites.len(), 1);
    }

    #[test]
    fn favorites_allow_same_display_name_when_id_and_url_differ() {
        let mut favorites = Favorites::new();
        assert!(favorites.add(station("id-a", "https://a.example/x.mp3", "Same Name")));
        assert!(favorites.add(station("id-b", "https://b.example/y.mp3", "Same Name")));
        assert_eq!(favorites.len(), 2);
    }

    #[test]
    fn favorites_deserialization_drops_duplicates() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.json");
        // Two array entries share a URL; the loaded collection must dedupe.
        let raw = r#"{
            "volume": 60,
            "theme": "minimal",
            "favorites": [
                {"id":"a","name":"A","url":"https://dup.example/s.mp3","homepage":null,
                 "country":null,"language":null,"tags":[],"codec":"Unknown","bitrate":null,
                 "votes":null,"click_count":null,"source":"BuiltIn"},
                {"id":"b","name":"B","url":"https://dup.example/s.mp3","homepage":null,
                 "country":null,"language":null,"tags":[],"codec":"Unknown","bitrate":null,
                 "votes":null,"click_count":null,"source":"BuiltIn"}
            ]
        }"#;
        std::fs::write(&path, raw).unwrap();
        let settings = load_from(&path).unwrap();
        assert_eq!(settings.favorites.len(), 1);
    }

    #[test]
    fn favorites_iterate_in_insertion_order() {
        // The Favorites ListSource builds its visible list from this iteration,
        // so insertion order must be preserved (favorites are user-curated, not
        // re-ranked).
        let favorites = Favorites::from_stations([
            station("a", "https://a.example/1.mp3", "A"),
            station("b", "https://b.example/2.mp3", "B"),
            station("c", "https://c.example/3.mp3", "C"),
        ]);
        let ids: Vec<&str> = favorites.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn favorites_remove_uses_id_or_url_identity() {
        let mut favorites =
            Favorites::from_stations([station("id-one", "https://a.example/1.mp3", "One")]);
        // Equivalent by URL even though id and name differ.
        let removed = favorites.remove(&station("id-other", "https://a.example/1.mp3", "Other"));
        assert!(removed);
        assert!(favorites.is_empty());
    }
}
