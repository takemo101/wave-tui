# Herdr Agent Pulse Design

**Status:** Approved for implementation

## Goal

Allow `wave-tui`, when launched as its official Herdr plugin, to quietly show the
live state of AI coding agents in the current Herdr workspace. The feature is a
read-only companion to radio playback: it must never affect audio, search,
settings, or normal standalone use.

This is an optional Herdr integration, not a plugin system inside `wave-tui`.
It stays within the product's work-session BGM purpose by presenting agent
activity as ambient context rather than a work-management dashboard.

## Scope

### Included

- An official Herdr plugin manifest in the `wave-tui` repository.
- A dedicated `wave-tui` radio tab opened from Herdr.
- Read-only current-workspace agent monitoring through the Herdr socket API.
- A compact Agent Pulse summary in Wide and Medium layouts.
- An Agent Pulse overlay opened with `a`.
- A visual Status Constellation, short active-agent list, selected-agent
  information card, and bounded completed history.
- Graceful no-Herdr, no-agent, socket-failure, and reconnect states.
- `--no-agent-pulse` to disable the integration for one launch.

### Excluded

- Reading pane output, prompts, files, or terminal scrollback.
- Focusing, creating, closing, sending text to, or otherwise controlling Herdr
  panes.
- OS notifications, notification sounds, or changes to volume, theme, station,
  playback, or visualizer selection.
- Cross-workspace monitoring.
- Persistent agent history or analytics.
- Agent visibility when a current workspace cannot be identified reliably.

## Plugin packaging and launch

`wave-tui` ships a `herdr-plugin.toml` in the same repository. It targets
macOS and Linux, builds the release binary through Cargo during plugin install,
and exposes an action that opens a dedicated radio tab. A dedicated tab is the
default because it preserves enough terminal area for the existing Wide or
Medium layout and does not constrain the player to Compact mode.

The plugin must invoke the built `wave-tui` binary as a normal Herdr pane. The
pane owns the audio process: closing the tab exits `wave-tui` and stops its
playback. Detaching and reattaching the Herdr session leaves the tab's process
under Herdr's normal lifecycle.

The manifest sets `min_herdr_version = "0.7.0"`, the documented version that
supports plugin manifests, plugin runtime context variables, and `agent.list`.

## Eligibility and startup

Agent Pulse is enabled only when all of these conditions are true:

1. `--no-agent-pulse` is not present.
2. The process was started by the official Herdr plugin.
3. `HERDR_ENV`, `HERDR_SOCKET_PATH`, and `HERDR_WORKSPACE_ID` are available.
4. The platform supports the Herdr socket transport.

The plugin environment is the authority for the current workspace. A regular
shell can run `wave-tui` under Herdr without a trustworthy workspace ID; in
that case Agent Pulse stays unavailable rather than guessing or showing agents
from another workspace. Standalone `wave-tui` retains its exact pre-integration
appearance and behavior.

## Herdr boundary and data flow

Add a focused Herdr adapter module (for example `src/herdr.rs`). It owns:

- environment eligibility parsing;
- socket connection and request/response JSON;
- a five-second `agent.list` polling loop;
- response normalization and current-workspace filtering;
- communication to the app event loop.

The adapter returns only an internal typed snapshot. It must not expose raw JSON
or socket concerns to `app` or `ui`.

Each live agent has:

- stable pane ID;
- agent type/name when supplied by Herdr;
- `working`, `blocked`, `done`, `idle`, or `unknown` status;
- a shortened working-directory label when supplied;
- a locally derived timestamp for when its current state was first observed.

The displayed duration (for example, `~12m`) is an estimate since
`wave-tui` first observed that agent in its current status, not an assertion
about the agent's true process start time.

`App` receives typed update actions over the existing event-loop boundary. Audio
threads, search workers, settings persistence, and UI rendering do not call the
Herdr socket directly.

## Agent lifecycle

- Every successful snapshot replaces the active current-workspace view.
- Agents sort by state: `working`, `blocked`, `idle`, `done`, then `unknown`.
  Equal states sort by stable display name.
- An agent moves to completed history when Herdr reports `done`, or when a
  previously live pane disappears from a later snapshot.
- Completed history is process-local only. It keeps the 20 newest entries and
  discards the oldest entry on overflow.
- The same pane must not create duplicate completed entries during unchanged
  polling snapshots.

## Connection states and recovery

| Condition | Normal Wide/Medium summary | Overlay | Other wave-tui behavior |
| --- | --- | --- | --- |
| Connected, active agents | State counts, such as `● 2 working · ○ 1 idle` | Constellation and active list | Unchanged |
| Connected, no current-workspace agents | `agents · none active` | Calm empty state | Unchanged |
| First missed/failed request | Dim last known state with `stale · reconnecting` | Last known state marked stale | Unchanged |
| No response for 15 seconds | Summary disappears | Overlay reports unavailable if opened | Unchanged; retry continues |
| Reconnected | Fresh state replaces stale state | Fresh state | Unchanged |
| Not eligible / standalone / explicit disable | No Agent Pulse UI | `a` does nothing | Exact current behavior |

Socket errors, malformed replies, and timeouts are recoverable adapter failures.
They must never panic the TUI or terminate playback. The reconnect loop retains
no durable data and performs no backoff that blocks normal terminal input.

## UI contract

### Normal layouts

- **Wide and Medium:** Now Playing contains a small Agent Pulse summary only
  while connected or briefly stale. It reports state counts, not individual
  output or prompts.
- **Compact:** The normal summary is hidden to preserve the existing Split Mini
  station and playback context. If the integration is active, `a` may still
  open the overlay.
- **Standalone / ineligible:** No reserved empty slot, hint, or "Not in Herdr"
  messaging appears. The Player area naturally uses its regular space.
- State changes receive one restrained visual acknowledgement only; they do not
  create toasts or audible cues.

### Agent Pulse overlay

`a` opens and `a` or `Esc` closes a temporary overlay. It is not Signal View
and does not alter search focus, station selection, or playback.

The overlay contains:

1. **Status Constellation.** Each active agent is a state-colored node. Working
   nodes have a slow, quiet pulse; blocked nodes are static; done nodes dim.
   Low-power mode renders all nodes statically. This animation does not touch
   audio or the audio visualizer.
2. **Short active list.** A readable companion list follows the selected sort
   order and shows name, agent type, short cwd label, state, and estimated
   state duration.
3. **Information card.** Selecting a node shows the same metadata. It never
   fetches pane output or offers pane actions.
4. **Completed disclosure.** `Completed (n)` expands to the bounded local
   history.

Keyboard navigation uses `Tab` or arrows to select nodes/list entries and
`Enter` to keep the information card visible. Mouse click selects a node or
list entry. All interactions are read-only.

## CLI and key handling

- Add `--no-agent-pulse`; it suppresses monitoring and all Agent Pulse UI for
  that run without modifying saved settings.
- Add `a` as the normal-layout Agent Pulse toggle only when the integration is
  active. It remains a no-op in standalone/ineligible launches.
- Signal View preserves its existing restricted key contract and does not show
  or open Agent Pulse.
- Existing search editing, focus movement, playback, theme, volume, favorites,
  and visualizer shortcuts retain their documented behavior.

## Module boundaries

- `herdr`: volatile integration adapter; environment, socket, protocol
  normalization, monitor lifecycle.
- `model`: stable typed agent snapshot/status primitives only if they are useful
  outside the adapter.
- `app`: Agent Pulse reducer state, lifecycle/history policy, selected item,
  overlay visibility, and derived display data.
- `ui`: rendering and mouse hit targets only. It dispatches actions rather than
  mutating Agent Pulse state directly.
- `cli`: parses `--no-agent-pulse`, owns monitor construction at the application
  boundary, and routes monitor events into `App`.

Dependencies remain acyclic: `herdr` may depend on typed model data; `app` may
consume its events; `ui` consumes app state but never calls `herdr`.

## Test plan

Tests must not require a live Herdr process, a Unix socket, audio hardware,
network access, or a real terminal.

- Parse eligible/ineligible plugin environments, including explicit CLI disable.
- Normalize valid and malformed `agent.list` payloads.
- Filter only the injected current workspace.
- Verify sort ordering, local observed-state durations, state changes, and
  de-duplication of completed entries.
- Verify 20-entry completed-history eviction.
- Verify stale after failed updates, unavailable after 15 seconds, and recovery
  after a fresh snapshot.
- Verify normal-layout summary visibility in Wide/Medium, Compact suppression,
  standalone suppression, overlay empty/stale states, and low-power static
  rendering.
- Verify `a`, `Esc`, keyboard selection, and mouse selection without changing
  station selection, playback state, or search focus.
- Verify existing app, audio, CLI, and UI tests remain green.

Manual verification on macOS/Linux covers plugin installation, dedicated
radio-tab launch, live status changes for multiple agents, pane disappearance,
Herdr detach/reattach, temporary socket loss/recovery, compact resize behavior,
and an ordinary standalone launch.

## Acceptance criteria

1. `herdr plugin install`/link can build and open `wave-tui` in a dedicated
   tab.
2. Only agents in the plugin invocation's current workspace appear.
3. Wide/Medium show a quiet count summary; Compact and standalone do not.
4. `a` opens a read-only constellation/list overlay with keyboard and mouse
   selection.
5. No agent terminal output is read or displayed.
6. Completed entries are local to the running app, bounded to 20, and include
   both `done` and disappeared panes.
7. Socket loss is recoverable and never affects playback or search.
8. `--no-agent-pulse` disables the feature for one run.
9. The suite passes `cargo fmt --check`, `cargo test`, `cargo check`, and
   `cargo clippy --all-targets -- -D warnings`.
