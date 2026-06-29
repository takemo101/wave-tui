# Splash Animation Design

## Summary

Add a quiet startup and shutdown splash to `wave-tui`: a short centered logo/message with a calm wave animation. The feature should make launch and exit feel polished without changing playback, search, visualizer modes, or app state behavior.

## Product Fit

`wave-tui` is a work-session BGM terminal app. The splash should be calm, brief, and easy to skip. It must not turn startup into a blocking ceremony or introduce flashy motion that conflicts with the Quiet Focus Pane direction.

## User Experience

### Startup Splash

When the app enters the alternate screen, show a short centered splash before the main UI appears. The startup logo should read as pixel art rather than plain text: a legible five-row `WAVE` mark rendered with terminal block glyphs, followed by the small `wave-tui` label and message. The logo is built from fixed-width letter cells so every row shares one display width and the centered block stays column-aligned; the renderer must not rely on per-line centering of ragged rows. Keep generous vertical spacing (blank lines) between the logo, label, and message, and one blank line below the message, so the splash never feels cramped. The startup splash is a calm, static logo card: it does not render the animated wave line (that animation belongs to the shutdown farewell).

```text
█   █    ███    █   █   █████
█   █   █   █   █   █   █
█ █ █   █████   █   █   ████
██ ██   █   █    █ █    █
█   █   █   █     █     █████

wave-tui

settling into the signal
```

Hold the static logo card for about 1.0 to 1.2 seconds. Any key press should skip the remaining splash and continue into the normal UI.

### Shutdown Splash

After the user quits with `q`, `Esc`, or `Ctrl-C`, but before restoring the terminal, show a shorter centered farewell:

```text
thanks for listening
see you next wave
```

Render the same calm wave animation for about 0.6 to 0.8 seconds. Any key press should skip the remaining shutdown splash.

### Low-Power Mode

Low-power mode should keep the same visual language but reduce work: fewer frames or a shorter duration. The splash must remain brief and deterministic.

## Rendering Direction

- Use the active theme for colors.
- Keep palette knowledge centralized in existing theme data; do not hard-code ad hoc `Color::*` palettes in the splash renderer.
- Use terminal block glyphs only for the compact pixel-art startup logo.
- Use simple wave glyphs for the animated wave line rather than heavy bars.
- Keep animation non-audio-reactive; it is a lifecycle transition, not a playback visualizer.
- Do not add a new `VisualizerMode`.

## Architecture

Add a focused UI submodule:

- `src/ui/splash.rs`
  - owns splash message layout, wave frame rendering, and pure tests;
  - exposes narrow entry points such as `render_startup_splash` and `render_shutdown_splash` with `pub(super)` visibility;
  - keeps helpers private.

Keep orchestration in the CLI lifecycle:

- `src/cli.rs`
  - after `TerminalGuard::new()` and before `event_loop(...)`, run the startup splash;
  - after `event_loop(...)` returns successfully and before terminal restore, run the shutdown splash;
  - skip shutdown splash if startup or event-loop setup returns an error before the TUI is usable.

Keep `src/ui.rs` focused on public UI entry points and module wiring:

- declare `mod splash;`;
- optionally expose small crate-private render wrappers used by the CLI splash loop.

## Input and Timing

The splash runner should:

- draw a frame;
- poll for key input using a small frame interval;
- exit early on any key event;
- exit when the configured duration elapses.

The implementation should avoid consuming non-key terminal events in a way that changes normal app behavior. Since startup splash runs before the main event loop and shutdown splash runs after it, key-to-skip behavior is isolated from app controls.

## Behavior Boundaries

Do not change:

- audio startup auto-play semantics;
- persisted settings format;
- keyboard mappings inside the main app;
- visualizer mode order or rendering;
- layout tiers or pane behavior;
- theme names or palette definitions, beyond using existing theme fields if needed.

## Testing

Prefer pure rendering tests for `src/ui/splash.rs`:

- startup splash contains the pixel-art `WAVE` logo, `wave-tui`, and `settling into the signal`;
- the logo rows share one left edge in the rendered buffer (no staggered/per-line-centered alignment) and form a contiguous block;
- blank rows separate the logo, label, and message so the splash is not cramped;
- the startup splash renders no wave glyphs and is identical across ticks (static logo card);
- the startup splash keeps a blank line below `settling into the signal` for bottom breathing room;
- shutdown splash contains `thanks for listening` and `see you next wave`;
- shutdown wave frames vary by tick while staying within the target area;
- renderer uses theme-provided styles and does not depend on app state;
- low-power timing configuration is shorter or lower-frame than normal timing.

Controller/lifecycle tests may cover timing configuration and skip behavior if easily injectable. Avoid tests that require a real terminal.

## Validation

Run at least:

```bash
cargo fmt --check
cargo test ui
cargo test app
cargo check
```

For final validation on the PR, also run:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
