# Agent Planets drifting particles design

**Status:** proposed, approved for specification; implementation requires a
separate reviewed plan.

## Goal

Replace the planet-attached status orbit, Working arc, and Done satellite with
a small field of particles that feels as though it drifts around each planet.
The field must stay quiet and terminal-legible, retain the project's
played-audio-only motion rule, and never reduce planet selection or privacy.

This design supersedes the Agent Planets Stage design's status ring/satellite
presentation. Disc-mask bodies, identity-derived Banded gas/Ice-cap/Cratered-
rock surfaces, layout tiers, and the Agent Details modal remain unchanged.

## Visual model

Each planet owns a deterministic particle field derived from its existing
identity seed. Every particle has an independent initial angle, radial band,
and drift rate. Particles stay outside the disc mask with a visible gap; they
are not arranged as a complete ring, regular polygon, or permanent orbit.

On a newly played visualizer frame, each particle advances a small,
identity-stable amount along its own slightly irregular path. The result is
slow, uncoordinated drift rather than a clock-driven animation or random
teleportation. No played frame means no movement: silence, pause, stale state,
and low-power mode retain their current frozen behavior. Low power freezes the
first audible particle geometry alongside the existing scope and planet
geometry.

Particles are rendered only within the planet's existing allocated slot. They
must not overlap another planet, title/volume chrome, footer, or the bounded
Agent Details modal. They are decorative and never enter the hit-test region:
only disc-mask body cells select a planet.

## Status language

Status is encoded by particle count, prominence, and color rather than by a
ring shape:

| Status | Particle treatment |
| --- | --- |
| Working | Four to six particles; one accent particle is brightest and visibly drifts with played audio. |
| Idle | Two to three muted particles; they retain positions until played audio advances them, but stay visually quiet. |
| Blocked | Two to three sparse particles in the error color; no cross, broken ring, or blinking treatment. |
| Done | One dim, distant muted particle until the next snapshot omits the agent. |
| Unknown | One or two very dim muted particles. |

At dense sizes, reduce particle count before reducing the disc mask. A one-cell
planet may use no particles if its slot cannot preserve the required gap.
Selected planets retain their existing emphasis by restyling their body and
particle field; no label or new marker is added.

## Boundaries

- Do not add time-based timers, persisted phase, new Herdr fields, or random
  state that makes a planet's identity unstable across a live session.
- Continue to use only played audio data for visual motion. Audio energy may
  bound the per-frame drift distance, but does not alter status semantics.
- Keep stale reconnection behavior: particle positions freeze and dim with the
  rest of the field. Unavailable hides the field; it closes details as today.
- Preserve all privacy rules. Particles encode no names, panes, workspaces,
  paths, terminal identifiers, or session metadata.

## Verification

Add focused pure rendering/geometry tests for deterministic particle placement,
minimum disc gap, status counts/colors, dense suppression, selection-only hit
testing, no ring/satellite glyphs, audio-frame movement, and frozen
silence/stale/low-power behavior. Manual checks cover six themes, resizes, live
mono/stereo playback, and visual readability at dense agent counts.
