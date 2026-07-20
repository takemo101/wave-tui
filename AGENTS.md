# AGENTS.md

Guidance for AI coding agents working on `wave-tui`.

## Entry points

Use this file as the agent workflow entry point. Durable project decisions live
in `docs/`; read the relevant docs before changing behavior or architecture.

Start here:

1. [`docs/SPEC.md`](docs/SPEC.md) — product specification and MVP scope.
2. [`docs/implementation-guidelines.md`](docs/implementation-guidelines.md) —
   implementation principles adapted from okite-ai skills.
3. [`docs/ui-design-decisions.md`](docs/ui-design-decisions.md) — selected UI
   direction from the design deck.
4. [`docs/audio-spike.md`](docs/audio-spike.md) — native audio spike results and
   playback caveats.
5. [`docs/herdr-plugin.md`](docs/herdr-plugin.md) — Herdr plugin operational
   manual (install, verify, open, develop, update, uninstall, troubleshoot).

Before changing Agent Pulse presentation, read the Agent Pulse sections of
`docs/ui-design-decisions.md` and `docs/SPEC.md`; they record the current
Agent Planets stage decisions (Dual Phase Scope, disc-mask planets, details
modal, interior surface status, solar orbits, focus brackets) and the
superseded presentation lineage. The dated per-iteration superpowers design
and plan documents were consolidated into the canonical docs above and
removed; the originals are preserved in git history.

Before changing playback, search, persistence, themes, layout, module
boundaries, or user-facing controls, re-read the relevant doc first.

Do not duplicate detailed design rules in this file. If a durable decision
changes, update the appropriate doc in the same change.

## Scope guard

`wave-tui` is a small terminal-first internet radio player for work-session BGM.
It is not a general music platform.

Stay inside the MVP unless the user explicitly expands scope:

- native Rust playback for MP3/AAC-centered HTTP streams;
- real FFT visualizer from played samples;
- curated catalog plus online Radio Browser search;
- previous station, volume, favorites, and theme persistence;
- ICY/Shoutcast now-playing metadata when available;
- three responsive layout tiers;
- Minimal, Neon, CRT, Solarized, Midnight, and Sakura themes;
- the optional observational Herdr Agent Pulse companion when launched as the
  official Herdr plugin (current presentation: the Agent Planets stage over
  the Dual Phase Scope — disc-mask planets, details modal, interior-only
  surface status, static sun with Working-only invisible orbits, and
  selection focus brackets — as recorded in `docs/ui-design-decisions.md`
  and `docs/SPEC.md`).
  It observes approved agent details via `agent.list` on the plugin's local Herdr
  socket (across that session's workspaces) and may explicitly focus the selected
  live pane via `agent.focus` or rename its explicit display name via
  `agent.rename`; it is not a daemon, remote control service, or an internal
  plugin system.
Do not add these in MVP without an explicit design update:

- Spotify/local library/player features;
- playlist management beyond favorites;
- custom station URL entry UI;
- full HLS/Opus/every-format support;
- media keys/MPRIS;
- daemon/IPC/remote control;
- plugin system;
- equalizer or lyrics.

## Architecture and module rules

Follow [`docs/implementation-guidelines.md`](docs/implementation-guidelines.md).
The important rules are:

- Keep the project as a single Rust crate for now.
- Use Rust 2018 module style: prefer `src/audio.rs` plus `src/audio/*.rs`, not
  `src/audio/mod.rs`.
- Avoid catch-all modules such as `utils`, `helpers`, `common`, or `misc`.
- Keep module dependencies acyclic.
- Keep stable/core modules independent of volatile adapter details.
- Start with private items, then widen to `pub(crate)` or `pub` only when needed.

Expected responsibility boundaries:

- `model`: domain vocabulary and always-valid types.
- `settings`: settings load/save and persistence format boundary.
- `catalog`: curated stations, station ranking, validation state.
- `search`: Radio Browser client, normalization, query cache.
- `audio`: native playback facade, decoder/output/analyzer/ICY events, and the
  playback-session lifecycle (`audio::session`: the control loop, cancellation,
  and worker reclamation, kept free of CPAL/Symphonia/HTTP details).
- `herdr`: Herdr plugin environment eligibility, Unix socket protocol, the
  `agent.list` monitor thread, and explicit `agent.focus` / `agent.rename`
  requests; the only module that sees Herdr JSON or opaque pane ids.- `theme`: theme names and palette definitions.
- `layout`: terminal size to layout tier policy.
- `app`: app state, actions, reducers, focus, selection, temporary failures.
- `ui`: Ratatui rendering only; do not put domain mutation logic here.
- `cli`: CLI argument parsing, boundary parsing, and key mapping; it must not
  depend on terminal or adapter lifecycle details.
- `runtime`: the composition root for a run — terminal ownership, adapter
  construction, splash, event-loop ownership, channel draining, and teardown
  ordering — behind private child modules (`debounce`, `persistence`,
  `search_worker`, `terminal`, `splash`, `key_policy`, `input`, `event_loop`).
  It depends on `cli`; `cli` never depends back on it.
  Key handling is split in two: `cli` maps a terminal event to a focus-agnostic
  outcome, `runtime::key_policy` decides as pure data what that outcome means in
  the active mode (normal, Signal View, Agent Planets stage, details modal,
  rename input), and `runtime::input` applies that decision at the single
  adapter boundary. Only the Agent Planets stage may defer to normal-mode
  policy, and only once, so a key reaches the adapters at most once.

## Domain and parsing rules

Use the okite-ai-inspired guidelines already captured in docs:

- Prefer domain primitives/smart constructors for constrained concepts such as
  `StationId`, `StreamUrl`, `ThemeName`, `VolumePercent`, `SearchQuery`,
  `BitrateKbps`, and `SampleRateHz`.
- Parse untrusted boundary data once, then pass typed values internally.
- Boundary data includes CLI args, settings JSON, Radio Browser JSON, curated
  catalog entries, stream URLs, and ICY metadata.
- Wrap behavior-rich collections such as stations, favorites, search results,
  and visualizer bands instead of scattering `Vec` loops and validation across
  modules.

## Audio rules

Native Rust playback is the primary architecture.

The validated spike path is:

```text
HTTP stream -> Symphonia decode -> CPAL output -> played-sample mirror -> RustFFT
```

Important caveats from [`docs/audio-spike.md`](docs/audio-spike.md):

- Treat Radio Browser `url_resolved` values as direct stream URLs first.
- Do not blindly append `/stream` to arbitrary station URLs.
- Add `/stream` only for curated base URLs that explicitly require it.
- Prefer CPAL output configs matching the stream sample rate.
- Add resampling or a clear unsupported-rate failure path before broad Radio
  Browser rollout.
- ICY `StreamTitle` parsing and full `icy-metaint` demuxing are implemented in
  `src/audio/icy.rs` and wired through `src/audio/decoder.rs`; both are covered
  by pure tests with synthetic byte streams.

Keep the audio control thread free of blocking network/decoder reads: connecting
runs on a connect worker and torn-down sessions are reclaimed in the background,
so Stop, station replacement, and Shutdown never wait on a stalled read. The
control thread still owns the CPAL stream. Because connects cannot be cancelled,
they are capped and coalesced to the newest request rather than queued, and a
connect worker's panic must surface as a recoverable failure instead of
stranding the request. See the control-thread, bounded cleanup, connect-cap, and
worker-panic sections of [`docs/audio-spike.md`](docs/audio-spike.md).

Broken remote stations, unsupported codecs, network timeouts, and unavailable
audio devices are recoverable failures. Report them as `Result` values or audio
failure events; do not crash the TUI for normal remote-stream failures.

## UI and design rules

Follow [`docs/ui-design-decisions.md`](docs/ui-design-decisions.md).

Selected direction:

- Overall personality: Quiet Focus Pane.
- Wide layout: Search Console.
- Medium/Compact layout: Split Mini.
- Visualizer: Spectrum Stack.
- Theme set: High Contrast Trio.

Practical rules:

- Default first-run theme is `Minimal` unless saved settings say otherwise.
- Keep the UI calm enough to live beside work for hours.
- Wide mode should make search input, result count, loading/cache/offline state,
  and ranked results prominent.
- Medium and compact modes should keep both station context and Now Playing
  visible; do not default compact mode to a full-screen visualizer.
- Use one shared `SpectrumStack` renderer across layout tiers.
- UI rendering should send actions to `App`; it should not mutate nested app
  state directly.

## Development workflow

Develop in small, testable slices.

1. Identify one behavior, module, or doc slice.
2. Re-read the relevant docs listed above.
3. Check whether the slice affects playback, search, persistence, themes,
   layout, module boundaries, or public controls.
4. Write or update focused tests before broadening scope.
5. Prefer pure domain/app tests before adapter or terminal integration.
6. Run the relevant checks.
7. Update durable docs when documented behavior or boundaries change.
8. Avoid opportunistic unrelated refactors.

For non-trivial code changes, follow the scope, contracts, and verification
status recorded in `docs/SPEC.md` unless the user asks to revise them.

### Delegated implementation/review workflow with asem

For non-trivial implementation work, prefer dogfooding asem Sessions while the
parent Session stays responsible for planning, final judgment, validation, and
version-control decisions.

Use the local asem skill when operating asem. Prefer MCP tools if available
(`create_session`, `send_message`, `list_messages`, `report_parent`,
`close_session`); otherwise use the CLI (`asem session create`,
`asem message send`, `asem message wait`, `asem report parent`,
`asem session close`).

Recommended flow:

1. Parent/orchestrator Session reads the relevant docs and mikan Issue, then
   prepares a bounded task prompt with scope, acceptance criteria, and checks.
2. Launch a worker child Session for exactly one implementation slice.
3. Wait for the child Report. Treat the Report as communication, not proof of
   success.
4. For non-trivial changes, launch a separate reviewer child Session to compare
   the implementation against the request, docs, tests, and these repo rules.
5. If review finds issues, send a Message back to the worker with concrete
   repair instructions and wait for another Report.
6. Parent Session runs final validation locally, updates mikan, and handles
   GitButler/version-control steps with `but` if needed.
7. Close child Sessions to preserve Message/Report history. Do not delete asem
   history unless the user explicitly asks.

Keep asem semantics narrow: Session status is process state, not work outcome;
Messages and Reports are coordination records, not task lifecycle state. Do not
edit `.asem/sessions/`, `.asem/current-session*.json`, `.asem/tokens/`, or other
asem runtime files directly.

## Testability rules

Default tests should not require real network, real audio devices, or a real
terminal UI.

Use pure tests or injected/fake dependencies for:

- settings path and filesystem behavior;
- Radio Browser responses;
- catalog ranking and session station health;
- search cache behavior;
- station ranking/filtering;
- favorite deduplication;
- app actions and focus movement;
- layout tier selection;
- theme lookup/cycling;
- FFT normalization and ICY parsing.

Manual verification is expected for:

- real CPAL audio output;
- real HTTP streams;
- ICY metadata against live stations;
- terminal rendering quality;
- responsive layout resize behavior;
- output device selection.

## Verification

For code changes, run at least:

```bash
cargo fmt --check
cargo test
cargo check
```

When touching the current audio spike, also run:

```bash
cargo test --test audio_spike
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Before finalizing larger implementation changes, also run:

```bash
cargo clippy -- -D warnings
```

For docs-only changes, inspect the diff and ensure Markdown renders cleanly. If
Markdown diagnostics are available, fix real issues and ignore obvious dictionary
false positives such as `ratatui`.

## GitButler / version-control workflow

This working tree may be GitButler-managed. Prefer `but` for version-control
write operations when GitButler is active.

- Use `but status -fv` before version-control mutations.
- Use `but` instead of git write commands.
- Do not run `git add`, `git commit`, `git push`, `git checkout`, `git switch`,
  `git merge`, `git rebase`, or `git stash` for write operations in a
  GitButler-managed workspace.
- Read-only git inspection is acceptable when needed.
- If GitButler is not set up for this directory, do not force it; ask or proceed
  without version-control mutation.
- Avoid creating PRs from temporary clones while this workspace is
  GitButler-managed, unless explicitly necessary. If a temporary clone is used,
  treat the main workspace as stale afterward: return here, verify `but status`,
  `but config target`, and `but pull --check`, then repair or sync before
  starting more work.
- Do not leave merged GitButler branches, old virtual branch records, or stale
  target refs applied in the main workspace after using a clean clone for PR
  work. Remote `origin/main` remains authoritative, but GitButler target and
  workspace state must be brought back into alignment before further commits.
- If `but pull --check` reports a merge-base error, stop normal feature work and
  repair GitButler state first; do not keep committing from workaround clones
  unless the user explicitly approves that escape hatch.

## Documentation rules

Keep durable design material in docs, not in `AGENTS.md`.

Update docs when changing:

- product scope or MVP behavior: `docs/SPEC.md`;
- implementation principles or module boundaries:
  `docs/implementation-guidelines.md`;
- UI direction, themes, or responsive layout decisions:
  `docs/ui-design-decisions.md`;
- audio architecture findings or spike conclusions: `docs/audio-spike.md`;
- Herdr plugin packaging or Agent Pulse behavior: `README.md`,
  `docs/herdr-plugin.md` (the plugin install/open/update/uninstall and
  troubleshooting manual), `docs/SPEC.md`, and `docs/ui-design-decisions.md`
  (which holds the current Agent Planets presentation decisions and the
  superseded lineage).

`AGENTS.md` should stay focused on agent workflow and pointers to canonical
docs.
