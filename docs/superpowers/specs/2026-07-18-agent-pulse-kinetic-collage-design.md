# Agent Pulse Kinetic Collage redesign

**Status:** Approved in design review on 2026-07-18. This document supersedes
all prior Agent Pulse presentation decisions, including the Quiet Companion,
Status Constellation, Beat Orbit, Bioluminescent Current, and Vector Trace
proposals. Local-only and read-only privacy boundaries remain unchanged.

## Goal

Make Agent Pulse a full-screen, album-art-inspired music visualizer. Music is
shown as a deep animated background composition; every agent is a small,
stable abstract album-art tile that feels part of that composition, never a
plain dot, marker, dashboard row, or card.

## Scope and boundaries

- Aggregate every agent returned by the current local Herdr control socket,
  across its workspaces. Do not discover other sessions or open another socket.
- Call only `agent.list`; never read pane output, prompts, files, or scrollback
  and never control panes.
- Wide and Medium normal layouts retain only `● n active`. Compact, Signal
  View, standalone, and disabled launches have no normal Agent Pulse surface.
- Previous Current/vector/waveform renderers are removed. Agent identity is
  represented only through a procedural tile, and a selected explicit name.

## Experience

### Entry and control

- `a` opens a single-view full-screen **Kinetic Collage** canvas. `a` and `Esc`
  return to the player.
- Signal View retains its existing contract and ignores `a`.
- Existing player shortcuts remain available on the canvas: playback, volume,
  theme, favorite, visualizer, and Signal View. Search and station navigation
  remain suppressed.

### Album-art tiles

- Every agent owns one stable, procedurally generated abstract album-art tile.
  Its identity-derived composition, palette arrangement, position, rotation,
  and base size remain recognizable across frames.
- All tiles stay visible at high agent counts. The collage contracts tile sizes
  and spacing rather than grouping or omitting agents.
- Actual RMS and assigned FFT-band energy move each tile with a small scale and
  offset, and add a one- or two-layer soft shadow trail. The art itself does
  not morph or swap in response to music.
- The active theme supplies every color. State is a restrained edge glow:
  Working strongest, Blocked `theme.error`, Idle muted, and Done muted/dim
  until the source snapshot omits it.
- Selecting a tile by Tab/arrows or mouse brings it forward and shows only its
  explicit Herdr `name` plus state. Tiles with no explicit name show no label;
  pane ID, workspace, cwd, and agent type never render.

### Music background

- Behind tiles, a low-contrast waveform/FFT trace and a deep theme-phosphor
  breathing vignette react to played audio. RMS controls the vignette spread
  and tile motion; FFT bands shape the trace. This is data-driven, not
  timer-only animation.
- Silence or missing audio leaves a calm, dim, static collage.
- In `--low-power`, background trace positions, tile positions, tile scale, and
  shadow trails are static; state edge glow and minimal brightness may update.

### Recovery

- Stale freezes the final live background and tile composition, then dims it
  with a restrained `reconnecting` indication.
- Unavailable hides tiles and reports calm unavailable copy.

## Architecture

- `herdr` keeps current same-socket cross-workspace normalization and private
  qualified identity.
- `app` keeps live agents, selection, connection state, current `VizFrame`,
  recent frame history, and stale visual capture. It owns no renderer animation
  state.
- `ui::agent_pulse` derives procedural tile art, stable collage layout,
  audio-driven transforms, background trace, shadow trails, and hit targets
  purely from App state, theme, geometry, and injected time.
- `cli` retains current full-screen entry/exit and non-recursive key routing,
  applying only tile-selection mouse actions.

## Verification

Add focused tests for:

- deterministic per-identity tile art/layout and no tile omission at dense
  counts;
- RMS/FFT-driven background, tile transform, and soft shadow trail growth;
- silence, low-power static composition, theme edge glow, Done removal, stale
  freeze/dim, and unavailable hiding;
- selected-name-only privacy, tile-only hit testing, full-screen controls, and
  unchanged global player shortcuts;
- normal summary and Compact/Signal View/standalone absence.

Manual checks cover a live stream's composition/cadence, all themes,
resize/dense multi-workspace agents, mouse selection, reconnection, low-power,
standalone, and disabled launch.
