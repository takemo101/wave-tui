# Agent Pulse Bioluminescent Current Implementation Plan

> **Status (2026-07-19): Superseded — do not execute.** The Kinetic Collage
> redesign
> (`docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`,
> plan `docs/superpowers/plans/2026-07-18-agent-pulse-kinetic-collage.md`)
> replaced the Bioluminescent Current presentation after the user rejected
> this direction on 2026-07-19. This plan's tasks were implemented (commits
> `db941a5`, `ea639cd`, `5c25fd7`) and its renderer has since been rewritten
> as the Kinetic Collage; the integration and privacy boundaries it relied on
> remain in force.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Beat Orbit canvas with a full-screen, FFT-derived Bioluminescent Current whose agent lights change glow, size, and short trails with real played audio.

**Architecture:** Preserve existing local same-socket cross-workspace agent normalization, live selection state, and full-screen input routing. Rewrite the UI renderer so it derives a continuous current, stable agent anchors, and trails from `App::viz()`, `App::viz_history()`, theme, geometry, and injected time; no new mutable animation state is added.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `VizFrame` RMS/FFT/history, Crossterm input, pure buffer tests.

## Global Constraints

- Use only the existing current-socket `agent.list` data; retain all cross-workspace and read-only/privacy boundaries.
- `a` opens/closes a single full-screen Current canvas; `Esc` closes it; Signal View ignores `a`.
- Current is continuous FFT-derived flow, not a timer-only animation.
- Every agent remains one light with a stable flow anchor; never group or omit lights.
- Agent lights react to assigned FFT energy and RMS through glow, size, and short local-flow trails.
- Only a selected light with explicit Herdr `name` shows `name · status`; no fallback label or hidden identifier may render.
- Theme colors convey state; silence is dim/static; stale freezes+dims; unavailable hides lights; low-power freezes flow, positions, and trails.
- Preserve existing global player shortcuts while the canvas is open and suppress search/station navigation there.
- Do not alter playback/audio decoding, add dependencies, persist pulse state, or touch pane content/control APIs.

---

### Task 1: Replace Beat Orbit geometry with pure Bioluminescent Current rendering

**Files:**

- Rewrite: `src/ui/agent_pulse.rs`
- Modify: `src/ui.rs:73-128, 878-939, 2368-2827`
- Modify: `src/ui/visualizer.rs:384-407` only if a shared FFT resampling helper needs a narrower signature or documentation update

**Interfaces:**

- Consumes: `&App`, `&Theme`, `&VizFrame`, `&[VizFrame]` from `App::viz_history()`, `Rect`, `Instant`, and `low_power: bool`.
- Produces: `render_canvas(app, theme, low_power, now, area, buf)` and `hit_test(area, column, row, app) -> Option<Action>`.
- Produces: a private `CurrentLayout` containing a band-resampled flow polyline, stable light anchors, current light cells, and trail cells.

- [ ] **Step 1: Write failing pure-current layout tests.**

```rust
#[test]
fn current_flow_tracks_fft_shape_not_elapsed_time() {
    let area = Rect::new(0, 0, 80, 24);
    let low = current_layout(&agents(3), &frame(0.2, vec![0.1, 0.8, 0.2]), &[], area, false);
    let high = current_layout(&agents(3), &frame(0.2, vec![0.8, 0.1, 0.8]), &[], area, false);
    assert_ne!(low.flow_cells, high.flow_cells);
}

#[test]
fn each_light_has_a_stable_anchor_and_a_short_history_trail() {
    let area = Rect::new(0, 0, 80, 24);
    let agents = agents(6);
    let first = current_layout(&agents, &frame(0.3, vec![0.2; 12]), &[], area, false);
    let next = current_layout(&agents, &frame(0.7, vec![0.8; 12]), &[frame(0.2, vec![0.1; 12])], area, false);
    assert_eq!(first.lights[0].anchor_seed, next.lights[0].anchor_seed);
    assert!(!next.lights[0].trail_cells.is_empty());
}
```

- [ ] **Step 2: Run the focused tests and verify failure.**

Run: `cargo test ui::agent_pulse::tests::current_`

Expected: FAIL because `CurrentLayout` and `current_layout` do not exist.

- [ ] **Step 3: Implement continuous flow and light geometry.**

Reuse `visualizer::spectrum_columns` to interpolate FFT bands to canvas width. Convert each column magnitude to a y-coordinate around the canvas middle. Assign each `AgentId` a stable x-column seed; use local flow tangent plus RMS/assigned-band energy for its visible size and one-to-three prior-frame trail cells.

```rust
let energy = (frame.rms * 0.55 + assigned_band * 0.45).clamp(0.0, 1.0);
let radius = if low_power { 0 } else { (energy * 2.0).round() as i16 };
let y = flow_y_at(anchor_x, &flow) + local_offset(radius, tangent);
let trails = history.iter().take(3).map(|old| light_cell_for(old, anchor_x));
```

When all bands and RMS are zero, return the same dim flow/light geometry regardless of `now`. In low-power, use the current frame only for color/brightness and return fixed positions with no trails.

- [ ] **Step 4: Write failing buffer tests for visual contract.**

```rust
#[test]
fn louder_audio_changes_light_glow_size_and_trails() {
    let quiet = render_current(agents(4), frame(0.05, vec![0.05; 16]), vec![], false);
    let loud = render_current(agents(4), frame(0.9, vec![0.9; 16]), vec![frame(0.4, vec![0.4; 16])], false);
    assert_ne!(quiet, loud);
    assert!(count_trail_cells(&loud) > count_trail_cells(&quiet));
}

#[test]
fn selected_explicit_name_is_the_only_rendered_agent_detail() {
    let mut app = app_with_named_and_unnamed_agents();
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    let text = buffer_text(render_current_for(&app));
    assert!(text.contains("research · working"));
    assert!(!text.contains("workspace-1"));
    assert!(!text.contains("pane-1"));
    assert!(!text.contains("claude"));
}
```

- [ ] **Step 5: Render full-screen Current and recovery states.**

Clear the composed player when the canvas is open. Draw current first, then state-colored lights and trails, then selected explicit-name label. Keep the header/count restrained. Draw Stale using frozen/dim last-frame geometry plus `stale · reconnecting`; draw Unavailable with no light/trail cells and `agents · unavailable · retrying`.

Replace all Beat Orbit-specific tests with Current tests for full-screen coverage, all-agent density, state colors, Done removal, silence, low power, stale/unavailable, selected-name-only privacy, Compact/Signal/hidden absence, and `hit_test` resolving only current light cells.

- [ ] **Step 6: Run UI verification.**

Run: `cargo fmt --check && cargo test ui && cargo check`

Expected: every command exits 0.

- [ ] **Step 7: Commit the renderer slice.**

```bash
but commit herdr-agent-pulse-design -m "feat: render Bioluminescent Current"
```

### Task 2: Verify full-screen controls against Current geometry

**Files:**

- Modify: `src/cli.rs:855-1165, 1853-2243`
- Modify: `src/ui.rs:90-96` only if Current hit-test naming/documentation changed

**Interfaces:**

- Consumes: `KeyOutcome`, `App`, `AudioHandle`, `SearchDebounce`, `Persistence` and Current `hit_test` actions.
- Produces: unchanged non-recursive `handle_beat_orbit_key` behavior with renamed Current-focused tests and particle-only mouse selection.

- [ ] **Step 1: Write failing control tests using Current light cells.**

```rust
#[test]
fn current_click_selects_a_light_without_moving_station_selection() {
    let mut app = connected_current_app();
    app.apply(Action::ToggleAgentOverlay);
    let station_index = app.selected_index();
    let (x, y) = first_current_light_hit(&app);
    handle_mouse(left_click(x, y), canvas_area(), &mut app);
    assert!(app.selected_agent().is_some());
    assert_eq!(app.selected_index(), station_index);
}

#[test]
fn current_keeps_global_player_shortcuts_and_suppresses_search_navigation() {
    let mut app = connected_current_app();
    app.apply(Action::ToggleAgentOverlay);
    assert_global_shortcuts_work(&mut app);
    assert_search_and_station_navigation_are_inert(&mut app);
}
```

- [ ] **Step 2: Run the focused tests and verify failure.**

Run: `cargo test cli::tests::current_`

Expected: FAIL because test helpers still search for Beat Orbit hit geometry or old names.

- [ ] **Step 3: Adapt only geometry-facing input tests and comments.**

Keep the existing Signal View-first and non-recursive gate intact. Update `handle_mouse` documentation from `selection/disclosure` to `light selection`, and replace any Beat Orbit helper with a scan of `ui::agent_pulse_hit_test` over Current canvas cells. Do not change normal player shortcut semantics.

- [ ] **Step 4: Run controller and full verification.**

Run:

```bash
cargo test cli
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

- [ ] **Step 5: Commit the input verification slice.**

```bash
but commit herdr-agent-pulse-design -m "test: verify Bioluminescent Current controls"
```

### Task 3: Synchronize durable docs and release verification

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md` only if its concise scope pointer needs revision
- Modify: `docs/superpowers/plans/2026-07-18-agent-pulse-beat-orbit.md` to mark it superseded

**Interfaces:**

- Consumes: final Current behavior from Tasks 1–2.
- Produces: user-facing instructions and manual verification checklist matching the code and current approved spec.

- [ ] **Step 1: Add executable documentation anchors.**

```rust
#[test]
fn current_normal_view_shows_only_active_count() {
    let text = buffer_text(render_buffer(&app_with_agents(agents(2)), 120, 36));
    assert!(text.contains("● 2 active"));
    assert!(!text.contains("research"));
}

#[test]
fn current_hides_details_until_a_light_is_selected() {
    let app = app_with_named_and_unnamed_agents();
    assert!(!buffer_text(render_current_for(&app)).contains("research"));
}
```

- [ ] **Step 2: Update product docs and decision records.**

Document Bioluminescent Current as the superseding visual decision: full-screen `a` view, FFT flow, RMS/band-driven glow/size/trails, selected-name-only labels, same-socket cross-workspace aggregation, state colors, silence, stale/unavailable, low power, privacy boundaries, and unchanged global shortcuts. Remove Beat Orbit/modal/list/card/history claims. Keep manual checks explicitly unperformed until actual live verification.

- [ ] **Step 3: Run final automated and manual-check recording gate.**

Run:

```bash
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

Record an unchecked manual checklist for live stream visual quality, theme legibility, terminal resize, multi-workspace density, click selection, reconnect, low-power, standalone, and disabled launch.

- [ ] **Step 4: Commit documentation.**

```bash
but commit herdr-agent-pulse-design -m "docs: document Bioluminescent Current"
```

## Plan self-review

- **Spec coverage:** Task 1 covers continuous FFT flow, all-agent lights, RMS/FFT glow-size-trails, quiet silence, low power, recovery, privacy, and pure geometry. Task 2 covers full-screen input and preserved player shortcuts. Task 3 covers durable docs and release verification.
- **Placeholder scan:** This plan contains no unresolved markers, omitted test bodies, or deferred implementation steps.
- **Type consistency:** `CurrentLayout` owns flow/light/trail geometry; `render_canvas` consumes `App::viz()` and `App::viz_history()`; `hit_test` returns only existing `Action::SelectAgent(AgentId)` values; CLI continues to apply those actions.
