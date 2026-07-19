# Agent Pulse Ringed Planets redesign

**Status:** Approved for implementation on 2026-07-19; its rendering slices
shipped on 2026-07-19. **Historical note (2026-07-19):** this document's
local-only, read-only, privacy, selection, and recovery contracts — and its
ring/satellite state language and selected `name · status` callout — remain
current, while its larger grey planet scale and surface presentation is
superseded by
[`2026-07-19-agent-pulse-pocket-planets-design.md`](2026-07-19-agent-pulse-pocket-planets-design.md)
(9×5-capped bodies with theme-colored Banded Worlds surfaces). This document
supersedes only the Agent Pulse **agent-frame presentation** in
[`2026-07-19-agent-pulse-lissajous-scope-design.md`](2026-07-19-agent-pulse-lissajous-scope-design.md).
The Dual Phase Scope, its real-audio data path, and all existing local-only,
read-only, privacy, recovery, and input contracts remain current.

## Goal

Keep the current Lissajous curve exactly as it is. Replace the large empty
agent frames with a scattered field of readable terminal planets, and make a
live selected agent's explicit name and state impossible to miss.

A planet must feel like a planet—round body, stable crater/shading detail, and
an orbit/ring—not like a square card, glyph tile, or dashboard marker.

## Scope and boundaries

- Do not change the Dual Phase Scope's analyzer/model data, two real phase
  traces, Comet Trace behavior, persistence, silence rule, or audio timing.
- Keep current same-socket Herdr visibility, `agent.list`-only access, private
  qualified identities, normal-layout quiet count, full-screen `a` canvas,
  Signal View suppression, standalone invisibility, and player-key fallthrough.
- Keep one stable visible planet per agent at dense counts; shrink planet
  radius/ring detail before omitting any agent.
- Do not add a list, card, side rail, remote control, new keybinding, or extra
  Herdr data.
- Never use an `×`/cross glyph or cross-shaped status treatment. Blocked state
  is a broken error-colored orbit.

## Experience

### Planet field

Each active agent owns one deterministic planet placement derived from its
private `AgentId`. Existing staggered field placement remains the source of
stable positions, but the rectangle becomes a visual bounding box rather than
visible square frame.

A planet is rendered from terminal-safe cells:

- a roughly circular filled body using theme-derived foreground/background
  shades;
- one or two deterministic crater/shading marks derived from the private seed;
- a thin elliptical orbit/ring that extends beyond the body when space allows.

Its radius, crater arrangement, and ring inclination remain recognizable across
frames. Real RMS/assigned-band motion may shift the whole planet and ring by
the same bounded transform; the body never morphs into another planet. The
scope remains visible behind every planet.

At dense sizes, the renderer reduces radius and removes optional crater/ring
cells in that order. A one-cell minimum planet remains visible and selectable.

### State language without crosses

State keeps the existing theme color vocabulary but is expressed through the
planet's ring/satellite rather than a square edge or a cross:

- **Working:** the playing-colored orbit is complete; one bright short arc on
  it advances from new real audio phase data, never wall-clock time.
- **Idle:** a muted complete thin orbit remains still.
- **Blocked:** `theme.error` draws a clearly **broken** orbit with a stable gap;
  there is no `×`, diagonal cross, or cross-like core.
- **Done:** the planet and orbit are dim; a small dim satellite remains nearby
  until the source snapshot omits the agent.
- **Unknown:** body and orbit remain muted and still, without a satellite.

This state treatment is visible without selection. It must remain restrained
so it does not obscure the Lissajous curve.

### Selection callout

Selecting a live planet by keyboard or click brings it forward and draws a
small callout attached beside the planet:

```text
aria · working
```

The callout contains exactly the explicit Herdr `name`, a separator, and the
normalized state label. It is not optional, footer-only, or allowed to be
covered by another planet. The renderer chooses the nearest in-bounds side of
the selected planet, avoids the title/footer and other planet bodies where
possible, and gives the selected callout draw order above all planets.

An unnamed selected agent has no callout. Pane ID, workspace ID, cwd, agent
type, inferred name, and raw status payload never render.

### Recovery and low power

- **Stale:** freeze the captured final scope, planet body/ring geometry,
  Working arc position, and callout placement; dim it under the existing
  reconnecting indication.
- **Unavailable:** hide planets, rings, callouts, and scope behind calm
  unavailable copy.
- **Low power:** retain the existing first-audible scope/geometry capture. Ring
  arc position, planet geometry, and callout placement stay frozen after that
  capture; fresh agent snapshots may update state color only.

## Architecture

- `audio`, `model`, `app`, and `cli` retain the already implemented Lissajous
  sample-pair, capture, and input behavior. No new audio or Herdr boundary is
  needed.
- `ui::agent_pulse` replaces the square-frame/core renderer with deterministic
  planet body/ring/satellite geometry. It derives planet/callout geometry from
  `App` state, theme, captured/live visualizer frame, and private identity only.
- Shared rendering/hit-test geometry must use the planet's body-or-ring bounds;
  a click on scope cells, persistence, a callout, or empty space must not select
  an agent.

## Verification

Add focused pure tests for:

- stable private-identity planet radius/crater/ring placement, status-independent
  base geometry, and one selectable planet per agent at dense counts;
- real audio-driven Working ring-arc advancement with no elapsed-time-only
  change; Idle/Blocked/Done/Unknown rings remain stationary;
- Blocked broken error ring and explicit absence of cross glyphs;
- scope cells remaining behind planets and unaffected by planet state;
- selected named callout content, callout visibility/draw order/collision
  avoidance, and no callout for unnamed agents;
- planet-only hit targets, low-power parity, stale freeze/dim, unavailable
  hiding, and unchanged player/keyboard behavior.

Manual checks remain required for live mono/stereo streams, six themes,
resize/dense multi-workspace agents, keyboard/mouse selection and callout
readability, reconnection, low power, standalone launch, and disabled launch.
