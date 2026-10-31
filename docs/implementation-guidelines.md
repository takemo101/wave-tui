# Implementation Guidelines

This project adopts a focused subset of the principles from
[`j5ik2o/okite-ai`](https://github.com/j5ik2o/okite-ai/tree/main/skills).

The goal is not to force a heavyweight enterprise architecture onto a small TUI
app. The goal is to keep the replacement modular, type-safe, testable, and easy
to evolve while avoiding the current `main.rs` god-module shape.

## Adopted Principles

### 1. Package Design: boundaries stop change waves

Source skill: `package-design`

Use module boundaries to isolate reasons for change, not just technical steps.

For `wave-tui`, the expected change boundaries are:

| Module | Change it when... | Hide from others |
| --- | --- | --- |
| `model` | domain vocabulary changes | raw primitive/string confusion |
| `settings` | persistence format/location changes | filesystem details |
| `catalog` | curated stations/ranking rules change | station scoring internals |
| `search` | Radio Browser API/search behavior changes | HTTP response shape |
| `audio` | playback/decoder/output/FFT changes | CPAL/Symphonia/RustFFT details |
| `theme` | colors/theme names change | hard-coded palette values |
| `layout` | breakpoint policy changes | width/height threshold logic |
| `app` | user actions/state transitions change | UI widget details |
| `ui` | rendering changes | domain mutation logic |
| `cli` | flags, help text, or key meaning change | terminal/adapter lifecycle |
| `runtime` | startup, teardown, or event-loop wiring changes | terminal, worker, and channel details |
| `herdr` | Herdr socket protocol or agent request behavior changes | Herdr JSON and opaque pane ids |

**Approved target, not yet implemented (dual-host Agent Planets).** To let one
run observe either Herdr or herdr-core (behavior in
[`SPEC.md`](SPEC.md#herdr-core-agent-host-approved-target-not-yet-implemented)),
the target boundaries are below; until the implementation issues land, the
`herdr` row above and the current module layout are what the code does.

| Module | Responsibility | Hide from others |
| --- | --- | --- |
| `model::agent` | shared agent vocabulary: opaque identity, status, display fields, host capabilities | which host produced a value |
| `herdr` | Herdr eligibility, socket protocol, `agent.list`, `agent.focus`/`agent.rename` | Herdr JSON and opaque pane ids |
| `herdr_core` | core eligibility, `terminal.list {}` polling, admission and membership rules | core JSON, addresses as wire strings, `status`/`state` raw values |
| `agent_host` | concrete facade that holds the one selected host and exposes monitor events plus capability-gated requests | which adapter is behind it |
| `runtime` | host selection (`--agent-host`, `--no-agent-pulse` precedence) and adapter construction | — |

Rules for the target:

- `agent_host` is a concrete type or enum, not a trait-object plugin layer; the
  two hosts are the only hosts.
- `app` and `ui` consume `model::agent` types and capabilities only and never
  import `herdr` or `herdr_core`. Whether a control exists is a capability, not
  an `if host == ...` test in UI code.
- `herdr` and `herdr_core` do not depend on each other; shared types move down
  into `model::agent`, not sideways.
- The legacy Herdr identity hash sequence and agent ordering stay byte-for-byte
  unchanged; adding a host must not move an existing planet.
- Boundary data is parsed once per adapter (`terminal.list` into admitted,
  typed agents) and never leaks core wire values past the adapter.
- Default tests use fake sockets or pure payloads; no live `hc`, Herdr, audio
  device, or terminal.

Larger modules split by responsibility behind a narrow facade rather than
growing: `audio::session` (playback-session lifecycle), `app::agent_pulse`
(Agent Pulse state kept out of the radio reducer), `ui::agent_pulse` (rendering
facade over private `geometry`/`surface`/`stage`/`modal`), and the `runtime`
children (`key_policy` decides what a key means as pure data; `input` applies
that decision once at the adapter boundary).

Rules:

- Each module must have a one-sentence responsibility.
- Avoid `utils`, `helpers`, `common`, and other catch-all modules.
- Prefer `pub(crate)` or private items; expose `pub` only for real contracts.
- Module dependencies should stay acyclic.
- Stable/core modules must not depend on volatile adapter modules.

### 2. Rust module pattern: single crate, 2018 module style

Source skill: `package-design/references/rust-patterns.md`

Use a single crate for now. The app is below the threshold where a workspace adds
value, and the replacement benefits from shared compile-time guarantees.

Use Rust 2018-style module entry files instead of `mod.rs`:

```text
src/audio.rs
src/audio/decoder.rs
src/audio/output.rs
src/audio/analyzer.rs
src/audio/icy.rs
```

`src/audio.rs` should be the public facade for the audio module and should keep
most submodule details private.

### 3. Domain model first

Source skill: `domain-model-first`

Implement stable domain types and pure logic before wiring TUI and live audio.

Recommended order:

1. domain models and settings types
2. pure ranking/filtering/layout/theme logic
3. app reducer/state transitions
4. adapters: Radio Browser, filesystem, audio runtime
5. TUI rendering and event loop integration

Tests should start at the pure domain/application layer. Live audio and terminal
rendering remain manual/integration verification points.

### 4. Domain primitives and always-valid models

Source skills:

- `domain-primitives-and-always-valid`
- `when-to-wrap-primitives`

Do not pass around ambiguous primitives when a domain concept has constraints or
can be confused with another value.

Use small Rust newtypes/smart constructors for:

- `StationId`
- `StationName`
- `StreamUrl`
- `ThemeName`
- `VolumePercent`
- `SearchQuery`
- `BitrateKbps`
- `SampleRateHz`

Guidelines:

- Constructors should reject impossible values.
- If a value exists, it should be valid.
- Keep fields private when the invariant matters.
- Do not wrap every primitive by default; wrap when it prevents invalid states,
  argument mixups, or repeated validation.

Current enforced invariants:

- `StreamUrl::parse` accepts only a real `http`/`https` URL with a non-empty
  host (a bare prefix such as `http://` is rejected) and keeps the original raw
  value in `DomainError::InvalidStreamUrl`. It never rewrites or appends to the
  URL; resolution rules such as curated `/stream` suffixes belong to `audio`.
- `VizFrame` and `PhaseTrace` keep their fields private and normalize bands, RMS,
  waveform, and phase pairs on construction. Renderers and `audio` use
  constructors and read-only accessors, so no caller can bypass normalization.
- Playback work is identified by a typed, monotonically increasing
  `PlaybackRequestId`. The controller allocates one per play or replay request
  and `App` ignores station-scoped audio events from any other request, so a
  late completion from a replaced or cancelled session cannot alter playback,
  selection, health, or metadata, including when the same station is replayed.

### 5. Parse, don't validate

Source skill: `parse-dont-validate`

At system boundaries, convert untrusted data into typed values once, then let the
rest of the app trust those types.

Boundaries in this project:

- CLI args
- settings JSON
- Radio Browser JSON
- curated catalog definitions
- HTTP stream URLs and ICY metadata

Examples:

- Parse raw `String` into `StreamUrl` before playback.
- Parse raw theme string into `ThemeName`, falling back to `Minimal` at the
  boundary.
- Parse raw volume into `VolumePercent` before storing in app state.
- Normalize Radio Browser station records into `Station` before ranking.

Avoid shotgun parsing: do not repeatedly check `url.trim().is_empty()` or
`volume <= 100` throughout unrelated modules.

### 6. First-class collections for station groups

Source skill: `first-class-collection`

Wrap collections when they carry domain behavior.

Good candidates:

- `Stations(Vec<Station>)`
- `Favorites(Vec<Station>)`
- `SearchResults(Vec<Station>)`
- `Categories(Vec<Category>)`
- `VizBands(Vec<f32>)`

Behaviors to keep with the collection:

- ranking and filtering
- selected-index bounds handling
- removing failed stations
- favorite deduplication
- non-empty checks for recommendations
- clamping/normalizing visualizer bands

This avoids scattering `Vec<Station>` loops across `app`, `ui`, `catalog`, and
`search`.

### 7. Error classification and Result-based handling

Source skills:

- `error-classification`
- `error-handling`

Classify abnormal states before choosing handling strategy.

| Classification | In `wave-tui` | Handling |
| --- | --- | --- |
| Error | bad station URL, unsupported codec, network timeout, invalid settings | return `Result`, show status, allow retry/fallback |
| Defect | impossible state transition, invalid hard-coded catalog, out-of-range internal index after clamping | assert/debug_assert or fix code |
| Fault | audio device unavailable, decoder/output thread failure | isolate, emit audio failure event, keep TUI alive |
| Failure | app cannot provide playback/search at all | visible offline/failure state, graceful exit only if unrecoverable |

Rules:

- Recoverable user/environment issues should be `Result<T, E>`.
- Do not panic for broken remote stations.
- Do not swallow defects with generic `catch all` handling.
- Audio thread failures should become `AudioEvent::Failed`, not crash the TUI.
- Settings, search, audio, and Herdr failures stay recoverable. A failed
  settings save must not leave a partial `settings.json` (the write is atomic;
  see `docs/SPEC.md`) and surfaces a nonfatal notice instead of being silently
  dropped.
- Background work that cannot be cancelled is bounded rather than queued:
  debounced search is latest-wins, connect attempts are capped and coalesced,
  and at most one Herdr focus request is in flight at a time.
- Use `anyhow` at app boundaries; use typed errors inside stable modules when it
  improves tests and decisions.

### 8. Tell, Don't Ask and Law of Demeter, applied lightly

Source skills:

- `tell-dont-ask`
- `law-of-demeter`

Use these principles to keep state transition logic out of UI rendering.

Prefer:

```rust
app.apply(Action::ToggleFavorite);
app.apply(Action::Audio(event));
```

Avoid UI code doing this kind of work:

```rust
if app.settings.favorites.iter().any(|s| s.id == station.id) {
    app.settings.favorites.retain(...);
}
```

Rules:

- UI asks for display data and sends actions; it should not mutate domain state
  by inspecting nested structures.
- `App` owns focus movement, selection bounds, favorite toggling, failed-station
  handling, and playback-state transitions.
- Deep chains such as `app.settings.previous_station.as_ref().unwrap().url...`
  should become intent-revealing methods.

### 9. Repository design, adapted to local persistence

Source skill: `repository-design`

This app does not need enterprise repository layering, but the persistence
boundary should still be narrow.

Use storage names based on domain concepts, not file formats:

- Good: `SettingsStore`, `FavoritesStore` if split later
- Avoid: `SettingsJsonRepository`, `FavoritesFileTable`, `StationDtoStore`

CQS-style guidance:

- Queries return values and do not mutate: `load() -> Result<Settings, _>`
- Commands mutate persistence and return success/failure: `save(&Settings) -> Result<(), _>`
- Domain behavior such as `toggle_favorite` belongs on `App`/`Favorites`, not in
  the store.

## Principles Not Adopted for MVP

These okite-ai skills are intentionally not used as primary guidance for the
MVP because they add more structure than this app needs right now:

- Full Clean Architecture: useful ideas, but too formal for a single binary TUI.
- CQRS/Event Sourcing skills: no event store or complex read/write model needed.
- Aggregate transaction boundary/cross-aggregate constraints: no multi-aggregate
  transactional domain in MVP.
- OpenSpec workflow: current docs/SPEC and implementation plan are sufficient.
- Custom linter creation: defer until architecture rules become hard to enforce
  manually.

## Review Checklist

Before each implementation task is considered done:

- [ ] The changed module still has one clear reason to change.
- [ ] No new `utils`, `helpers`, `common`, or catch-all module was introduced.
- [ ] Boundary inputs are parsed into typed values once.
- [ ] Recoverable failures are represented as `Result`/events, not panics.
- [ ] UI rendering did not absorb app-state mutation logic.
- [ ] Public API surface stayed minimal.
- [ ] Tests cover the module's public contract or pure logic.
