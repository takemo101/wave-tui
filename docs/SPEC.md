# wave-tui Replacement Specification

## Purpose

`wave-tui` is a terminal-first internet radio player for work sessions. It should live comfortably in a terminal/herdr pane, resume the user's background audio quickly, and provide a polished cliamp-inspired visual experience.

The replacement should preserve the current project's spirit — a lightweight Rust TUI radio — but replace the current ad-hoc implementation with a coherent product and technical design.

## Current State Analysis

> **Note (post-replacement):** This section records the *pre-replacement*
> baseline that motivated the redesign. The replacement described in the rest of
> this document is now implemented (native playback, real FFT, online search,
> persistence, themes, layouts, ICY, and offline handling). It is kept as
> historical context; see "Success Criteria" below for the current verification
> status.

Current repository shape:

- Rust binary crate: `wave-tui`
- Main UI and app state are concentrated in `src/main.rs` (~623 lines)
- `src/api.rs` fetches Radio Browser top-voted stations via blocking `reqwest`
- Playback is delegated to external `ffplay`, falling back to `mpv`
- Visualizer is simulated; it is not linked to audio samples
- Favorites exist only in memory and are lost on exit
- Search is local filtering over the already-loaded top-vote list
- Genre discovery is a fixed tag cycle over the same loaded list
- Config/state persistence is not implemented

Important current limitations:

- No real audio analysis/FFT
- No persisted last station, volume, theme, or favorites
- No true online search-as-you-type
- No station health handling beyond a status message
- No responsive layout model
- No theme system
- No ICY/Shoutcast now-playing metadata
- Playback control is limited by the external player process model

## Product Direction

### Primary Use Case

The app is a **work-session BGM TUI radio**, not a full music library or Spotify-like player.

It should prioritize:

- quick resume of the previous station
- low-friction station discovery
- pleasant always-on visual presence
- stable background playback
- minimal cognitive load while working

### UX Inspiration

Visual inspiration: [`cliamp`](https://github.com/bjarneo/cliamp)

Desired visual qualities:

- quiet enough to live beside work for hours
- black/dark terminal canvas
- crisp status bars and panel borders
- responsive layouts that look intentional at different pane sizes
- real spectrum/level-meter visualizer from playback audio
- themes that range from restrained focus mode to high-contrast neon/CRT

Design deck decision:

- Overall personality: **Quiet Focus Pane**
  - Default mood should be calm, low-saturation, and non-distracting.
  - The app can still become vivid through theme choice and FFT movement.
- Wide layout: **Search Console**
  - Online search and result quality are the primary wide-screen workflow.
- Medium/Compact layout: **Split Mini**
  - Keep both station list and Now Playing visible in constrained panes.
- Visualizer: **Spectrum Stack**
  - Use cliamp-like vertical FFT bars as the recognizable visual signature.
- Theme set: **High Contrast Trio**
  - Minimal, Neon, and CRT should feel meaningfully different.

Reference implementation for audio architecture: [`late-sh` / `late-cli`](https://github.com/mpiorowski/late-sh), especially its `cpal` + `symphonia` + `rustfft` pipeline.

Implementation guidance is captured in `docs/implementation-guidelines.md`. It
adopts the relevant parts of `j5ik2o/okite-ai` skills: package boundaries,
Rust module style, domain-model-first development, domain primitives,
Parse-Don't-Validate, first-class collections, and recoverability-based error
handling.

## Core Decisions

### Startup Behavior

- On normal launch, automatically replay the previous station.
- On first launch, or if the previous station cannot be played, start silently and show recommended stations.
- Startup should not block on validating every candidate station.

### Playback Engine

- Replace external `ffplay`/`mpv` as the primary engine.
- Build a Rust-native playback path:
  - HTTP stream fetch via `reqwest`
  - decoding via `symphonia`
  - audio output via `cpal`
  - playback sample ring buffer via `ringbuf`
  - FFT analysis via `rustfft`
- True FFT-linked visualizer is a core requirement.
- Initial supported stream formats: MP3/AAC-centered HTTP streams.
- Treat Radio Browser `url_resolved` values as direct stream URLs first; do not
  blindly append `/stream` to arbitrary station URLs.
- Include sample-rate handling for CPAL output. Prefer a device config matching
  the stream rate, and add resampling or an explicit unsupported-rate failure
  path before broad station rollout.
- Non-supported or broken stations are treated as playback failures.
- The MVP should not depend on `mpv` for normal playback.

#### Native Audio Spike Findings

The technical spike in `docs/audio-spike.md` successfully played a live MP3
radio stream through the Rust-native path and printed changing FFT bars from
actual playback samples.

Validated path:

```text
HTTP stream -> Symphonia decode -> CPAL output -> played-sample mirror -> RustFFT
```

Spike command that succeeded:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Implementation consequences:

- Keep the Rust-native player as the primary playback architecture.
- Treat arbitrary Radio Browser URLs as direct stream URLs first.
- Add `/stream` only for curated base URLs that explicitly require it.
- Choose CPAL output configs that match stream sample rate when possible.
- Add resampling, or a clear unsupported-rate failure path, before relying on
  broad Radio Browser playback.
- ICY title parsing and full `icy-metaint` stream splitting are implemented in
  the main audio path. `src/audio/icy.rs` strips metadata bytes before Symphonia
  decoding and `src/audio/decoder.rs` requests/activates ICY framing when
  available.

### Failed Stations

When a station fails to play:

- Mark it temporarily unavailable for the current session.
- Display it as disabled/dimmed or move it out of the active candidate set.
- Present or select the next viable candidate.
- Do not permanently blacklist it in MVP.

### Catalog Model

The app uses **built-in curated station candidates plus online Radio Browser search**.

Built-in catalog:

- split into `Music` and `Spoken / News`
- small number of categories
- each category has curated station candidates
- candidates include enough metadata for display, ranking, and validation

Initial category direction:

- Music: Lofi, Ambient, Jazz, Classical, Electronic, plus possibly a small Chill/Focus grouping
- Spoken / News: Japanese and English news/talk candidates

Station reachability is treated as a runtime concern in the MVP. Candidates are
parsed into typed values up front, then broken or unsupported streams are marked
as session-only failures after playback attempts; the app does not run a separate
background catalog validation job.

### Search

Search is online as the user types.

Requirements:

- 300–500ms debounce
- cancel/ignore stale in-flight searches
- cache repeated query results
- search results ranked by playback likelihood and popularity
- prioritize stations with:
  - non-empty URL
  - supported/likely-supported codec such as MP3/AAC
  - reasonable bitrate
  - Radio Browser popularity/click/vote signals

### Favorites and Persistence

Persist:

- previous station
- volume
- favorites
- selected theme

Do not include custom URL station entry in MVP.

Favorites are added from:

- built-in catalog stations
- online search results

Planned polish for `MIK-014` and `MIK-016` uses a unified station-list source
model. The app should track the active `ListSource` explicitly rather than
letting the visible list be anonymous. Sources are:

- `AllStations`
- `Section(Music | Spoken / News)`
- `Category(Lofi | Ambient | Jazz | Classical | Electronic | News | Talk)`
- `Favorites`
- `Search`

The Wide `Browse` pane should become a flat source picker containing `All
Stations`, `Favorites`, both sections, and all categories. While Browse is
focused, `j`/`k` or arrows move the Browse selection and `Enter` applies the
source, replaces the station list, resets station selection safely, and moves
focus to Stations. While Stations is focused, the same keys keep their current
station-list behavior.

Favorites-only view is built from persisted `Settings::favorites`, so saved
favorite stations are reachable even when absent from the current catalog/search
view. Removing a favorite while the Favorites source is active immediately
removes it from the list and clamps selection. Empty Favorites remains on the
Favorites source and shows a helpful empty-state message instead of silently
falling back to All Stations.

Search is also a `ListSource`, but it remembers the previous non-search source.
Clearing search with `Esc` restores that previous source (for example Favorites
or Lofi), with All Stations as the default previous source.

### Offline / Network Failure

If network is unavailable or Radio Browser cannot be reached:

- show a clear offline state/screen
- still allow retrying cached previous station, favorites, and built-in candidates
- make offline status visually obvious
- do not fail the entire app immediately

### Now Playing Metadata

MVP includes ICY/Shoutcast metadata support.

Now Playing should display:

- station name
- category/tags/country when available
- codec/bitrate when available
- current ICY title/program metadata when available
- playback state
- volume

If ICY metadata is unavailable, gracefully show station-level metadata only.

### Layout

Use 3 responsive layout tiers.

#### Wide: Search Console

Search-first layout for large terminals.

Recommended structure:

- top: search input/status strip with debounce/cache/loading state
- main-left/center: online search results and station list as the largest region
- side: section/category shortcuts for Music and Spoken/News
- right: Now Playing, Spectrum Stack visualizer, transport/status
- footer: key hints and network/offline state

Wide mode should feel like a fast radio discovery console: type, see ranked
results, play/favorite, and keep the current stream visible.

#### Medium: Split Mini

Balanced list + player layout.

Recommended structure:

- upper or left region: compact station/search result list
- lower or right region: Now Playing + Spectrum Stack visualizer
- persistent footer: `Tab`, `/`, `Enter`, `Space`, `f`, `t`, `q`

Medium mode should avoid hiding the station list. It may reduce visualizer height
or metadata detail, but both browsing and playback should remain visible.

#### Compact: Split Mini, reduced

Constrained-pane layout for herdr-style small terminals.

Recommended structure:

- top: 3-6 visible station rows or focused search results
- middle: current station, ICY title if available, compact Spectrum Stack
- bottom: one-line status/key hints

Compact mode should not become a full-screen visualizer by default. It should
preserve enough station context that the user can change streams without opening
a separate drawer.

### Keyboard Model

Use focus-based TUI navigation:

- `Tab` / `Shift+Tab`: move focus between panes/regions
- `j` / `k` or arrows: move within focused list/control
- `Enter`: play/select focused station/action
- `/`: search input
- `Space`: Stop/Play toggle for the current station
- `+` / `-`: volume up/down
- `f`: toggle favorite
- `t`: cycle theme
- `?`: key help, if included
- `q` / `Esc`: quit/back depending on context

Space semantics:

- If playing: stop current station.
- If stopped and a current station exists: reconnect/play that station.

### Themes

MVP includes the **High Contrast Trio** selected in the design deck.

1. `Minimal`
   - default/recommended first-run mood
   - quiet dark background, low-saturation foreground, restrained spectrum bars
   - optimized for long work sessions and low visual fatigue
2. `Neon`
   - cliamp-like black background, high-saturation spectrum colors, cyan/magenta accents
   - used when the user wants the app to feel vivid and audio-reactive
3. `CRT`
   - retro green/amber phosphor terminal feel
   - optional personality theme with visible scanline/terminal nostalgia cues if they remain readable

The theme system should be data-driven with a `Theme` structure rather than hard-coded colors throughout UI code.

Theme implementation notes:

- All themes share the same layout and component hierarchy.
- Spectrum bars map to `spectrum_low`, `spectrum_mid`, and `spectrum_high`.
- Minimal should avoid loud glow effects by default.
- Neon may use glow-like color contrast, but not terminal effects that hurt readability.
- CRT may use green/amber accents, but scanline effects should be optional or subtle.

Theme polish for `MIK-017` expands the built-in theme set to six stable
lowercase persisted names, cycled by `t` in this order:

1. `minimal`
2. `neon`
3. `crt`
4. `solarized`
5. `midnight`
6. `sakura`

`Minimal` remains the default and unknown persisted theme names still fall back
to `minimal` at the settings boundary. The `t` key remains a simple one-way
cycle through the documented order; no theme picker is planned for this set.

`Solarized`, `Midnight`, and `Sakura` were added as names with placeholder
palettes in `MIK-028` and now carry their own distinct, dark-canvas-readable
colors (`MIK-029`).

### Visualizer Modes

The MVP visualizer is Spectrum Stack. `MIK-015` polish keeps that as the default
while adding the selectable visualizer modes below, all driven by real audio
data. `VizFrame` carries a low-resolution, normalized time-domain `waveform`
series in addition to FFT bands and RMS. UI code receives only this small
drawing-oriented shape, not raw audio buffers.

Visualizer modes (all implemented and selectable via `v`):

1. `SpectrumStack` — current vertical FFT bars and default mode.
2. `PeakDots` — FFT bars with a dot/peak emphasis.
3. `WaveScope` — waveform line/scope display from `VizFrame::waveform`.
4. `MirrorWave` — symmetrical waveform display for a calmer oscilloscope feel.
5. `AmbientPulse` — low-noise RMS/band-driven ambient display.

Every mode should use the full width of its allocated visualizer pane by
resampling/interpolating the available bands or waveform points to the render
area width. The layout does not need to become a full-width visualizer panel, and
compact mode must still keep station context plus playback visible. The `v` key
cycles visualizer mode and the selected mode is persisted like theme/volume.

### CLI Options

MVP includes a minimal practical CLI surface:

- `--theme <name>`: override saved theme for this run and/or select theme
- `--volume <0-100>`: startup volume override
- `--no-auto-play`: start silently even if previous station exists
- `--audio-output-device <name>`: CPAL output device name
- `--low-power`: force low-power UI mode

Potential optional flag if straightforward:

- `--search <query>` or positional search query to start in search mode

### Low-Power Behavior

MVP includes automatic and CLI-controlled low-power behavior.

- `--low-power` explicitly lowers visual update cadence.
- Compact/small layouts may automatically reduce visualizer/update intensity.
- Audio playback must remain unaffected.

### Implementation Principles

Use the project-specific guide in `docs/implementation-guidelines.md` while
implementing the replacement.

Applied principles:

- Use package/module boundaries to stop change waves.
- Keep the app as a single Rust crate using 2018 module style; avoid `mod.rs`.
- Start with domain models and pure logic before adapters and TUI wiring.
- Prefer always-valid domain primitives for constrained values.
- Parse untrusted boundary data once, then pass typed values internally.
- Wrap station/favorite/result collections when behavior would otherwise scatter.
- Classify abnormal states before choosing `Result`, assertion, event, or failure UI.
- Keep UI rendering declarative: UI sends actions; `App` mutates state.

Non-applied principles for MVP:

- Full Clean Architecture is too formal for this single-binary TUI.
- CQRS/Event Sourcing and aggregate transaction patterns are unnecessary.
- Custom architecture linters can wait until manual review becomes insufficient.

### Testing Strategy

Use automated tests for core logic:

- settings serialization/deserialization
- station ranking/filtering
- search cache behavior
- temporary failed-station state
- layout tier selection
- theme lookup
- FFT normalization/band mapping where deterministic
- domain primitive smart constructors and parse boundaries
- first-class collection behavior such as favorite deduplication and failed-station filtering

Use manual verification for:

- real audio output
- terminal rendering quality
- ICY metadata behavior against live stations
- device selection

## Non-Goals for MVP

- Spotify/local file/music-library player features
- playlist management beyond favorites
- custom station URL entry UI
- full HLS/Opus/every-format support
- real pause/resume with stream position preservation
- media keys/MPRIS
- plugin system
- equalizer
- lyrics
- remote daemon / IPC control
- user-editable custom theme files

## Success Criteria

MVP is successful when:

1. Launching `wave-tui` resumes the previous station automatically.
2. First launch or failed previous station starts silently with curated recommendations.
3. MP3/AAC radio streams play through the Rust-native audio path.
4. The visualizer reacts to actual playback audio via FFT.
5. Online search updates while typing with debounce and cached results.
6. Search results are ranked toward likely playable/popular stations.
7. Favorites, previous station, volume, and theme persist across restarts.
8. Failed stations are temporarily disabled during the session.
9. ICY metadata appears when available.
10. Wide/medium/compact terminal sizes each have intentional layouts.
11. Six themes are available and switchable.
12. `cargo test` covers core non-UI logic.
13. `cargo check` passes without errors.

### Success Criteria — Verification Status

Status as of the MIK-012 finalization pass. "Automated" means covered by
`cargo test` (pure app/UI/catalog/search/audio-helper tests, no network, audio,
or real terminal). "Manual" means it requires real audio output, live streams,
or interactive resize and is exercised via the manual checklist below, not CI.

| # | Criterion | Status | Evidence / notes |
| - | --------- | ------ | ---------------- |
| 1 | Resume previous station on launch | Automated (logic) + Manual (end-to-end) | `cli::startup_play_command` tests; full resume needs a real run. |
| 2 | First launch / failed previous starts silently with recommendations | Automated (logic) + Manual | `startup_is_silent_*` tests; catalog is the default visible list. |
| 3 | MP3/AAC streams play via the native path | Manual | Verified by the `audio_spike` tool (`docs/audio-spike.md`); no automated audio. |
| 4 | Visualizer reacts to real audio via FFT | Automated (helpers) + Manual | analyzer normalization tests; live reaction is manual. |
| 5 | Online search updates while typing, debounced + cached | Automated | `cli::SearchDebounce` and `search::SearchCache` tests. |
| 6 | Results ranked toward playable/popular | Automated | `catalog::station_score` / `search` ranking tests. |
| 7 | Favorites, previous station, volume, theme persist | Automated | `settings` roundtrip + `cli::Persistence` policy tests. |
| 8 | Failed stations temporarily disabled for the session | Automated | `catalog::SessionStationHealth` + `app::on_failed` tests. |
| 9 | ICY metadata appears when available | Automated (parse) + Manual | `audio::icy` synthetic-byte tests; live ICY is manual. |
| 10 | Wide/medium/compact layouts are intentional, no overlap | Automated (render) + Manual | per-tier `ui` render tests; visual quality is manual. |
| 11 | Six themes available and switchable | Automated | `theme` lookup/cycle + `ui` themed-render tests. |
| 12 | `cargo test` covers core non-UI logic | Automated | full suite green. |
| 13 | `cargo check` passes | Automated | part of the verification commands. |

Offline behavior (the "Offline / Network Failure" section): a failed online
search sets an offline state and an explicit offline search status without
crashing (`cli::apply_search_response`), the indicator renders in every layout
tier, and built-in retry candidates stay visible
(`ui::offline_state_is_visible_in_every_tier`,
`offline_search_status_is_visible_in_every_tier`,
`offline_still_shows_builtin_retry_candidates_in_every_tier`).

#### Remaining gaps

- **Manual-only criteria are unverified in this pass.** Items 1–4, 9, and 10
  that depend on real audio, live streams, live ICY, or interactive resize were
  not run here (no audio device / TTY in this environment). They remain on the
  manual checklist in `MIK-012` / the implementation plan Task 12.
- **No dedicated favorites browse view.** Favorites persist and the previous
  station auto-resumes, and the built-in catalog stays available offline, so
  "retry previous / built-in" is satisfied. However there is no in-app list that
  shows only favorites; retrying a favorite that is not in the catalog or the
  current results is not yet reachable by a keystroke. This is a known MVP gap,
  not a regression, and is out of MIK-012's offline scope.
- **Section/category shortcuts are display-only.** The Wide "Browse" pane lists
  Music and Spoken/News categories but selecting one is not yet wired to filter
  the visible list (only `Esc`/clear-search restores the full catalog).
