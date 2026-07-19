# Agent Planets Drifting Particles Implementation Plan

> **Superseded (2026-07-19):** this plan was never executed. Its
> drifting-particles presentation is superseded by
> [`2026-07-19-agent-planets-orbiting-particles-focus.md`](2026-07-19-agent-planets-orbiting-particles-focus.md),
> whose approved revision shipped thin per-status atmospheres with no
> particles at all. The tasks below are historical record.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Agent Planets' attached status rings, Working arc, and Done
satellite with small, identity-stable particles that drift only when played
audio updates.

**Architecture:** Keep particle geometry private to `src/ui/agent_pulse.rs`.
`planet_geometry` will produce disc-only hit targets plus rendered particle
cells; its frame argument provides deterministic audio progress, including the
already-selected stale/low-power frame. `render_planet` will paint the existing
disc surface followed by particles, with no ring mask or timer state.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `VizFrame`, existing
identity seed and active theme palette helpers, pure UI tests, GitButler `but`.

## Global Constraints

- Remove status rings, Working arcs, and Done satellites; retain explicit 7×5,
  5×3, 3×3, and one-cell disc masks and Banded Worlds surface treatment.
- Particle positions are deterministic from the private agent identity and the
  played-audio frame; no wall-clock or random persistent state is allowed.
- Silence, stale state, and `--low-power` freeze particle geometry. Low power
  continues using the first audible captured visualizer frame.
- Particles remain inside the existing allocated tile, outside the disc body
  with a visible gap, and are never selection hit targets.
- Status treatment is: Working 4–6 particles with one bright accent; Idle 2–3
  muted; Blocked 2–3 sparse error-colored; Done one dim distant; Unknown 1–2
  very dim muted. Dense tiles reduce particles first; one-cell discs may have
  none.
- Preserve same-socket read-only privacy, all Agent Details behavior, no
  full-tile shadow, and byte-identical standalone/ineligible output.
- Run `cargo fmt --check`, `cargo test`, `cargo check`, and
  `cargo clippy --all-targets -- -D warnings` before each commit. Leave live
  terminal/audio checks unchecked.

---

### Task 1: Replace ring geometry with deterministic particle geometry

**Files:**

- Modify: `src/ui/agent_pulse.rs:99-117, 476-799, 1859-2009`

**Interfaces:**

- Consumes: `CollageTile { seed, rect, energy }`, `DiscGeometry`,
  `AgentStatus`, `VizFrame`, and the stage field `Rect`.
- Produces: `PlanetGeometry { base_body, mask, body, craters, particles,
  hit_cells }`, where `particles` is a private `Vec<ParticleCell>` and
  `hit_cells` contains `body` only.

- [ ] **Step 1: Replace ring-specific geometry tests with particle tests**

  Replace `stable_identity_produces_a_round_body_craters_and_ring`,
  `blocked_planet_uses_a_broken_error_orbit_without_cross_glyphs`,
  `working_arc_changes_with_audio_but_other_planet_states_do_not`, and
  `done_planet_keeps_a_dim_satellite_and_unknown_has_none` with focused tests:

  ```rust
  #[test]
  fn stable_identity_produces_a_round_body_and_gapped_particles() {
      let geometry = planet_geometry(&tile, area, AgentStatus::Working, &frame);
      assert!(!geometry.particles.is_empty());
      assert!(geometry.particles.iter().all(|particle| {
          !geometry.body.contains(&particle.cell)
              && particle.cell.0 >= tile.rect.x
              && particle.cell.0 < tile.rect.x + tile.rect.width
              && particle.cell.1 >= tile.rect.y
              && particle.cell.1 < tile.rect.y + tile.rect.height
      }));
      assert_eq!(geometry.hit_cells, geometry.body);
  }

  #[test]
  fn particles_advance_only_when_the_played_frame_changes() {
      assert_ne!(particles_for(working, frame_a), particles_for(working, frame_b));
      assert_eq!(particles_for(working, frame_a), particles_for(working, frame_a));
  }

  #[test]
  fn particle_status_counts_and_emphasis_match_the_visual_contract() {
      assert!((4..=6).contains(&particles_for(working, frame).len()));
      assert!((2..=3).contains(&particles_for(idle, frame).len()));
      assert_eq!(particles_for(done, frame).len(), 1);
      assert!((1..=2).contains(&particles_for(unknown, frame).len()));
  }
  ```

- [ ] **Step 2: Run the focused tests and confirm they fail**

  Run: `cargo test ui::agent_pulse::tests::stable_identity_produces_a_round_body_and_gapped_particles ui::agent_pulse::tests::particles_advance_only_when_the_played_frame_changes ui::agent_pulse::tests::particle_status_counts_and_emphasis_match_the_visual_contract`

  Expected: FAIL because `PlanetGeometry` has no `particles` field and the old
  ring-based implementation remains.

- [ ] **Step 3: Introduce private particle types and geometry**

  Remove `RING_GLYPH`, `WORKING_ARC_GLYPH`, `SATELLITE_GLYPH`, `ring_mask`,
  `ring_cells`, `broken_ring`, and `working_arc`. Add a private rendering
  record and deterministic construction helpers:

  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  struct ParticleCell {
      cell: (u16, u16),
      accent: bool,
  }

  fn particle_count(status: AgentStatus, mask: DiscMask) -> usize { /* match Global Constraints */ }

  fn particles_for(
      tile: &CollageTile,
      disc: &DiscGeometry,
      status: AgentStatus,
      frame: &VizFrame,
      area: Rect,
  ) -> Vec<ParticleCell> { /* seed + phase_signature(frame), reject body-adjacent/out-of-tile cells */ }
  ```

  Use a small fixed offset table with seed- and `phase_signature(frame)`-
  derived index/stride rather than trigonometry, timers, or random sampling.
  Reject an offset if it lies on or immediately adjacent to a body cell, lies
  outside `tile.rect`/`area`, or duplicates another particle. Build
  `hit_cells` from `body.iter().copied().collect()` only. For a one-cell disc,
  return no particles when no gapped offset remains.

- [ ] **Step 4: Run focused tests and the module test suite**

  Run: `cargo test ui::agent_pulse::tests`

  Expected: PASS. Old ring/arc/satellite tests are removed or replaced; dense
  reduction, private identity stability, and body-only hit geometry remain
  covered.

- [ ] **Step 5: Commit the geometry slice**

  Run: `but diff`

  Commit the `src/ui/agent_pulse.rs` geometry/test file ID:

  ```bash
  but commit agent-pulse-ringed-planets -m "refactor: model Agent Planets particles" --changes <agent-pulse-id>
  ```

### Task 2: Render particles and preserve frozen, body-only interaction

**Files:**

- Modify: `src/ui/agent_pulse.rs:947-1041, 1238-1389, 2254-2772`

**Interfaces:**

- Consumes: `PlanetGeometry.particles`, `AgentStatus`, `Theme`, tile audio
  energy, stale state, selection state, and existing stage field rendering.
- Produces: particle-only status rendering; unchanged `hit_test` calls that
  select only disc body cells.

- [ ] **Step 1: Write failing render and interaction tests**

  Add tests beside the existing phase/planet rendering tests:

  ```rust
  #[test]
  fn particles_replace_ring_arc_and_satellite_glyphs() {
      let text = buffer_text(&render_collage_for(&working_app, false, Instant::now()));
      assert!(text.contains(PARTICLE_GLYPH));
      assert!(!text.contains(RING_GLYPH));
  }

  #[test]
  fn stale_and_low_power_freeze_particle_cells() {
      assert_eq!(particle_cells(stale_at_frame_a), particle_cells(stale_at_frame_b));
      assert_eq!(particle_cells(low_power_at_frame_a), particle_cells(low_power_at_frame_b));
  }

  #[test]
  fn particle_cells_never_select_an_agent() {
      let particle = geometry.particles[0].cell;
      assert_eq!(hit_test(field, particle.0, particle.1, false, &app), None);
  }
  ```

  Keep the existing stale test's dim assertion and add a dense one-cell case
  that verifies no particle is rendered when it would touch the disc.

- [ ] **Step 2: Run the focused tests and confirm they fail**

  Run: `cargo test ui::agent_pulse::tests::particles_replace_ring_arc_and_satellite_glyphs ui::agent_pulse::tests::stale_and_low_power_freeze_particle_cells ui::agent_pulse::tests::particle_cells_never_select_an_agent`

  Expected: FAIL because `render_planet` still paints `geometry.ring`,
  `geometry.working_arc`, and `geometry.satellite`.

- [ ] **Step 3: Render status-colored particles after the disc**

  Replace the ring/arc/satellite loops in `render_planet` with one particle
  loop. Use a private `PARTICLE_GLYPH: &str = "·"`; resolve status color via
  existing `status_color`; make Blocked use `theme.error`; apply `DIM` for
  Idle/Done/Unknown and silent/stale state; apply `BOLD` only to the Working
  accent particle when energy is above `BRIGHT_ENERGY` and not low power.
  Preserve `theme.selection_style()` for every selected particle.

  Do not change `render_agent_planets_stage` frame-source precedence or
  `hit_test`: the stale/low-power captured `VizFrame` must feed particle
  geometry automatically, and hit testing must keep consuming
  `geometry.hit_cells`.

- [ ] **Step 4: Run focused tests and relevant regression tests**

  Run:

  ```bash
  cargo test ui::agent_pulse::tests::particles_replace_ring_arc_and_satellite_glyphs
  cargo test ui::agent_pulse::tests::stale_and_low_power_freeze_particle_cells
  cargo test ui::agent_pulse::tests::particle_cells_never_select_an_agent
  cargo test ui::agent_pulse::tests::planets_do_not_change_phase_scope_cells_or_elapsed_time_behavior
  cargo test ui::agent_pulse::tests::dense_planet_field_renders_one_selectable_body_per_agent
  ```

  Expected: PASS. The scope remains clock-free; particle cells are not clickable;
  stale and low-power geometry remains frozen.

- [ ] **Step 5: Commit the rendering slice**

  Run: `but diff`

  ```bash
  but commit agent-pulse-ringed-planets -m "feat: drift Agent Planets particles" --changes <agent-pulse-id>
  ```

### Task 3: Align durable documentation and run the release gate

**Files:**

- Modify: `README.md:What it shows, Connection loss and recovery, Privacy and read-only limits`
- Modify: `docs/SPEC.md:Herdr Agent Pulse visual contract and verification checklist`
- Modify: `docs/ui-design-decisions.md:Agent Pulse decision record`
- Modify: `docs/superpowers/specs/2026-07-19-agent-planets-stage-design.md:historical ring vocabulary`
- Modify: `docs/superpowers/plans/2026-07-19-agent-planets-drifting-particles.md:completion checkboxes`

**Interfaces:**

- Consumes: implemented particle behavior from Tasks 1–2 and the approved
  `2026-07-19-agent-planets-drifting-particles-design.md`.
- Produces: one current user-facing description of particle status language and
  manual validation checklist; older ring wording explicitly marked historical.

- [ ] **Step 1: Write documentation acceptance checks before editing prose**

  Add a short checklist to this plan's Task 3 notes and verify the final diff
  contains all of these truths: no current document claims a permanent status
  ring, Working arc, Done satellite, or selectable particle; it states
  audio-only drift, frozen silence/stale/low-power geometry, dense suppression,
  and body-only selection.

- [ ] **Step 2: Update the current documentation**

  In the user-facing README and product/UI docs, replace present-tense
  ring/arc/satellite language with the approved count/color treatment. Mark the
  ring vocabulary in the older stage design as superseded by
  `2026-07-19-agent-planets-drifting-particles-design.md`; preserve historical
  records rather than rewriting their completion history. Add manual checklist
  items for six themes, dense slots, live audio drift, silence/pause stillness,
  reconnect, and low-power freeze.

- [ ] **Step 3: Inspect the documentation diff**

  Run: `but diff`

  Expected: current docs agree with the drifting-particles design; no
  user-facing current claim describes attached rings, side tags, or an
  unbounded Activity field.

- [ ] **Step 4: Run the full automated and release gates**

  Run:

  ```bash
  cargo fmt --check
  cargo test
  cargo check
  cargo clippy --all-targets -- -D warnings
  cargo build --release
  ```

  Expected: every command exits 0. Do not mark manual checks as completed.

- [ ] **Step 5: Commit documentation and plan completion**

  Run: `but diff`

  ```bash
  but commit agent-pulse-ringed-planets -m "docs: describe drifting Agent Planets particles" --changes <documentation-ids>
  ```
