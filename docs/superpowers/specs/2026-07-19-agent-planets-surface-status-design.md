# Agent Planets surface status design

**Status:** approved design; implemented 2026-07-19 per
[`2026-07-19-agent-planets-surface-status.md`](../plans/2026-07-19-agent-planets-surface-status.md)
(automated gates green; live manual checks in `docs/SPEC.md` remain
pending). Planet body positions are governed by the later
[`2026-07-19-agent-planets-solar-orbit-design.md`](2026-07-19-agent-planets-solar-orbit-design.md),
which supersedes this design's planet-motion rule only — the interior
status treatments below remain current.

## Goal

Remove the planet atmosphere so the Agent Planets field stays visually quiet
beside the Lissajous scope. Preserve status visibility with small,
played-audio-driven changes inside each planet's existing disc mask.

This supersedes the atmosphere treatment in
`2026-07-19-agent-planets-orbiting-particles-focus-design.md`. The planet body,
Banded Worlds identity surfaces, selection brackets, Agent Details modal,
privacy, and body-only hit testing remain unchanged.

## Status language

Status never draws outside a disc mask. It uses only existing body cells and
active-theme colors:

| Status | Surface treatment |
| --- | --- |
| Working | A narrow bright identity-surface band advances through eligible body cells on each newly played audio frame. |
| Idle | The identity surface remains still and muted. |
| Blocked | One existing crater/surface cell weakly pulses in `theme.error`; no cross, blink, ring, or exterior glow. |
| Done | The complete body surface remains dim. |
| Unknown | The complete body surface remains muted and nearly still. |

The Working band and Blocked pulse are deterministic functions of the played
phase frame plus the private identity seed. They never use elapsed time. With
silence, pause, stale state, or `--low-power`, the body treatment freezes with
the existing visualizer frame capture policy.

## Selection and boundaries

The selected planet keeps four foreground-only corner brackets. They draw after
the body, use `theme.selection_bg` as a visible line color with no background,
and never encode status or enter hit testing.

Remove every atmosphere glyph, geometry helper, test, and documentation claim.
Do not add particles, shadows, external rings, new Herdr fields, timers,
persistence, or private metadata. Dense one-cell planets retain their body but
may omit an animated status cell when no safe detail exists.

## Verification

Test no atmosphere glyph/geometry remains; each status's interior treatment;
Working band movement and Blocked pulse only on changed played frames; frozen
silence/stale/low-power output; selection brackets; dense suppression;
body-only hit testing; and unchanged modal privacy.
