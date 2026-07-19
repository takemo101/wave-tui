# Agent Pulse Pocket Planets refinement

**Status:** Approved for implementation on 2026-07-19; implemented on
2026-07-19 (rendering slice shipped, automated gates green; live manual
checks in `docs/SPEC.md` remain pending).
This document supersedes the Ringed Planets document's planet scale and surface
presentation in
[`2026-07-19-agent-pulse-ringed-planets-design.md`](2026-07-19-agent-pulse-ringed-planets-design.md).
The existing Dual Phase Scope, Ringed Planets state language, selected callout,
and all local-only/read-only/privacy/recovery/input contracts remain current.

> **Historical presentation note (2026-07-19):** this document's scope,
> privacy, and Banded Worlds theme-surface contracts remain current context,
> but its shadowed full-canvas layout, selected-only callout, and calculated
> (ellipse-derived, 9×5-capped) planet geometry are superseded by the Agent
> Planets Stage redesign in
> [`2026-07-19-agent-planets-stage-design.md`](2026-07-19-agent-planets-stage-design.md)
> (centered stage chrome, explicit round disc masks, no shadows). The stage
> design's own Side Tags were in turn superseded by the Agent details modal
> of
> [`2026-07-19-agent-planets-details-modal-design.md`](2026-07-19-agent-planets-details-modal-design.md),
> and the Ringed Planets state-ring language and selected callout named as
> current in the status line above are likewise historical now — replaced
> by the thin status atmospheres and focus brackets of
> [`2026-07-19-agent-planets-orbiting-particles-focus-design.md`](2026-07-19-agent-planets-orbiting-particles-focus-design.md)
> as revised (no orbiting particles). The original body below is preserved
> unchanged as design history.

## Goal

Make Agent Pulse planets feel like small, varied worlds scattered around the
already-approved Lissajous scope—not large grey objects that compete with it.

Keep the scope exactly as it is. Shrink planets to leave the phase curves
readable, then give every planet a stable theme-derived color pair and an
identity-derived surface family.

## Scope and boundaries

- Do not change audio/model/App/CLI code, phase-pair data, Dual Phase Scope
  traces, persistence, silence behavior, or timing.
- Keep the existing deterministic planet field, state ring language, planet-only
  hit testing, and selected `explicit name · status` callout.
- Keep every agent visible at dense counts. A body may fall to one cell, but no
  agent is grouped or omitted.
- Use only colors supplied by the active theme (`spectrum_color`, foreground,
  muted, selection, playing, and error); never add fixed RGB colors.
- Surface color and pattern are identity language only. State remains on the
  ring/satellite and must not be inferred from the body palette.
- Never render a cross or cross-like blocked treatment. The blocked ring stays
  an error-colored broken orbit.

## Experience

### Pocket scale

Normal planets have a body no wider than nine terminal cells and no taller than
five cells. Their ring may extend one cell beyond the body. This is an upper
bound, not a required size: available grid-cell space chooses a smaller
proportional body first.

At dense sizes, reduce body radius before removing decoration:

1. body plus ring plus surface pattern;
2. body plus ring;
3. body only;
4. one selectable body cell.

The Lissajous scope remains visible around and between planets. The renderer
must not reserve a large square/rectangle background behind a planet.

### Banded Worlds surfaces

Every planet derives one stable surface family and two stable palette positions
from its private identity. The body uses the selected theme spectrum colors;
crater/shading marks use the darker or muted member of that pair. Three
families are sufficient:

- **Banded gas world:** two or three short horizontal atmosphere bands;
- **Ice-cap world:** a lighter polar cap with a darker lower hemisphere;
- **Cratered rock world:** a mostly solid body with one or two compact crater
  marks.

The family, palette positions, and mark locations do not morph from music,
state, time, or selection. Audio may still apply the existing bounded movement
to the whole body and ring.

### State and selection

Retain the current Ringed Planets state language:

- Working has a bright audio-driven arc on its complete ring.
- Idle has a still muted ring.
- Blocked has a stable-gap error ring, never a cross glyph.
- Done has a dim satellite; Unknown remains muted without a satellite.

Selecting a named live planet keeps the existing top-layer callout:

```text
aria · working
```

It contains exactly the explicit Herdr name and normalized status. The callout
must remain visible on top of the smaller planet field, choose an in-bounds
non-colliding candidate where available, and never make its text a hit target.
An unnamed planet still has no label or fallback identifier.

### Recovery and low power

Stale and low-power preserve the existing capture semantics. Planet body/ring
positions and Working arc position freeze with the current captured geometry.
Fresh agent snapshots may update the per-status ring treatment (for example,
the Broken ring) while positions stay frozen. The smaller surface pattern stays
identity-stable in every state. Unavailable hides planets, callouts, and scope
behind current calm copy.

## Architecture

- `ui::agent_pulse` only: derive a `PlanetSurface` and `PlanetPalette` from the
  existing private seed, cap the body dimensions, and paint surface bands/caps/
  craters inside the existing clipped planet body cells.
- Reuse the existing `PlanetGeometry` for render and hit-test parity. Smaller
  body/ring cells must drive both; no new UI state or input route is needed.
- Keep the current collision-aware selected callout renderer. Add direct unit
  coverage for right/left/below/above and all-collide fallback candidates so
  its collision policy is independently tested rather than inferred through the
  render helper.

## Verification

Add or update focused pure tests for:

- normal body cap (≤9×5) and dense decoration reduction with one selectable
  body per agent;
- deterministic seed-derived Banded gas/Ice-cap/Cratered-rock family, palette
  positions, and surface cells across audio/status/time changes;
- distinct active-theme colors used for planet surfaces with no hard-coded RGB;
- unchanged scope cells, state ring behavior, stale/low-power geometry, and
  planet-only hit test parity after smaller geometry;
- selected callout text/draw order/privacy plus direct tests forcing every
  candidate-placement branch and all-collide fallback.

Manual checks remain required for all six themes, low and high agent counts,
resize, keyboard/mouse callout readability, live mono/stereo scope, reconnect,
low power, standalone launch, and disabled launch.
