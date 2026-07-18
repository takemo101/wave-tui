# Agent Pulse Beat Orbit Implementation Plan

> **Status (2026-07-18): Superseded — do not execute.** The Bioluminescent
> Current redesign
> (`docs/superpowers/specs/2026-07-18-agent-pulse-bioluminescent-current-design.md`,
> plan `docs/superpowers/plans/2026-07-18-agent-pulse-bioluminescent-current.md`)
> replaced the Beat Orbit presentation before release. Tasks 1–3 of this plan
> were implemented (commits `41b1bc5`, `09d9a55`); the cross-workspace
> aggregation, live-only state, and non-recursive full-screen input routing
> from Tasks 1 and 3 survive in the current implementation, while the Beat
> Orbit ring renderer from Task 2 was rewritten as the Bioluminescent Current
> canvas. Task 4's documentation sync was never performed under this plan; the
> durable docs were synchronized under the Bioluminescent Current plan's
> Task 3 instead.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Agent Pulse detail modal with a full-screen, music-reactive Beat Orbit that renders every agent returned by the current Herdr socket as a stable, selectable particle.

**Architecture:** `herdr` will retain a private workspace-qualified identity while normalizing every returned agent. `app` will keep only live agent/selection/connection state. `ui::agent_pulse` will render a pure full-screen Beat Orbit from `App`, `VizFrame`, geometry, and injected time; `cli` will route opening, closing, selection, and existing player shortcuts.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing native audio `VizFrame` (RMS + FFT bands), Crossterm input, existing pure buffer/reducer/CLI tests.

## Global Constraints

- Aggregate only agents returned by the current local Herdr control socket; do not create sockets or discover other Herdr sessions.
- Only `agent.list` is permitted; no pane output, prompts, files, scrollback, or pane control.
- Show every agent as one particle; never group or omit agents because of count.
- `a` opens/closes full-screen Beat Orbit; `Esc` closes it; Signal View ignores `a`.
- Particle identity and orbit slot must remain stable across status changes.
- Selected particles show explicit `name` only. If absent, show no label; do not reveal pane ID, cwd, or agent type.
- Use theme colors only. Working is strongest, Blocked uses `theme.error`, Idle is muted, Done fades then disappears on the next absent snapshot.
- Use actual RMS/FFT values, not a timer-only animation. Silence is dim and static.
- In `--low-power`, positions and trails are fixed; state colors and minimal brightness may still update.
- Wide/Medium normal UI shows only `● n active`; Compact/Signal View/standalone/disabled show no normal Agent Pulse line.
- Stale freezes and dims the last orbit with `reconnecting`; Unavailable hides particles with calm unavailable copy.
- Existing playback, volume, theme, favorite, and visualizer controls remain available while Beat Orbit is open. Station navigation/search remains suppressed there.
- Do not persist Agent Pulse state.

---

### Task 1: Normalize all socket agents and simplify Agent Pulse state

**Files:**

- Modify: `src/herdr.rs:20-131, tests`
- Modify: `src/app.rs:100-305, 447-588, 950-1102, 1266-1303, tests`

**Interfaces:**

- Consumes: `RawAgent { pane_id, workspace_id, agent, name, cwd, agent_status }` from Herdr JSON.
- Produces: `AgentSnapshot { id: AgentId, name: Option<String>, status: AgentStatus }` for `Action::AgentSnapshot`.
- Produces: `App::active_agents() -> &[AgentView]`, `App::selected_agent() -> Option<&AgentView>`, and `App::agent_pulse_connection()` for the renderer.

- [ ] **Step 1: Write failing adapter tests for cross-workspace aggregation and identity.**

```rust
#[test]
fn parses_agents_from_every_workspace_with_qualified_identity() {
    let parsed = parse_agent_list(
        r#"{"result":{"agents":[
          {"pane_id":"p1","workspace_id":"alpha","name":"research","agent_status":"working"},
          {"pane_id":"p1","workspace_id":"beta","name":"review","agent_status":"idle"}
        ]}}"#,
    )
    .unwrap();

    assert_eq!(parsed.len(), 2);
    assert_ne!(parsed[0].id, parsed[1].id);
}
```

- [ ] **Step 2: Run the targeted adapter test and verify failure.**

Run: `cargo test herdr::tests::parses_agents_from_every_workspace_with_qualified_identity`

Expected: FAIL because parsing still accepts a workspace filter and snapshots lack a qualified identity.

- [ ] **Step 3: Introduce a private qualified identity and remove current-workspace filtering.**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AgentId {
    workspace_id: String,
    pane_id: String,
}

pub struct AgentSnapshot {
    pub(crate) id: AgentId,
    pub name: Option<String>,
    pub status: AgentStatus,
}

fn parse_agent_list(line: &str) -> Option<Vec<AgentSnapshot>> {
    let agents = serde_json::from_str::<RawResponse>(line).ok()?.result?.agents?;
    Some(agents.into_iter().filter_map(|value| {
        let raw = serde_json::from_value::<RawAgent>(value).ok()?;
        Some(AgentSnapshot {
            id: AgentId { workspace_id: raw.workspace_id, pane_id: raw.pane_id },
            name: raw.name,
            status: normalize_status(raw.agent_status.as_deref()),
        })
    }).collect())
}
```

Change `request_agent_list` and its callers to call `parse_agent_list(line.trim_end())`; retain `HerdrContext.workspace_id` only if the live protocol still requires it for eligibility, otherwise remove it with its tests.

- [ ] **Step 4: Write failing reducer tests for stable identity, selection, and done removal.**

```rust
#[test]
fn identical_pane_ids_from_two_workspaces_remain_distinct_and_selectable() {
    let mut app = app_with_agents(vec![agent("alpha", "p1", "research", Working),
                                       agent("beta", "p1", "review", Idle)]);
    app.apply(Action::SelectAgent(agent_id("beta", "p1")));
    assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("review"));
}

#[test]
fn done_agent_remains_live_until_the_next_snapshot_omits_it() {
    let t0 = Instant::now();
    let mut app = App::new(Settings::default(), Catalog::curated());
    app.apply(Action::AgentSnapshot {
        agents: vec![agent("alpha", "p1", Some("research"), AgentStatus::Done)],
        now: t0,
    });
    assert_eq!(app.active_agents().len(), 1);
    app.apply(Action::AgentSnapshot { agents: vec![], now: t0 + Duration::from_secs(5) });
    assert!(app.active_agents().is_empty());
}
```

- [ ] **Step 5: Remove completed-history/detail state and implement live-only reconciliation.**

Remove `CompletedAgent`, `completed`, `completed_disclosed`, `ToggleCompletedAgents`, and their accessors/actions/tests. Replace pane-ID strings with `AgentId` for active matching and selection. Preserve `observed_at` only when `AgentId` and `AgentStatus` match; preserve Done in the active snapshot so UI can dim it, and remove it when a later snapshot omits it.

```rust
let carried = previous.iter().find(|view| {
    view.id == snapshot.id && view.status == snapshot.status
});
```

- [ ] **Step 6: Run focused tests.**

Run: `cargo test herdr::tests:: && cargo test app::tests::agent_`

Expected: PASS; cross-workspace snapshots stay distinct, selection is stable, no completed-history API remains.

- [ ] **Step 7: Commit the reducer/adapter slice.**

```bash
but commit herdr-agent-pulse-design -m "feat: aggregate Agent Pulse agents across workspaces"
```

### Task 2: Build the pure full-screen Beat Orbit renderer

**Files:**

- Modify: `src/ui/visualizer.rs:384-407`
- Rewrite: `src/ui/agent_pulse.rs`
- Modify: `src/ui.rs:73-128, 878-939, 2368-2827`

**Interfaces:**

- Consumes: `&App`, `&Theme`, `&VizFrame`, `Instant`, `Rect`, and `low_power: bool`.
- Produces: `agent_pulse::render_canvas(app, theme, low_power, now, area, buf)` and `agent_pulse::hit_test(area, column, row, app) -> Option<Action>`.
- Produces: `visualizer::spectrum_columns(bands, width)` as `pub(super)` for shared FFT resampling.

- [ ] **Step 1: Write pure layout tests before rendering.**

```rust
#[test]
fn orbit_slots_are_stable_when_status_changes() {
    let before = beat_orbit_layout(&[view("alpha", "p1", Working)], Rect::new(0, 0, 100, 30));
    let after = beat_orbit_layout(&[view("alpha", "p1", Blocked)], Rect::new(0, 0, 100, 30));
    assert_eq!(before.particles[0].anchor, after.particles[0].anchor);
}

#[test]
fn dense_layout_keeps_one_particle_per_agent() {
    let layout = beat_orbit_layout(&many_views(80), Rect::new(0, 0, 50, 15));
    assert_eq!(layout.particles.len(), 80);
}
```

- [ ] **Step 2: Run layout tests and verify they fail.**

Run: `cargo test ui::agent_pulse::tests::orbit_`

Expected: FAIL because `BeatOrbitLayout` and `beat_orbit_layout` do not exist.

- [ ] **Step 3: Add deterministic concentric-ring layout and real-frame motion.**

Use a stable hash of `AgentId` for a ring/slot seed, then assign compact glyph size/spacing from terminal area. Derive visual displacement from RMS and a band selected by the stable seed. Never use random state.

```rust
let band = frame.bands.get(seed % frame.bands.len()).copied().unwrap_or(0.0);
let energy = (frame.rms * 0.65 + band * 0.35).clamp(0.0, 1.0);
let radial = if low_power { 0.0 } else { energy * ring_motion };
let point = anchor.offset_from_center(radial, seed_angle);
```

Expose existing `spectrum_columns` as `pub(super)` rather than copying FFT interpolation.

- [ ] **Step 4: Write buffer tests for visual behavior.**

```rust
#[test]
fn rms_and_bands_move_orbit_without_timer_only_motion() {
    let quiet = render_canvas(frame_with(0.05, vec![0.0; 8]), false, t0);
    let loud = render_canvas(frame_with(0.90, vec![0.8; 8]), false, t0);
    assert_ne!(quiet, loud);
}

#[test]
fn silence_and_low_power_keep_particle_positions_static() {
    assert_eq!(render_canvas(silent_frame(), false, t0), render_canvas(silent_frame(), false, t1));
    assert_eq!(particle_positions(render_canvas(loud_frame(), true, t0)),
               particle_positions(render_canvas(loud_frame(), true, t1)));
}
```

- [ ] **Step 5: Render state, selection, and recovery states.**

Render one full-frame canvas with a title/count, theme-colored particles, selected explicit-name label only, and a restrained footer hint. Do not render list/card/history/cwd/pane ID/agent type. Render Stale from the last layout with `reconnecting` and dim styling. Render Unavailable with no particle glyphs. Done particles use muted dim styling until the snapshot omits them.

- [ ] **Step 6: Replace old modal tests and verify normal surfaces.**

Replace list/card/disclosure/modal tests with buffer tests for full-screen coverage, selected-name-only behavior, no-name suppression, state colors, stale/unavailable, all-agent density, and hidden/Compact/Signal View exact absence. Change normal Wide/Medium summary expectation to only `● n active`.

- [ ] **Step 7: Run UI tests and commit.**

Run: `cargo test ui`

Expected: PASS.

```bash
but commit herdr-agent-pulse-design -m "feat: render music-reactive Agent Pulse orbit"
```

### Task 3: Route full-screen controls without regressing player controls

**Files:**

- Modify: `src/cli.rs:855-1140, 1853-2243`
- Modify: `src/ui.rs:90-96`

**Interfaces:**

- Consumes: `KeyOutcome`, `App`, `AudioHandle`, `SearchDebounce`, `Persistence`.
- Produces: an Agent Pulse key gate that consumes selection/search/list-navigation keys but delegates documented global player controls to the existing normal handling path without recursive `handle_key` calls.

- [ ] **Step 1: Write failing controller tests.**

```rust
#[test]
fn beat_orbit_a_and_escape_toggle_without_changing_station_focus() {
    let mut app = connected_pulse_app();
    let focus = app.focus();
    assert_eq!(handle_key(key(KeyCode::Char('a')), &mut app, &audio, &mut debounce, &mut persistence), Flow::Continue);
    assert!(app.is_agent_overlay_open());
    assert_eq!(app.focus(), focus);
    assert_eq!(handle_key(key(KeyCode::Esc), &mut app, &audio, &mut debounce, &mut persistence), Flow::Continue);
    assert!(!app.is_agent_overlay_open());
}

#[test]
fn beat_orbit_selection_does_not_move_station_selection() {
    let mut app = connected_pulse_app();
    let station = app.selected_index();
    app.apply(Action::ToggleAgentOverlay);
    handle_key(key(KeyCode::Tab), &mut app, &audio, &mut debounce, &mut persistence);
    assert!(app.selected_agent().is_some());
    assert_eq!(app.selected_index(), station);
}

#[test]
fn beat_orbit_keeps_volume_theme_playback_favorite_and_visualizer_controls() {
    let mut app = connected_pulse_app();
    app.apply(Action::ToggleAgentOverlay);
    let original_theme = app.settings().theme;
    handle_key(key(KeyCode::Char('t')), &mut app, &audio, &mut debounce, &mut persistence);
    assert_ne!(app.settings().theme, original_theme);
}
```

- [ ] **Step 2: Run the focused tests and verify failure.**

Run: `cargo test cli::tests::beat_orbit_`

Expected: FAIL because the old overlay handler consumes all global controls and still assumes a card/list modal.

- [ ] **Step 3: Split outcome routing into pulse-local and existing-global paths.**

Create a non-recursive helper returning whether an outcome was consumed by Beat Orbit:

```rust
fn handle_beat_orbit_key(outcome: KeyOutcome, app: &mut App) -> Option<Flow> {
    match outcome {
        KeyOutcome::Quit => Some(Flow::Quit),
        KeyOutcome::ToggleAgentPulse | KeyOutcome::ExitOrBack => {
            app.apply(Action::CloseAgentOverlay); Some(Flow::Continue)
        }
        KeyOutcome::FocusNext | KeyOutcome::SelectNext => {
            app.apply(Action::SelectNextAgent); Some(Flow::Continue)
        }
        KeyOutcome::FocusPrevious | KeyOutcome::SelectPrevious => {
            app.apply(Action::SelectPreviousAgent); Some(Flow::Continue)
        }
        KeyOutcome::BeginSearch | KeyOutcome::SearchChar(_) | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch | KeyOutcome::SelectFirst | KeyOutcome::SelectLast
        | KeyOutcome::Play => Some(Flow::Continue),
        _ => None,
    }
}
```

When `None`, execute the existing non-overlay outcome branch exactly once. Keep Signal View before this gate.

- [ ] **Step 4: Update mouse routing and tests.**

Keep mouse capture conditional on monitor existence. Map only full-screen particle hit targets to `SelectAgent`; return `None` for Stale, Unavailable, Hidden, Signal View, and outside-canvas clicks.

- [ ] **Step 5: Run CLI tests and commit.**

Run: `cargo test cli`

Expected: PASS.

```bash
but commit herdr-agent-pulse-design -m "feat: add Beat Orbit controls"
```

### Task 4: Update durable docs and run release verification

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md` only if its concise scope pointer needs revision
- Modify: `docs/superpowers/plans/2026-07-16-herdr-agent-pulse.md` to mark its superseded presentation steps

**Interfaces:**

- Consumes: final Beat Orbit behavior from Tasks 1–3.
- Produces: user instructions and a manual checklist that match the code and new design spec.

- [ ] **Step 1: Write/update doc assertions in existing Rust tests before changing prose.**

```rust
#[test]
fn normal_view_shows_only_active_count() {
    let app = app_with_agents(vec![agent("alpha", "p1", Some("research"), AgentStatus::Working)]);
    let text = buffer_text(render_buffer(&app, 120, 36));
    assert!(text.contains("● 1 active"));
    assert!(!text.contains("research"));
}

#[test]
fn beat_orbit_hides_agent_details_until_selected() {
    let mut app = app_with_agents(vec![agent("alpha", "p1", Some("research"), AgentStatus::Working)]);
    app.apply(Action::ToggleAgentOverlay);
    assert!(!buffer_text(render_orbit(&app, loud_frame())).contains("research"));
    app.apply(Action::SelectNextAgent);
    assert!(buffer_text(render_orbit(&app, loud_frame())).contains("research · working"));
}

#[test]
fn cross_workspace_agents_render_as_particles() {
    let app = app_with_agents(vec![agent("alpha", "p1", Some("research"), AgentStatus::Working),
                                   agent("beta", "p1", Some("review"), AgentStatus::Idle)]);
    assert_eq!(count_orbit_particles(&render_orbit(&app, loud_frame())), 2);
}
```

- [ ] **Step 2: Update README and product docs.**

Document `a` full-screen Beat Orbit, selected-name-only labels, same-session cross-workspace aggregation, local-only/read-only boundaries, `--no-agent-pulse`, no-audio quiet state, low-power reduced motion, and stale/unavailable behavior. Remove obsolete list/card/completed/cwd descriptions.

- [ ] **Step 3: Update decision records.**

Link `docs/superpowers/specs/2026-07-18-agent-pulse-beat-orbit-design.md` as the superseding presentation decision. Keep the 2026-07-16 spec as historical context, but state which presentation rules it no longer governs.

- [ ] **Step 4: Run full automated verification.**

Run:

```bash
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

- [ ] **Step 5: Record honest manual checks and commit.**

Record unchecked/manual evidence for live multi-workspace aggregation, real-stream RMS/FFT response, theme legibility, dense particle count, resize, mouse selection, stale/reconnect, low-power, standalone, and disabled launch.

```bash
but commit herdr-agent-pulse-design -m "docs: document Agent Pulse Beat Orbit"
```

## Plan self-review

- **Spec coverage:** Task 1 covers same-socket cross-workspace aggregation and live-only state. Task 2 covers full-screen stable Beat Orbit, audio response, identity labels, all-agent density, state colors, quiet silence, stale/unavailable, and low power. Task 3 covers full-screen controls and input preservation. Task 4 covers documentation and automated/manual verification.
- **Placeholder scan:** This plan contains no unresolved markers, omitted test bodies, or deferred implementation steps. Each task commits only after its working tree contains that task's reviewed changes.
- **Type consistency:** `AgentId` is the identity consumed by `AgentSnapshot`, `AgentView`, selection actions, orbit layout, and mouse routing. `render_canvas` consumes `VizFrame` through `App::viz()` and uses the shared `visualizer::spectrum_columns` helper.
