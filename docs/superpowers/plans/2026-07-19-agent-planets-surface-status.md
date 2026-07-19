# Agent Planets Surface Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace external status atmospheres with quiet, audio-driven status
changes inside Agent Planets disc masks.

**Architecture:** Keep private geometry and rendering in `src/ui/agent_pulse.rs`.
Status treatment selects existing safe body/surface cells from the played phase
frame and identity seed; body-only hit testing and bracket focus remain intact.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `VizFrame`, pure UI tests.

## Global Constraints

- Delete atmosphere glyphs/geometry/rendering/tests; never add external rings,
  particles, shadows, timers, persistence, or Herdr data.
- Working moves a narrow bright interior band; Blocked weakly pulses an interior
  error cell; Idle is still muted; Done dim; Unknown nearly still muted.
- Every movement is played-audio-frame driven and freezes for silence, stale,
  and low power.
- Brackets stay foreground-only (`theme.selection_bg` foreground, no background)
  and body-only hit testing remains unchanged.

---

### Task 1: Model interior status cells test-first

**Files:**

- Modify: `src/ui/agent_pulse.rs`

- [ ] Add failing geometry tests that assert no atmosphere cells, Working band
  movement, Blocked pulse, static Idle/Unknown, Done dim, and no detail for
  one-cell discs.
- [ ] Replace atmosphere geometry with private interior-status-cell geometry
  chosen from existing body/surface cells using `phase_signature(frame) + seed`.
- [ ] Run `cargo fmt --check` and focused `ui::agent_pulse` tests.
- [ ] Commit: `refactor: model Agent Planets surface status`.

### Task 2: Render and validate interior status language

**Files:**

- Modify: `src/ui/agent_pulse.rs`

- [ ] Add failing buffer tests for no atmosphere glyphs, theme error Blocked
  pulse, Working-only movement, freeze behavior, brackets, and body-only hits.
- [ ] Render status cells inside the body after identity surface paint; preserve
  bracket layering and stale/low-power dimming.
- [ ] Run `cargo fmt --check`, `cargo test`, `cargo check`, and
  `cargo clippy --all-targets -- -D warnings`.
- [ ] Commit: `feat: render Agent Planets surface status`.

### Task 3: Synchronize durable docs and release validation

**Files:**

- Modify: `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`, current
  Agent Planets design/plan records, and this plan.

- [ ] Replace current atmosphere claims with interior surface status language;
  mark the atmosphere record historical; keep manual checks unchecked.
- [ ] Run `cargo fmt --check`, `cargo test`, `cargo check`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
- [ ] Commit: `docs: describe Agent Planets surface status`.
