# Herdr Agent Pulse Implementation Plan

> **Status (2026-07-19): Superseded as a presentation record — do not
> re-execute.** This plan's integration work (packaging, eligibility,
> monitoring, reducers) was implemented and remains the shipped foundation,
> but its summary/overlay presentation was replaced by later redesigns,
> currently the Kinetic Collage
> (`docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`,
> plan `docs/superpowers/plans/2026-07-18-agent-pulse-kinetic-collage.md`).
> The local-only and read-only privacy boundaries recorded here remain in
> force.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `wave-tui` as an official Herdr plugin and add a read-only Agent Pulse that visualizes current-workspace agents without changing standalone radio behavior.

**Architecture:** A focused `herdr` adapter owns plugin-environment eligibility, Unix socket protocol parsing, and a five-second monitoring thread. `App` owns all Agent Pulse lifecycle, selection, stale/reconnect, and bounded-history transitions; `ui` only renders its summary and overlay; `cli` routes typed monitor events. The official `herdr-plugin.toml` launches the release binary in a dedicated tab.

**Tech Stack:** Rust 2021, Ratatui 0.29, Crossterm 0.28, Serde JSON, Unix domain sockets, Herdr plugin/socket API 0.7.0+.

## Global Constraints

- `min_herdr_version = "0.7.0"`; plugin supports macOS and Linux only.
- Enable Agent Pulse only when the official plugin supplies `HERDR_ENV=1`,
  `HERDR_SOCKET_PATH`, and `HERDR_WORKSPACE_ID`, unless `--no-agent-pulse` is set.
- Poll `agent.list` every five seconds; never query pane output or mutate Herdr.
- Filter strictly to the injected current workspace; never guess a workspace.
- Socket errors are recoverable; stale after the first failure and unavailable after 15 seconds.
- Keep the 20 newest completed agents only in process memory.
- Wide/Medium show a compact summary; Compact and Signal View do not. `a` opens the read-only overlay only when eligible.
- Agent activity changes colors/low-rate animation only; never changes audio, playback, search, settings, theme, or OS notifications.
- Run `cargo fmt --check`, `cargo test`, `cargo check`, and `cargo clippy --all-targets -- -D warnings` before requesting review.

---

## File structure

| File | Responsibility |
| --- | --- |
| `herdr-plugin.toml` | Herdr package metadata, Cargo release build, dedicated-tab pane, and open action. |
| `src/herdr.rs` | Herdr environment parsing, agent snapshot normalization, Unix socket requests, and monitor thread. |
| `src/lib.rs` | Declares the focused Herdr adapter module. |
| `src/app.rs` | Typed Agent Pulse state, reducer actions, current-state timing, completed-history policy, and overlay selection. |
| `src/cli.rs` | Parses `--no-agent-pulse`, starts/stops the monitor, routes monitor events and mouse/key input. |
| `src/ui.rs` | Inserts the Wide/Medium summary and delegates Agent Pulse overlay rendering. |
| `src/ui/agent_pulse.rs` | Read-only summary/constellation/list/history/info-card rendering and hit testing. |
| `README.md` | Documents plugin install, tab launch, controls, privacy, and standalone fallback. |
| `docs/SPEC.md` | Records the approved Herdr Agent Pulse product behavior. |
| `docs/ui-design-decisions.md` | Records Quiet Companion / Status Constellation visual decisions. |

### Task 1: Herdr adapter and official plugin package

**Files:**

- Create: `src/herdr.rs`, `herdr-plugin.toml`
- Modify: `src/lib.rs`
- Test: module tests in `src/herdr.rs`

**Produces:**

```rust
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(15);

pub(crate) struct HerdrContext {
    pub socket_path: PathBuf,
    pub workspace_id: String,
}
pub(crate) enum AgentStatus { Working, Blocked, Done, Idle, Unknown }
pub(crate) struct AgentSnapshot {
    pub pane_id: String,
    pub agent: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub status: AgentStatus,
}
pub(crate) enum MonitorEvent { Snapshot(Vec<AgentSnapshot>), Failed }
pub(crate) fn context_from_env(disabled: bool) -> Option<HerdrContext>;
pub(crate) fn spawn_monitor(context: HerdrContext) -> HerdrMonitor;
```

- [ ] **Step 1: Write failing environment and response-normalization tests.** Cover exact plugin eligibility, missing workspace rejection, `--no-agent-pulse` rejection, workspace filtering, unknown status normalization, and malformed/missing `agents` arrays.
- [ ] **Step 2: Run `cargo test herdr` and confirm the tests fail because the module/types do not exist.**
- [ ] **Step 3: Implement private raw Serde DTOs and the typed `AgentStatus`/`AgentSnapshot` conversion.** Require `pane_id` and `workspace_id`; preserve optional agent/name/cwd; map only `working`, `blocked`, `done`, and `idle` explicitly.
- [ ] **Step 4: Implement `context_from_env`.** Require the exact `HERDR_ENV=1` value and non-empty socket/workspace values; return `None` for every ineligible case without logging or panic.
- [ ] **Step 5: Implement the Unix socket request and monitoring thread.** Each iteration sends one newline-delimited JSON-RPC `agent.list` request, applies a three-second read timeout, forwards `Snapshot` or `Failed`, sleeps five seconds, and exits when its stop sender is dropped. Keep socket framing/JSON private to `herdr`.
- [ ] **Step 6: Add `pub mod herdr;` to `src/lib.rs`; add `herdr-plugin.toml`.** Use id `wave-tui.radio`, version `0.1.4`, `min_herdr_version = "0.7.0"`, Cargo release build, pane placement `tab`, command `./target/release/wave-tui`, and an `open` action that calls `$HERDR_BIN_PATH plugin pane open` for that entrypoint.
- [ ] **Step 7: Run `cargo fmt --check && cargo test herdr && cargo check`.**
- [ ] **Step 8: Commit `feat: add Herdr monitoring adapter and plugin manifest`.**

### Task 2: Pure Agent Pulse app state and lifecycle reducer

**Files:**

- Modify: `src/app.rs`
- Test: module tests in `src/app.rs`

**Consumes:** `crate::herdr::{AgentSnapshot, AgentStatus}`.

**Produces:**

```rust
pub(crate) enum AgentPulseConnection { Hidden, Connected, Stale, Unavailable }
pub(crate) struct AgentView { /* pane id, labels, status, observed_at */ }
pub(crate) struct CompletedAgent { /* AgentView plus completed_at */ }
pub(crate) enum AgentOverlay { Closed, Open }
pub(crate) enum Action {
    // existing variants
    AgentSnapshot { agents: Vec<AgentSnapshot>, now: Instant },
    AgentPollFailed { now: Instant },
    ToggleAgentOverlay,
    CloseAgentOverlay,
    SelectNextAgent,
    SelectPreviousAgent,
    SelectAgent(String),
    ToggleCompletedAgents,
}
```

- [ ] **Step 1: Write reducer tests before fields/variants.** Cover active sort order; duration reset only after a status change; `done` and disappeared panes moving once to completed; 20-entry eviction; initial failure/stale/unavailable thresholds; fresh snapshot recovery; empty connected state; and overlay selection clamping.
- [ ] **Step 2: Run `cargo test app::` and confirm failures identify the missing Agent Pulse contract.**
- [ ] **Step 3: Add private `AgentPulse` state to `App`.** It owns connection state, active `Vec<AgentView>`, `VecDeque<CompletedAgent>`, selected active pane ID, overlay visibility, disclosure state, and last successful snapshot time. Initialize it hidden so all standalone behavior stays unchanged.
- [ ] **Step 4: Add pure reducer helpers.** `apply_agent_snapshot`, `mark_agent_poll_failed`, `reconcile_completed`, `sort_active_agents`, `clamp_agent_selection`, and display accessors must be the only way UI/CLI observes mutable Agent Pulse state.
- [ ] **Step 5: Wire all Agent Pulse actions through `App::apply`.** `ToggleAgentOverlay` is a no-op while hidden or Signal View is active; close actions must preserve existing station/search focus and selection.
- [ ] **Step 6: Run `cargo fmt --check && cargo test app && cargo check`.**
- [ ] **Step 7: Commit `feat: add Agent Pulse app state`.**

### Task 3: CLI monitor lifecycle, keys, mouse input, and flags

**Files:**

- Modify: `src/cli.rs`
- Test: module tests in `src/cli.rs`

**Consumes:** `herdr::context_from_env`, `herdr::spawn_monitor`, and Agent Pulse `Action`s.

- [ ] **Step 1: Add failing CLI tests.** Assert `--no-agent-pulse` parses in both forms; help text documents it; `a` maps to `ToggleAgentOverlay` outside search; Signal View ignores it; and non-Agent Pulse `a` is harmless.
- [ ] **Step 2: Extend `CliArgs`, `USAGE`, and `parse_args`.** Add `no_agent_pulse: bool`; parse exactly `--no-agent-pulse` without persistence changes.
- [ ] **Step 3: Start an optional monitor in `run_app` after `App::new`.** Call `context_from_env(args.no_agent_pulse)` and retain the monitor only when it returns context. Ensure monitor teardown happens before return even after terminal errors.
- [ ] **Step 4: Extend `Runtime`/`event_loop` to drain `MonitorEvent`s.** Dispatch snapshots/failures with `Instant::now()` after audio/search drains; call an app stale-time refresh each loop so 15-second unavailable state occurs even without another event.
- [ ] **Step 5: Extend `KeyOutcome` and `handle_key`.** Route `a` to Agent Pulse toggle/close, route overlay Tab/arrows/Enter to its selection actions, and consume overlay keys before normal station navigation. Keep existing Signal View routing first.
- [ ] **Step 6: Handle `Event::Mouse`.** Pass click coordinates to a pure UI hit-test function, dispatch only returned read-only selection/disclosure actions, and leave all non-overlay clicks unchanged.
- [ ] **Step 7: Run `cargo fmt --check && cargo test cli && cargo check`.**
- [ ] **Step 8: Commit `feat: wire Herdr monitor into the CLI`.**

### Task 4: Quiet Companion and Status Constellation UI

**Files:**

- Create: `src/ui/agent_pulse.rs`
- Modify: `src/ui.rs`
- Test: module tests in `src/ui.rs` and `src/ui/agent_pulse.rs`

**Consumes:** read-only Agent Pulse display accessors from `App`.

- [ ] **Step 1: Write failing buffer-render tests.** Verify Wide/Medium display `Agents` counts when connected, show `agents · none active` when connected-empty, show stale text once, and omit all Pulse text in Compact, Signal View, hidden/standalone, and unavailable states.
- [ ] **Step 2: Write failing overlay tests.** Verify constellation/list/info-card labels, `Completed (n)` disclosure, stale/empty/unavailable copy, and low-power static output. Add hit-test tests mapping each rendered node/list row to its pane id without mutating `App`.
- [ ] **Step 3: Add `ui::agent_pulse` as a private submodule.** Keep geometry, node placement, one-shot status-change highlight, rendering, and hit testing there; it must not call the Herdr adapter or mutate app state.
- [ ] **Step 4: Insert `render_agent_summary` into non-compact `render_now_playing`.** Reuse theme colors only; reserve no rows when hidden/unavailable. Keep the Compact call path untouched.
- [ ] **Step 5: Render the overlay after normal Wide/Medium/Compact composition and before returning from `render_into`.** Clear its centered rect, draw constellation plus readable active list and information card, and use the current theme for all colors.
- [ ] **Step 6: Add `agent_pulse_hit_test(area, column, row, app) -> Option<Action>`.** It returns selection/toggle actions only and supports mouse selection; CLI remains owner of applying them.
- [ ] **Step 7: Run `cargo fmt --check && cargo test ui && cargo check`.**
- [ ] **Step 8: Commit `feat: render Herdr Agent Pulse`.**

### Task 5: Documentation, verification, and release-facing plugin checks

**Files:**

- Modify: `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`
- Test: existing unit suite and manual plugin checklist

- [ ] **Step 1: Update `README.md`.** Document Herdr 0.7.0+, `herdr plugin install <owner>/wave-tui`, dedicated radio-tab launch, `a`, overlay controls, `--no-agent-pulse`, read-only/privacy limits, and ordinary standalone fallback.
- [ ] **Step 2: Update `docs/SPEC.md`.** Add Agent Pulse to scope as an optional official-Herdr integration, preserving the no remote-control/no daemon/non-plugin-system boundaries.
- [ ] **Step 3: Update `docs/ui-design-decisions.md`.** Record Quiet Companion, Status Constellation + short list, Compact suppression, static low-power motion, and standalone invisibility.
- [ ] **Step 4: Run the full automated gate.**

```bash
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: all commands exit 0.

- [ ] **Step 5: Perform documented manual checks.** Link the plugin locally, open the dedicated tab, exercise active/done/disappeared agents, temporarily remove socket access, resize Wide/Medium/Compact, detach/reattach Herdr, and run standalone `wave-tui --no-auto-play`.
- [ ] **Step 6: Inspect `but diff`; commit `docs: document Herdr Agent Pulse`.**

## Review gates

1. After Task 2, review reducer coverage and confirm no adapter/socket detail leaked into `app`.
2. After Task 4, review the UI against the approved deck: Quiet Companion in Wide/Medium, no normal Compact/Signal/standalone Pulse, and a read-only constellation overlay.
3. After Task 5, request an independent code review before PR creation.

## Self-review

- **Spec coverage:** Tasks 1–4 cover packaging, eligibility, socket monitoring, current-workspace filtering, typed events, lifecycle/history, stale recovery, CLI/key/mouse behavior, and all UI states. Task 5 covers durable docs, full checks, and manual validation.
- **Placeholder scan:** No task contains an unresolved placeholder or deferred behavior; each named test and command has an expected outcome.
- **Type consistency:** `AgentSnapshot` flows from `herdr` to `Action::AgentSnapshot`; app exposes read-only display accessors to `ui`; UI returns `Action` from hit testing; `cli` remains the sole event-loop dispatcher.
