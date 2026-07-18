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
- a short, skippable lifecycle splash that feels polished without delaying work
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
- After entering the terminal alternate screen, show a short skippable startup
  splash before the main UI: a pixel-art `WAVE` logo reveals left-to-right,
  followed by the `wave-tui v{package version}` label and
  `settling into the signal`.
- The startup splash omits the wave-glyph line; the startup motion is limited
  to the logo reveal. The shutdown splash may use a small calm wave animation
  with the farewell copy `thanks for listening` / `see you next wave`.
- The splash is presentational only. It must not change auto-play decisions,
  station selection, settings persistence, search startup, or playback state.

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

When a successful Radio Browser search result population exists, Browse `All
Stations`, sections, and categories filter that current search result set rather
than the curated catalog: `All Stations` shows all current results, and section
and category sources filter the full result population (always from the full
population, not the already-filtered visible list). Category membership for
Radio Browser stations is inferred from a small, conservative tag/name alias
dictionary. When no search population exists, Browse keeps its curated fallback:
`All Stations` shows the curated catalog and category sources show curated
category stations. Browse rail labels stay stable in both modes; the
search/status strip carries the active filter context (for example
`filter: Jazz`). If a genre filter matches zero stations in the current search
results, the Results pane shows a specific empty state such as `No Jazz results
in current search` instead of silently falling back to curated stations.
`Favorites` is never a search filter — it always shows persisted favorites.

Favorites-only view is built from persisted `Settings::favorites`, so saved
favorite stations are reachable even when absent from the current catalog/search
view. Removing a favorite while the Favorites source is active immediately
removes it from the list and clamps selection. Empty Favorites remains on the
Favorites source and shows a helpful empty-state message instead of silently
falling back to All Stations.

Search is also a `ListSource`, but it remembers the previous non-search source.
Clearing search with `Esc` restores that previous source (for example Favorites
or Lofi), with All Stations as the default previous source. Clearing search also
drops the search result population so the preserved Browse source rebuilds from
the curated catalog. A failed or offline search keeps the last successful search
population available for filtering while the offline/error state is shown.

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
  - when width allows, Results render as a table-like station comparison list
    with station, codec, bitrate, and locale columns
  - as the pane narrows, metadata collapses before falling back to compact list rows
- side: section/category shortcuts for Music and Spoken/News
- right: Now Playing, Spectrum Stack visualizer, transport/status
- footer: key hints and network/offline state

Wide mode should feel like a fast radio discovery console: type, scan ranked
results in a responsive table when space allows, play/favorite, and keep the
current stream visible.

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
- `Space`: Stop/Play toggle for the current station while the station list is focused
- `+` / `-`: volume up/down
- `f`: toggle favorite
- `t`: cycle theme
- `?`: key help, if included
- `q` / `Esc`: quit/back depending on context

Space semantics:

- Only applies while the station/results list is focused; in Search, Browse, or
  Now Playing focus it does not move the station cursor or toggle playback.
- If playing: stop current station.
- If stopped and a current station exists: reconnect/play that station.

### Signal View

Signal View is an explicit visual-player mode entered with `z` from the normal
TUI. It hides the Search, Browse, and Stations discovery UI so the current
station can be presented center-stage with a large visualizer. It is temporary
display state: it is not persisted across launches and has no CLI startup flag.

While Signal View is active:

- `z` or `Esc` returns to the normal UI; `q` still quits the app.
- `Space`, `+`/`-`, `v`, `t`, and `f` keep their normal behavior.
- `f` toggles favorite state for the *current* station shown on screen, not the
  hidden station-list selection.
- Search, focus movement, Browse, and station navigation/selection keys are
  ignored silently.

Signal View displays the app's current station (the idle prompt
`Select a station, then press z` when none exists), shows the ICY now-playing
title when available and otherwise the station name, and keeps the user in the
mode across stopped/connecting/playing/failed states. Favorite state matches the
station list: a small `★` is shown only when the current station is favorited;
non-favorites show no empty marker. It does not pause, cancel, or clear
background search/list state. The visualizer reuses the currently selected theme
and visualizer mode and receives the largest flexible layout region, so it is
meaningfully larger than the normal Now Playing visualizer on medium and large
panes. The title metadata includes a thin, near-full-width volume bar without
mixing in the current visualizer mode label. Signal View does not add playlist,
queue, search, or new station-selection behavior, and it does not become the
default compact layout.

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

1. `SpectrumStack` — particle-filled vertical FFT analyzer columns and default mode.
2. `PeakDots` — FFT peak dots with a five-frame real-audio trail; the current dot is strongest and older dots fade as quieter marks.
3. `SkylinePeaks` — stateless FFT skyline: a bright peak cap over a digital 0/1
   binary tail, distinct from both `SpectrumStack` and `PeakDots`.
4. `WaveScope` — waveform line/scope display from `VizFrame::waveform`.
5. `MirrorWave` — symmetrical waveform display for a calmer oscilloscope feel.
6. `AmbientPulse` — low-noise RMS/band-driven ambient display.

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
- `--no-agent-pulse`: disable the optional Herdr Agent Pulse integration for
  this run only; never persisted and never applied to settings

Potential optional flag if straightforward:

- `--search <query>` or positional search query to start in search mode

### Low-Power Behavior

MVP includes automatic and CLI-controlled low-power behavior.

- `--low-power` explicitly lowers visual update cadence.
- Compact/small layouts may automatically reduce visualizer/update intensity.
- Audio playback must remain unaffected.
- The Agent Pulse Dual Phase Scope renders statically in low-power mode:
  phase-trace/persistence positions, frame geometry, shadow trails, and the
  Working spinner orientation are frozen while state edge/core colors still
  refresh. The frozen geometry is captured from the first *audible*
  visualizer frame after startup (RMS above the silence threshold with real
  phase data); until such a frame arrives, low power renders the live frame.

### Herdr Agent Pulse (Optional Integration)

`wave-tui` ships as an official Herdr plugin and, only when launched by that
plugin, shows a **read-only Agent Pulse**: the live status of the AI coding
agents visible on that Herdr session's local control socket, presented as
ambient context beside radio playback. The integration design (packaging,
eligibility, monitoring) is
`docs/superpowers/specs/2026-07-16-herdr-agent-pulse-design.md`; its
presentation is superseded by the approved
`docs/superpowers/specs/2026-07-19-agent-pulse-lissajous-scope-design.md`
(which in turn supersedes the interim Kinetic Collage presentation in
`docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`).
This section records the product behavior as implemented.

This is an optional Herdr integration, not a plugin system inside `wave-tui`,
and it does not weaken the non-goals below: there is still no daemon, no IPC
remote control of `wave-tui`, and no way for agent activity to change audio,
playback, search, settings, themes, or the visualizer.

#### Packaging and launch

- `herdr-plugin.toml` at the repository root is the official manifest: plugin
  id `wave-tui.radio`, `min_herdr_version = "0.7.0"`, macOS and Linux, a
  Cargo release build during plugin install, and an `open` action that opens
  the release binary in a **dedicated Herdr tab** (placement `tab`) so the
  player keeps its Wide/Medium layout.
- The tab owns the audio process: closing the tab exits `wave-tui` and stops
  playback; detach/reattach follows Herdr's normal pane lifecycle.

#### Eligibility

Agent Pulse is enabled if and only if all of these hold:

1. `--no-agent-pulse` is absent.
2. `HERDR_ENV` is exactly `1`.
3. `HERDR_SOCKET_PATH` is set and non-empty.
4. `HERDR_WORKSPACE_ID` is set and non-empty.

The plugin environment is the authority for eligibility; the injected
workspace id is trusted plugin context and is not used to filter the display.
Every ineligible launch (standalone, incomplete environment, or explicit
disable) keeps the exact pre-integration appearance and behavior — no
reserved rows, no hints, `a` is a silent no-op, and mouse capture is not
enabled.

#### Monitoring and data flow

- A focused `herdr` adapter module owns environment parsing, the Unix socket
  transport, newline-delimited JSON-RPC framing, and `agent.list` payload
  normalization; nothing else in the app sees raw JSON or sockets.
- A background thread polls `agent.list` every 5 seconds with a 3-second
  socket I/O timeout and forwards typed snapshot/failure events into the
  existing event loop; the reducer in `app` owns all lifecycle state.
- Every agent returned by the current socket is normalized — across all of
  that Herdr session's workspaces — under a private workspace-qualified
  identity, so identical pane ids in different workspaces stay distinct. No
  other Herdr sessions or sockets are ever discovered or opened.
- Statuses `working`, `blocked`, `done`, and `idle` are mapped explicitly;
  anything else (or a missing status) becomes `unknown`. Entries missing
  required ids and malformed payloads are dropped/rejected without crashing.
- The observed-at timestamp is internal reducer state (preserved while an
  agent's identity and status are unchanged); no duration is displayed.

#### Lifecycle and history

- Each successful snapshot replaces the live view. Agents sort working,
  blocked, idle, done, then unknown.
- A `done` agent stays in the live view, rendered faded, until a later
  snapshot omits it; then it disappears.
- There is no completed-agent history and no agent detail store: live
  snapshot, selection, and connection state are the only Agent Pulse state,
  all process-local.

#### Connection states and recovery

| Condition | Wide/Medium summary | Canvas (`a`) |
| --- | --- | --- |
| Connected | `● n active` count | Live Dual Phase Scope (`agents · none active` when empty) |
| First failed poll | Count dims | Last live scope (traces/persistence/frames/cores/trails) frozen, dimmed, `stale · reconnecting` banner |
| ≥ 15 seconds without success | Summary disappears | Frames and traces hidden behind `agents · unavailable · retrying` |
| Fresh snapshot | Live count | Live scope |

Socket errors, malformed replies, and timeouts are recoverable: they never
panic the TUI or interrupt playback, and polling continues. A per-loop timer
tick advances the 15-second unavailable threshold even when no further monitor
event arrives. The stale freeze uses the visualizer frames captured at the
Connected→Stale edge, so later audio frames and elapsed time do not thaw it.

#### UI and input contract

- Wide and Medium add exactly one `● n active` line to Now Playing — a count
  only, never names. Compact shows no Agent Pulse line (but `a` still opens
  the canvas while the integration is active); Signal View never shows Agent
  Pulse and ignores `a`.
- `a` opens the full-screen **Dual Phase Scope** canvas, replacing the whole
  player surface; `a`/`Esc` close it and `q`/`Ctrl+C` still quit. Agent
  Pulse opens a full-screen Dual Phase Scope with two real-audio Lissajous
  traces: overlapping phase portraits of paired played samples, never a
  scrolling amplitude-over-time waveform. With stereo output the primary
  trace plots the played left/right sample pairs; mono streams pair the
  played mono mix with the same mix at a documented 29-sample lag, and the
  secondary trace always uses a distinct 97-sample mono lag. Up to two dim
  phosphor-persistence layers echo recent real visualizer frames, and a
  breathing theme-phosphor vignette spreads with RMS. Nothing advances from
  wall-clock time: identical visualizer data renders identical cells, and
  silence leaves the scope dim and still.
- The canvas renders one small, stable frame rectangle per agent, laid out
  deterministically from the agent's private workspace-qualified identity so
  frames stay recognizable and never swap positions. Agent frames keep
  state-colored edges; Working has an audio-driven spinner core (`◜◝◞◟`,
  whose orientation advances only from newly received played-audio phase
  data), while Idle (`◌`), Blocked (`×`), and Done (`·`) remain stationary.
  RMS combined with each frame's assigned FFT band moves its rectangle with
  a small bounded scale/offset and adds a one- or two-layer soft shadow
  trail drawn from real recent visualizer frames. Dense terminals shrink
  frame size and spacing rather than omitting frames; every agent keeps one
  visible frame.
- State colors come from the active theme only: working edges glow strongest
  (the playing color), blocked uses the error color for edge and core;
  idle/done/unknown stay muted and a done frame stays muted/dim until its
  snapshot omits it. In `--low-power`, phase-trace/persistence positions,
  frame geometry, shadow trails, and spinner orientation are frozen (from
  the first audible frame captured after startup) while state edge/core
  colors still update.
- `Tab`/`Shift+Tab`/arrows/`j`/`k` select a frame, bringing it forward.
  A selected named agent shows only `name · status`, placed near its frame;
  an unnamed selection shows no label. Search (`/`) and station
  navigation/selection (`g`/`G`/`Home`/`End`/`Enter`) are consumed, so
  canvas input can never play a station or move station selection.
- The documented global player shortcuts fall through with their exact normal
  semantics and side effects: `Space` (playback toggle, still conditional on
  the station list being the focused pane underneath), `+`/`-` (volume), `f`
  (favorite for the station-list selection), `t` (theme), `v` (visualizer
  mode), and `z` (Signal View, which replaces the canvas surface).
- Mouse capture is enabled only for eligible plugin launches, solely to feed
  frame-selection clicks; background trace, vignette, and shadow cells
  resolve nothing. Clicks resolve against the frame geometry actually drawn
  (including low-power frozen geometry) and only while the connection is
  `Connected`. During stale/unavailable states selection is frozen entirely —
  mouse clicks and keyboard selection both change nothing, while `a`/`Esc`
  still close the canvas. Selection input, from either device, must not act
  on data that may no longer be current.

#### Privacy and read-only guarantees

- Only `agent.list` is ever called. Pane output, prompts, files, and
  scrollback are never read; panes are never focused, created, closed, sent
  text, or otherwise controlled.
- Every agent reported by the plugin invocation's local Herdr socket is
  shown, across that session's workspaces; other Herdr sessions are never
  discovered.
- Only a selected frame's explicit Herdr `name` is ever rendered. There is
  no fallback label; pane ids, workspace ids, working directories, and agent
  types never appear on screen.
- Agent activity changes colors/low-rate rendering only — never audio,
  playback, search, settings, theme, visualizer, or OS notifications.
- Nothing is persisted: agent state exists only in process memory, and
  `--no-agent-pulse` is never written to settings.

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
- Herdr Agent Pulse logic without a live Herdr process, socket, or terminal:
  plugin-environment eligibility, cross-workspace `agent.list` payload
  normalization, lifecycle/stale reducers, key/mouse routing, and
  summary/Dual Phase Scope canvas rendering

Use manual verification for:

- real audio output
- terminal rendering quality
- ICY metadata behavior against live stations
- device selection
- live Herdr plugin behavior (see the Agent Pulse manual checklist below)

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

The optional Herdr Agent Pulse integration does not change these non-goals:
`wave-tui` ships *as* a Herdr plugin but has no plugin system of its own, runs
no daemon, and accepts no remote control — its Herdr socket use is outbound,
read-only `agent.list` monitoring only.

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
12. Startup/shutdown lifecycle splash is short, skippable, theme-driven, and does
    not alter playback/search/app state behavior.
13. `cargo test` covers core non-UI logic.
14. `cargo check` passes without errors.

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
| 12 | Lifecycle splash is short, skippable, theme-driven, and behavior-neutral | Automated (render/timing) + Manual | `ui::splash` render/timing tests cover logo reveal, no startup wave glyphs, version label, spacing, and shutdown wave; real terminal visual polish is manual. |
| 13 | `cargo test` covers core non-UI logic | Automated | full suite green. |
| 14 | `cargo check` passes | Automated | part of the verification commands. |

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
- **Favorites and Browse filtering are now wired.** Earlier MIK-012 notes about
  missing favorites/category browse behavior are resolved: the Browse pane has a
  `Favorites` source for all saved favorites, and Music/Spoken section/category
  entries apply their corresponding station filters. When a successful search
  result population exists, those section/category sources filter the current
  search results; otherwise they fall back to the curated catalog.

### Herdr Agent Pulse — Verification Status

Status as of the Lissajous Scope documentation pass (2026-07-19).

**Automated (all green in this pass):** `cargo fmt --check`, `cargo test`,
`cargo check`, and `cargo clippy --all-targets -- -D warnings` all exit 0.
The suite covers, without any live Herdr process, socket, audio, or terminal:

- exact plugin-environment eligibility and every ineligible/disabled case
  (`herdr` module tests);
- `agent.list` request framing, cross-workspace payload normalization with
  workspace-qualified identity, status mapping, and malformed-payload
  rejection (`herdr`);
- monitor failure reporting and clean shutdown against a nonexistent socket
  (`herdr`);
- snapshot/lifecycle reducers: sort order, cross-workspace identity
  distinctness and selection, done agents staying until a snapshot omits
  them, the stale-edge visualizer freeze capture, stale → unavailable
  (15 s) → recovery transitions, and the per-loop staleness tick (`app`);
- normalized `PhaseTrace` clamping/pairing, typed `PlayedSample` stereo/mono
  boundaries, and analyzer phase traces built from real played samples —
  stereo left/right primary pairs plus distinct documented mono lags
  (29-sample primary fallback, 97-sample secondary) — with no scrolling
  waveform substitution (`model`, `audio::output`, `audio::analyzer`);
- `--no-agent-pulse` parsing/help text, `a` routing, Signal View suppression,
  the canvas key gate (frame selection, suppressed search/station
  navigation, preserved global player shortcuts, Signal View delegation),
  and monitor/mouse-capture/click routing including the low-power geometry
  path (`cli`);
- the App-owned low-power visual capture: silent startup frames capture
  nothing, the first audible frame (RMS above the silence threshold with
  real phase data) becomes the frozen geometry source while live frames
  still update colors, and disabling the policy clears the capture (`app`);
- summary visibility per tier and connection state, full-screen canvas
  coverage, two centered clock-free Dual Phase Scope traces with dim
  phosphor persistence, deterministic per-identity frame layout,
  one-frame-per-agent density, RMS/FFT-driven frame motion and shadow
  trails, silence stillness, audio-driven Working spinner cores versus
  stationary Idle/Blocked/Done/Unknown cores, stale/low-power frozen
  geometry, state edge glow and `theme.error` Blocked cores,
  selected-name-only privacy with near-frame label placement, stale
  freeze/unavailable rendering, and frame-only hit testing (`ui`,
  `ui::agent_pulse`).

**Manual checklist (NOT run in this pass).** This documentation pass was
performed without a Herdr 0.7.0+ installation, a real terminal session, or
audio output, so none of the following live checks were executed here. They
are recorded as environment-dependent residual verification and should be
run on macOS and/or Linux with a real Herdr 0.7.0+ session before treating
the release as fully validated:

- [ ] Install or link the plugin locally and confirm the Cargo release build
      runs during `herdr plugin install`/link.
- [ ] Open the `Open wave-tui radio tab` action; confirm a dedicated tab with
      the Wide or Medium layout and working playback.
- [ ] Play a real **stereo** stream with the canvas open and judge the live
      Dual Phase Scope: the primary trace draws a left/right phase portrait,
      the secondary trace differs from it, dim phosphor persistence echoes
      recent motion, frames breathe with RMS/band energy and grow soft
      shadow trails, Working spinner cores advance with the music, and
      silence leaves a dim, still scope with no timer motion — never a
      scrolling waveform.
- [ ] Play a real **mono** stream and confirm both lagged-mono traces stay
      non-diagonal, distinct oscilloscope figures rather than a flat line or
      a scrolling waveform.
- [ ] Cycle all six themes on the canvas and confirm traces, vignette, frame
      edges, and status cores stay legible on each.
- [ ] With agents across multiple workspaces of the same Herdr session,
      confirm `● n active` counts them all and the canvas keeps one
      recognizable frame per agent at stable positions, including dense
      agent counts and after resizes.
- [ ] Select frames with keyboard and mouse clicks; confirm clicks land only
      on frame cells, the selected frame comes forward, only explicit
      `name · status` labels render near the frame, and unnamed agents show
      no label.
- [ ] Resize the tab through Wide/Medium/Compact; confirm the summary hides
      in Compact while `a` still opens the full-screen canvas, and the
      canvas redraws cleanly across sizes.
- [ ] Temporarily remove socket access; confirm the dimmed count and frozen
      `stale · reconnecting` canvas (traces, frames, cores, and trails
      frozen and dimmed), the 15-second `agents · unavailable · retrying`
      state hiding every frame and trace, and full recovery when the socket
      returns — with playback unaffected throughout, and mouse clicks and
      keyboard selection changing nothing while stale/unavailable.
- [ ] Detach and reattach the Herdr session; confirm the tab process and
      playback follow Herdr's normal pane lifecycle.
- [ ] Run `wave-tui --low-power` inside Herdr and confirm frozen trace,
      persistence, frame, shadow, and spinner geometry while state
      edge/core colors still refresh — including that a launch into silence
      renders the live frame until audio becomes audible, after which the
      first audible frame stays the frozen geometry — and that clicks still
      select against the frozen geometry.
- [ ] Run standalone `wave-tui --no-auto-play` outside Herdr and inside a
      plain Herdr shell pane (no plugin env); confirm zero Agent Pulse UI,
      inert `a`, and unchanged terminal mouse behavior.
- [ ] Launch via the plugin with `--no-agent-pulse` and confirm the
      integration stays fully disabled for that run.
