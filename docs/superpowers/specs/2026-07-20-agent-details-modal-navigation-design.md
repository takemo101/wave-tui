# Agent Details modal navigation design

**Status:** approved design; implementation requires a reviewed plan.

## Goal

Let users cycle Agent Planets while the Agent Details modal stays open. The
modal must immediately show the newly selected agent without exposing new
metadata or permitting unrelated controls.

## Interaction

While details are open, `Tab`/Down/`j` selects the next live planet and
`Shift+Tab`/Up/`k` selects the previous live planet, using the existing cyclic
order. The modal remains open and reads the newly selected agent's permitted
details. `Enter`/`Esc` still close details; `a` still closes details and the
stage. Mouse selection, player/theme/search/Signal View controls remain
modal-local and consumed. Stale/unavailable selection remains inert.

## Boundaries

The modal follows the App selection instead of preserving a separate hidden
selection. It renders only the existing allowed fields (`name`, `agent`,
normalized status, `terminal_title` Activity), remains read-only, and does not
change hit testing, persistence, or Herdr protocol behavior.

## Verification

Cover next/previous wrapping with details open, modal content updating across
agents, close behavior, modal-local non-navigation keys, stale/unavailable
inertness, and privacy regression tests.
