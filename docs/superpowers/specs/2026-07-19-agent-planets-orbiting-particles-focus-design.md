# Agent Planets orbiting particles and focus design

**Status:** approved design; implementation requires a revised plan.

## Supersession

This design supersedes the unimplemented drifting-particles status language in
`2026-07-19-agent-planets-drifting-particles-design.md`. It replaces the
previous attached status ring, Working arc, and Done satellite while preserving
the Agent Planets stage, disc-mask bodies, Banded Worlds surface identities,
Agent Details modal, and all privacy/input rules.

## Visual roles

Three separate treatments avoid overloading one visual element:

1. **Orbiting particles** are ambient decoration around every planet. A stable,
   evenly spaced set of particles moves around the disc and pulses in a regular
   cadence only when played audio frames arrive. Their count and spacing do not
   encode agent status.
2. **Atmosphere** is a thin, non-particle outer glow that communicates status.
   It never becomes the former complete status ring or a clickable target.
3. **Four corner brackets** are the only selected-agent focus treatment. They
   surround the selected planet's allocated disc area and do not change the
   particle field or atmosphere semantics.

Particles and atmosphere remain within the planet's existing allocated tile,
leave a visible gap around the disc body, and are omitted before the disc mask
at dense sizes. Only disc-mask body cells select a planet.

## Status language

| Status | Atmosphere | Motion |
| --- | --- | --- |
| Working | Bright accent atmosphere | One brighter atmosphere segment advances around the outer edge on each newly played audio frame. |
| Idle | Thin muted atmosphere | Static. |
| Blocked | Thin error-colored atmosphere | Static; no blinking, cross, or broken orbit. |
| Done | Dim muted atmosphere | Static. |
| Unknown | Nearly invisible neutral atmosphere | Static. |

The Working atmosphere motion is a small, deterministic accent segment, not a
full orbit ring. It uses the played phase frame plus private identity seed; it
never uses elapsed time. A paused/silent stream, stale connection, or
`--low-power` leaves it still. Low power freezes the first audible geometry
with the existing scope/planet capture policy.

## Particle motion

Each non-dense planet gets the same evenly spaced particle count around its
body. Identity fixes the initial phase; a played phase frame advances the group
as a whole by discrete terminal cells. Brightness follows a stable repeating
pattern: one lead particle is brightest and the remaining particles alternate
between normal and dim. The pattern advances only with played audio, so silence
has no timer-driven animation.

Particles use theme-derived spectrum colors and never take the error color,
encode status, become focus markers, or participate in hit testing. A one-cell
disc, or any tile that cannot keep the required body gap, renders no particles
or atmosphere rather than crowding the field.

## Selection, recovery, and boundaries

The selected planet draws four theme-selection-colored corner brackets outside
its disc, after the planet and atmosphere. Brackets are decorative, bounded to
the tile, and are not hit targets. They remain visible while the selected
planet is live; opening Agent Details preserves the selected visual state.

Stale freezes and dims particles, atmosphere, brackets, discs, and scope.
Unavailable hides the entire field and closes details as already specified.
No Herdr data, persistence, timer, location metadata, or new control is added.

## Verification

Cover deterministic equal particle spacing; frame-driven group motion and
regular brightness cadence; silence/stale/low-power freezes; all five
atmosphere status colors; Working-only atmosphere segment movement; four
selected brackets; dense suppression; body-only hit testing; no old
ring/arc/satellite glyphs; and unchanged Agent Details privacy/rendering.
Manual checks cover six themes, real mono/stereo playback, resize/dense fields,
selection, reconnect, low-power, and pause/silence.
