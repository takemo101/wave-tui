# Agent Planets Details Modal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace permanent planet labels with a selected-agent, read-only compact-record details modal opened by `Enter`.

**Architecture:** `herdr` parses only the approved identity/presentation fields into typed snapshots; `app` owns the ephemeral modal state and lifecycle; `ui::agent_pulse` renders the modal and removes all tag geometry; `cli` handles modal-local input before normal canvas input. The stage keeps its existing disc masks, status rings, scope, title, volume, footer hint, and cyclic selection.

**Tech Stack:** Rust 2018, Ratatui, Serde JSON, existing App reducer and pure buffer tests.

## Global Constraints

- Start from committed `05422f5`; discard the obsolete, uncommitted Task 1D runtime-label Side Tag edits in `src/herdr.rs` and `src/ui/agent_pulse.rs` before Task 1.
- The only renderable Herdr fields are explicit `name`, runtime `agent`, normalized `status`, and `terminal_title` as Activity.
- Never render pane/workspace/cwd/foreground cwd/terminal ID/tab ID/session/raw status or any other JSON field.
- Remove every permanent Side Tag, its placement/collision reservation, and its tests; status rings remain the only ambient status encoding.
- `Enter` opens details only for a selected live planet; `Enter`/`Esc` close the modal; `a` closes modal and Agent Planets; `q`/Ctrl-C keep global quit behavior.
- While details are open, selection, playback, volume, theme, favorite, visualizer, search, and Signal View inputs are consumed with no mutation.
- Stale dims the selected details with reconnecting copy; Unavailable, stage close, Signal View, and selection disappearance close the modal.
- Use theme-derived styles only. Keep network/audio/terminal integration out of default tests.

---

### Task 1: Parse typed agent detail fields at the Herdr boundary

**Files:**

- Modify: `src/herdr.rs:61-170, 310-410`
- Modify: `src/app.rs:150-240, 924-948, 2438-2674`

**Interfaces:**

- Consumes: raw current-socket `agent.list` objects with `name`, `agent`, `terminal_title`, `agent_status`, plus opaque ids.
- Produces: `pub(crate) struct AgentDetails { pub(crate) name: Option<String>, pub(crate) agent: Option<String>, pub(crate) activity: Option<String> }`; `AgentSnapshot { id, details, status }`; `AgentView { id, details, status, observed_at }`.
- Preserves: `AgentId` opacity, status normalization, sort order, no fallback from prohibited source fields.

- [ ] **Step 1: Write failing Herdr parsing tests.**

```rust
#[test]
fn parser_keeps_the_four_allowed_detail_fields_and_ignores_location_metadata() {
    let parsed = parse_agent_list(r#"{"jsonrpc":"2.0","id":1,"result":{"agents":[{
        "pane_id":"pane-private","workspace_id":"workspace-private",
        "name":"  research  ","agent":"  claude  ",
        "terminal_title":"  Review the modal  ","agent_status":"working",
        "cwd":"/private","tab_id":"tab-private","terminal_id":"term-private"
    }]}}"#).unwrap();
    let detail = &parsed[0].details;
    assert_eq!(detail.name.as_deref(), Some("research"));
    assert_eq!(detail.agent.as_deref(), Some("claude"));
    assert_eq!(detail.activity.as_deref(), Some("Review the modal"));
    assert_eq!(parsed[0].status, AgentStatus::Working);
}

#[test]
fn parser_omits_blank_allowed_detail_fields_without_private_fallbacks() {
    let parsed = parse_agent_list(r#"{"jsonrpc":"2.0","id":1,"result":{"agents":[{
        "pane_id":"pi","workspace_id":"claude","name":" ","agent":"\t",
        "terminal_title":" ","cwd":"/private","agent_status":"idle"
    }]}}"#).unwrap();
    assert_eq!(parsed[0].details, AgentDetails::default());
}
```

Add an App snapshot test proving `AgentView.details` preserves all three allowed values while sorting/selection still use the opaque identity.

- [ ] **Step 2: Run focused tests and verify failure.**

Run:

```bash
cargo test herdr::tests::parser_keeps_the_four_allowed
cargo test herdr::tests::parser_omits_blank_allowed
cargo test app::tests::agent_snapshot_connects
```

Expected: FAIL because the committed snapshot has one `name` field and does not parse `agent` or `terminal_title`.

- [ ] **Step 3: Implement the typed boundary mapping.**

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentDetails {
    pub(crate) name: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) activity: Option<String>,
}

fn nonblank(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[derive(Deserialize)]
struct RawAgent {
    pane_id: String,
    workspace_id: String,
    #[serde(default)] name: Option<String>,
    #[serde(default)] agent: Option<String>,
    #[serde(default)] terminal_title: Option<String>,
    #[serde(default)] agent_status: Option<String>,
}
```

Map only `nonblank(raw.name)`, `nonblank(raw.agent)`, and
`nonblank(raw.terminal_title)` into `AgentDetails`; do not deserialize any
other display candidate. Replace old `name` test fixtures/constructors with
`AgentDetails` values and keep `AgentId::new(workspace_id, pane_id)` private to
the display layer.

- [ ] **Step 4: Run focused and full verification.**

Run:

```bash
cargo fmt --check
cargo test herdr::tests
cargo test app::tests::agent_snapshot
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0 and a parsed snapshot contains no private
location/session field.

- [ ] **Step 5: Commit the boundary slice.**

```bash
but commit agent-pulse-ringed-planets -m "feat: parse Agent Planets details"
```

### Task 2: Add App-owned details-modal lifecycle

**Files:**

- Modify: `src/app.rs:160-240, 323-430, 494-542, 924-1088, 1256-1283, 2931-3201`

**Interfaces:**

- Consumes: `AgentDetails`, selected `AgentId`, `AgentPulseConnection`, overlay state, snapshot/recovery reducers.
- Produces: `AgentDetailsOverlay::{Closed, Open(AgentId)}`, `Action::{OpenAgentDetails, CloseAgentDetails}`, `App::{is_agent_details_open, selected_agent_details}`.
- Preserves: radio/session state, cyclic selection, standalone/ineligible behavior, stale capture, and selection identity.

- [ ] **Step 1: Write failing reducer lifecycle tests.**

```rust
#[test]
fn details_open_only_for_a_selected_connected_agent_and_close_without_radio_mutation() {
    let mut app = app_with_agents(vec![agent("w", "p", details("pi", "working"))]);
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::OpenAgentDetails);
    assert!(!app.is_agent_details_open());
    app.apply(Action::SelectNextAgent);
    app.apply(Action::OpenAgentDetails);
    assert_eq!(app.selected_agent_details().unwrap().agent.as_deref(), Some("pi"));
    app.apply(Action::CloseAgentDetails);
    assert!(!app.is_agent_details_open());
}

#[test]
fn details_close_on_overlay_close_signal_view_unavailable_and_missing_selection() {
    // Open details, then assert Closed after each independent transition.
}

#[test]
fn stale_details_remain_for_the_selected_identity_and_recover_live() {
    // Open live details, apply AgentPollFailed, assert Open; apply fresh snapshot,
    // assert the same identity exposes refreshed detail values.
}
```

- [ ] **Step 2: Run reducer tests and verify failure.**

Run:

```bash
cargo test app::tests::details_open_only
cargo test app::tests::details_close_on
cargo test app::tests::stale_details_remain
```

Expected: FAIL because no details-overlay state/actions/getters exist.

- [ ] **Step 3: Implement lifecycle state and reducers.**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentDetailsOverlay { Closed, Open(AgentId) }

fn open_agent_details(&mut self) {
    if self.agent_selection_interactive() {
        if let Some(id) = self.agent_pulse.selected.clone() {
            self.agent_pulse.details = AgentDetailsOverlay::Open(id);
        }
    }
}

fn close_agent_details(&mut self) {
    self.agent_pulse.details = AgentDetailsOverlay::Closed;
}
```

Initialize closed. Close it from `close_agent_overlay`, `toggle_signal_view`,
`mark_agent_poll_failed` when the threshold transitions to Unavailable, and
snapshot selection clamping when its stored identity is absent. Keep it open
for Stale; `selected_agent_details` must resolve against the currently selected
identity, returning `None` if the state is not Open or identity differs.

- [ ] **Step 4: Run lifecycle/full verification.**

Run:

```bash
cargo fmt --check
cargo test app::tests::details_
cargo test app::tests::stale_
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0; no radio-setting or playback assertion changes.

- [ ] **Step 5: Commit the reducer slice.**

```bash
but commit agent-pulse-ringed-planets -m "feat: manage Agent Planets details modal"
```

### Task 3: Remove permanent tags and render the compact record modal

**Files:**

- Modify: `src/ui/agent_pulse.rs:812-1516, 2389-3355`

**Interfaces:**

- Consumes: `App::selected_agent_details`, selected agent status, modal-open getter, stale/connection state, stage field rect, and `Theme`.
- Produces: `render_agent_details_modal`, bounded detail-row/activity helpers, side-tag-free `render_agent_planets_stage`.
- Preserves: disc/ring geometry, planet-only hit testing, selected emphasis, phase scope, title/volume/footer, unavailable field, theme-only colors.

- [ ] **Step 1: Replace Side Tag assertions with failing modal/render tests.**

```rust
#[test]
fn stage_never_renders_permanent_agent_labels_or_tag_reservations() {
    let app = app_with_details_agents();
    let tags = stage_tags(&app);
    assert!(tags.is_empty());
    let text = buffer_text(&render_stage(&app, 120, 36));
    assert!(!text.contains("pi"));
    assert!(!text.contains("working"));
}

#[test]
fn selected_agent_details_render_compact_record_in_allowed_field_order() {
    let mut app = app_with_details_agents();
    select_and_open_details(&mut app, "pi");
    let text = buffer_text(&render_stage(&app, 120, 36));
    assert!(text.contains("Agent details"));
    assert_in_order(&text, &["name", "research", "agent", "pi", "status", "working", "activity", "Review the modal"]);
    assert!(!text.contains("pane-private"));
}

#[test]
fn details_activity_truncates_and_stale_modal_is_dimmed_with_reconnecting_copy() {
    // Render a narrow stage with a long activity, then stale it and assert
    // bounded output plus reconnecting copy.
}

#[test]
fn unavailable_stage_has_no_details_modal() {
    // Open details then force Unavailable and assert the existing unavailable copy,
    // no `Agent details`, and no allowed fields in the buffer.
}
```

Delete/replace permanent-tag unit tests: tag placement candidates, collision
reservations, truncation, selected-tag draw order, and runtime-label-to-tag
coverage. Keep the existing tests that prove tags are not hit targets by
asserting hit testing remains body/ring-only.

- [ ] **Step 2: Run render tests and verify failure.**

Run:

```bash
cargo test ui::agent_pulse::tests::stage_never_renders
cargo test ui::agent_pulse::tests::selected_agent_details_render
cargo test ui::agent_pulse::tests::details_activity
cargo test ui::agent_pulse::tests::unavailable_stage_has_no_details
```

Expected: FAIL because permanent tags still render and no modal exists.

- [ ] **Step 3: Remove tag layout and render a theme-only modal.**

Delete `PlanetTag`, `tag_name`, tag rectangle/collision helpers,
`planet_tag_placements`, and `render_tag`. Stop reserving tag cells; preserve
`PlanetGeometry::hit_cells` as only body/ring cells.

```rust
fn render_agent_details_modal(app: &App, theme: &Theme, stale: bool, field: Rect, buf: &mut Buffer) {
    let Some(view) = app.selected_agent_details() else { return };
    let rows = detail_rows(view); // name, agent, normalized status, activity; omit None
    let area = centered_detail_rect(field, rows);
    Clear.render(area, buf);
    // Render `Agent details`, stable key/value rows, bounded wrapped activity,
    // and `reconnecting` in stale state with palette-derived dim styling.
}
```

Call it after the planets/scope render so it is topmost. Constrain it to the
stage field, use `Clear`, `Block`/`Paragraph`, and only theme palette styles.
Do not create a compact Side Tag substitute; clipped Activity must not escape
the modal rect.

- [ ] **Step 4: Run UI/full verification.**

Run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo test ui::tests::agent_planets
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0; no permanent agent text appears before opening
details and all modal text stays inside the stage field.

- [ ] **Step 5: Commit the rendering slice.**

```bash
but commit agent-pulse-ringed-planets -m "feat: show Agent Planets details modal"
```

### Task 4: Route Agent Planets modal input and update footer copy

**Files:**

- Modify: `src/cli.rs:220-310, 900-940, 1124-1160, 2310-2440`
- Modify: `src/ui/agent_pulse.rs:1132-1140, 3176-3355`

**Interfaces:**

- Consumes: `KeyOutcome::{Play, ExitOrBack, ToggleAgentPulse, Quit}`, `App` details actions/getter, Agent Planets canvas gate.
- Produces: modal-first `handle_collage_key` behavior and footer copy `Enter details`.
- Preserves: global quit, closed-canvas keys, cyclic selection, `z` no-op in Agent Planets, and normal Single View behavior elsewhere.

- [ ] **Step 1: Write failing key and footer tests.**

```rust
#[test]
fn enter_opens_details_only_for_selected_planet_and_never_plays_radio() {
    let mut app = connected_overlay_with_selected_agent();
    let playback = app.playback();
    assert_eq!(handle_collage_key(KeyOutcome::Play, &mut app), Some(Flow::Continue));
    assert!(app.is_agent_details_open());
    assert_eq!(app.playback(), playback);
}

#[test]
fn details_modal_consumes_controls_enter_and_escape_close_and_a_closes_stage() {
    let mut app = connected_overlay_with_open_details();
    for outcome in [KeyOutcome::SelectNext, KeyOutcome::TogglePlayback, KeyOutcome::VolumeUp, KeyOutcome::CycleTheme, KeyOutcome::ToggleSignalView] {
        assert_eq!(handle_collage_key(outcome, &mut app), Some(Flow::Continue));
        assert!(app.is_agent_details_open());
    }
    handle_collage_key(KeyOutcome::Play, &mut app);
    assert!(!app.is_agent_details_open());
    // Repeat with ExitOrBack; ToggleAgentPulse closes both stage and modal.
}

#[test]
fn stage_footer_advertises_enter_details_without_persistent_tag_copy() {
    let text = buffer_text(&render_stage(&connected_selected_app(), 120, 36));
    assert!(text.contains("Enter details"));
    assert!(!text.contains("name · status"));
}
```

- [ ] **Step 2: Run key/footer tests and verify failure.**

Run:

```bash
cargo test cli::tests::enter_opens_details
cargo test cli::tests::details_modal_consumes
cargo test ui::agent_pulse::tests::stage_footer_advertises
```

Expected: FAIL because `Play` is currently consumed without opening details and no modal-first branch exists.

- [ ] **Step 3: Implement modal-first routing.**

At the start of `handle_collage_key`, preserve `Quit`; then, when
`app.is_agent_details_open()`:

```rust
match outcome {
    KeyOutcome::ToggleAgentPulse => app.apply(Action::CloseAgentOverlay),
    KeyOutcome::ExitOrBack | KeyOutcome::Play => app.apply(Action::CloseAgentDetails),
    _ => {}
}
return Some(Flow::Continue);
```

For non-modal open canvas, map `KeyOutcome::Play` to
`Action::OpenAgentDetails` and return `Continue`; keep all existing selection
and `ToggleSignalView` handling. Replace the footer's selection wording with
`Enter details`; keep `a/Esc close` stage wording and do not add `z`.

- [ ] **Step 4: Run input/full verification.**

Run:

```bash
cargo fmt --check
cargo test cli::tests::collage
cargo test cli::tests::enter_opens_details
cargo test cli::tests::details_modal_consumes
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0; normal `Enter` playback behavior remains
covered by existing non-canvas tests.

- [ ] **Step 5: Commit the input slice.**

```bash
but commit agent-pulse-ringed-planets -m "feat: control Agent Planets details"
```

### Task 5: Synchronize documentation and run release gate

**Files:**

- Modify: `AGENTS.md`
- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `docs/superpowers/specs/2026-07-19-agent-planets-stage-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-planets-stage.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-pocket-planets.md`
- Modify: `docs/superpowers/specs/2026-07-19-agent-pulse-pocket-planets-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-planets-details-modal.md`

**Interfaces:**

- Consumes: completed modal behavior and this approved design.
- Produces: a truthful current Agent Planets documentation chain; historical Pocket records retained with supersession notes.

- [ ] **Step 1: Update durable behavior and history.**

Document Agent Planets' no-tag stage, selected `Enter` details modal, allowed
`name`/`agent`/`status`/Activity fields, prohibited private fields, modal keys,
Stale/Unavailable behavior, and all footer/Single View/cyclic navigation rules.
Mark permanent Side Tags and runtime-label-only tag behavior as superseded.
Keep Pocket bodies intact as historical records. Add this modal design/plan to
AGENTS and the docs map.

- [ ] **Step 2: Update automated/manual verification status.**

Check only completed automated plan boxes. Keep every live-Herdr/manual box
unchecked; add checks for `pi`/`claude`/Activity field visibility, no permanent
tags, modal keys, stale/unavailable modal transitions, and resize truncation.

- [ ] **Step 3: Inspect documentation and run final gates.**

Run a relative-link check and stale-claim scan for `Side Tag`, `agent type`,
`Vol`, `AGENT PLANETS`, and old `z` behavior. Then run:

```bash
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: Markdown links resolve; stale presentation claims are removed or
explicitly historical; every Rust command exits 0.

- [ ] **Step 4: Commit docs.**

```bash
but commit agent-pulse-ringed-planets -m "docs: document Agent Planets details"
```

## Plan self-review

- **Spec coverage:** Task 1 covers only allowed-field parsing and privacy; Task 2 covers modal ownership, selection/recovery lifecycle; Task 3 removes every Side Tag and renders the compact record; Task 4 covers all modal keys and footer text; Task 5 covers durable docs, historical notes, manual checklist, and release gate.
- **Placeholder scan:** No unresolved markers or generic test instructions remain. Each code-changing task contains concrete assertions, implementation signatures, commands, and expected results.
- **Type consistency:** Task 1 defines `AgentDetails`; Task 2 stores `AgentDetailsOverlay`; Task 3 reads `App::selected_agent_details`; Task 4 invokes the Task 2 actions; Task 5 documents the resulting behavior.
