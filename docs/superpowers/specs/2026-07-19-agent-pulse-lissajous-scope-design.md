# Agent Pulse Lissajous Scope redesign

**Status:** Approved in design review on 2026-07-19. This document supersedes
Kinetic Collage's **presentation** decisions in
[`2026-07-18-agent-pulse-kinetic-collage-design.md`](2026-07-18-agent-pulse-kinetic-collage-design.md).
The existing local-only, read-only, same-socket, and privacy contracts remain
current.

## Goal

Make the Agent Pulse canvas read as an oscilloscope, not as a scrolling
waveform. The full-screen field should show a real, music-driven dual
Lissajous/phase trace behind calm agent frames. An agent should communicate
state through a small animated-or-still core while retaining the currently
approved state-colored frame edge.

## Scope and boundaries

- Keep every existing Herdr boundary: current local control socket only,
  `agent.list` only, no pane output, no remote discovery, and no control.
- Keep the normal-layout quiet count, the full-screen `a` canvas, Signal View's
  `a` suppression, standalone invisibility, and existing player-key fallthrough.
- Keep stable private identities, deterministic dense layout, tile hit testing,
  stale/unavailable recovery, low-power behavior, and selected-name-only
  privacy.
- Replace the Kinetic Collage scrolling waveform/FFT trace and abstract
  album-art interiors. Do not add a dashboard, list, card, detail pane, or
  status history.

## Experience

### Dual Phase Scope

The canvas background contains two overlapping, low-contrast phase portraits:

1. a primary trace in the theme's main visualizer color; and
2. a secondary trace in the theme's complementary visualizer color.

A phase portrait plots paired samples on X/Y axes; it does **not** scroll a
sample amplitude across time. For stereo output, the primary pair uses the
played left/right samples. For mono output or an unavailable stereo pair, it
uses the same played sample window at two documented, different sample lags.
The secondary trace uses a different pairing/lag. This gives every supported
stream an oscilloscope-like Lissajous curve while keeping the source real
played audio.

Both traces are centered in the canvas and remain behind agent frames. A short
phosphor persistence trail may show recent real trace positions. No trace may
advance from wall-clock time: silence or absent audio is calm, dim, and still.
RMS may gently brighten the traces; it must not turn them back into a
scrolling waveform.

### Agent frames and spinner cores

Every agent retains its stable, deterministically laid-out frame rectangle,
selection ordering, state-colored edge, and bounded audio-driven displacement.
The frame's former abstract album-art interior becomes a small centered core:

- **Working** shows a compact spinner whose orientation advances only from
  newly received played-audio visualizer data. Its edge remains the strongest
  theme playing-color glow.
- **Idle** shows a quiet, stationary hollow core (`◌`-like).
- **Blocked** shows a stationary stop/error core (`×`-like) using
  `theme.error`; its edge remains the error glow.
- **Done** shows a dim stationary completion core until the source snapshot
  omits it.
- **Unknown** remains muted and stationary.

The core is status language, not agent identity. Agent identity remains private
and is represented only by the qualified internal ID used for stable layout.
The glyph set may use terminal-safe equivalents when width handling requires
it, but it must preserve the moving Working / still non-Working distinction.

### Selection

Selecting a live agent by keyboard or tile click brings its frame forward and
shows exactly `explicit Herdr name · status` near that frame. An unnamed agent
shows no label. Never render a pane ID, workspace ID, cwd, agent type, or
inferred fallback name.

Selection remains frozen while stale or unavailable. `a` and `Esc` continue to
close/open the canvas in those states.

### Recovery and low power

- **Stale:** freeze the final live dual trace, frame geometry, core orientation,
  and selection; dim the entire composition under the existing restrained
  reconnecting indication.
- **Unavailable:** hide all frames and traces behind calm unavailable copy.
- **Low power:** freeze both phase-trace positions/persistence, frame geometry,
  and Working spinner orientation. State edge/core colors may refresh from a
  new snapshot, but no geometry or spinner animation advances.

## Architecture

- `audio` remains the owner of played samples. Its visualizer/analyzer boundary
  supplies a small, normalized paired-sample representation sufficient to form
  two phase portraits; it must not expose playback implementation details to
  the UI.
- `app` continues to own the current visualizer frame, short real-frame
  history, agent snapshot, selection, connection status, and stale capture.
  It owns no renderer clock or animation state.
- `ui::agent_pulse` derives dual phase-trace cells, phosphor persistence,
  deterministic frames, audio-driven frame transforms, status cores, labels,
  and hit targets purely from app state, theme, geometry, and injected time.
  Injected elapsed time alone must not alter output.
- `cli` retains the existing full-screen entry, close, player shortcut, and
  selection-routing rules.

## Verification

Add or update focused pure tests for:

- phase traces using paired/lagged audio values and never drawing a scrolling
  time-domain waveform;
- two distinct deterministic phase traces, silence stillness, short real-frame
  persistence, and no elapsed-time-only motion;
- Working core advancement on new audio data, stationary non-Working cores,
  and low-power/stale freezes;
- deterministic frame layout and dense-agent visibility after interiors change;
- state edge colors, selected explicit-name-and-status rendering, unnamed
  no-label behavior, and privacy exclusions;
- live-only selection, recovery, unavailable hiding, and unchanged global
  player shortcuts.

Manual checks remain required for live mono and stereo streams, all six themes,
resize/dense multi-workspace agents, mouse and keyboard selection,
reconnection, low power, standalone launch, and disabled launch.
