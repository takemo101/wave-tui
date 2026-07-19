# Agent Planets Stage redesign

**Status:** Approved for implementation on 2026-07-19; implemented on
2026-07-19 (stage rendering and footer/`z` input slices shipped, automated
gates green; live manual checks in `docs/SPEC.md` remain pending).
This document supersedes Pocket Planets' canvas layout, planet mask, label, and
shadow presentation in
[`2026-07-19-agent-pulse-pocket-planets-design.md`](2026-07-19-agent-pulse-pocket-planets-design.md).
The Dual Phase Scope, same-socket/read-only/privacy boundaries, and stable
identity-based surface palette remain current. This document's original
status-ring/satellite vocabulary is superseded by the thin status
atmospheres and selection focus brackets of
[`2026-07-19-agent-planets-orbiting-particles-focus-design.md`](2026-07-19-agent-planets-orbiting-particles-focus-design.md)
(as revised — the approved revision removed that design's orbiting
particles), and its permanent Side Tags by the Agent details modal of
[`2026-07-19-agent-planets-details-modal-design.md`](2026-07-19-agent-planets-details-modal-design.md).
Those status atmospheres are in turn superseded by the interior-only
surface status of
[`2026-07-19-agent-planets-surface-status-design.md`](2026-07-19-agent-planets-surface-status-design.md),
and this document's audio-driven planet motion by the static central sun
and Working-only invisible orbits of
[`2026-07-19-agent-planets-solar-orbit-design.md`](2026-07-19-agent-planets-solar-orbit-design.md);
atmosphere/motion language below is historical record.

## Goal

Make `a` open a centered **Agent Planets** stage that feels as deliberate as
Single View while remaining distinct from it: current station context and
volume are always visible, the Lissajous scope stays center-stage, and a
selected agent opens a readable details modal on demand.

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
- Keep every agent visible at dense counts without persistent text tags. The
  selected details modal may render only non-empty explicit `name`, `agent`
  runtime label, normalized status, and `terminal_title` Activity fields.
  Never reveal pane IDs, workspace IDs, cwd, terminal/session IDs, raw status,
  or any other identity/location field.

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
craters paint only mask cells. Status decoration stays outside the disc
silhouette and never creates a vertical/horizontal cross or a large
box. All planet shadow drawing is removed.

Banded gas, Ice-cap, and Cratered-rock surfaces retain their stable
identity-derived, active-theme-only palettes. State was originally encoded
by a ring/satellite (Working arc, Idle ring, Blocked broken error arc
without a cross, Done satellite, Unknown muted); that vocabulary is now
historical, superseded by the thin per-status atmospheres of the
orbiting-particles-focus design as revised, which are themselves
superseded by the surface-status design's interior treatments.

### Selected Agent Details

Planets keep no permanent name/status tag. Click or cycle to a live planet, then
press `Enter` to open a centered compact record. It shows non-empty values in
stable `label: value` rows: explicit `name`, `agent` runtime label, normalized
`status`, then `terminal_title` as Activity. Activity is presentation text, not
a structured task model; it truncates inside the modal. The modal clears only
its bounded field rectangle and uses theme-derived colors.

`Enter` or `Esc` closes only details; `a` closes details and Agent Planets. While
details are open, click selection, keyboard selection, player, theme, favorite,
visualizer, search, and Signal View controls are consumed. Global `q`/Ctrl-C
still quit. `Enter` without a selected live planet is a no-op.

### Recovery and input

Stale freezes scope, disc mask positions, and status-atmosphere positions;
an open
modal keeps the selected last snapshot dimmed with `reconnecting` in its title.
Unavailable closes details and hides the stage field behind calm unavailable
copy. Low power preserves its current first-audible geometry capture; fresh
agent snapshots may update status treatment but not disc/atmosphere
positions.

`Tab`/Down and `Shift+Tab`/Up cycle live-planet selection, wrapping at both ends;
`j`/`k` follow those same next/previous cyclic actions. Clicks select their hit
planet directly only while details are closed. `z` is ignored only while Agent
Planets is open.

## Architecture

- `ui::agent_pulse` owns stage partitioning, fixed disc masks and
  atmosphere cycles, no-shadow
  planet rendering, and bounded details-modal rendering.
- `ui` owns normal-screen footer hints and renders `a Agent Planets` only when
  the existing Agent Pulse eligibility state is visible.
- `cli` consumes `z` in the active Agent Planets key path before normal Signal
  View routing. It does not alter `z` outside that path.
- `app` continues to own station/ICY title, volume, selection, connection, and
  visual captures; no new persistent or Herdr state is added.

## Verification

Add focused tests for:

- fixed 7×5/5×3/3×3 disc masks, no full-rectangle shadow cells, no cross-like
  silhouette, theme-only stable surface paint, and dense reduction;
- stage header/title fallback, volume bar/value, and normal footer hint only in
  eligible modes;
- no permanent labels; modal field ordering/truncation/privacy, modal keyboard
  and mouse consumption, and selection-only planet hit testing;
- `z` ignored in Agent Planets while unchanged Signal View routing works
  elsewhere; existing player/selection controls remain correct;
- scope unchanged, stale modal reconnect indication, low-power
  disc/atmosphere freezes, and unavailable closing/hiding.

Manual checks remain required for live mono/stereo scope, six themes, modal
resize readability, keyboard/mouse selection, `z` behavior in/out of Agent
Planets, reconnect, low power, standalone, disabled launch, and plugin
detach/reattach.
