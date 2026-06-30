# Signal View Design

## Summary

Add an explicit **Signal View** design direction for `wave-tui`: a visual-player mode entered from the normal TUI with `z`. Signal View hides search, Browse, and station-selection UI so the current station can be presented as a calm, center-stage radio visualizer.

This is a design-only spec. It does not implement the mode. The selected visual direction is **Quiet Signal** from the design deck: spare, centered, minimal, and aligned with the existing Quiet Focus Pane personality.

## Product Fit

`wave-tui` is a terminal-first internet radio player for work-session BGM. The normal UI is optimized for finding and controlling stations. Signal View is for the moments after selection, when the user wants to leave one station playing and enjoy a larger visualizer without the discovery interface.

This fits the MVP as an explicit opt-in mode. It does not change the default responsive layouts, and it preserves the existing rule that compact mode should not become a full-screen visualizer by default.

## Goals

- Provide a stylish, low-distraction visual-player mode for the current station.
- Make the real audio visualizer feel more prominent without changing normal Search/Browse workflows.
- Keep interaction predictable: easy to enter, easy to leave, hard to get trapped.
- Reuse existing theme and visualizer-mode choices rather than introducing a separate rendering system.

## Non-Goals

- Do not add playlist management, queueing, or new station discovery behavior.
- Do not add a CLI flag such as `--signal-view` in the first implementation.
- Do not persist whether Signal View was active on exit.
- Do not add a Signal View-specific visualizer in the first implementation.
- Do not add title marquee animation, fade transitions, or per-theme bespoke layouts in the first implementation.

## User Experience

### Entry and Exit

- Press `z` from the normal TUI to enter Signal View.
- Press `z` again or `Esc` to return to the normal TUI.
- Press `q` to quit the app, even while Signal View is active.

Signal View is a temporary display mode, not a navigation pane. It should not participate in `Tab` focus order.

### Allowed Keys

While Signal View is active, only these controls remain active:

| Key | Behavior |
| --- | --- |
| `z` | Return to normal UI |
| `Esc` | Return to normal UI |
| `q` | Quit app |
| `Space` | Toggle playback |
| `+` / `=` | Volume up |
| `-` / `_` | Volume down |
| `v` | Cycle visualizer mode |
| `t` | Cycle theme |
| `f` | Toggle favorite for the current station |

Search, Browse, station list navigation, focus movement, and station selection keys are disabled while Signal View is active. Disabled keys should be ignored silently.

### Station Target

Signal View displays the app's current station, not the currently selected row in the hidden station list.

This matters for `f`: in Signal View, `f` toggles favorite state for the current station shown on screen. It must not toggle a hidden station-list selection.

### No Current Station

If no current station exists, pressing `z` still enters Signal View. The mode shows a quiet idle state with a short prompt such as:

```text
Select a station, then press z
```

This keeps the `z` key behavior consistent and lets users discover the mode safely.

### Playback States

If a current station exists, Signal View stays centered on that station across playback states:

- Playing: show the ICY title when available, otherwise station name.
- Stopped: show the current station with a quiet/silent visualizer state.
- Connecting: show the current station with a small `connecting` status label.
- Failed: remain in Signal View and show a short failure status label.

Playback errors should not automatically exit Signal View.

### Metadata Priority

The primary title text should be:

1. ICY/Shoutcast now-playing title, when present.
2. Current station name, when no ICY title is available.
3. Idle copy, when there is no current station.

Long ICY titles or station names should be centered and constrained to at most two lines. Overflow should be clipped or ellipsized rather than pushing layout regions out of view.

### Favorite State

Signal View should show the current station's favorite state with a small `♥` / `♡` indicator near the title or metadata line. The indicator should be subtle and theme-colored.

### Key Hint

Show a thin, low-contrast footer line:

```text
z/Esc back · Space play · v visual · +/- volume · q quit
```

This hint should be visible enough to prevent trapping users, but visually quieter than the title and visualizer.

## Selected Visual Direction: Quiet Signal

The selected design-deck option is **Quiet Signal**.

Quiet Signal characteristics:

- center-stage composition;
- lots of negative space;
- minimal or no heavy panel borders;
- metadata arranged above the visualizer;
- large visualizer below the title;
- subdued footer hint;
- current theme colors respected;
- calm enough for long work sessions.

The first implementation should avoid ornate chrome. Signal View should feel like the normal right-side Now Playing pane has been expanded into a quiet, full-screen listening surface.

## Layout Direction

Signal View should bypass the normal Wide/Medium/Compact composition. Instead, `ui::render_into` can branch early when Signal View is active and render a dedicated full-area layout.

Recommended vertical structure:

1. Small mode label or station/status metadata.
2. Main title area, centered.
3. Large visualizer area.
4. Thin key-hint footer.

The visualizer should receive the largest flexible region in the layout. On medium and large panes, target roughly half or more of the available terminal height for the visualizer after reserving title, metadata, and footer rows. This should feel meaningfully larger than the normal Now Playing pane, whose visualizer is intentionally capped in the standard layouts.

On small terminals, preserve the same Center Stage concept but drop lower-priority details first:

1. Keep title and visualizer.
2. Keep back/quit hint if possible.
3. Drop visualizer-mode/theme labels before dropping station title.
4. Use compact status labels.

Signal View should not fall back to the normal Compact UI merely because the terminal is small.

## Theme and Visualizer Behavior

Signal View uses the currently selected theme and the currently selected visualizer mode.

- `t` cycles theme as it does in normal UI.
- `v` cycles visualizer mode as it does in normal UI.
- The selected visualizer mode remains persisted through existing settings behavior.
- Signal View active/inactive state itself is not persisted.

The first implementation should reuse the existing visualizer renderer over a larger area. It should not introduce a dedicated Signal View visualizer yet.

## Search and Background State

Signal View is display-only. It should not pause, cancel, or clear background app state.

If a search was in progress before entering Signal View, it may continue and update app state in the background. Returning to the normal UI should show the current search/list state.

## Architecture Direction

Candidate state changes:

- Add a display-mode flag to `App`, for example `DisplayMode::Normal | DisplayMode::SignalView` or an equivalent boolean if the project prefers minimal state.
- Add reducer actions for entering/leaving/toggling Signal View.
- Add a current-station favorite action so Signal View can favorite the displayed station rather than the hidden selected row.

Candidate CLI/input changes:

- Map `z` to toggle Signal View in normal mode.
- While Signal View is active, route only the allowed key subset to app actions.
- Treat disabled keys as no-ops.
- Preserve `q` as app quit.

Candidate UI changes:

- Branch in the top-level render path before layout-tier selection when Signal View is active.
- Add a focused `render_signal_view` helper.
- Reuse existing visualizer rendering with a larger `Rect`.
- Keep rendering read-only; UI should not mutate app state directly.

## Documentation Updates

When implemented, update:

- `docs/SPEC.md` with the Signal View behavior and key model.
- `docs/ui-design-decisions.md` with the Quiet Signal visual direction.
- `README.md` keybinding/help copy if user-facing controls are listed there.

## Acceptance Criteria

Design acceptance for future implementation:

- `z` enters Signal View from normal UI.
- `z` and `Esc` return from Signal View to normal UI.
- `q` still quits the app while Signal View is active.
- Signal View hides search, Browse, and station-list UI.
- Signal View displays current station data, not hidden list selection data.
- If ICY title exists, it is the primary title; otherwise station name is primary.
- Long titles do not break layout and are constrained to two lines.
- The visualizer receives the largest flexible layout region and is meaningfully larger than the normal Now Playing visualizer on medium and large panes.
- Signal View can be entered with no current station and shows a quiet idle state.
- Stopped, connecting, playing, and failed states are represented without leaving Signal View.
- `Space`, `+/-`, `v`, `t`, and current-station `f` work while Signal View is active.
- Search/navigation/focus keys are ignored silently while Signal View is active.
- Current theme and visualizer mode are respected.
- Signal View active state is not persisted across app launches.

## Testing Direction

Prefer pure reducer and render tests:

- toggling Signal View changes display mode but not selected station/source/search state;
- `Esc`/`z` exits Signal View;
- `q` remains a quit outcome in Signal View;
- disabled keys are no-ops in Signal View;
- allowed keys still dispatch their existing actions;
- `f` toggles favorite state for `current`, not hidden selection;
- render output in Signal View omits Search/Browse/Stations labels;
- render output includes current station or ICY title;
- no-current-station render shows idle copy;
- small terminal render remains bounded and does not panic.

## Validation for Future Implementation

Run at least:

```bash
cargo fmt --check
cargo test app
cargo test ui
cargo test cli
cargo check
```

Before merging a larger implementation, also run:

```bash
cargo clippy --all-targets -- -D warnings
```
