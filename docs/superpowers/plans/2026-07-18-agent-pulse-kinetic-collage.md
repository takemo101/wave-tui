# Agent Pulse Kinetic Collage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Bioluminescent Current with a full-screen, music-reactive Kinetic Collage of stable procedural album-art tiles, one tile per agent.

**Architecture:** Keep the existing same-socket agent aggregation, selection, stale visual snapshot, and non-recursive control routing. Replace only `ui::agent_pulse` presentation: it will derive stable tile motifs and staggered layout from private `AgentId`, then apply real RMS/FFT/history-derived transform, soft shadow trail, background trace, and vignette in pure Buffer rendering.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `VizFrame`/history, existing theme palette, pure UI/CLI/reducer tests.

## Global Constraints

- No new dependencies, playback changes, persistence, sockets, Herdr methods, pane reads, or pane control.
- One visible tile per agent, including dense counts; never group/omit tiles.
- Tile art and base layout are deterministic per private `AgentId`; use staggered rectangles with asymmetric/diagonal terminal motifs, not unsupported geometric rotation.
- RMS and assigned FFT-band energy move tile scale/offset and one- or two-layer soft shadow trails; tile motif never morphs/swaps with audio.
- Background is a theme-phosphor breathing vignette plus low-contrast actual waveform/FFT trace behind tiles.
- State color is edge glow only: Working strongest, Blocked `theme.error`, Idle muted, Done muted/dim until source omission.
- Only selected tiles with explicit `name` render `name · status`; no fallback identifier, pane/workspace/cwd/agent type may render.
- Silence is dim/static; low-power freezes background/tile geometry/trails with only state edge glow/minimal brightness updates.
- Stale freezes/dims the final live collage snapshot; unavailable hides tiles with calm copy.
- Preserve a/Esc, Signal View-first, global player shortcuts, search/station suppression, and tile-only Connected mouse hit testing.

---

### Task 1: Render the pure Kinetic Collage canvas

**Files:**

- Rewrite: `src/ui/agent_pulse.rs`
- Modify: `src/ui.rs:73-128, 878-939, 2368-2827`
- Modify: `src/ui/visualizer.rs:344-407` to expose a shared waveform resampler as `pub(super)` if Current’s private helper cannot be reused

**Interfaces:**

- Consumes: `&App`, `&Theme`, `&VizFrame`, `&[VizFrame]`, `Rect`, `Instant`, `low_power: bool`.
- Produces: `CollageLayout { background, tiles }`, `render_canvas`, and pure `hit_test` returning only `Action::SelectAgent(AgentId)`.
- Produces: private `AlbumMotif` values (`Record`, `Diagonal`, `Stripe`, `Frame`) picked from stable identity hash.

- [x] **Step 1: Write failing deterministic motif and layout tests.**

```rust
#[test]
fn tile_motif_and_staggered_rect_stay_stable_for_an_agent_identity() {
    let area = Rect::new(0, 0, 120, 36);
    let agent = agent("alpha", "p1", Some("research"), AgentStatus::Working);
    let first = collage_layout(&[agent.clone()], &silent_frame(), &[], area, false);
    let later = collage_layout(&[agent], &loud_frame(), &[quiet_frame()], area, false);
    assert_eq!(first.tiles[0].motif, later.tiles[0].motif);
    assert_eq!(first.tiles[0].base_rect, later.tiles[0].base_rect);
}

#[test]
fn dense_collage_keeps_one_tile_per_agent() {
    let layout = collage_layout(&many_agents(80), &silent_frame(), &[], Rect::new(0, 0, 50, 15), false);
    assert_eq!(layout.tiles.len(), 80);
}
```

- [x] **Step 2: Run focused tests and verify failure.**

Run: `cargo test ui::agent_pulse::tests::tile_`

Expected: FAIL because `CollageLayout` and `collage_layout` do not exist.

- [x] **Step 3: Implement deterministic staggered tile layout and motifs.**

Assign each `AgentId` a stable hash. Derive a grid cell, bounded base rectangle, motif, and motif palette arrangement from that hash. At dense sizes reduce width/height before reducing visibility. Render motifs with Ratatui cell background/foreground styles and glyph patterns (`░`, `▒`, `╱`, `╲`, `◌`), never fixed RGB values.

```rust
let seed = stable_seed(&view.id);
let motif = AlbumMotif::ALL[seed as usize % AlbumMotif::ALL.len()];
let base_rect = staggered_tile_rect(seed, index, agents.len(), area);
let accent = state_edge_style(view.status, theme);
render_motif(buf, base_rect, motif, theme, accent);
```

- [x] **Step 4: Write failing audio-motion buffer tests.**

```rust
#[test]
fn rms_and_fft_expand_tiles_and_add_soft_shadow_trails() {
    let quiet = render_collage(agents(4), frame(0.05, vec![0.05; 16]), vec![], false);
    let loud = render_collage(agents(4), frame(0.9, vec![0.9; 16]), vec![frame(0.4, vec![0.4; 16])], false);
    assert_ne!(quiet, loud);
    assert!(count_shadow_cells(&loud) > count_shadow_cells(&quiet));
}

#[test]
fn selected_explicit_name_is_the_only_tile_detail() {
    let mut app = app_with_named_and_unnamed_agents();
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    let text = buffer_text(render_collage_for(&app));
    assert!(text.contains("research · working"));
    assert!(!text.contains("workspace-1"));
    assert!(!text.contains("pane-1"));
    assert!(!text.contains("claude"));
}
```

- [x] **Step 5: Render background, motion, recovery, and hit targets.**

Use RMS to set a low-contrast theme-phosphor vignette radius/intensity. Draw an actual waveform/FFT trace behind tile shadows and tiles. Compute energy as `rms * 0.55 + assigned_band * 0.45`; use it for bounded tile translation/scale and up to two soft shadow rectangles from recent history. Draw only tile rectangles as mouse targets. Render Stale from `App::stale_viz()` captured frame/history, dimmed; render Unavailable without tiles.

- [x] **Step 6: Replace Current tests with Collage contract tests.**

Add buffer tests for background trace/vignette response, stable tile art, all-tile density, state edge glow, quiet silence, low-power static geometry, stale freeze/dim, unavailable hiding, selected-name-only privacy, tile-only hit testing, full-screen coverage, and Compact/Signal/hidden absence. Remove Current-specific trace/light tests that no longer describe the product.

- [x] **Step 7: Run UI verification and commit.**

Run:

```bash
cargo fmt --check
cargo test ui
cargo check
```

Expected: every command exits 0.

```bash
but commit herdr-agent-pulse-design -m "feat: render Kinetic Collage"
```

### Task 2: Verify input behavior against collage tile geometry

**Files:**

- Modify: `src/cli.rs:855-1165, 1853-2243`
- Modify: `src/ui.rs:90-96` only if hit-test documentation needs Current-to-Collage terminology

**Interfaces:**

- Consumes: existing `KeyOutcome`, `App`, audio/persistence handles, and Collage `hit_test`.
- Produces: unchanged non-recursive full-screen input behavior with tests that find actual tile hit cells.

- [x] **Step 1: Write failing collage control tests.**

```rust
#[test]
fn collage_click_selects_a_tile_without_moving_station_selection() {
    let mut app = connected_collage_app();
    app.apply(Action::ToggleAgentOverlay);
    let station_index = app.selected_index();
    let (x, y) = first_collage_tile_hit(&app);
    handle_mouse(left_click(x, y), canvas_area(), false, &mut app);
    assert!(app.selected_agent().is_some());
    assert_eq!(app.selected_index(), station_index);
}

#[test]
fn collage_keeps_global_shortcuts_and_suppresses_search_navigation() {
    let mut app = connected_collage_app();
    app.apply(Action::ToggleAgentOverlay);
    assert_global_shortcuts_work(&mut app);
    assert_search_and_station_navigation_are_inert(&mut app);
}
```

- [x] **Step 2: Run focused tests and verify failure.**

Run: `cargo test cli::tests::collage_`

Expected: FAIL because Current-specific helper names and comments remain.

- [x] **Step 3: Adapt only geometry-facing helpers, tests, and comments.**

Keep Signal View-first and `handle_current_key`’s non-recursive behavior unchanged. Rename user-facing Current wording to Kinetic Collage where it describes this surface. Scan `ui::agent_pulse_hit_test` across canvas cells to find a real tile target, and pass `low_power` through the existing mouse path if the current implementation requires it for exact drawn geometry.

- [x] **Step 4: Run controller and full verification.**

Run:

```bash
cargo test cli
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

- [x] **Step 5: Commit input verification.**

```bash
but commit herdr-agent-pulse-design -m "test: verify Kinetic Collage controls"
```

### Task 3: Finalize supersession docs and standards hygiene

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md` only for concise scope/pointer corrections
- Modify: `docs/superpowers/plans/2026-07-18-agent-pulse-bioluminescent-current.md` with a superseded banner
- Modify: `docs/superpowers/specs/2026-07-18-agent-pulse-bioluminescent-current-design.md` with a superseded header
- Modify: `docs/superpowers/specs/2026-07-18-agent-pulse-beat-orbit-design.md` and `docs/superpowers/plans/2026-07-16-herdr-agent-pulse.md` with superseded headers
- Modify: `src/herdr.rs` and `src/app.rs` only to remove stale `dead_code` allowances/comments and genuinely unused `observed_duration`
- Modify: `src/ui.rs` to restore `signal_view_long_title_render_stays_bounded` from `origin/main` if it remains absent

**Interfaces:**

- Consumes: final Collage behavior and existing test helpers.
- Produces: truthful user docs, historical supersession records, no stale lints/comments, and restored Signal View regression coverage.

- [x] **Step 1: Write/update executable documentation anchors and regression coverage.**

```rust
#[test]
fn collage_normal_view_shows_only_active_count() {
    let text = buffer_text(render_buffer(&app_with_agents(agents(2)), 120, 36));
    assert!(text.contains("● 2 active"));
    assert!(!text.contains("research"));
}

#[test]
fn signal_view_long_title_render_stays_bounded() {
    let mut app = base_app();
    play_first(&mut app);
    app.apply(Action::Audio(AudioEvent::IcyTitle {
        station: app.current_station().unwrap().id.clone(),
        title: "A deliberately long ICY title that must remain inside the Signal View layout".to_string(),
    }));
    app.apply(Action::ToggleSignalView);
    let buf = render_buffer(&app, 44, 15);
    assert!(buf.content.iter().all(|cell| cell.symbol().len() <= 4));
}
```

- [x] **Step 2: Remove stale allowances and synchronize docs.**

Remove now-false `#![allow(dead_code)]` in `src/herdr.rs`; remove false follow-up-task comments/attributes from `AgentView`/App accessors; delete `AgentView::observed_duration` when no production caller remains. Document Kinetic Collage’s procedural tiles, background trace/vignette, audio motion, edge glow, selected-name-only privacy, recovery, low power, controls, and unchecked manual verification. Mark all prior visual designs/plans superseded without altering their historical integration/privacy context.

- [x] **Step 3: Run final automated and manual-check recording gate.**

Run:

```bash
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

Record an unchecked manual checklist for live stream composition/cadence, all themes, resize/dense multi-workspace agents, click selection, reconnect, low power, standalone, and disabled launch.

- [ ] **Step 4: Commit docs and hygiene.**

```bash
but commit herdr-agent-pulse-design -m "docs: document Kinetic Collage"
```

## Plan self-review

- **Spec coverage:** Task 1 implements procedural stable album-art tiles, music background, real RMS/FFT transforms/shadows, privacy, recovery, low power, and all-tile density. Task 2 preserves input behavior while adapting hit geometry. Task 3 synchronizes docs, restores lost Signal View coverage, and removes stale standards debt.
- **Placeholder scan:** This plan contains no unresolved markers, omitted test bodies, or deferred implementation steps.
- **Type consistency:** `CollageLayout` owns background/tile/shadow geometry; `render_canvas` consumes `App::viz()`, `App::viz_history()`, and `App::stale_viz()`; `hit_test` returns only existing `Action::SelectAgent(AgentId)` values; CLI applies those actions unchanged.
