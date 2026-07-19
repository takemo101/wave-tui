# Agent Planets Stage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the `a` canvas into a centered Agent Planets stage with true round disc-mask planets, per-planet name/state side tags, station title and volume context, a normal footer hint, and `z` suppression while the stage is open.

**Architecture:** Keep the existing Dual Phase Scope and App/Herdr state intact. Replace `ui::agent_pulse`'s calculated ellipse/ring/shadow presentation with discrete disc/ring masks and stage partitions; layout each named planet with a two-line side tag. Reuse the exact Single View `signal_view_volume_line` helper inside the Agent Planets title block; add only the normal UI footer eligibility hint and a CLI canvas-local `z` consume rule, without persisting new state.

**Tech Stack:** Rust 2018, Ratatui `Buffer`/`Layout`, existing `App`/Theme/volume helpers, pure UI and CLI tests, GitButler `but`.

## Global Constraints

- User-facing copy says **Agent Planets** in Title Case (including `Agent Planets · n active`); retain technical `AgentPulse` state and the read-only same-socket `agent.list` boundary.
- Preserve the current Dual Phase Scope data, two real traces, persistence, silence rule, audio timing, stale capture, and first-audible low-power capture.
- `a` opens/closes Agent Planets. Outside Agent Planets, `z` keeps its current Single View behavior; inside Agent Planets, `z` is consumed and must not enter Single View.
- In eligible normal layouts only, render the footer hint `a Agent Planets`. Standalone, disabled, and ineligible layouts remain byte-identical with no hint.
- Remove every full-tile/rectangle planet shadow. Scope phosphor persistence may remain behind discs.
- Render the exact existing Single View `signal_view_volume_line` directly beneath the centered station/ICY title in the Agent Planets title block; do not use the Now Playing `volume_gauge_line`, `Vol` label, dedicated bottom volume row, or a separate numeric suffix.
- Planet bodies use only 7×5, 5×3, 3×3, or one-cell discrete masks; no calculated rectangle/ellipse body or ring can create a cross-like silhouette.
- Each named planet always has a two-line adjacent tag: explicit name then normalized status. Tags never reveal pane/workspace/cwd/type/fallback names; unnamed planets have no tag.
- Tags prefer right, then left, below, above; non-overlap checks cover other discs and tags. Long names truncate, status remains. Selected tag is bright and draws last.
- Keep state ring vocabulary: Working audio-driven arc, Idle muted ring, Blocked broken error arc without cross glyph, Done satellite, Unknown muted.
- Preserve planet-only selection; tags/scope/persistence/empty cells are not hit targets.
- In the interactive Agent Planets overlay only, next (`Tab`/Down/`j`) wraps last→first and previous (`Shift+Tab`/Up/`k`) wraps first→last; stale/unavailable selection remains inert and normal-layout navigation is unchanged.
- Run format, focused tests, full tests, check, Clippy, and release build; live manual checks remain unchecked.

---

### Task 1: Render the centered stage, disc masks, and side tags

**Files:**

- Modify: `src/ui/agent_pulse.rs:348-1298, 1330-2998`
- Modify: `src/ui.rs:259-420, 2403-2618`

**Interfaces:**

- Consumes: existing `App` station/ICY title, `VolumePercent`, `AgentView`, `VizFrame`, `Theme`, and active-agent selection/connection state.
- Produces: private `AgentStageLayout`, `DiscMask`, `disc_geometry`, `ring_mask`, `PlanetTag`, `planet_tag_placements`, and `render_agent_planets_stage`.
- Preserves: `phase_layers`, real phase mapping, `App` visual capture precedence, existing action routing, and `agent_pulse_hit_test` public surface.

- [x] **Step 1: Add failing stage/disc/tag tests.**

In `src/ui/agent_pulse.rs` tests, replace ellipse/rectangle-only geometry assertions with these contract tests:

```rust
#[test]
fn planet_disc_masks_are_round_and_never_draw_rectangle_shadows() {
    let stage = render_stage_with_agents(3, 120, 36);
    assert!(has_disc_mask(&stage, DiscMask::Large7x5));
    assert!(!has_full_tile_shadow(&stage));
    assert!(!buffer_text(&stage).contains('╲'));
    assert!(!buffer_text(&stage).contains('╱'));
}

#[test]
fn dense_disc_masks_reduce_7x5_to_5x3_to_3x3_then_one_cell() {
    let sparse = stage_layout_for(agents(3), Rect::new(0, 0, 120, 28));
    let dense = stage_layout_for(agents(80), Rect::new(0, 0, 50, 15));
    assert!(sparse.planets.iter().any(|planet| planet.mask == DiscMask::Large7x5));
    assert_eq!(dense.planets.len(), 80);
    assert!(dense.planets.iter().all(|planet| !planet.body.is_empty()));
}

#[test]
fn named_planets_render_two_line_side_tags_and_unnamed_planets_do_not() {
    let mut app = app_with_named_and_unnamed_agents();
    app.apply(Action::ToggleAgentOverlay);
    let text = buffer_text(&render_stage_for(&app));
    assert!(text.contains("research"));
    assert!(text.contains("working"));
    assert!(!text.contains("workspace-1"));
    assert!(!text.contains("pane-1"));
    assert!(!tag_for_unnamed_agent(&app).is_some());
}

#[test]
fn selected_tag_draws_last_and_tag_layout_avoids_discs_and_tags() {
    let (buf, layout) = crowded_stage_with_selected_agent();
    let tag = selected_tag(&layout).unwrap();
    assert_eq!(cell_text(&buf, tag.rect.x, tag.rect.y), "r");
    assert!(!tag_intersects_any_other_disc_or_tag(tag, &layout));
}
```

In `src/ui.rs` tests, add the stage chrome contract:

```rust
#[test]
fn agent_planets_stage_centers_now_playing_title_and_volume_bar() {
    let mut app = app_with_agents(named_agents(2));
    play_first(&mut app);
    app.apply(Action::ToggleAgentOverlay);
    let text = buffer_text(&render_buffer(&app, 120, 36));
    assert!(text.contains("Agent Planets"));
    assert!(text.contains(app.now_playing_title().unwrap()));
    assert!(text.contains("64%"));
}
```

- [x] **Step 2: Run focused tests and verify failure.**

Run:

```bash
cargo test ui::agent_pulse::tests::planet_disc_masks
cargo test ui::agent_pulse::tests::named_planets_render_two_line
cargo test ui::tests::agent_planets_stage_centers
```

Expected: FAIL because stage partitions, discrete masks, tag layout, and stage chrome do not exist.

- [x] **Step 3: Add stage partitions and reuse station/volume presentation.**

Replace the current full-area `collage_area` presentation with a private stage layout that reserves header, centered title, scope field, Now Playing label, volume, and footer rows while retaining a positive field at small sizes:

```rust
struct AgentStageLayout {
    heading: Rect,
    title_block: Rect,
    field: Rect,
    footer: Rect,
}

fn agent_stage_layout(area: Rect) -> AgentStageLayout {
    let title_height = if area.height >= 15 { 3 } else { 2 };
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(title_height),
        Constraint::Min(1),
        Constraint::Length(1),
    ]).split(area);
    AgentStageLayout { heading: rows[0], title_block: rows[1], field: rows[2], footer: rows[3] }
}
```

Render centered `Agent Planets · n active`. Render current ICY title, otherwise station name, otherwise `no station playing`; use the same truncation/centering approach as Signal View. Extract `signal_view_volume_line` as `pub(super)` without altering Signal View output, then append that exact `volume n%` / accent `─` / muted `·` line as the lowest title-block line, directly below the station/ICY title—exactly the placement it has in Signal View. Do not call `volume_gauge_line`, render a dedicated volume row, or append a separate numeric suffix. Use a stage footer containing selection/close/player hints but do not advertise `z`.

- [x] **Step 4: Replace calculated planets and shadows with discrete masks.**

Delete full-tile shadow rendering from `render_canvas`; keep `phase_layers` persistence. Replace `pocket_rect`, `ellipse_of`, `body_cells`, and computed `ring_cells` with explicit row masks:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscMask { Large7x5, Medium5x3, Small3x3, Dot }

const LARGE_DISC: [&str; 5] = ["  ███  ", " █████ ", "███████", " █████ ", "  ███  "];
const MEDIUM_DISC: [&str; 3] = [" ███ ", "█████", " ███ "];
const SMALL_DISC: [&str; 3] = [" █ ", "███", " █ "];
```

Choose the largest mask whose width/height fit the agent slot without crowding its tag reservation. Convert only non-space mask characters to body cells. Define an equally explicit, non-cross-like ring-arc cell list around each mask; Working selects a short segment using the existing phase signature; Blocked removes a stable segment. Surface cells must be a subset of body mask cells. `PlanetGeometry::hit_cells` remains body plus drawn ring cells only.

- [x] **Step 5: Layout and render permanent two-line side tags.**

Replace the selected-only callout with `PlanetTag { agent_index, rect, name, status, selected }`. For every explicit name, choose a two-row candidate at right/left/below/above the disc. Reject candidates colliding with prior disc cells, ring cells, or tag cells; reserve the chosen tag cells before placing the next unit. If all candidates collide, use the in-bounds right fallback and draw it last. Truncate name with an ellipsis to the chosen width; keep status line untruncated where the stage field permits. Tags use muted theme text; selected tag uses `theme.selection_style()` and renders after discs/tags.

Keep BandedGas/IceCap/CrateredRock colors inside the disc body and status color on ring/satellite only. Re-run stale/low-power tests with stage field/tag coordinates: captured geometry includes disc/ring/tag placement; fresh status snapshots may change ring treatment but do not move tag positions.

- [x] **Step 6: Verify UI behavior and commit stage rendering.**

Run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo test ui::tests::agent_planets
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

```bash
but commit agent-pulse-ringed-planets -m "feat: stage Agent Planets"
```

### Task 1B: Align Agent Planets heading and volume exactly with Single View

**Files:**

- Modify: `src/ui.rs:259-420, 2205-2331`
- Modify: `src/ui/agent_pulse.rs:854-1008, 2463-2618`

**Interfaces:**

- Consumes: existing private `signal_view_volume_line`, current station/ICY title, stage layout, and theme.
- Produces: Title Case `Agent Planets · n active` heading and a stage title block whose volume line is byte-for-byte the Single View line.
- Preserves: Single View output, phase field, disc/tag geometry, footer, selection, and every player control.

- [ ] **Step 1: Add failing parity tests.**

```rust
#[test]
fn agent_planets_heading_uses_single_view_title_case() {
    let mut app = app_with_agents(named_agents(2));
    app.apply(Action::ToggleAgentOverlay);
    let text = buffer_text(&render_buffer(&app, 120, 36));
    assert!(text.contains("Agent Planets · 2 active"));
    assert!(!text.contains("AGENT PLANETS"));
}

#[test]
fn agent_planets_reuses_the_exact_single_view_volume_line_in_its_title_block() {
    let mut app = app_with_agents(named_agents(2));
    play_first(&mut app);
    app.apply(Action::ToggleAgentOverlay);
    let stage = buffer_text(&render_buffer(&app, 120, 36));
    let signal_line = signal_view_volume_line(&app, &Theme::Minimal, 120).to_string();
    assert!(stage.contains(&signal_line));
    assert!(stage.contains("volume 64%"));
    assert!(!stage.contains("Vol "));
}
```

- [ ] **Step 2: Run parity tests and verify failure.**

Run:

```bash
cargo test ui::tests::agent_planets_heading_uses
cargo test ui::tests::agent_planets_reuses
```

Expected: FAIL because the stage has uppercase heading and its own `volume_gauge_line`/dedicated volume row.

- [ ] **Step 3: Extract and reuse the Single View line without changing Single View.**

Change `signal_view_volume_line` visibility to `pub(super)`; do not change its bytes, prefix, styles, or Signal View call site. Replace `AgentStageLayout::{now_playing, volume}` with `title_block`, and partition the stage as heading → multi-line title block → field → footer. Render Title Case `Agent Planets · n active`. Append `super::signal_view_volume_line(app, theme, layout.title_block.width)` directly below the centered station/ICY title as the lowest title metadata line. Remove stage calls to `volume_gauge_line`, `Vol`, dedicated volume row, and numeric suffix.

- [ ] **Step 4: Run UI/full verification and commit the repair.**

Run:

```bash
cargo fmt --check
cargo test ui::tests::agent_planets
cargo test ui::tests::signal_view
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0; Signal View snapshots remain unchanged.

```bash
but commit agent-pulse-ringed-planets -m "fix: align Agent Planets volume with Single View"
```

### Task 1C: Cycle Agent Planets keyboard selection at both ends

**Files:**

- Modify: `src/app.rs:1016-1054, 2965-3050`
- Modify: `src/cli.rs:1118-1158, 2310-2405` only if an existing key-routing test needs an explicit wrap assertion

**Interfaces:**

- Consumes: existing overlay-only `Action::SelectNextAgent` and `Action::SelectPreviousAgent` reducer paths plus the existing `Tab`/arrow/`j`/`k` key mapping.
- Produces: cyclic selection over sorted live agents while the Agent Planets overlay is interactive.
- Preserves: no-selection starts (next→first, previous→last), stale/unavailable inertness, direct click selection, and normal-layout navigation.

- [ ] **Step 1: Change focused reducer tests to require wraparound.**

```rust
#[test]
fn agent_selection_next_wraps_last_to_first() {
    let mut app = app_with_agents(named_agents(3));
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectAgent(agent_id("ws", "gamma")));
    app.apply(Action::SelectNextAgent);
    assert_eq!(selected_name(&app).as_deref(), Some("alpha"));
}

#[test]
fn agent_selection_previous_wraps_first_to_last() {
    let mut app = app_with_agents(named_agents(3));
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectAgent(agent_id("ws", "alpha")));
    app.apply(Action::SelectPreviousAgent);
    assert_eq!(selected_name(&app).as_deref(), Some("gamma"));
}
```

Keep/assert tests that stale and unavailable selection stays inert. If the CLI suite does not already establish key routing, add a canvas-only test for `Tab`, Down, `Shift+Tab`, and Up reaching the corresponding cyclic actions.

- [ ] **Step 2: Run reducer/key tests and verify failure.**

Run:

```bash
cargo test app::tests::agent_selection
cargo test cli::tests::collage
```

Expected: the end-boundary assertions fail because selection currently clamps.

- [ ] **Step 3: Implement overlay-local cyclic reducers.**

In `select_next_agent`, after the existing `agent_selection_interactive` guard and empty-list guard, choose `(current + 1) % len` or first on no selection. In `select_previous_agent`, choose `len - 1` from first or no selection, otherwise `current - 1`. Do not alter `handle_key` or navigation outside Agent Planets; existing canvas mapping supplies `Tab`/Down as next and `Shift+Tab`/Up as previous.

- [ ] **Step 4: Run verification and commit the behavior repair.**

Run:

```bash
cargo fmt --check
cargo test app::tests::agent_selection
cargo test cli::tests::collage
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0 and stale/unavailable tests remain unchanged.

```bash
but commit agent-pulse-ringed-planets -m "feat: cycle Agent Planets selection"
```

### Task 2: Add normal footer hint and suppress z inside Agent Planets

**Files:**

- Modify: `src/ui.rs:996-1040, 2403-2618`
- Modify: `src/cli.rs:880-1028, 1122-1148, 1950-2597`

**Interfaces:**

- Consumes: existing Agent Pulse eligibility/summary state, `KeyOutcome::ToggleSignalView`, and `App::is_agent_overlay_open()`.
- Produces: normal-layout `a Agent Planets` footer hint only when integration is eligible; canvas-local `z` consume behavior.
- Preserves: standalone/disabled byte-identical UI, `z` Signal View behavior outside canvas, and documented player shortcuts in canvas.

- [x] **Step 1: Add failing footer and key-routing tests.**

```rust
#[test]
fn eligible_normal_footer_advertises_a_agent_planets() {
    let app = connected_agent_app();
    let text = buffer_text(&render_buffer(&app, 120, 36));
    assert!(text.contains("a Agent Planets"));
}

#[test]
fn standalone_and_disabled_footer_do_not_advertise_agent_planets() {
    assert!(!buffer_text(&render_buffer(&base_app(), 120, 36)).contains("Agent Planets"));
    let disabled = hidden_agent_app();
    assert!(!buffer_text(&render_buffer(&disabled, 120, 36)).contains("Agent Planets"));
}

#[test]
fn z_is_consumed_in_agent_planets_but_toggles_signal_view_elsewhere() {
    let mut canvas = connected_collage_app();
    canvas.apply(Action::ToggleAgentOverlay);
    handle_key(key(KeyCode::Char('z')), &mut canvas, &fake_audio(), &mut debounce(), &mut persistence());
    assert!(!canvas.is_signal_view());

    let mut normal = connected_collage_app();
    handle_key(key(KeyCode::Char('z')), &mut normal, &fake_audio(), &mut debounce(), &mut persistence());
    assert!(normal.is_signal_view());
}
```

- [x] **Step 2: Run focused tests and verify failure.**

Run:

```bash
cargo test ui::tests::eligible_normal_footer
cargo test cli::tests::z_is_consumed_in_agent_planets
```

Expected: FAIL because normal footer has no `a` hint and `handle_collage_key` falls `ToggleSignalView` through to normal routing.

- [x] **Step 3: Render the conditional normal footer hint and consume z locally.**

In `render_footer`, build the hint vector dynamically. Append `("a", "Agent Planets")` only when the normal layout has an eligible visible Agent Pulse surface; do not reserve a placeholder span otherwise. Keep Compact suppression if the existing summary surface is absent unless the agreed normal-footer width can accommodate the hint.

In `handle_collage_key`, add `KeyOutcome::ToggleSignalView => Some(Flow::Continue)` to the consumed canvas-local arm. Do not add `ToggleSignalView` to any other canvas behavior; outside the overlay, `handle_key` continues applying `Action::ToggleSignalView` exactly as today.

- [x] **Step 4: Run controller/UI regression suite and commit.**

Run:

```bash
cargo fmt --check
cargo test ui::tests::
cargo test cli::tests::collage_
cargo test cli::tests::z_is_consumed_in_agent_planets
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

```bash
but commit agent-pulse-ringed-planets -m "fix: keep Single View out of Agent Planets"
```

### Task 3: Synchronize current docs and final release gate

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-19-agent-pulse-pocket-planets-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-pocket-planets.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-planets-stage.md` only to check completed steps after implementation

**Interfaces:**

- Consumes: completed Agent Planets Stage behavior and its approved spec.
- Produces: current user docs; Pocket Planets records preserved as historical presentation context; manual checklist remains unchecked.

- [x] **Step 1: Update current behavior copy.**

Document all of these facts:

```text
The user-facing canvas is Agent Planets and opens with a.
Eligible normal footers show a Agent Planets; standalone, disabled, and ineligible runs do not.
Agent Planets centers the current station/ICY title and the exact Single View `signal_view_volume_line` directly beneath it around the unchanged Lissajous scope.
Every named disc-mask planet has name and status Side Tags; z is ignored while Agent Planets is open and still opens Single View outside it.
```

State that disc masks replace rectangle shadows and calculated planet silhouettes. Preserve all privacy, no-cross, mono/stereo, stale, and low-power claims.

- [x] **Step 2: Mark Pocket Planets presentation historical.**

At the top of `2026-07-19-agent-pulse-pocket-planets-design.md`, add a dated note saying its scope/privacy/theme-surface contracts remain context but its shadowed layout, selected-only callout, and old planet geometry are superseded by Agent Planets Stage. Preserve the original body and keep all prior design history links valid.

- [x] **Step 3: Record manual checks and run final gate.**

In `docs/SPEC.md`, leave live checks unchecked: mono/stereo streams, six themes, disc/tag stage resize+dense readability, keyboard/mouse selection, `z` inside/outside Agent Planets, reconnect, low power, standalone, disabled launch, and detach/reattach. Then run:

```bash
but diff agent-pulse-ringed-planets
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: every command exits 0; no manual box is checked by automation.

- [ ] **Step 4: Commit final documentation.**

```bash
but commit agent-pulse-ringed-planets -m "docs: document Agent Planets stage"
```

## Plan self-review

- **Spec coverage:** Task 1 implements center stage, discrete discs, no shadows, permanent tags, stage volume/title, and privacy/capture behavior. Task 2 adds conditional normal footer discovery plus canvas-local `z` suppression without changing external Signal View behavior. Task 3 records the final presentation and manual verification truthfully.
- **Placeholder scan:** No unresolved placeholders, generic test instructions, or unspecified implementation details remain. Manual checks are explicitly named live work.
- **Type consistency:** `AgentStageLayout` partitions `render_canvas`; `DiscMask` feeds `PlanetGeometry` and hit cells; `PlanetTag` feeds tag layout/render; existing `KeyOutcome::ToggleSignalView` is consumed only by `handle_collage_key`; no new App state or Herdr boundary is introduced.
