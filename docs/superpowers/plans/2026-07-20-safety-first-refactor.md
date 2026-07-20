# Safety-First Refactoring Implementation Plan

> **For agentic workers:** Work on exactly one mikan Issue at a time. Use an independent reviewer before marking it complete. Do not modify unrelated, currently dirty Agent Planets work.

**Goal:** Make playback, search, persistence, and Herdr control resilient before decomposing controller and Agent Planets code.

**Architecture:** Preserve the existing single-crate, reducer-first structure. First improve correctness at the typed boundaries and asynchronous adapters; then split controller and Agent Planets internals only after behavior is covered by focused tests. `ui` remains render-only, `app` remains pure state mutation, and `herdr` remains the sole socket/JSON boundary.

**Tech Stack:** Rust 2021, Ratatui, Crossterm, std::sync::mpsc, reqwest, Symphonia, CPAL, serde_json.

## Global Constraints

- Preserve the documented MVP scope and Agent Pulse privacy/presentation contract in `docs/SPEC.md` and `docs/ui-design-decisions.md`.
- Preserve the single-crate, acyclic module structure; use `src/<module>.rs` plus child files, never `mod.rs` or catch-all modules.
- Parse untrusted data once at boundaries; do not weaken existing domain invariants.
- Keep terminal UI rendering free of mutations and external I/O.
- Treat network, audio device, socket, and persistence failures as recoverable.
- Every issue starts by reading `AGENTS.md` plus its listed relevant docs, and ends with focused tests, full checks, and an independent review.
- The checkout contains unrelated, uncommitted Agent Planets work. Do not reformat, revert, stage, or change it outside the assigned issue.

---

## Execution Order

### Task 1: Validate HTTP stream URLs as real URLs

**Files:** `src/model.rs`; possibly `Cargo.toml` / `Cargo.lock` if adding a URL parser.

1. Add failing model tests for hostless, whitespace-only-host, non-HTTP, and valid HTTP(S) stream URLs.
2. Replace prefix/length validation in `StreamUrl::parse` with parsed scheme plus non-empty host validation; preserve the original raw value in `DomainError::InvalidStreamUrl`.
3. Run `cargo test model`, then the full verification suite.

**Produces:** `StreamUrl` instances that satisfy the documented HTTP(S)-host invariant.

### Task 2: Seal visualizer model invariants

**Files:** `src/model.rs`, `src/audio/analyzer.rs`, `src/app.rs`, `src/ui/visualizer.rs`, `src/ui/agent_pulse.rs`, and affected tests.

1. Identify all `VizFrame` / `PhaseTrace` field writes and reads.
2. Add constructor/accessor tests for normalized bands, waveform, RMS, and phase-pair lengths.
3. Make invariant-bearing fields private; add read-only accessors/iterators needed by audio and renderers.
4. Replace all direct callers; ensure no public mutation bypasses normalization.
5. Run focused model/audio/UI tests and full verification.

**Produces:** rendering-oriented immutable data that can only be constructed in valid form.

### Task 3: Make settings persistence atomic and observable

**Files:** `src/settings.rs`, `src/cli.rs`, `src/app.rs` only if a reducer-backed nonfatal notice is required; docs if UI behavior changes.

1. Add filesystem-injected tests covering successful save, failed temp write, failed rename, and existing-file preservation on failure.
2. Serialize once, write a uniquely named temporary file beside the target, flush/sync it, then atomically rename it into place.
3. Keep `settings::save` fallible; stop silently discarding errors in the controller. Surface a nonfatal notice using the smallest existing status mechanism.
4. Run settings/CLI tests and full verification.

**Produces:** no partially-written `settings.json` after a failed write, with a visible recoverable failure.

### Task 4: Coalesce debounced search to latest-wins

**Files:** `src/cli.rs`; `src/search.rs` only for an injectable fake/transport seam if necessary.

1. Add a deterministic worker/controller test: queue slow request A, then B and C; prove only C starts after A and only C may update the reducer.
2. Replace unbounded FIFO behavior with a latest-request coalescing loop while preserving query-generation stale-result protection and cache ownership.
3. Preserve clean worker shutdown and the 300–500ms existing debounce contract.
4. Run CLI/search tests and full verification.

**Produces:** fast typing cannot create an unbounded stale HTTP backlog.

### Task 5: Give playback requests a generation identity

**Files:** `src/audio.rs`, `src/app.rs`, `src/cli.rs`, `src/model.rs` only if the ID type belongs there, plus tests.

1. Add a typed monotonically increasing playback request/session identifier.
2. Extend `AudioCommand::Play` and station-scoped `AudioEvent`s with that identity; leave global volume/stop events deliberately scoped and documented.
3. Have the controller allocate an ID for every play/replay request and App record the currently expected ID.
4. Add reducer tests proving stale Connecting/Playing/Failed/Viz/ICY events cannot alter current playback, selection, health, previous station, or metadata; include replaying the same station.
5. Run app/audio/CLI tests and full verification.

**Produces:** event ownership based on a request instance, rather than ambiguous station identity.

### Task 6: Make audio control responsive under blocked I/O

**Files:** `src/audio.rs`, `src/audio/decoder.rs`, possibly a focused new `src/audio/session.rs` module and tests.

1. Add deterministic fake-blocking decoder/session tests proving Stop, Play replacement, and Shutdown return control without waiting for a network read timeout.
2. Separate command reception from cancellable playback session teardown. Preserve CPAL lifetime ownership and guarantee every worker eventually joins without leaks.
3. Wire cancellation into blocking HTTP/decoder paths where possible; document bounded cleanup behavior where a library call cannot be interrupted.
4. Verify that the generation identity from Task 5 rejects any completion that races after cancellation.
5. Run audio tests, integration tests, full verification, and the documented manual audio-spike check when an audio device/network are available.

**Depends on:** Task 5.

**Produces:** responsive transport controls without stale completion state.

### Task 7: Bound Herdr focus requests

**Files:** `src/herdr.rs`, `src/app.rs`, `src/cli.rs`, affected Agent Pulse tests.

1. Add tests for repeated focus input against a blocked fake socket/transport: at most one active request, predictable notices, and later retry after completion.
2. Add a bounded in-flight focus guard or a single command worker within `herdr`; keep raw socket and opaque pane IDs confined to that module.
3. Keep focus I/O off the terminal event loop and preserve explicit user-only focus behavior.
4. Run herdr/app/CLI tests and full verification.

**Produces:** repeated `o`/`O` cannot spawn unbounded detached workers.

### Task 8: Extract terminal runtime orchestration from CLI parsing

**Files:** create `src/runtime.rs` and focused child modules as justified; modify `src/cli.rs`, `src/lib.rs`, tests.

1. Characterization-test startup, cleanup ordering, worker shutdown, and event-draining behavior before moving code.
2. Retain `CliArgs`, `parse_args`, help/version, and override parsing in `cli`.
3. Move terminal guard, runtime construction, event loop ownership, and adapter channel draining into a private runtime composition boundary.
4. Keep `App` pure and preserve audio/search/Herdr teardown ordering.
5. Run CLI/binary-entry tests and full verification.

**Depends on:** Tasks 3–7.

**Produces:** a narrow CLI boundary and a testable runtime composition root.

### Task 9: Split key policy from runtime effects

**Files:** `src/runtime.rs` and private children created by Task 8; `src/cli.rs` only for key mapping; tests.

1. Lock current normal, Signal View, Agent Planets, modal, and rename input behavior with table-driven routing tests.
2. Separate key interpretation/policy from side effects such as audio command send, persistence, debounce scheduling, and Herdr requests.
3. Keep one explicit command/effect boundary so a key is dispatched at most once.
4. Run focused routing tests and full verification.

**Depends on:** Task 8.

**Produces:** small, independently testable input-mode handlers.

### Task 10: Extract Agent Pulse reducer substate

**Files:** create `src/app/agent_pulse.rs` or equivalent Rust-2018 child; modify `src/app.rs`, `src/herdr.rs` only for type imports, tests.

1. Preserve all existing Agent Pulse snapshot, stale/unavailable, freeze, selection, details, rename, and focus tests before extraction.
2. Move `AgentPulse`, agent display state, and Agent Pulse-only actions/reducer helpers behind a narrow app-internal API.
3. Do not let Herdr data alter radio state; preserve active-agent sort and identity semantics.
4. Run app/herdr/CLI/UI tests and full verification.

**Depends on:** Tasks 7 and 9.

**Produces:** radio reducer code no longer carries Agent Pulse lifecycle implementation details.

### Task 11: Decompose Agent Planets geometry and stage rendering

**Files:** split `src/ui/agent_pulse.rs` into responsibility-focused Rust-2018 child files; update `src/ui.rs`, tests.

1. Characterize geometry and buffer output before moving code, especially stale and low-power freeze behavior.
2. Extract pure layout/orbit/disc geometry first, then surface/status treatment, then stage/modal rendering; keep the top-level module as the narrow rendering facade.
3. Preserve disc-mask planets, interior-only status, static sun, Working-only invisible orbits, focus brackets, and non-hit-testable decoration.
4. Keep all render functions read-only and ensure hit testing consumes the same geometry as rendering.
5. Run Agent Pulse/UI tests, full verification, and manual TTY resize review.

**Depends on:** Task 10.

**Produces:** smaller rendering units with shared geometry and no behavioral drift.

## Per-Issue Completion Checklist

- Run `cargo fmt --check`.
- Run the focused tests listed in the Issue.
- Run `cargo test`, `cargo check`, and `cargo clippy --all-targets -- -D warnings`.
- Run `lens_diagnostics` for edited files before completion.
- Update canonical docs only when an observable contract or durable architecture boundary changes.
- Append validation and reviewer evidence to the mikan Issue, then move it to completed only after review approval.
