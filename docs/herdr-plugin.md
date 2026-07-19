# wave-tui Herdr plugin manual

`wave-tui` ships as an official Herdr plugin (plugin id `wave-tui.radio`,
manifest [`herdr-plugin.toml`](../herdr-plugin.toml)). Installed through
Herdr's plugin manager, it opens the radio player in a **dedicated Herdr
tab** — keeping the Wide/Medium layouts usable — and, only for those plugin
launches, enables the optional read-only
[Herdr Agent Planets](../README.md#herdr-agent-planets-optional) companion.

This page is the operational manual: install, verify, open, develop, update,
uninstall, and troubleshoot. Feature behavior, controls, and privacy limits
live in the [README](../README.md#herdr-agent-planets-optional) and
[`docs/SPEC.md`](SPEC.md).

## Requirements

- **Herdr 0.7.0 or newer** — the plugin needs plugin manifests, plugin
  runtime context, and the `agent.list` socket API
  (`min_herdr_version = "0.7.0"` in the manifest).
- **macOS or Linux** — the platforms declared by the manifest.
- **A Rust toolchain (`cargo`)** — installation builds the release binary
  with `cargo build --release`. On Linux, native `cpal` audio output may
  additionally need system audio libraries before the build links (see the
  [README troubleshooting](../README.md#troubleshooting)).
- Network access for the radio streams themselves.

## Install from GitHub

```bash
herdr plugin install takemo101/wave-tui
```

Herdr fetches the repository, reads `herdr-plugin.toml`, and runs the
manifest's `cargo build --release` build step. The plugin registers one pane
entrypoint (`radio`, placement `tab`) and one action (`open`).

## Verify the installation

```bash
herdr plugin list --plugin wave-tui.radio --json
```

Expect a record for plugin id `wave-tui.radio` with its version and the
`radio` pane entrypoint. If nothing is listed, see
[Troubleshooting](#troubleshooting).

## Open the radio tab

Through the plugin's `open` action (the same action Herdr's UI exposes as
`Open wave-tui radio tab`):

```bash
herdr plugin action invoke open --plugin wave-tui.radio
```

Or open the pane entrypoint directly — equivalent, without going through the
action:

```bash
herdr plugin pane open --plugin wave-tui.radio --entrypoint radio --placement tab --focus
```

From a repository checkout, `just herdr-open` rebuilds the release binary and
then opens the installed plugin's dedicated tab with the same pane command.

The tab owns the audio process: closing the tab exits `wave-tui` and stops
playback, and detaching/reattaching the Herdr session leaves the process
under Herdr's normal pane lifecycle.

## Optional `config.toml` keybinding

No configuration is required for the plugin to install or run. To open the
Agent Planets radio tab with a Herdr command key, add this to
`~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+r"
type = "plugin_action"
command = "wave-tui.radio.open"
description = "Open wave-tui Agent Planets"
```

Restart or reload Herdr after editing its configuration. The binding invokes
the same `open` plugin action as the command above and opens a dedicated tab
in the focused workspace. Change `prefix+r` to any unused command key.

Plugin-specific settings are not currently read by `wave-tui`; if they are
introduced later, they belong in
`~/.config/herdr/plugins/config/wave-tui.radio/`, not Herdr's main config.

## Local development

From a checkout of this repository:

```bash
just herdr-dev
```

This builds the release binary, links the checkout as the plugin with
`herdr plugin link`, and opens the dedicated tab. The equivalent manual flow:

```bash
cargo build --release
herdr plugin link /path/to/wave-tui
herdr plugin pane open --plugin wave-tui.radio --entrypoint radio --placement tab --focus
```

While the checkout is linked, rebuild (`cargo build --release` or
`just build-release`) and reopen the tab to pick up changes.

## Update or reinstall

Reinstall from GitHub to pick up a newer release of the plugin:

```bash
herdr plugin install takemo101/wave-tui
```

For a linked development checkout, pull the changes and rerun
`just herdr-dev` (or rebuild and reopen the tab).

## Uninstall

```bash
herdr plugin uninstall wave-tui.radio
```

If your Herdr version names the lifecycle commands differently, check
`herdr plugin --help`. Removing the plugin does not touch a separately
installed standalone `wave-tui` binary or your saved settings; for the
standalone binary, use the [README uninstall steps](../README.md#uninstall).

## What Agent Planets requires

The Agent Planets companion enables itself only when **all** of the following hold at launch:

1. `--no-agent-pulse` was not passed.
2. `HERDR_ENV` is exactly `1`.
3. `HERDR_SOCKET_PATH` is set and non-empty.
4. `HERDR_WORKSPACE_ID` is set and non-empty.

The official plugin launch supplies these variables; you never set them by
hand. In every other case — a standalone launch, a plain shell inside Herdr
without plugin context, or an explicit `--no-agent-pulse` — `wave-tui` keeps
its exact pre-integration appearance: no reserved rows, no hints, and `a`
does nothing.

When active, the integration polls the read-only `agent.list` API on the
session's local Unix socket every 5 seconds and shows every agent that
session reports, across its workspaces. It never reads pane output, never
controls panes, and persists nothing. Disable it for one run with
`--no-agent-pulse` (never written to settings).

## Troubleshooting

**`herdr plugin install` fails during the build.** The build step runs
`cargo build --release`, so a Rust toolchain must be on `PATH`. On Linux,
missing system audio libraries can also fail the link step — install your
distribution's audio development packages first.

**The plugin is not in `herdr plugin list`.** Confirm Herdr is 0.7.0 or
newer, then reinstall. For a development checkout, confirm the link step ran
(`just herdr-dev` runs it for you).

**The action or pane open does nothing.** Invoke the pane entrypoint
directly (`herdr plugin pane open ...` above) to bypass the action wrapper;
the action shells out to `$HERDR_BIN_PATH plugin pane open`, so a stale or
missing Herdr binary path breaks the action while the direct command still
works.

**No `● n active` line appears.** Check the four eligibility conditions
above — the most common cause is running `wave-tui` from a plain Herdr shell
instead of the plugin tab. The Compact layout also hides the summary line by
design (while `a` still opens the stage), and Signal View never shows Agent
Pulse.

**`a` does nothing.** The launch was standalone or ineligible; Agent Planets
exists only for eligible plugin launches.

**The stage shows `· reconnecting` or `agents · unavailable · retrying`.**
Socket polls are failing; playback is unaffected. Polling continues and a
successful snapshot restores the live view automatically. If it never
recovers, the Herdr session that launched the plugin may have ended —
reopen the tab from a live session.

**Terminal text selection stopped working in the tab.** Eligible plugin
launches enable mouse capture for planet selection; use `Shift`+drag for
native terminal text selection.

**Audio problems.** Stream, device, and codec issues are independent of the
plugin — see the [README troubleshooting](../README.md#troubleshooting)
section.
