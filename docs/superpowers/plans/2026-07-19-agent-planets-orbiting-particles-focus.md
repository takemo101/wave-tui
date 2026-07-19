# Agent Planets Orbiting Particles and Focus Implementation Plan

> **Revision note (2026-07-19):** the approved atmosphere-only revision
> removed the orbiting-particle decoration from this plan's scope — no
> particles ship. Thin per-status atmospheres (Working traveling accent,
> Idle breathing muted, Blocked weak deterministic error pulse, Done dim
> afterglow, Unknown near-static) and the four selection focus brackets
> shipped instead, per the revised
> [`2026-07-19-agent-planets-orbiting-particles-focus-design.md`](../specs/2026-07-19-agent-planets-orbiting-particles-focus-design.md).
> Particle tasks below are preserved as history.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace status rings with decorative regular orbiting particles,
status atmospheres, and selected-planet corner brackets.

**Architecture:** `src/ui/agent_pulse.rs` remains the sole rendering/geometry
owner. Private geometry separates decorative particles, status atmosphere cells,
and focus bracket cells from body-only hit cells. Existing stale and low-power
frame selection drives all movement without new state.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, current `VizFrame` and private
identity seed helpers, pure UI tests, GitButler `but`.

## Global Constraints

- Supersede `2026-07-19-agent-planets-drifting-particles-design.md` with the
  approved `2026-07-19-agent-planets-orbiting-particles-focus-design.md`.
- Particles are equal-count/equal-spacing decoration, never status, focus, or
  hit targets; movement/brightness derives only from played audio frames.
- Atmosphere carries status; only Working moves a small accent segment.
- Four corner brackets are selection-only and decorative.
- Silence, pause, stale, and low-power freeze all visual geometry; unavailable
  hides the field. No timers, persistence, Herdr fields, or privacy changes.

---

### Task 1: Establish private orbit, atmosphere, and focus geometry

**Files:**

- Modify: `src/ui/agent_pulse.rs:90-800` and its geometry tests

**Interfaces:**

- Replace the partial `ParticleCell` implementation with private records for
  `OrbitParticle { cell, bright }`, `AtmosphereCell { cell, accent }`, and
  `FocusBracket { cell, glyph }`.
- `PlanetGeometry` exposes `body`, `particles`, `atmosphere`, `brackets`, and
  `hit_cells`; `hit_cells == body`.

- [ ] Write failing tests for equal particle spacing/count across all statuses,
  played-frame-only phase advancement, Working-only atmosphere accent movement,
  static Idle/Blocked/Done/Unknown atmosphere, four bounded brackets, and
  body-only hit targets.
- [ ] Run `cargo test ui::agent_pulse::tests` and confirm the old ring-based
  tests fail.
- [ ] Remove compatibility ring/arc/satellite fields, constants, helpers, and
  stale test aliases. Use one discrete offset cycle around each mask; select
  every Nth offset for equal spacing, rotate by `phase_signature(frame) + seed`,
  and reject body-adjacent/out-of-tile cells. Generate a thin atmosphere from a
  separate, non-particle offset cycle. For Working, mark one frame-selected
  atmosphere cell as accent; all other statuses keep static atmosphere cells.
- [ ] Generate four bracket corner cells only for selected render geometry,
  clipped to the tile and outside the atmosphere/body gap.
- [ ] Run `cargo fmt --check` and `cargo test ui::agent_pulse::tests`.
- [ ] Commit: `refactor: model Agent Planets orbit and focus`.

### Task 2: Render the three visual roles and remove old status language

**Files:**

- Modify: `src/ui/agent_pulse.rs:1230-1390` and render/hit-test tests

**Interfaces:**

- Particles render with theme spectrum color and a regular bright/normal/dim
  cadence; atmosphere uses status color; brackets use `theme.selection_style()`.

- [ ] Write failing buffer tests asserting no old ring/arc/satellite glyphs,
  particle cadence and all five atmosphere treatments, Working accent movement,
  four selected brackets, and particles/atmosphere/brackets never selecting.
- [ ] Replace the current partial particle loop with three ordered passes:
  atmosphere, disc surface, particles, then selected brackets. Preserve stale
  dimming and existing modal layering.
- [ ] Update stale, unavailable, dense, body-only interaction, and theme tests
  to assert the new contract rather than ring behavior.
- [ ] Run `cargo fmt --check`, `cargo test`, `cargo check`, and
  `cargo clippy --all-targets -- -D warnings`.
- [ ] Commit: `feat: render Agent Planets orbit and focus`.

### Task 3: Align current documentation and perform release validation

**Files:**

- Modify: `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`,
  `docs/superpowers/specs/2026-07-19-agent-planets-stage-design.md`, and this
  plan's checkboxes.

- [ ] Replace all current ring/arc/satellite and random-drift claims with the
  three-role model: equal orbiting particles, status atmosphere, and four
  selection brackets. Mark earlier presentation records historical.
- [ ] Add unchecked manual checks for audio-only orbit/cadence, Working aura
  movement, every status color, bracket selection, dense suppression, silence,
  stale reconnect, low power, and six themes.
- [ ] Run `cargo fmt --check`, `cargo test`, `cargo check`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
- [ ] Commit: `docs: describe Agent Planets orbit and focus`.
