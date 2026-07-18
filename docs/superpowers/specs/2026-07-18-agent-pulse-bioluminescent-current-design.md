# Agent Pulse Bioluminescent Current redesign

**Status:** Approved in design review on 2026-07-18. This document supersedes
all Agent Pulse presentation decisions in
`2026-07-16-herdr-agent-pulse-design.md` and
`2026-07-18-agent-pulse-beat-orbit-design.md`. Their local-only and read-only
privacy boundaries remain in force.

## Goal

Make Agent Pulse a beautiful, organic ambient music visualizer. The screen is
primarily a continuous, bioluminescent audio current; agents are subtle lights
within that current, rather than the subject of a dashboard or orbit diagram.

## Scope and boundaries

- Agent Pulse aggregates every agent returned by the current local Herdr
  control socket, across its workspaces. It does not discover other Herdr
  sessions or open another socket.
- It calls only `agent.list`; it never reads pane output, prompts, files, or
  scrollback and never controls panes.
- Wide and Medium normal layouts show only `● n active`. Compact, Signal View,
  standalone, and disabled launches show no normal Agent Pulse surface.
- The previous modal/list/card/completed-history and Beat Orbit ring geometry
  are removed.

## Experience

### Entry and canvas

- `a` opens a single-view, full-screen **Bioluminescent Current** canvas.
  `a` and `Esc` return to the normal player.
- Signal View retains its existing contract and ignores `a`.
- Existing player shortcuts remain available on the canvas: playback, volume,
  theme, favorite, visualizer, and Signal View. Search and station navigation
  remain suppressed while the canvas is open.

### Audio current and agent lights

- The current is a continuous flow line derived from played-sample FFT bands.
  Per-band magnitude changes its height, thickness, and glow; it is not a
  timer-only animation.
- Every agent maps to one stable position along the flow. No agent is grouped
  or omitted at high counts; dense terminals shrink spacing/glyphs rather than
  hiding lights.
- Each agent light reacts to its assigned FFT band and current RMS: brightness,
  size, and a short trail along the local flow direction change continuously
  with music. Silence or a missing audio frame leaves the current and lights
  small, dim, and still.
- Trails are derived from existing recent visualizer frames, so rendering stays
  pure and needs no new mutable animation store.
- Theme colors communicate state: Working has the strongest active glow,
  Blocked uses the theme error color, Idle is muted, and Done fades in muted
  color until its source snapshot removes it.
- Selecting a light by Tab/arrows or mouse shows only its explicit Herdr
  `name` plus state. No label is shown when `name` is absent; pane ID,
  workspace, cwd, and agent type never render.

### Recovery and low-power mode

- Stale freezes the last current and light trails, dims them, and shows a
  restrained `reconnecting` indication.
- Unavailable hides agent lights and shows calm unavailable copy.
- In `--low-power`, flow positions, light positions, and trails are static;
  state color and minimal brightness updates remain.

## Architecture

- `herdr` retains the existing same-socket, cross-workspace normalization and
  private qualified agent identity.
- `app` retains live agent/selection/connection state plus its existing recent
  `VizFrame` history. It does not own renderer animation state.
- `ui::agent_pulse` replaces Beat Orbit layout with pure Current geometry from
  `App`, the current `VizFrame`, `viz_history`, theme, terminal geometry, and
  injected render time.
- `cli` retains current full-screen entry/exit and non-recursive key routing;
  it applies only particle selection actions for mouse clicks.

## Verification

Add focused tests for:

- FFT-derived continuous flow changes, RMS-driven light size/glow, and
  trail placement derived from recent frame history;
- silence and low-power static geometry; state colors and Done removal;
- all-agent density, stable identity-to-flow placement, selected-name-only
  privacy, stale freeze/dim, and unavailable hiding;
- full-screen entry/exit, global player shortcut preservation, selection, and
  particle-only mouse routing;
- normal summary, Compact/Signal View/standalone absence, and no cross-session
  access.

Manual checks cover a live stream's visual feel and cadence, theme legibility,
terminal resize, dense multi-workspace agents, mouse selection, reconnection,
low-power mode, standalone launch, and disabled launch.
