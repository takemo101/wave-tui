# Agent Planets Stage redesign

**Status:** Approved for implementation on 2026-07-19; implemented on
2026-07-19 (stage rendering and footer/`z` input slices shipped, automated
gates green; live manual checks in `docs/SPEC.md` remain pending).
This document supersedes Pocket Planets' canvas layout, planet mask, label, and
shadow presentation in
[`2026-07-19-agent-pulse-pocket-planets-design.md`](2026-07-19-agent-pulse-pocket-planets-design.md).
The Dual Phase Scope, same-socket/read-only/privacy boundaries, stable
identity-based surface palette, and status-ring vocabulary remain current.

## Goal

Make `a` open a centered **Agent Planets** stage that feels as deliberate as
Single View while remaining distinct from it: current station context and
volume are always visible, the Lissajous scope stays center-stage, and every
named agent has a nearby readable name/state tag.

Planets must look unmistakably round in a terminal. Remove the rectangle-like
planet shadow and calculated ellipse/ring shapes that create a cross-like
silhouette.

## Scope and boundaries

- User-facing copy says **Agent Planets**. Keep technical `AgentPulse` state,
  Herdr protocol, and module names unchanged unless a compiler-driven rename is
  strictly required.
- On eligible normal app layouts, add one footer hint: `a Agent Planets`.
  Standalone, ineligible, and disabled launches remain byte-identical and show
  no hint.
- `a` opens/closes Agent Planets as today. While Agent Planets is open, `z` is
  consumed as a no-op; it must not enter Single View. Outside Agent Planets,
  `z` keeps its existing Single View behavior.
- Do not change audio/model/App/Herdr/persistence boundaries, `agent.list`,
  player shortcuts, or the Dual Phase Scope's data/timing/persistence.
- Do not draw planet shadow rectangles or any other full-tile shadow. Scope
  phosphor persistence remains allowed behind planets.
- Keep every agent visible and named agents tagged at dense counts; never
  reveal pane IDs, workspace IDs, cwd, agent type, raw status, or fallback
  names.

## Experience

### Center stage

Agent Planets uses the same centered hierarchy as Single View:

1. centered `Agent Planets · n active` heading, matching Single View's
   Title Case presentation;
2. centered current ICY title, otherwise station name, otherwise calm no-station
   copy;
3. the exact Single View volume line directly under that title;
4. Lissajous scope and the small planet field;
5. compact footer hints including selection and close behavior, but not `z` as
   a Single View action.

Agent Planets reuses the existing `signal_view_volume_line` display verbatim:
`volume n%` followed by the same accent `─` fill and muted `·` remainder. It
occupies the same lowest-priority title-metadata position directly beneath the
station/ICY title, not a stage-specific bottom gauge. The Now Playing title and
volume display use the existing app station/ICY/volume state and do not expose
agent-private data.

### Disc-mask planets

Every planet body uses one of three explicit terminal-safe disc masks—7×5,
5×3, or 3×3—rather than an equation-derived filled rectangle/ellipse.

```text
  ███       ███       █
 █████     █████     ███
███████     ███       █
 █████
  ███
```

The renderer chooses the largest mask fitting the allocated slot; dense fields
fall through 7×5 → 5×3 → 3×3 → one body cell. Surface bands, ice caps, and
craters paint only mask cells. The status orbit is a short discrete arc around
the disc silhouette; it never creates a vertical/horizontal cross or a large
box. All planet shadow drawing is removed.

Banded gas, Ice-cap, and Cratered-rock surfaces retain their stable
identity-derived, active-theme-only palettes. State remains encoded by the
ring/satellite: Working arc, Idle ring, Blocked broken error arc without a
cross, Done satellite, and Unknown muted.

### Per-planet Side Tags

Each named planet has a two-line side tag adjacent to its disc:

```text
aria
working
```

The first line is the explicit Herdr name; the second is the normalized state.
Tags are always part of the planet layout unit. The default position is right of
the disc; collision resolution tries left, below, then above. A selected
planet's tag becomes bright and draws last; it replaces the old separate
selected-only callout, avoiding duplicate information.

Tags never overlap another planet, another tag, the center heading/title, the
volume region, or the footer when a non-overlapping candidate exists. Long
names truncate safely to the tag width, while the state line remains visible.
At extreme density, shrink the disc before shrinking tag width; names may
truncate but named-agent tags are never silently removed. Unnamed planets have
no tag.

### Recovery and input

Stale freezes scope, disc mask positions, status-arc positions, and side-tag
placements, then dims them under the reconnecting indication. Unavailable
hides the stage field and tags behind calm unavailable copy. Low power preserves
its current first-audible geometry capture; fresh agent snapshots may update
status treatment but not disc/tag positions.

`Tab`/Down and `Shift+Tab`/Up cycle live-planet selection, wrapping at both ends;
`j`/`k` follow those same next/previous cyclic actions. Clicks select their
hit planet directly. `Space`, volume, theme, favorite, and visualizer controls
keep their documented player behavior. `z` is ignored only while Agent Planets
is open.

## Architecture

- `ui::agent_pulse` owns stage partitioning, fixed disc/ring masks, tag layout,
  no-shadow planet rendering, and planet/tag collision checks.
- `ui` owns normal-screen footer hints and renders `a Agent Planets` only when
  the existing Agent Pulse eligibility state is visible.
- `cli` consumes `z` in the active Agent Planets key path before normal Signal
  View routing. It does not alter `z` outside that path.
- `app` continues to own station/ICY title, volume, selection, connection, and
  visual captures; no new persistent or Herdr state is added.

## Verification

Add focused tests for:

- fixed 7×5/5×3/3×3 disc masks, no full-rectangle shadow cells, no cross-like
  body/ring silhouette, theme-only stable surface paint, and dense reduction;
- stage header/title fallback, volume bar/value, and normal footer hint only in
  eligible modes;
- named two-line side tags, selected tag emphasis/draw order, safe truncation,
  collision candidate order against both discs and tags, unnamed no-tag, and
  privacy exclusions;
- `z` ignored in Agent Planets while unchanged Signal View routing works
  elsewhere; existing player/selection controls remain correct;
- scope unchanged, planet-only hit testing, stale/low-power tag freezes, and
  unavailable hiding.

Manual checks remain required for live mono/stereo scope, six themes, resize
and dense tag readability, keyboard/mouse selection, `z` behavior in/out of
Agent Planets, reconnect, low power, standalone, disabled launch, and plugin
detach/reattach.
