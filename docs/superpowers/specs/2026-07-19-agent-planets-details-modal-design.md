# Agent Planets details modal design

**Status:** Approved 2026-07-19; implementation pending.

## Purpose

Replace always-visible per-planet name/status Side Tags with a focused, read-only
Agent Details modal. The Agent Planets field stays quiet and readable at dense
counts while a selected agent can be inspected deliberately.

This design supersedes the Agent Planets Stage design's permanent Side Tag
presentation only. The same-socket, read-only Herdr boundary; Lissajous scope;
disc masks; status rings; footer discovery; title/volume layout; and selection
cycling remain unchanged.

> **Note (2026-07-19):** the status rings left unchanged above were later
> replaced by the thin status atmospheres and selection focus brackets of
> [`2026-07-19-agent-planets-orbiting-particles-focus-design.md`](2026-07-19-agent-planets-orbiting-particles-focus-design.md)
> as revised (no orbiting particles); those atmospheres were in turn
> replaced by the interior-only surface status of
> [`2026-07-19-agent-planets-surface-status-design.md`](2026-07-19-agent-planets-surface-status-design.md),
> and planet body motion by the solar orbits of
> [`2026-07-19-agent-planets-solar-orbit-design.md`](2026-07-19-agent-planets-solar-orbit-design.md).
> This modal design itself remains current.

## User flow

1. Open Agent Planets with `a`.
2. Select a live planet with click, `Tab`/Down/`j`, or
   `Shift+Tab`/Up/`k`.
3. Press `Enter` to open a centered Agent Details modal for that selection.
4. Press `Enter` or `Esc` to close only the modal. Press `a` to close the modal
   and Agent Planets together.

`Enter` with no selected planet is a no-op. While the modal is open, selection,
player, theme, favorite, visualizer, and Signal View controls are consumed; they
must not mutate the selected agent, playback, or other app state. Global quit
controls (`q` and Ctrl-C) retain their existing behavior.

## Modal content and privacy

The compact-record modal has a centered `Agent details` heading and a stable
key/value body. It renders only non-empty allowed fields, in this order:

1. `name` — the explicit Herdr `name`, when supplied;
2. `agent` — the Herdr `agent` runtime label (for example `pi` or `claude`);
3. `status` — the existing normalized status vocabulary;
4. `activity` — the Herdr `terminal_title`.

Activity is presentation text, not a structured task model. It wraps or
truncates to the modal body and is omitted when unavailable or blank. A missing
explicit name does not suppress the independently available `agent` row.

No pane ID, workspace ID, cwd, foreground cwd, terminal ID, tab ID, session
payload, raw status, or other `agent.list` field may render. Those values remain
private even when they are present in the response.

## Stage and responsive rendering

Planet bodies retain their existing theme-derived surface and normalized status
ring. Remove all Side Tag geometry, tag placement, tag collision reservations,
and tag rendering; tags are neither click targets nor a compact-mode fallback.
The selected planet continues to use its existing selected emphasis.

The modal is centered over the Agent Planets field, clears its own rectangle,
and uses only theme-derived colors. Its body takes priority over the underlying
scope at short heights: title/field content may clip behind the modal, but the
modal remains bounded to the stage and its long Activity value truncates rather
than overflowing. The existing stage header, Single View-equivalent volume line,
and footer remain visible when terminal space permits.

## Recovery

A modal opened while the connection is live remains open during Stale and shows
the last captured allowed details dimmed with a reconnecting indication. New
snapshots refresh its fields because it follows the selected agent identity. If
the connection becomes Unavailable, the modal closes and the existing calm
unavailable Agent Planets field is shown. Hidden/standalone/disabled modes never
open the stage or modal.

## Boundaries and state

`herdr` remains the sole owner of Herdr JSON. It parses only the four allowed
presentation fields above into typed agent snapshot/view data; `AgentId` remains
opaque. `app` owns one ephemeral modal-open state tied to the current selected
agent identity and closes it when selection disappears, the stage closes, Signal
View activates, or the connection becomes Unavailable. `ui::agent_pulse` renders
stage and modal only; it does not mutate state. `cli` maps Agent Planets-local
`Enter` to the modal action and consumes modal-local keys.

## Verification

Pure tests cover:

- parsing allowed `name`, `agent`, and `terminal_title` fields while rejecting
  every prohibited identity/location field;
- explicit-name/agent/activity blank normalization and field ordering;
- no persistent tags or tag hit-targets in every layout tier;
- modal open/close, no-selection no-op, selection disappearance, stage close,
  Signal View, Stale, and Unavailable transitions;
- `Enter`/`Esc`/`a` and consumed player/navigation keys while the modal is open;
- compact record rendering, Activity truncation, theme-only colors, and no
  prohibited value in the buffer.

Live manual checks remain separate: current-socket `pi`/`claude` label and
terminal-title rendering, modal controls, stale/unavailable transitions, all
themes, resize behavior, standalone/disabled launch, and detach/reattach.
