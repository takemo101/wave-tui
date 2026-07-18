# Agent Pulse Ringed Planets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the current Dual Phase Scope intact while replacing square agent frames with selectable, stateful Ringed Planets and guaranteed selected-agent callouts.

**Architecture:** Change only Agent Pulse's pure UI presentation and its current-design documentation. `CollageTile` continues to supply deterministic private-identity placement and real audio transforms; `ui::agent_pulse` derives circular body, crater, orbit, satellite, and callout cells from each tile. The already-shipped audio/model/App/CLI Lissajous pipeline remains untouched.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `VizFrame`/App visual capture, existing theme palette, pure UI tests, GitButler `but`.

## Global Constraints

- Preserve the current Dual Phase Scope exactly: paired real-audio data, two traces, Comet Trace behavior, persistence, silence threshold, audio timing, stale capture, and low-power audible-first capture.
- No new dependencies, persistence format, Herdr method/socket, playback command, control, keybinding, list, card, or detail rail.
- Keep same-socket `agent.list` visibility only; never display pane/workspace/cwd/agent type/raw status/fallback identifiers.
- Keep every agent visible at dense counts, with deterministic private-identity placement. Shrink radius, then optional crater/ring detail, before omitting any agent.
- A planet must render as a round body plus stable crater/shading and an orbit/ring when room permits; it must not show a square frame.
- Never render `×`, a diagonal cross, or a cross-like blocked-state symbol. Blocked uses an error-colored broken orbit with a stable gap.
- Working's bright orbit arc moves only from real new phase-frame data; Idle, Blocked, Done, and Unknown geometry stays still across audio frames and wall-clock time.
- A selected named live planet must show exactly `name · status` in an in-bounds callout drawn above all planets; unnamed planets have no callout.
- Planet body/ring cells are the only selection targets. Scope, history, shadow, callout, and empty cells never select agents.
- Run `cargo fmt --check`, focused tests, `cargo test`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo build --release`. Keep live manual checks explicitly unchecked.

---

## File structure and responsibility map

- `src/ui/agent_pulse.rs` — replaces frame/core paint with pure planet geometry, ring-state rendering, collision-aware callout placement, matching hit tests, and tests.
- `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`, `AGENTS.md` — record the final current Ringed Planets presentation and manual verification status.
- `docs/superpowers/specs/2026-07-19-agent-pulse-lissajous-scope-design.md` — additive historical note: its Lissajous/audio/recovery contracts remain current; only its agent-frame presentation is superseded.
- `docs/superpowers/plans/2026-07-19-agent-pulse-ringed-planets.md` — implementation record and completed checklist.

---

### Task 1: Render deterministic Ringed Planets without changing the scope

**Files:**

- Modify: `src/ui/agent_pulse.rs:115-860, 886-1881`

**Interfaces:**

- Consumes: existing `CollageTile { index, seed, base_rect, rect, energy, shadows }`, `AgentView`, `VizFrame`, `Theme`, `App::viz_history()`, and `AgentStatus`.
- Produces: private `PlanetGeometry { body, ring, crater, satellite, hit_cells }`, `planet_geometry`, `working_arc`, and `render_planet`.
- Preserves: `phase_layers`, `render_vignette`, `collage_layout` placement/audio transforms, stale/low-power source precedence, and global key routing.

- [ ] **Step 1: Replace frame/core expectations with failing planet-contract tests.**

In `src/ui/agent_pulse.rs` tests, replace tests that assert `▒` frame edges, `◌`/`×`/`·` core glyphs, and blank frame interiors. Add these focused tests and retain phase tests unchanged:

```rust
#[test]
fn stable_identity_produces_a_round_body_craters_and_ring() {
    let area = Rect::new(0, 0, 120, 36);
    let agent = view("work-a", "pane-1", AgentStatus::Working);
    let first = planet_geometry(&tile_for(&agent, area, phase_frame()), area, AgentStatus::Working, &phase_frame());
    let later = planet_geometry(&tile_for(&agent, area, phase_frame_with_offset(0.8)), area, AgentStatus::Working, &phase_frame_with_offset(0.8));
    assert_eq!(first.base_body, later.base_body);
    assert_eq!(first.craters, later.craters);
    assert!(!first.body.is_empty());
    assert!(first.ring.len() >= 2);
}

#[test]
fn blocked_planet_uses_a_broken_error_orbit_without_cross_glyphs() {
    let buf = render_planet_status(AgentStatus::Blocked, phase_frame());
    assert!(count_broken_ring_cells(&buf) > 0);
    assert!(!buffer_text(&buf).contains('×'));
    assert!(!buffer_text(&buf).contains('╳'));
    assert!(!buffer_text(&buf).contains('╲'));
}

#[test]
fn working_arc_changes_with_audio_but_other_planet_states_do_not() {
    let quiet = render_all_status_planets(phase_frame_with_offset(0.1));
    let loud = render_all_status_planets(phase_frame_with_offset(0.7));
    assert_ne!(working_arc_cells(&quiet), working_arc_cells(&loud));
    assert_eq!(idle_planet_cells(&quiet), idle_planet_cells(&loud));
    assert_eq!(blocked_planet_cells(&quiet), blocked_planet_cells(&loud));
    assert_eq!(done_planet_cells(&quiet), done_planet_cells(&loud));
}
```

- [ ] **Step 2: Run focused tests and verify they fail.**

Run:

```bash
cargo test ui::agent_pulse::tests::stable_identity_produces
cargo test ui::agent_pulse::tests::blocked_planet_uses
cargo test ui::agent_pulse::tests::working_arc_changes
```

Expected: FAIL because `PlanetGeometry`, planet rendering helpers, and ring-specific buffer assertions do not exist.

- [ ] **Step 3: Derive clipped planet geometry from existing tile bounds.**

Replace `render_tile`'s rectangle iteration with `planet_geometry`. Keep `base_rect` as the stable geometry source used by tests and dense layout; use `rect` for audio-transformed live geometry. A planet uses a terminal-aspect-aware ellipse centered in `tile.rect`:

```rust
struct PlanetGeometry {
    base_body: Vec<(u16, u16)>,
    body: Vec<(u16, u16)>,
    craters: Vec<(u16, u16)>,
    ring: Vec<(u16, u16)>,
    working_arc: Vec<(u16, u16)>,
    satellite: Option<(u16, u16)>,
    hit_cells: Vec<(u16, u16)>,
}

fn is_body_cell(dx: i32, dy: i32, radius_x: i32, radius_y: i32) -> bool {
    dx * dx * radius_y * radius_y + dy * dy * radius_x * radius_x
        <= radius_x * radius_x * radius_y * radius_y
}
```

Iterate the clipped `tile.rect` cells, use `is_body_cell` for the round body, and calculate ring candidates around the horizontal ellipse at `radius_x + 1`. Derive crater positions and ring inclination from `tile.seed`; only retain a crater/ring candidate when it is in `area`. For one- or two-cell dense bounds, emit the center body cell and no optional ring/crater cells. `hit_cells` contains body and ring cells only.

- [ ] **Step 4: Render planet state without a square or cross glyph.**

Implement `render_planet` with this draw order: soft shadow, body, crater/shading, dim ring, state-specific ring/satellite, selected highlight. Use only `Theme` colors/styles:

```rust
match view.status {
    AgentStatus::Working => draw_ring(buf, &geometry.ring, edge_style(...)),
    AgentStatus::Idle => draw_ring(buf, &geometry.ring, muted_ring_style),
    AgentStatus::Blocked => draw_ring(buf, &broken_ring(&geometry.ring, tile.seed), theme.error_style()),
    AgentStatus::Done => draw_dim_ring_and_satellite(buf, &geometry, theme),
    AgentStatus::Unknown => draw_dim_ring(buf, &geometry.ring, theme),
}
```

`broken_ring` removes one deterministic contiguous arc segment and never draws a core glyph. `working_arc` selects a short contiguous segment of the complete ring from `phase_signature(&viz.primary_phase)` plus `tile.seed`; it uses a bright playing style and is not timer-derived. Idle/Blocked/Done/Unknown do not consult `phase_signature` for geometry. Selected styling brightens existing body/ring cells without replacing them with a rectangle.

Keep `phase_layers` and its glyphs/functions byte-for-byte unless Rust formatting requires a nearby line change. Scope layers draw before planets, so planets remain in front while the scope remains visible around them.

- [ ] **Step 5: Adapt dense, state, freeze, and scope regression tests.**

Replace frame-cell helpers with `count_planet_cells`, `planet_geometry_cells`, and ring/body-specific helpers. Keep the existing 80-agent layout test but add a Buffer-level dense assertion:

```rust
#[test]
fn dense_planet_field_renders_one_selectable_body_per_agent() {
    let app = collage_app(many_snapshots(80));
    let buf = render_collage_for(&app, false, Instant::now());
    assert_eq!(count_distinct_planet_bodies(&buf), 80);
}

#[test]
fn planets_do_not_change_phase_scope_cells_or_elapsed_time_behavior() {
    let before = phase_cells_only(&render_collage(3, phase_frame(), vec![], false));
    let after = phase_cells_only(&render_collage_with_statuses(phase_frame()));
    assert_eq!(before, after);
}
```

Adapt stale/low-power assertions to compare planet/ring/working-arc geometry instead of core glyphs. Verify stale and low-power freeze phase, planets, ring arc, satellite, and selection presentation while fresh status colors can still refresh. Run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo check
```

Expected: every command exits 0.

- [ ] **Step 6: Commit the planet rendering slice.**

```bash
but commit agent-pulse-lissajous-scope -m "feat: render Agent Pulse planets"
```

### Task 2: Guarantee selected-agent callouts and planet-only selection

**Files:**

- Modify: `src/ui/agent_pulse.rs:498-561, 577-765, 770-781, 1551-1850`
- Modify: `src/cli.rs:2208-2556` only for updated planet mouse fixtures/tests

**Interfaces:**

- Consumes: `PlanetGeometry::hit_cells`, current selected `AgentView`, `collage_area`, and existing Connected-only Action routing.
- Produces: private `CalloutPlacement { rect, anchor }`, `selection_callout`, and planet-cell hit testing returning only existing `Action::SelectAgent(AgentId)`.
- Preserves: `a`/`Esc`, player shortcut fallthrough, Signal View suppression, station/search suppression, stale/unavailable selection gate, and no new input bindings.

- [ ] **Step 1: Add failing callout and hit-target tests.**

```rust
#[test]
fn selected_named_planet_has_a_visible_callout_drawn_above_other_planets() {
    let mut app = app_with_named_and_unnamed_agents();
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    let (buf, layout) = render_collage_with_layout(&app, false);
    let callout = selected_callout_rect(&app, &layout).unwrap();
    assert!(buffer_text(&buf).contains("research · working"));
    assert!(!callout_intersects_any_planet_body(callout, &layout));
    assert_eq!(cell_text(&buf, callout.x, callout.y), "r");
}

#[test]
fn unnamed_planet_has_no_callout_and_no_private_fallback() {
    let mut app = app_with_only_unnamed_agent();
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
    assert!(!text.contains("workspace-1"));
    assert!(!text.contains("pane-1"));
    assert!(!text.contains("working"));
}

#[test]
fn ring_or_body_click_selects_but_scope_callout_and_empty_cells_do_not() {
    let app = connected_collage_app();
    let layout = layout_for(&app, false);
    assert!(hit_at(first_ring_or_body_cell(&layout), &app).is_some());
    assert!(hit_at(scope_only_cell(&layout), &app).is_none());
    assert!(hit_at(callout_only_cell(&layout), &app).is_none());
}
```

- [ ] **Step 2: Run focused tests and verify they fail.**

Run:

```bash
cargo test ui::agent_pulse::tests::selected_named_planet
cargo test ui::agent_pulse::tests::unnamed_planet
cargo test ui::agent_pulse::tests::ring_or_body_clicks
```

Expected: FAIL because callout placement still uses `selection_label_rect` and hit testing still accepts every rectangle cell.

- [ ] **Step 3: Implement collision-aware, top-layer callout placement.**

Delete `selection_label_rect`. Build candidate one-row callout rectangles in this fixed priority: right of body, left, below, above. Clamp width to the canvas and reserve title/footer rows. Select the first in-bounds candidate that intersects no unselected planet body/ring bound; if all candidates collide, choose the clamped right candidate but still render it after every planet.

```rust
struct CalloutPlacement { rect: Rect, anchor: (u16, u16) }

fn selection_callout(
    selected: &PlanetGeometry,
    others: impl Iterator<Item = &PlanetGeometry>,
    area: Rect,
    width: u16,
) -> CalloutPlacement { /* candidate order above */ }
```

In `render_canvas`, render all nonselected planets, selected planet, then the selected named callout last with `set_stringn`. This draw order makes the label visible even if the bounded fallback overlaps a planet. The callout contains only `format!("{name} · {}", status_label(status))`.

- [ ] **Step 4: Restrict hit testing to planet geometry and maintain low-power parity.**

Replace `rect_contains(tile.rect, column, row)` with membership in `planet_geometry(...).hit_cells`. Do not add a hit target for the callout or scope/persistence/shadow cells. Reuse the exact same frame selection and `planet_geometry` arguments as `render_canvas`: stale is already rejected; low-power chooses `app.low_power_viz().unwrap_or(app.viz())`; live chooses `app.viz()`.

Update CLI mouse helpers to scan `hit_cells`, not rectangle cells. Add a low-power test that selects a drawn ring/body cell from the capture and proves a scope-only cell does nothing.

- [ ] **Step 5: Run controller/privacy/full verification and commit.**

Run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo test cli::tests::collage_
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: every command exits 0.

```bash
but commit agent-pulse-lissajous-scope -m "fix: show selected Agent Pulse details"
```

### Task 3: Replace the uncommitted Lissajous docs slice with final Ringed Planets docs

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-19-agent-pulse-lissajous-scope-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-ringed-planets.md` only to check completed steps and record manual checks after implementation

**Interfaces:**

- Consumes: the approved `2026-07-19-agent-pulse-ringed-planets-design.md` and final renderer behavior.
- Produces: truthful current docs; the earlier Lissajous spec remains the audio/scope historical record while its square-frame presentation is clearly superseded.

- [ ] **Step 1: Replace stale frame/core wording with exact final behavior.**

In each current user-facing document, state all of these facts:

```text
Agent Pulse keeps its full-screen real-audio Dual Phase Scope.
Agents are deterministic Ringed Planets: round bodies with stable craters and orbits.
Working has an audio-driven bright orbit arc; Idle is still; Blocked is an error-colored broken orbit; Done has a dim satellite.
A selected named planet shows only `name · status` in a visible callout; no cross glyph is used.
```

Retain accurate mono/stereo lag, no-scrolling-waveform, stale/unavailable, and first-audible low-power capture wording from the uncommitted Lissajous docs slice. Change only the now-obsolete square-frame/core claims. In `AGENTS.md`, make Ringed Planets the current presentation pointer before any Agent Pulse display change.

- [ ] **Step 2: Add an additive Lissajous presentation supersession note.**

At the top of `docs/superpowers/specs/2026-07-19-agent-pulse-lissajous-scope-design.md`, state that its current audio/phase/scope/recovery contracts remain in force but its square-frame, core glyph, and selected-label presentation is superseded by `2026-07-19-agent-pulse-ringed-planets-design.md`. Preserve its original body as history.

Do not alter the historical Kinetic Collage spec again unless a link needs correction.

- [ ] **Step 3: Keep manual checks honestly incomplete and run the final gate.**

In `docs/SPEC.md`, leave every manual item unchecked until a human performs it: live mono and stereo scope, six themes, resize/dense planet field, keyboard/mouse selection and visible callout, reconnection, low power, standalone, and disabled launch. Inspect the docs diff, then run:

```bash
but diff agent-pulse-lissajous-scope
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: every command exits 0; no manual item is checked by automated commands.

- [ ] **Step 4: Commit final documentation.**

```bash
but commit agent-pulse-lissajous-scope -m "docs: document Agent Pulse planets"
```

## Plan self-review

- **Spec coverage:** Task 1 preserves the Lissajous Scope while replacing only square agent presentation with deterministic planets and non-cross status rings. Task 2 guarantees selected details and exact planet-only hit targets. Task 3 converts the currently uncommitted Lissajous docs slice into accurate final current docs and preserves historical decisions.
- **Placeholder scan:** The plan contains no unresolved markers or deferred test bodies. Named manual checks remain intentionally unchecked because they require live Herdr/audio/terminal verification.
- **Type consistency:** `CollageTile` feeds `PlanetGeometry`; `PlanetGeometry::hit_cells` feeds both `render_planet` and `hit_test`; `CalloutPlacement` is created from the selected/other geometry and rendered last; no task changes the phase/audio/App capture interfaces.
