# Agent Planets solar orbit design

**Status:** approved design; implemented 2026-07-19 per
[`2026-07-19-agent-planets-solar-orbit.md`](../plans/2026-07-19-agent-planets-solar-orbit.md)
(automated gates green; live manual checks in `docs/SPEC.md` remain
pending).

## Goal

Replace music-driven planet body motion with a quiet solar system: a small
static sun centered in the Agent Planets field, with Working agents slowly
orbiting it on invisible concentric circular paths. This creates motion that
complements rather than competes with the Lissajous background.

This supersedes the surface-status design's planet-motion rule only. Interior
status treatments, foreground-only selection brackets, Agent Details, privacy,
and body-only hit testing remain unchanged.

## Motion model

- Draw one static, theme-derived sun at the field center. It is decorative and
  never a hit target.
- Every planet owns a seed-derived circular orbit radius, initial angle, and
  angular velocity. Orbit guide lines never render.
- Working planets advance from a monotonic clock; their seed-derived velocities
  differ within a deliberately slow bounded range.
- Idle, Blocked, Done, and Unknown planets freeze at their last Working
  position. A Working→non-Working transition captures that angle; a later
  Working transition resumes from the captured angle.
- Planet bodies no longer use RMS/FFT scale or positional transforms. Lissajous
  scope retains its existing audio-driven behavior independently.
- Stale and low-power freeze the complete solar layout. Unavailable hides the
  sun and planets. Compact/dense layouts reduce orbit radii and planet masks
  without omitting agents; if space cannot preserve a sun/body gap, drop the
  smallest orbiting bodies before the centered sun.

## Boundaries

The clock exists only to advance Working orbit phases; no orbit position,
transition capture, timer data, or private Herdr metadata is persisted.
Selection brackets remain body-adjacent decoration and hit testing remains body
only. The normal view stays byte-identical. Planet status remains interior-only
and never adds external glow, ring, particle, shadow, or orbit line.

## Verification

Test deterministic seed radii/speeds/initial phases; static sun; no orbit-guide
cells; Working motion across elapsed time; frozen non-Working positions and
resume; no audio-driven body transform; stale/low-power/unavailable behavior;
dense fallback; sun/decorative non-hit testing; selection brackets; and interior
status/privacy regressions.
