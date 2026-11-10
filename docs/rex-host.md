# Rex Agent Planets host

## Run

Start the rebuilt player **inside a local Rex terminal**:

```bash
cargo run --bin wave-tui -- --agent-host rex --no-auto-play
# Installed binary:
wave-tui --agent-host rex --no-auto-play
```

Rex supplies `REX_SERVER` and `REX_SESSION`. Both must be nonblank, the
server must be a local `unix:///...` endpoint, and the Rex CLI must exist.
The adapter looks for executable `rex` in absolute directories on `PATH`
(ignoring empty and relative entries); on macOS it also checks the
bundled CLI in `/Applications/Rex.app` and `/Applications/Rex Beta.app`.
No symlink installation is required for those standard Mac installations.
Other/custom installations must put `rex` on `PATH`.

`auto` retains the existing Herdr → herdr-core priority and considers Rex
last. Use explicit `--agent-host rex` if the terminal inherited another
host's environment. Explicit hosts never fall back. `off` and
`--no-agent-pulse` disable all host resolution. Nothing is persisted.

Press `a` for Agent Planets, select with Tab/arrows, Enter for the table,
and `o`/`O` to focus the selected record's terminal. Rename is not offered:
a Rex block label is not an agent/status-record name.

## What appears

The adapter observes placed terminal blocks in **one Rex session on one
local server**, not other sessions or connected hosts. Each current OSC 7501
status record with a nonblank `effective_app` becomes one planet. Root and
child records are distinct; children may inherit an application from a
parent. The private identity is `(session, block, record)`.

- Agent: the reported/inherited `effective_app`.
- Name: the record's optional, program-reported `title`; not the terminal
  title or generic block label. It may label the planet and table row.
- Activity: blank. `msg`, prompts, screen output, process arguments, working
  directories, owner/PID, progress and raw ids never reach the app/UI.
- `working`, `blocked`, `done`, `idle` map directly. `error`, missing and
  unknown states use the existing Unknown presentation.

This is **declared program status, not automatic agent detection**. An agent
that never emits OSC 7501 does not appear. Non-AI programs that report an
application appear too. Rex skill installation teaches agents Rex commands;
it must not be assumed to provide continuous status reporting.

Every successful poll replaces membership. Clearing a record or losing its
application removes it. Rex can retain `done`/`error` after process exit, so
those records can remain visible; there is no inferred process-exit timer.
A failed poll retains the last view and uses the normal stale/unavailable
ladder. In particular, a block closing between enumeration and status lookup
can fail that entire poll; partial snapshots are deliberately not treated as
success, so unrelated planets do not disappear because their data was absent.
A removed or detached block's focus may fail recoverably.

## Transport and safety

One owned worker runs a fixed Lua observation script through `rex do`.
The script calls `session.list_blocks`, then the terminal `program_status`
method for placed terminal blocks. It projects only permitted fields into a
versioned JSON snapshot. It never reads a terminal screen or sends input.
An explicit focus request uses a separate fixed script calling
`session.focus_block`; the block id is JSON data, never interpolated code.
No shell is used to launch Rex.

Every command uses the resolved executable, explicit server/session,
`--autostart=false`, and Rex's `--timeout 3s`. An additional **three-second
total deadline** and **8 MiB stdout cap** bound the complete attempt across
all RPC calls. Stderr is discarded. Nonzero exit, missing methods, malformed
JSON, oversized output and timeout are recoverable failures with no raw
server error shown. Timed-out/cancelled children are killed and reaped.
No server/daemon is started and there are no event subscriptions.

Polling starts immediately, then waits five seconds after each completed
poll. Poll and focus commands are serialized on the worker; at most one
focus is outstanding, including one waiting for the current poll. Extra
focus presses are ignored rather than queued. Drop/stop cancels the active
command and joins the worker; radio/UI threads never wait for a CLI call.

## Verification and source evidence

Verified on Rex `0.0.1793981048-g01d6c4`, commit
`01d6c4fe68668db3521570d7fbc9b6b112b1e572`, API revision **11**:

- `session.list_blocks` and terminal `program_status` read successfully.
- A disposable session emitted five OSC 7501 records (working, blocked,
  done, idle, error); the Rust monitor observed all five with the expected
  mapping.
- Explicit Rust-monitor focus returned success and `session.view` confirmed
  that `focused_block_id` changed from another split to the selected block.
- An additional real-player smoke check in a disposable Rex session (isolated
  HOME/settings, no autoplay) opened the five-planet stage and details table,
  confirmed focus-only hints/no rename hint, and used the `o` key to move from
  the player to the fixture terminal.
- Only the disposable test sessions were removed afterward.
- Actual agent-specific status emission and visual quality of the TUI inside
  Rex have **not** been manually reviewed. The smoke check inspected terminal
  text, not a screenshot or responsive resizing. Existing UI regression tests cover
  the unchanged stage rendering; no real audio playback was exercised here.

Default tests need no Rex installation. The opt-in live test expects an
already provisioned disposable session with five records whose app is
`wave-tui-test`, one per state above:

```bash
WAVE_TUI_REX_LIVE_TEST=1 \
REX_SERVER=unix:///actual/rex.sock REX_SESSION=session:TEST_ID \
cargo test live_rex_observes_and_focuses_disposable_session -- --ignored
```

The test focuses a pane in that session, so do not point it at a working
session. It does not create or remove sessions itself.

Primary references (installed server schemas take precedence over website
versions):

- [Program Status Protocol / OSC 7501](https://www.superlogical.com/rex/docs/build/program-status)
- [Lua API: terminal methods](https://www.superlogical.com/rex/docs/reference/lua)
- [Server API](https://www.superlogical.com/rex/docs/automate/server-api)
- [Server API methods](https://www.superlogical.com/rex/docs/reference/api)
- [CLI targeting and environment](https://www.superlogical.com/rex/docs/automate/cli)

The installed `program_status` result has a `records` array. Each record
has `state`, optional `id` (absent at root), `app`, `effective_app`, `title`
and other fields that are deliberately excluded. The `effective_app` field
already applies ancestor inheritance. Rex encodes a Lua empty table as `{}`;
the versioned snapshot decoder accepts only an empty object or an array for
`entries`, not arbitrary nonempty objects.
