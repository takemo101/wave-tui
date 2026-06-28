# wave-tui Replacement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current ad-hoc external-player radio TUI with a polished work-session radio app featuring native Rust playback, real FFT visualization, online search, curated catalog, persistence, themes, and responsive layouts.

**Architecture:** Keep the existing Rust binary crate, but split the current `main.rs` into focused modules. Use a native audio pipeline inspired by `late-cli`: HTTP stream fetch/decode, CPAL output, ring buffer of played samples, RustFFT analysis, and channel-based updates to the TUI app state.

**Tech Stack:** Rust 2021, Ratatui, Crossterm, Tokio, Reqwest, Serde/JSON, Directories, CPAL, Symphonia, Ringbuf, RustFFT, Clap, Anyhow.

## Global Constraints

- Primary use case: work-session BGM TUI radio, not a full music library.
- Startup should auto-play the previous station unless `--no-auto-play` is passed.
- First launch or failed previous station starts silently with curated recommendations.
- Primary playback path must be Rust-native, not `ffplay`/`mpv`.
- MVP stream support is MP3/AAC-centered HTTP streams.
- FFT visualizer must use actual played audio samples.
- Search must be online-as-you-type with 300–500ms debounce and query cache.
- Persist previous station, volume, favorites, and selected theme.
- Include three built-in themes: Minimal, Neon, CRT.
- Default visual mood is Quiet Focus Pane; default theme should be Minimal unless a saved theme exists.
- Include Wide, Medium, and Compact layout tiers.
- Include ICY metadata when available.
- Use automated tests for core logic; manual verification for real audio/TUI rendering.
- Follow `docs/implementation-guidelines.md` for module boundaries, typed parsing,
  error handling, and UI/app responsibility separation.

## Technical Spike Baseline

`docs/audio-spike.md` records the native audio spike. It validated this path:

```text
HTTP stream -> Symphonia decode -> CPAL output -> played-sample mirror -> RustFFT
```

The successful spike command was:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Plan implications:

- Keep Rust-native playback as the primary implementation path.
- Do not blindly append `/stream` to Radio Browser `url_resolved` values.
- Support direct stream URLs without file extensions.
- Select a CPAL output config matching the stream sample rate when possible.
- Add resampling, or emit a clear unsupported-rate playback failure, before
  broad Radio Browser rollout.
- ICY title parsing and full `icy-metaint` demuxing are implemented in
  `src/audio/icy.rs` and wired through `src/audio/decoder.rs`; synthetic-byte
  tests cover metadata stripping and title-change events.

## Design Deck Baseline

The design deck selections define the UI contract for the first implementation:

- Overall personality: **Quiet Focus Pane**
  - calm dark UI, restrained contrast, not distracting during work
- Wide layout: **Search Console**
  - search input and ranked online results are the largest/primary region
- Medium/Compact layout: **Split Mini**
  - station list and Now Playing remain visible together
- Visualizer: **Spectrum Stack**
  - vertical FFT bars are the primary audio-reactive visual signature
- Themes: **High Contrast Trio**
  - Minimal, Neon, and CRT should look meaningfully different

Implementation implications:

- Build UI components around a quiet default, then let themes intensify it.
- Do not make compact mode a full-screen visualizer by default.
- Do not hide station context behind a drawer in MVP compact mode.
- Make search state visually prominent in Wide mode: query, loading, cached,
  offline/error, result count, and selected station.
- Use the same `SpectrumStack` rendering component across all layout tiers.

## Implementation Guidelines Baseline

`docs/implementation-guidelines.md` maps relevant `j5ik2o/okite-ai` skills to
this project. Apply these during every task:

- Package boundaries stop change waves; avoid god modules and catch-all modules.
- Keep a single Rust crate and use 2018 module style, not `mod.rs`.
- Build domain models and pure logic before adapters and terminal rendering.
- Use domain primitives/smart constructors for constrained values.
- Parse untrusted CLI/settings/API/catalog inputs once at the boundary.
- Wrap behavior-rich station/favorite/result vectors as first-class collections.
- Treat recoverable environment/remote failures as `Result` or events, not panics.
- Keep UI rendering from mutating nested app state directly; dispatch actions.

Do not apply full Clean Architecture, CQRS/Event Sourcing, or aggregate
transaction patterns unless the project grows beyond the current MVP scope.

---

## Target File Structure

### Modify

- `Cargo.toml`
  - Add native audio, CLI, and testing-friendly dependencies.
- `src/main.rs`
  - Shrink to CLI parse, terminal setup/teardown, app bootstrap, and error handling.
- `README.md`
  - Update controls, features, runtime requirements, and MVP behavior.

### Create

- `src/model.rs`
  - Shared domain models: `Station`, `StationId`, `StationSource`, `PlaybackState`, `NowPlaying`, `Section`, `Category`, `CodecKind`.
- `src/settings.rs`
  - Persistent settings load/save for previous station, volume, favorites, theme.
- `src/theme.rs`
  - `Theme`, built-in themes, theme lookup/cycling.
- `src/layout.rs`
  - Terminal-size-to-layout-tier selection and pane geometry policy.
- `src/catalog.rs`
  - Built-in curated catalog, station candidate validation state, ranking helpers.
- `src/search.rs`
  - Radio Browser client, search result normalization, ranking, query cache.
- `src/audio.rs`
  - Public audio runtime facade and events; declares private audio submodules.
- `src/audio/decoder.rs`
  - HTTP stream decoder using Symphonia.
- `src/audio/output.rs`
  - CPAL output stream and played-sample ring writer.
- `src/audio/analyzer.rs`
  - FFT analyzer producing visualizer bands and RMS.
- `src/audio/icy.rs`
  - ICY metadata parser/extractor.
- `src/app.rs`
  - App state, actions, reducers, focus model, debounce state, temporary failed stations.
- `src/ui.rs`
  - Ratatui rendering, delegated by layout tier.
- `src/cli.rs`
  - Clap-based CLI args.

### Tests

Use module-local tests initially:

- `src/settings.rs` tests: JSON roundtrip and defaults.
- `src/theme.rs` tests: lookup/cycle/fallback.
- `src/layout.rs` tests: tier selection.
- `src/catalog.rs` tests: ranking and temporary failure filtering.
- `src/search.rs` tests: cache and result ranking.
- `src/audio/analyzer.rs` tests: deterministic band normalization helpers.
- `src/audio/icy.rs` tests: metadata parsing.
- `src/app.rs` tests: actions update state correctly.

---

## Shared Interfaces

Define these early and keep names stable across tasks.

```rust
// src/model.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct StationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StationSource {
    BuiltIn,
    RadioBrowser,
    Favorite,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodecKind {
    Mp3,
    Aac,
    Other(String),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Station {
    pub id: StationId,
    pub name: String,
    pub url: String,
    pub homepage: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub codec: CodecKind,
    pub bitrate: Option<u32>,
    pub votes: Option<u32>,
    pub click_count: Option<u32>,
    pub source: StationSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Connecting,
    Playing,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VizFrame {
    pub bands: Vec<f32>,
    pub rms: f32,
}
```

```rust
// src/audio.rs
#[derive(Debug, Clone)]
pub enum AudioCommand {
    Play(Station, u8),
    Stop,
    SetVolume(u8),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Connecting(Station),
    Playing(Station),
    Stopped,
    Failed { station: Station, message: String },
    VolumeChanged(u8),
    Viz(crate::model::VizFrame),
    IcyTitle { station_id: crate::model::StationId, title: String },
}
```

---

### Task 1: Dependencies, module skeleton, and compile gate

**Files:**

- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Create: all module files listed above with minimal compiling definitions

**Interfaces:**

- Produces empty modules and dependency foundation used by every later task.

- [ ] **Step 1: Update dependencies**

Add these dependencies to `Cargo.toml`:

```toml
clap = { version = "4.5", features = ["derive"] }
cpal = "0.16"
ringbuf = "0.4"
rustfft = "6.4"
symphonia = { version = "0.5", features = ["mp3", "aac", "isomp4", "adts"] }
```

Keep existing dependencies unless a later task removes unused ones after migration.

- [ ] **Step 2: Create module skeletons**

Create each new file with either module declarations or minimal public structs/enums. `src/main.rs` should declare:

```rust
mod app;
mod audio;
mod catalog;
mod cli;
mod layout;
mod model;
mod search;
mod settings;
mod theme;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    cli::run()
}
```

Create `src/cli.rs`:

```rust
use anyhow::Result;
use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "wave-tui")]
pub struct CliArgs {
    #[arg(long)]
    pub theme: Option<String>,
    #[arg(long)]
    pub volume: Option<u8>,
    #[arg(long)]
    pub no_auto_play: bool,
    #[arg(long)]
    pub audio_output_device: Option<String>,
    #[arg(long)]
    pub low_power: bool,
    #[arg(long)]
    pub search: Option<String>,
}

pub fn run() -> Result<()> {
    let _args = CliArgs::parse();
    Ok(())
}
```

- [ ] **Step 3: Run compile gate**

Run:

```bash
cargo check
```

Expected: compilation succeeds, possibly with unused-code warnings.

---

### Task 2: Domain models and settings persistence

**Files:**

- Create/Modify: `src/model.rs`
- Create/Modify: `src/settings.rs`

**Interfaces:**

- Produces `Station`, `StationId`, `Settings`, `SettingsStore`.
- Later tasks consume settings for startup auto-play, favorites, volume, and theme.

- [ ] **Step 1: Add model definitions**

Implement the shared interfaces from this plan's “Shared Interfaces” section in `src/model.rs`.

- [ ] **Step 2: Add settings type**

Implement:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub volume: u8,
    pub theme: String,
    pub previous_station: Option<crate::model::Station>,
    pub favorites: Vec<crate::model::Station>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 60,
            theme: "minimal".to_string(),
            previous_station: None,
            favorites: Vec::new(),
        }
    }
}
```

Add functions:

```rust
pub fn config_path() -> anyhow::Result<std::path::PathBuf>;
pub fn load() -> anyhow::Result<Settings>;
pub fn save(settings: &Settings) -> anyhow::Result<()>;
```

Use `directories::ProjectDirs::from("works", "takemo101", "radio")` and store JSON at `config_dir()/settings.json`.

- [ ] **Step 3: Add tests**

Add tests in `src/settings.rs`:

```rust
#[test]
fn default_settings_are_safe_for_first_launch() {
    let settings = Settings::default();
    assert_eq!(settings.volume, 60);
    assert_eq!(settings.theme, "minimal");
    assert!(settings.previous_station.is_none());
    assert!(settings.favorites.is_empty());
}

#[test]
fn settings_roundtrip_json_preserves_favorites() {
    let station = crate::model::Station {
        id: crate::model::StationId("demo".to_string()),
        name: "Demo".to_string(),
        url: "https://example.com/stream.mp3".to_string(),
        homepage: None,
        country: Some("Japan".to_string()),
        language: Some("Japanese".to_string()),
        tags: vec!["news".to_string()],
        codec: crate::model::CodecKind::Mp3,
        bitrate: Some(128),
        votes: Some(10),
        click_count: Some(20),
        source: crate::model::StationSource::BuiltIn,
    };
    let settings = Settings {
        volume: 42,
        theme: "crt".to_string(),
        previous_station: Some(station.clone()),
        favorites: vec![station],
    };
    let json = serde_json::to_string(&settings).unwrap();
    let decoded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, settings);
}
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test settings
```

Expected: settings tests pass.

---

### Task 3: Themes and responsive layout policy

**Files:**

- Create/Modify: `src/theme.rs`
- Create/Modify: `src/layout.rs`

**Interfaces:**

- Produces `Theme`, `ThemeName`, `LayoutTier`, and `layout_tier(width, height)`.
- Later `ui.rs` consumes theme colors and layout tier.

- [ ] **Step 1: Implement themes**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Minimal,
    Neon,
    Crt,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: ThemeName,
    pub label: &'static str,
    pub background: ratatui::style::Color,
    pub foreground: ratatui::style::Color,
    pub dim: ratatui::style::Color,
    pub accent: ratatui::style::Color,
    pub accent_alt: ratatui::style::Color,
    pub good: ratatui::style::Color,
    pub warn: ratatui::style::Color,
    pub bad: ratatui::style::Color,
    pub spectrum_low: ratatui::style::Color,
    pub spectrum_mid: ratatui::style::Color,
    pub spectrum_high: ratatui::style::Color,
}

pub fn by_name(name: &str) -> Theme;
pub fn next_name(current: ThemeName) -> ThemeName;
```

- [ ] **Step 2: Implement layout tier selection**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutTier {
    Wide,
    Medium,
    Compact,
}

pub fn layout_tier(width: u16, height: u16) -> LayoutTier {
    if width >= 120 && height >= 28 {
        LayoutTier::Wide
    } else if width >= 82 && height >= 22 {
        LayoutTier::Medium
    } else {
        LayoutTier::Compact
    }
}
```

- [ ] **Step 3: Add tests**

```rust
#[test]
fn layout_tier_uses_wide_for_large_terminals() {
    assert_eq!(layout_tier(140, 40), LayoutTier::Wide);
}

#[test]
fn layout_tier_uses_medium_for_mid_sized_panes() {
    assert_eq!(layout_tier(100, 24), LayoutTier::Medium);
}

#[test]
fn layout_tier_uses_compact_for_small_panes() {
    assert_eq!(layout_tier(80, 20), LayoutTier::Compact);
}
```

Theme tests:

```rust
#[test]
fn unknown_theme_falls_back_to_minimal() {
    assert_eq!(by_name("missing").name, ThemeName::Minimal);
}

#[test]
fn theme_cycle_visits_all_builtin_themes() {
    assert_eq!(next_name(ThemeName::Minimal), ThemeName::Neon);
    assert_eq!(next_name(ThemeName::Neon), ThemeName::Crt);
    assert_eq!(next_name(ThemeName::Crt), ThemeName::Minimal);
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test theme layout
```

Expected: all theme/layout tests pass.

---

### Task 4: Curated catalog, ranking, and temporary failures

**Files:**

- Create/Modify: `src/catalog.rs`
- Modify: `src/model.rs` if additional category structs are needed

**Interfaces:**

- Produces `built_in_sections()`, `rank_stations()`, `SessionStationHealth`.
- Later app/search uses ranking and failed-station filtering.

- [ ] **Step 1: Implement section/category structs**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub stations: Vec<crate::model::Station>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: &'static str,
    pub label: &'static str,
    pub categories: Vec<Category>,
}
```

- [ ] **Step 2: Add built-in catalog**

Implement `pub fn built_in_sections() -> Vec<Section>` with small curated placeholders that are valid station records. Use stable IDs like:

- `music.lofi.radio-paradise`
- `music.ambient.soma-drone-zone`
- `music.jazz.adroit-jazz`
- `spoken.news.bbc-world-service`
- `spoken.news.nhk-radio-news`

Use real URLs only after manual validation. During this task, tests should not rely on network.

- [ ] **Step 3: Implement ranking**

```rust
pub fn station_score(station: &crate::model::Station) -> i64 {
    let codec = match station.codec {
        crate::model::CodecKind::Mp3 | crate::model::CodecKind::Aac => 1_000,
        crate::model::CodecKind::Other(_) => 100,
        crate::model::CodecKind::Unknown => 0,
    };
    let bitrate = station.bitrate.unwrap_or(0).min(320) as i64;
    let votes = station.votes.unwrap_or(0).min(10_000) as i64;
    let clicks = station.click_count.unwrap_or(0).min(10_000) as i64;
    codec + bitrate + votes / 10 + clicks / 20
}

pub fn rank_stations(mut stations: Vec<crate::model::Station>) -> Vec<crate::model::Station> {
    stations.sort_by_key(|s| std::cmp::Reverse(station_score(s)));
    stations
}
```

- [ ] **Step 4: Implement session failure state**

```rust
#[derive(Debug, Default, Clone)]
pub struct SessionStationHealth {
    failed: std::collections::HashSet<crate::model::StationId>,
}

impl SessionStationHealth {
    pub fn mark_failed(&mut self, id: crate::model::StationId) { self.failed.insert(id); }
    pub fn is_failed(&self, id: &crate::model::StationId) -> bool { self.failed.contains(id) }
    pub fn retain_playable(&self, stations: Vec<crate::model::Station>) -> Vec<crate::model::Station> {
        stations.into_iter().filter(|s| !self.is_failed(&s.id)).collect()
    }
}
```

- [ ] **Step 5: Add tests**

Test ranking prefers MP3/AAC and popular stations. Test `mark_failed` removes a station from `retain_playable`.

- [ ] **Step 6: Run tests**

```bash
cargo test catalog
```

Expected: catalog tests pass.

---

### Task 5: Radio Browser online search with cache

**Files:**

- Create/Modify: `src/search.rs`
- Modify: `src/model.rs` if Radio Browser response fields require helpers

**Interfaces:**

- Produces `RadioBrowserClient`, `SearchCache`, `SearchResult`.
- App uses this from a debounced task.

- [ ] **Step 1: Define client and cache**

```rust
#[derive(Debug, Clone)]
pub struct RadioBrowserClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Default)]
pub struct SearchCache {
    entries: std::collections::HashMap<String, Vec<crate::model::Station>>,
}

impl SearchCache {
    pub fn get(&self, query: &str) -> Option<Vec<crate::model::Station>>;
    pub fn insert(&mut self, query: &str, stations: Vec<crate::model::Station>);
}
```

- [ ] **Step 2: Implement response normalization**

Deserialize Radio Browser fields:

```rust
#[derive(Debug, serde::Deserialize)]
struct RadioBrowserStation {
    stationuuid: Option<String>,
    name: String,
    url_resolved: String,
    homepage: Option<String>,
    country: Option<String>,
    language: Option<String>,
    tags: Option<String>,
    codec: Option<String>,
    bitrate: Option<u32>,
    votes: Option<u32>,
    clickcount: Option<u32>,
}
```

Map codec strings to `CodecKind::Mp3`, `CodecKind::Aac`, `Other`, or `Unknown`.

- [ ] **Step 3: Implement online search**

Use Radio Browser endpoint such as:

```text
/json/stations/search?name=<query>&hidebroken=true&limit=50&order=votes&reverse=true
```

Return normalized and ranked stations.

- [ ] **Step 4: Add cache tests**

No network in unit tests. Test `SearchCache` returns cloned cached results and distinguishes query strings after trimming/lowercasing if implemented.

- [ ] **Step 5: Run tests**

```bash
cargo test search
```

Expected: search cache and normalization tests pass.

---

### Task 6: Audio analyzer and ICY parser in isolation

**Files:**

- Create/Modify: `src/audio/analyzer.rs`
- Create/Modify: `src/audio/icy.rs`
- Modify: `src/audio.rs`

**Interfaces:**

- Produces deterministic analyzer helpers and ICY parsing used by runtime.

- [ ] **Step 1: Implement analyzer config and helpers**

Create:

```rust
#[derive(Debug, Clone)]
pub struct AnalyzerConfig {
    pub fft_size: usize,
    pub band_count: usize,
    pub gain: f32,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        Self { fft_size: 1024, band_count: 16, gain: 3.0 }
    }
}

pub fn soft_compress(x: f32) -> f32 {
    let k = 2.0;
    (k * x) / (1.0 + k * x)
}

pub fn normalize_value(x: f32, gain: f32) -> f32 {
    let amplified = x * gain;
    if amplified >= 100.0 {
        1.0
    } else {
        soft_compress(amplified).clamp(0.0, 1.0)
    }
}
```

- [ ] **Step 2: Implement ICY title extraction**

```rust
pub fn parse_stream_title(metadata: &str) -> Option<String> {
    let marker = "StreamTitle='";
    let start = metadata.find(marker)? + marker.len();
    let rest = &metadata[start..];
    let end = rest.find("';")?;
    let title = rest[..end].trim();
    if title.is_empty() { None } else { Some(title.to_string()) }
}
```

- [ ] **Step 3: Add tests**

```rust
#[test]
fn parses_icy_stream_title() {
    assert_eq!(
        parse_stream_title("StreamTitle='Artist - Track';StreamUrl='';"),
        Some("Artist - Track".to_string())
    );
}

#[test]
fn ignores_empty_stream_title() {
    assert_eq!(parse_stream_title("StreamTitle='';"), None);
}

#[test]
fn normalize_value_clamps_to_one() {
    assert_eq!(normalize_value(100.0, 3.0), 1.0);
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test audio
```

Expected: analyzer and ICY tests pass.

---

### Task 7: Native audio runtime

**Files:**

- Create/Modify: `src/audio.rs`
- Create/Modify: `src/audio/decoder.rs`
- Create/Modify: `src/audio/output.rs`
- Create/Modify: `src/audio/analyzer.rs`

**Interfaces:**

- Consumes `AudioCommand`, `AudioEvent`, `Station`, `VizFrame`.
- Produces `AudioRuntime::spawn(...) -> AudioHandle`.

- [ ] **Step 1: Implement public handle**

```rust
pub struct AudioHandle {
    pub command_tx: std::sync::mpsc::Sender<AudioCommand>,
    pub event_rx: std::sync::mpsc::Receiver<AudioEvent>,
}

pub struct AudioRuntimeConfig {
    pub output_device: Option<String>,
    pub low_power: bool,
}

pub fn spawn(config: AudioRuntimeConfig) -> AudioHandle;
```

- [ ] **Step 2: Implement decoder based on late-cli pattern**

Use `reqwest::blocking::get`, `symphonia::default::get_probe`, and `SampleBuffer<f32>` to yield interleaved `f32` samples.

Important spike finding: treat Radio Browser `url_resolved` as a direct stream URL first. Do not blindly append `/stream` to arbitrary station URLs; only curated base URLs should opt into that behavior.

Decoder must expose:

```rust
pub struct StreamDecoder { /* private fields */ }
impl StreamDecoder {
    pub fn new_http(url: &str) -> anyhow::Result<Self>;
    pub fn sample_rate(&self) -> u32;
    pub fn channels(&self) -> usize;
}
impl Iterator for StreamDecoder { type Item = f32; }
```

- [ ] **Step 3: Implement CPAL output**

Create a bounded sample queue and write samples to the selected/default CPAL output stream. Mirror played mono-mixed samples into a separate ring for analyzer.

Use a CPAL output config matching the stream sample rate when available. If the selected device cannot output that rate, either resample before enqueueing samples or return a clear unsupported-rate error that marks the station failed. The spike proved 44.1 kHz direct playback works, but also showed resampling is required for robust MVP coverage.

- [ ] **Step 4: Wire command loop**

Audio thread behavior:

- `Play(station, volume)` stops previous stream, emits `Connecting`, starts decoder/output/analyzer, emits `Playing`.
- `Stop` stops current stream and emits `Stopped`.
- `SetVolume(v)` updates volume and emits `VolumeChanged(v)`.
- Decoder/output errors emit `Failed { station, message }`.

- [ ] **Step 5: Manual smoke test command**

Use the existing spike binary as the first smoke test before integrating audio
into the TUI:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Expected: audio plays through the default CPAL output device and the terminal
prints changing `fft ...` bars.

After the runtime is wired into the app, run:

```bash
cargo run -- --no-auto-play
```

Expected: app starts without attempting audio until the user selects a station;
there is no panic.

---

### Task 8: App state, actions, focus, and debounce model

**Files:**

- Create/Modify: `src/app.rs`

**Interfaces:**

- Consumes settings, catalog, search, audio events.
- Produces `App`, `Action`, focus model, and state update methods used by `ui.rs` and `main.rs`.

- [ ] **Step 1: Define app state**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane { Sections, Stations, Search, Player }

#[derive(Debug, Clone)]
pub struct App {
    pub settings: crate::settings::Settings,
    pub focus: FocusPane,
    pub search_query: String,
    pub search_pending: bool,
    pub sections: Vec<crate::catalog::Section>,
    pub visible_stations: Vec<crate::model::Station>,
    pub selected_station: usize,
    pub playback_state: crate::model::PlaybackState,
    pub now_playing_title: Option<String>,
    pub viz: crate::model::VizFrame,
    pub health: crate::catalog::SessionStationHealth,
    pub offline: bool,
}
```

- [ ] **Step 2: Implement actions**

```rust
#[derive(Debug, Clone)]
pub enum Action {
    MoveNext,
    MovePrevious,
    FocusNext,
    FocusPrevious,
    SearchChanged(String),
    SearchResults(Vec<crate::model::Station>),
    PlaySelected,
    ToggleStopPlay,
    ToggleFavorite,
    CycleTheme,
    Audio(crate::audio::AudioEvent),
    SetOffline(bool),
}
```

- [ ] **Step 3: Implement reducer methods**

Implement `App::new(settings, sections)`, `App::apply(action)`, `App::selected_station()`, and helpers for favorites/theme/failed station updates.

- [ ] **Step 4: Add tests**

Tests should cover:

- failed audio event marks station failed
- favorite toggle adds/removes by station id
- focus cycles through panes
- search results replace visible stations and reset selection

- [ ] **Step 5: Run tests**

```bash
cargo test app
```

Expected: app reducer tests pass.

---

### Task 9: Terminal UI rendering

**Files:**

- Create/Modify: `src/ui.rs`
- Modify: `src/app.rs` if UI needs derived helpers

**Interfaces:**

- Consumes `App`, `Theme`, `LayoutTier`.
- Produces `pub fn draw(f: &mut ratatui::Frame, app: &mut App, theme: &Theme)`.

- [ ] **Step 1: Implement top-level draw**

```rust
pub fn draw(f: &mut ratatui::Frame, app: &mut crate::app::App, theme: &crate::theme::Theme) {
    match crate::layout::layout_tier(f.area().width, f.area().height) {
        crate::layout::LayoutTier::Wide => draw_wide(f, app, theme),
        crate::layout::LayoutTier::Medium => draw_medium(f, app, theme),
        crate::layout::LayoutTier::Compact => draw_compact(f, app, theme),
    }
}
```

- [ ] **Step 2: Wide Search Console layout**

Render:

- header/status strip
- prominent search input with loading/cache/offline/result-count state
- largest region: ranked online search results or current station list
- side region: Music and Spoken/News section/category shortcuts
- right region: Now Playing with Spectrum Stack FFT bars
- footer key hints

Wide mode success condition: a user can type a query, compare results, play a
station, favorite it, and still see the current stream status without changing
screens.

- [ ] **Step 3: Medium Split Mini layout**

Render a balanced station list plus player view:

- station/search list stays visible with enough rows to be useful
- Now Playing and Spectrum Stack stay visible at the same time
- reduce metadata detail before hiding either major region

- [ ] **Step 4: Compact Split Mini layout**

Render a reduced two-part layout:

- top: 3-6 station/search rows, depending on height
- middle: current station, ICY title if available, compact Spectrum Stack
- bottom: one-line status and key hints

Compact mode must not default to a full-screen visualizer or hidden drawer. The
user should be able to change stations with visible context.

- [ ] **Step 5: Manual rendering check**

Run in terminals roughly:

```bash
cargo run -- --no-auto-play
```

Manually resize to:

- Wide: ≥120x28
- Medium: about 100x24
- Compact: about 80x20 or smaller

Expected: no overlaps, footer visible, focused pane visually distinct.

---

### Task 10: Main event loop integration

**Files:**

- Modify: `src/main.rs`
- Modify: `src/cli.rs`
- Modify: `src/app.rs`
- Modify: `src/settings.rs`

**Interfaces:**

- Wires CLI, settings, catalog, audio runtime, search debounce, and UI.

- [ ] **Step 1: Move terminal setup into main**

Use current `main.rs` terminal setup/teardown pattern, but call `ui::draw` and `App::apply`.

- [ ] **Step 2: Apply CLI overrides**

Startup order:

1. Parse CLI.
2. Load settings or default.
3. Apply `--theme`, `--volume`, `--no-auto-play`, `--low-power`, `--audio-output-device`.
4. Build catalog sections.
5. Spawn audio runtime.
6. Create app state.
7. If auto-play is enabled and previous station exists, send `AudioCommand::Play(previous, volume)`.

- [ ] **Step 3: Implement key handling**

Map keys:

- `q`: quit
- `Esc`: back/clear search/quit depending context
- `Tab`: `Action::FocusNext`
- `BackTab`: `Action::FocusPrevious`
- `j`/Down: `Action::MoveNext`
- `k`/Up: `Action::MovePrevious`
- `Enter`: play selected
- `Space`: stop/play toggle
- `/`: focus search and clear/prepare input
- `+`/`=`: volume +5 or +10
- `-`: volume -5 or -10
- `f`: toggle favorite
- `t`: cycle theme

- [ ] **Step 4: Implement search debounce loop**

Use a timestamp or generation counter. When search query changes, spawn/update a debounced async task. Ignore stale results by query generation.

- [ ] **Step 5: Persist on changes and shutdown**

Persist when:

- volume changes
- theme changes
- favorite toggles
- a station successfully starts playing
- app exits cleanly

- [ ] **Step 6: Run full check**

```bash
cargo test
cargo check
```

Expected: all tests pass and app compiles.

---

### Task 11: ICY metadata integration

Status: implemented by MIK-011. `src/audio/icy.rs` provides the
`icy-metaint` demux/read adapter, `src/audio/decoder.rs` requests and activates
ICY framing when present, and app/UI surfaces change-only ICY titles for the
current station.

**Files:**

- Modify: `src/audio/decoder.rs`
- Modify: `src/audio/icy.rs`
- Modify: `src/audio.rs`
- Modify: `src/app.rs`
- Modify: `src/ui.rs`

**Interfaces:**

- Audio runtime emits `AudioEvent::IcyTitle`.
- App stores `now_playing_title`.
- UI displays title when present.

- [x] **Step 1: Request ICY metadata**

When opening HTTP streams, send header:

```text
Icy-MetaData: 1
```

Read `icy-metaint` response header when present.

- [x] **Step 2: Extract metadata blocks**

For streams with `icy-metaint`, split audio bytes and metadata blocks before feeding audio bytes to Symphonia. Emit title changes through audio event channel.

- [x] **Step 3: App/UI handling**

On `AudioEvent::IcyTitle`, update `app.now_playing_title` if station id matches current station.

Now Playing display priority:

1. ICY title if present
2. Station name
3. Tags/country/codec/bitrate as secondary metadata

- [ ] **Step 4: Manual test against ICY station**

Use a known ICY station from built-in catalog.

Expected: title appears when station provides metadata; no crash when it does not.

---

### Task 12: Offline state, docs, and final verification

**Files:**

- Modify: `src/app.rs`
- Modify: `src/ui.rs`
- Modify: `src/search.rs`
- Modify: `README.md`
- Modify: `docs/SPEC.md` only if implementation intentionally changes scope

**Interfaces:**

- Offline state is visible in UI and does not prevent retrying cached/favorite/built-in stations.

- [ ] **Step 1: Search/network failures set offline state**

If Radio Browser search or catalog validation fails due to network-like errors, dispatch `Action::SetOffline(true)`.

Clear offline state after a successful online search or validation.

- [ ] **Step 2: Render offline screen/banner**

Wide/Medium/Compact should all show clear offline state. Do not hide previous/favorite/built-in retry options.

- [ ] **Step 3: Update README**

Document:

- purpose
- controls
- auto-play previous station
- built-in catalog and online search
- favorites persistence
- themes
- native playback and supported formats
- `--no-auto-play`, `--theme`, `--volume`, `--audio-output-device`, `--low-power`
- troubleshooting for no audio output
- implementation principles from `docs/implementation-guidelines.md`

- [ ] **Step 4: Final automated verification**

Run:

```bash
cargo fmt --check
cargo test
cargo check
```

Expected: all pass.

- [ ] **Step 5: Final manual verification checklist**

Verify manually:

- first launch without settings starts silently with recommendations
- selecting a station plays audio
- visualizer reacts to real audio
- quitting and relaunching resumes previous station
- `--no-auto-play` starts silently
- search updates while typing
- favorite persists across restart
- theme cycles and persists
- failed station is temporarily disabled
- offline banner appears during network failure
- wide/medium/compact layouts do not overlap
- changed modules satisfy `docs/implementation-guidelines.md` review checklist

---

## Self-Review

### Spec Coverage

Covered by plan:

- Native Rust playback: Tasks 6–7
- True FFT: Tasks 6–7 and UI in Task 9
- ICY metadata: Task 11
- Previous station/volume/favorites/theme persistence: Tasks 2 and 10
- Online search with debounce/cache: Tasks 5 and 10
- Built-in curated catalog: Task 4
- Temporary failed-station handling: Tasks 4 and 8
- Responsive layouts: Tasks 3 and 9
- Three themes: Task 3 and UI in Task 9
- CLI options: Tasks 1 and 10
- Offline state: Task 12
- Tests: Each core task includes tests; final gate in Task 12

### Placeholder Scan

No task uses TBD/TODO/fill-in placeholders as requirements. Some curated station URLs must be validated during implementation before finalizing the built-in catalog; the implementation task explicitly says to use real URLs only after manual validation and not to rely on network in tests.

### Type Consistency

Shared type names used across tasks:

- `Station`, `StationId`, `CodecKind`, `StationSource`
- `VizFrame`
- `AudioCommand`, `AudioEvent`
- `Settings`
- `Theme`, `ThemeName`
- `LayoutTier`
- `SessionStationHealth`
- `App`, `Action`, `FocusPane`

These names are defined before being consumed by later tasks.
