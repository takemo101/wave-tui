# Agent Pulse Beat Orbit redesign

> **Status (2026-07-19): Superseded.** The Beat Orbit presentation was first
> replaced by the Bioluminescent Current redesign
> (`2026-07-18-agent-pulse-bioluminescent-current-design.md`), which was in
> turn superseded by the Kinetic Collage redesign
> (`2026-07-18-agent-pulse-kinetic-collage-design.md`) — the current
> presentation decision. The privacy and local-only boundaries recorded here
> remain in force.

**Original status:** Approved in design review on 2026-07-18. This document
superseded Agent Pulse presentation decisions in
`2026-07-16-herdr-agent-pulse-design.md`; its privacy and local-only boundaries
remain in force.

## Goal

Turn Agent Pulse into a full-screen, music-reactive terminal visualizer rather
than an agent-detail modal. Every visible agent becomes a firefly-like particle
that moves with the music while still communicating its current state.

## Scope and boundaries

- Agent Pulse aggregates every agent returned by the current Herdr control
  socket, across its workspaces. It does not discover other Herdr sessions,
  open another socket, or access remote state.
- It remains read-only: only `agent.list` is called. It never reads pane output,
  prompts, files, or scrollback, and never controls panes.
- The normal player remains unchanged except for one small `● n active` count
  in Wide and Medium layouts. Compact retains no normal Pulse line.
- The previous short agent list, information card, completed disclosure, and
  normal-layout state-count details are removed.

## Experience

### Entry and canvas

- `a` opens a full-screen **Agent Pulse Beat Orbit** canvas; `a` and `Esc`
  return to the player.
- Signal View keeps its existing input contract and ignores `a`.
- Existing global player shortcuts, including volume, theme, playback,
  favorite, and visualizer controls, remain available while the canvas is open.
- Tab, arrows, or a mouse click select a particle. Only the selected particle
  displays a short label: explicit Herdr agent name when present; no label is
  displayed when it is absent. No pane-ID or agent-type fallback is shown.

### Particle field

- There is one particle per agent; none are hidden or grouped at high counts.
- Particles occupy stable, concentric rings. Their ring and angular slot derive
  from stable agent identity, so a status change does not make a particle jump.
  Dense terminals reduce particle glyph size and spacing, but retain every
  particle.
- All particles react to played-sample analysis, not a timer-only animation.
  Current RMS expands/contracts the rings and controls brightness; FFT energy
  adds small motion and trail intensity. Silent or missing audio leaves a quiet,
  dim, static field.
- The active theme supplies every color. State affects color and response:
  Working is strongest; Idle is muted; Blocked uses the theme error color; Done
  fades in muted color and disappears once the source snapshot removes it.
- Low-power mode fixes positions and trails, while retaining state colors and
  minimal brightness updates.

### Connection states

- Connected: live beat-reactive particles.
- Stale: freeze the last known orbit, dim it, and show a restrained
  `reconnecting` indication.
- Unavailable: hide agent particles and show a calm unavailable indication.
- Hidden/ineligible/explicitly disabled: no Agent Pulse UI and exact standalone
  behavior.

## Architecture

- `herdr` normalizes all agents from its current control socket and carries a
  stable, private cross-workspace identity for deduplication; it remains the
  only Herdr JSON/socket owner.
- `app` owns aggregate agent lifecycle, selection, and connection state. It
  removes the old completed-history/display-detail policy.
- `ui` derives a pure Beat Orbit layout from `App`, the active `VizFrame`,
  terminal geometry, and render time. It performs no mutation or adapter work.
- `cli` continues to own monitor construction and input routing only.

## Verification

Add focused tests for:

- cross-workspace aggregation and identity stability;
- all-agent packing, stable ring position, selected-name-only labels, and
  no-name label suppression;
- deterministic RMS/FFT-driven orbit changes, quiet silence, and reduced-motion
  low-power behavior;
- state colors, done fade/removal, stale freeze/dim, and unavailable hiding;
- full-screen entry/exit, selection, mouse routing, and preservation of global
  player shortcuts;
- no Pulse surface for standalone, disabled, Compact normal layout, or Signal
  View.

Manual verification covers real multi-workspace Herdr agents, music-reactive
motion under an actual stream, theme legibility, dense agent counts, terminal
resize, mouse selection, reconnection, low-power mode, and standalone launch.
