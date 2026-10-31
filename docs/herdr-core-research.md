# herdr-core source evidence

Evidence note for the herdr-core Agent Planets host. It records what
the official herdr-core sources say; it is **not** the behavior contract.
The behavior is specified in
[`SPEC.md`](SPEC.md#herdr-core-agent-host),
and the adapter and host selection built on these facts are described there.
Where a statement below is a schema or source fact it says so; adapter
behavior wave-tui builds on those facts lives in `SPEC.md`.

## Pinned revision

All claims are pinned to herdr-core commit
[`4a3591aa835c197f57a5d1b8e8951a75e2a79faa`](https://github.com/herdrdev/herdr-core/commit/4a3591aa835c197f57a5d1b8e8951a75e2a79faa)
(2026-10-30), API schema version `0.1.15`, protocol generation 1. Re-verify
against the installed or released core version before relying on a newer
release.

Pinned sources:

- Schema: [`schema/released.openrpc.json`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/schema/released.openrpc.json)
  (`schema/hc.openrpc.json` carries the same version) and
  [`schema/README.md`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/schema/README.md).
- API dispatch: [`crates/herdr-core/src/api.rs`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/api.rs).
- Socket paths: [`crates/herdr-core/src/paths.rs`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/paths.rs).
- Server: [`crates/herdr-core/src/server.rs`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/server.rs).
- Terminals and status:
  [`terminals.rs`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/terminals.rs),
  [`status.rs`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/status.rs).
- Security model: [`docs/security.md`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/docs/security.md).
- Product page (prose only, not an API reference): <https://herdr.dev/core/>.

## Identity

herdr-core is the headless terminal runtime behind the `hc` CLI
([`herdrdev/herdr-core`](https://github.com/herdrdev/herdr-core)); the Herdr
app is one client of the same API. Its protocol is not the Herdr GUI socket
protocol: `agent.list`, `HERDR_ENV`, `HERDR_SOCKET_PATH`, and
`HERDR_WORKSPACE_ID` do not exist in core. `server.rs::serve` runs the
listener without a GUI dependency.

## Schema and source facts

**Transport.** JSON-RPC 2.0, one message per line on a Unix socket (framed mode
is negotiated by `hello`). The schema exposes no authentication parameters.
`docs/security.md` and `paths.rs` establish that protection is filesystem
access control on private runtime directories and the socket: any process that
can open the socket can use the whole API, so it is a trusted local boundary.

**Socket endpoint resolution** (`paths.rs::Paths::from_env`): a nonempty
`HC_SOCKET` wins; otherwise a configured socket path; otherwise a default
runtime path derived from `HC_SESSION` (default `default`) under the platform
runtime directory. `HC_SESSION` or the default path alone therefore only names
where a socket *would* be; it does not show that a core is running or that
this process belongs to one.

**Terminal context.** [`api/run.rs:202–203`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/api/run.rs#L202-L203)
injects `HC_TERMINAL_ID` and `HC_TERMINAL_SOCKET` (the core's actual endpoint)
into a new terminal. The adapter uses this injected endpoint after an
explicit `HC_SOCKET` override, but only in a terminal-id-qualified context.
On Unix, both macOS and Linux use `$HOME/.local/state` and `$HOME/.config`
as defaults. With no runtime directory, the socket is placed directly in the
session state directory, not an extra `herdr-core` child; see
[`platform/unix.rs:84–105`](https://github.com/herdrdev/herdr-core/blob/4a3591aa835c197f57a5d1b8e8951a75e2a79faa/crates/herdr-core/src/platform/unix.rs#L84-L105).

**`terminal.list`.** Takes optional `all` and `labels` parameters (and no
`on` parameter); wave-tui's call is the empty-params `terminal.list {}`.
Without `all`, finished terminals are excluded. Result is `ListResult.terminals`.
Per `api.rs`, enumeration covers this machine and then added or connected
remote machines, with remote addresses of the form `machine/t7`; each
machine's query is bounded to about three seconds, so a slow remote can make a
whole call take longer than a client's own attempt deadline. There is no agent-type
filter and no workspace field: core lists terminals, not a curated agent
inventory.

**`Terminal` fields used.**

- `address` — permanent terminal address on its core (`t7`, or `machine/t7`).
  Not workspace/pane identity.
- `name` — nullable, the explicit terminal name (set at `terminal.run`).
- `title` — the terminal title.
- `state` — lifecycle, `live` or `finished`; separate from agent status.
- `status` — nullable program-reported OSC 7501 record: `app`, `state`, and
  optional `title`, `msg`, `kind`, `progress`. `Status.state` is a plain string
  in the schema; documented values are `idle`, `working`, `done`, `blocked`,
  `error`. A program may clear its status (`status.rs` treats `clear`
  specially). `status.rs` applies no agent taxonomy.
- `reported` — whether the program ever sent an OSC 7501 report.

None of `name`, `title`, `reported`, or the mere presence of a terminal shows
that it is an agent. `status.app` is the only program-declared application
label.

**Status versus terminal lifetime.** `state` (`live`/`finished`) describes the
terminal, which can stay `live` after a program inside it ends, for example
while a shell remains. `status` describes only what a program last declared. A
source review of this revision found no event or field that reports the end of
an arbitrary program running inside a terminal, and `done` is a turn-completion
status, not a process exit. During the MIK-078 diagnosis, the user reported
an exited agent whose planet remained, while read-only `terminal.list`
snapshots from the confirmed endpoint showed live terminals with null status.
The snapshots were not matched to a captured before/after trace of that
specific agent (no identifiers recorded). They do not establish when its
status cleared or guarantee that every program clears its status on exit.

**Events.** The method is `events.subscribe` (plural; there is no
`event.subscribe`). It returns a terminal snapshot with a sequence number and
streams notifications (`created`, `lost`, `restarted`, `keep`, `finished`,
`removed`, `status`, `turn`, `title`, `labels`, `size`, `doc`), with a 15 s
default heartbeat and a fresh snapshot for subscribers that missed events.
wave-tui does not use it (see `SPEC.md`).

**Focus and rename: unsupported.** The released schema has no `agent.focus`,
`agent.rename`, or terminal focus/rename method. `terminal.attach` takes over a
terminal and the framed `FOCUS` control frame concerns an attached viewer
channel; neither focuses a pane. `terminal.run`'s `name` applies only at
creation. `machine.rename` renames a remote-machine alias, not a terminal.

## What the facts imply for wave-tui

These are conclusions, not further source facts; `SPEC.md` holds the resulting
design.

- Read-only observation is feasible through `terminal.list {}` and an
  app-side qualification rule; parity with the Herdr GUI adapter is not.
- Qualification must come from `state = live` with a *currently* nonblank
  `status.app`, re-evaluated on every successful snapshot. A cleared or blank
  application is the only program-declared sign that the program no longer
  claims the terminal, so wave-tui stops displaying it then; this is a product
  policy over declared data, not a claim that core reports agent exit.
  Within a displayed terminal, `error` and unrecognized states carry no
  reliable working/blocked/done/idle meaning and therefore map to Unknown,
  never to an invented status.
- Core can only report what programs declare. A live terminal whose program
  never sent a status, or whose `status` is currently cleared, cannot be told
  apart from a non-agent by this API; membership is deliberately conservative.
- Connecting to a socket the user can access is a trust decision; an endpoint
  that is merely the default path is not enough to opt in (see `SPEC.md`
  eligibility).
