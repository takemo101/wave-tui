# Agent Planets Solar Orbit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render a static central sun and slow, varied Working-only circular
planet orbits without visible guide lines or music-driven planet motion.

**Architecture:** Keep orbit state in private `app`/`ui::agent_pulse` data.
The reducer captures a planet angle when its status stops Working; rendering
uses elapsed monotonic time only for current Working phases. The UI keeps
body-only hit testing and decorative sun/brackets.

**Tech Stack:** Rust 2018, Ratatui, `Instant`, existing App reducer and UI tests.

## Global Constraints

- Sun static/centered/non-hit; no visible orbit tracks.
- Working only moves; other statuses freeze at their transition angle and resume
  from it when Working returns.
- Seed derives radius/start/speed. Remove RMS/FFT planet body movement.
- No persistence/private metadata; stale/low-power freeze; unavailable hides.

---

### Task 1: Model solar orbit state and time-freeze transitions

**Files:**

- Modify: `src/app.rs`, `src/ui/agent_pulse.rs`

- [ ] Add failing reducer tests for Working→Idle capture, resume, snapshot
  omission cleanup, and no persistent settings changes.
- [ ] Add private per-agent orbit phase capture plus a monotonic render epoch;
  seed-derived radius/initial angle/speed helpers.
- [ ] Remove audio scale/offset from `collage_layout`; calculate a centered sun
  and circular working/frozen positions.
- [ ] Run focused App/UI tests and commit `refactor: model Agent Planets solar orbits`.

### Task 2: Render sun, Working motion, and dense interactions

**Files:**

- Modify: `src/ui/agent_pulse.rs`

- [ ] Add failing buffer tests for sun/no guide lines, elapsed Working motion,
  frozen non-Working positions, no audio body movement, dense fallback, and
  sun non-hit testing.
- [ ] Render the static theme-derived sun before planets; render Working orbits
  from time and captured non-Working positions; preserve brackets/status body
  treatment and scope layering.
- [ ] Run fmt/test/check/clippy and commit `feat: render Agent Planets solar orbits`.

### Task 3: Synchronize docs and release validation

**Files:**

- Modify: `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`, current
  design/plan records.

- [ ] Replace atmosphere/audio-body-motion language with invisible solar orbits
  and Working-only timing; keep manual checks unchecked.
- [ ] Run fmt/test/check/clippy/release build and commit docs.
