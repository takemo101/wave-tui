# Splash Animation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a quiet startup and shutdown splash with a centered pixel-art `WAVE` startup logo, short messages, and a calm wave animation.

**Architecture:** Rendering lives in a focused `src/ui/splash.rs` submodule, while `src/cli.rs` owns terminal lifecycle timing and skip behavior. The splash is deterministic, theme-driven, and separate from app state, audio, search, and visualizer modes.

**Tech Stack:** Rust, Ratatui `Buffer`/`Frame`, Crossterm event polling, existing `Theme` palette, existing cargo test/check/clippy workflow.

## Global Constraints

- Preserve the merged design spec: `docs/superpowers/specs/2026-06-28-splash-animation-design.md`.
- Startup splash renders a compact five-row pixel-art `WAVE` mark, then the small `wave-tui` label and `settling into the signal` message.
- Shutdown copy is exactly `thanks for listening` and `see you next wave`.
- Startup splash runs after entering alternate screen and before the main UI event loop.
- Shutdown splash runs after the main event loop exits and before terminal restore.
- Any key press skips the remaining splash.
- Low-power mode uses fewer frames or a shorter duration.
- Use existing theme colors; do not introduce ad hoc `Color::*` palettes in the splash renderer.
- Do not change audio startup auto-play semantics, visualizer mode order/rendering, settings format, layout tiers, or main app key mappings.
- Do not edit `.asem/**` runtime files.
- Worker sessions must not commit; the parent session handles mikan status and GitHub PR/merge via API.

---

## File Structure

- Create `src/ui/splash.rs`
  - Owns pure splash rendering, pixel-art startup logo constants, message constants, timing configuration, and renderer tests.
  - Exposes only `pub(super)` functions/types needed by `src/ui.rs`: `SplashKind`, `SplashTiming`, `render_splash_into`, `normal_timing`, and `low_power_timing`.
- Modify `src/ui.rs`
  - Add `mod splash;`.
  - Add narrow `pub(crate)` wrappers so the CLI can render splash frames without importing the private submodule directly.
- Modify `src/cli.rs`
  - Add a small splash runner after `TerminalGuard::new()` and around normal loop shutdown.
  - Keep the main event loop unchanged except for invoking the splash before and after it.
- Modify `.mikan/active/MIK-037.md`
  - Append final implementation/validation report when done.

---

### Task 1: Pure Splash Renderer and Timing

**Files:**

- Create: `src/ui/splash.rs`
- Modify: `src/ui.rs`

**Interfaces:**

- Consumes: `crate::theme::Theme`, Ratatui `Buffer`, `Rect`.
- Produces:
  - `pub(crate) use splash::{SplashKind, SplashTiming};` from `src/ui.rs` or equivalent wrapper exports.
  - `pub(crate) fn splash_timing(low_power: bool) -> SplashTiming`.
  - `pub(crate) fn render_splash(kind: SplashKind, theme: &Theme, tick: u16, area: Rect, buf: &mut Buffer)`.

- [ ] **Step 1: Add failing renderer tests in `src/ui/splash.rs`**

Create `src/ui/splash.rs` with tests first. Use this skeleton and let it fail because the public items/functions are not implemented yet:

```rust
use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::theme::{Theme, ThemeName};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SplashKind {
    Startup,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplashTiming {
    pub duration: Duration,
    pub frame_interval: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(kind: SplashKind, tick: u16) -> String {
        let theme = ThemeName::Minimal.theme();
        let area = Rect::new(0, 0, 48, 12);
        let mut buf = Buffer::empty(area);
        render_splash_into(kind, &theme, tick, area, &mut buf);
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf.cell((x, y)).symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn startup_splash_contains_pixel_logo_label_and_message() {
        let out = render_to_string(SplashKind::Startup, 0);
        // Distinctive bands of the assembled WAVE mark (logo is built from
        // fixed-width letter cells; assert on its glyphs, not a fixed slice).
        assert!(out.contains("█ █ █"), "startup should render the pixel-art WAVE logo");
        assert!(out.contains("█████"));
        assert!(out.contains("wave-tui"));
        assert!(out.contains("settling into the signal"));
    }

    #[test]
    fn shutdown_splash_contains_farewell_message() {
        let out = render_to_string(SplashKind::Shutdown, 0);
        assert!(out.contains("thanks for listening"));
        assert!(out.contains("see you next wave"));
    }

    #[test]
    fn shutdown_wave_frame_changes_by_tick() {
        // The wave animation lives only on the shutdown farewell; the startup
        // splash is a static logo card with no wave glyphs.
        let first = render_to_string(SplashKind::Shutdown, 0);
        let second = render_to_string(SplashKind::Shutdown, 1);
        assert_ne!(first, second, "wave animation should change between ticks");
        assert!(first.contains('~') || first.contains('≈') || first.contains('∿'));
        assert!(second.contains('~') || second.contains('≈') || second.contains('∿'));
    }

    #[test]
    fn low_power_timing_is_not_more_expensive_than_normal() {
        let normal = normal_timing(SplashKind::Startup);
        let low = low_power_timing(SplashKind::Startup);
        assert!(low.duration <= normal.duration);
        assert!(low.frame_interval >= normal.frame_interval);
    }

    #[test]
    fn tiny_area_is_safe() {
        let theme = ThemeName::Minimal.theme();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        render_splash_into(SplashKind::Startup, &theme, 0, area, &mut buf);
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test ui::splash
```

Expected: fail with errors for missing `render_splash_into`, `normal_timing`, and `low_power_timing`.

- [ ] **Step 3: Implement the pure renderer in `src/ui/splash.rs`**

Replace the file with complete implementation. Keep colors from `Theme` only; no `Color::*` literals are needed here.

```rust
use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::theme::Theme;

// Build the WAVE logo from fixed-width 5x5 letter cells joined by a constant
// gap so every assembled row shares one display width. Equal-width rows are what
// keeps the centered block column-aligned — do NOT hand-write ragged rows and
// rely on per-line centering, which staggers them.
const LOGO_W: [&str; 5] = ["█   █", "█   █", "█ █ █", "██ ██", "█   █"];
const LOGO_A: [&str; 5] = [" ███ ", "█   █", "█████", "█   █", "█   █"];
const LOGO_V: [&str; 5] = ["█   █", "█   █", "█   █", " █ █ ", "  █  "];
const LOGO_E: [&str; 5] = ["█████", "█    ", "████ ", "█    ", "█████"];
const LOGO_GAP: &str = "   ";
// Assembled rows (for reference); render with blank-line spacing around the
// label and message so the splash is not cramped. The startup splash is a
// static logo card and renders NO wave line (the wave animation is shutdown-only):
//   █   █    ███    █   █   █████
//   █   █   █   █   █   █   █
//   █ █ █   █████   █   █   ████
//   ██ ██   █   █    █ █    █
//   █   █   █   █     █     █████
const STARTUP_LABEL: &str = "wave-tui";
const STARTUP_MESSAGE: &str = "settling into the signal";
const SHUTDOWN_TITLE: &str = "thanks for listening";
const SHUTDOWN_MESSAGE: &str = "see you next wave";
const WAVE_GLYPHS: [char; 3] = ['~', '≈', '∿'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SplashKind {
    Startup,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplashTiming {
    pub duration: Duration,
    pub frame_interval: Duration,
}

pub(super) fn normal_timing(kind: SplashKind) -> SplashTiming {
    match kind {
        SplashKind::Startup => SplashTiming {
            duration: Duration::from_millis(1_100),
            frame_interval: Duration::from_millis(90),
        },
        SplashKind::Shutdown => SplashTiming {
            duration: Duration::from_millis(700),
            frame_interval: Duration::from_millis(90),
        },
    }
}

pub(super) fn low_power_timing(kind: SplashKind) -> SplashTiming {
    match kind {
        SplashKind::Startup => SplashTiming {
            duration: Duration::from_millis(700),
            frame_interval: Duration::from_millis(140),
        },
        SplashKind::Shutdown => SplashTiming {
            duration: Duration::from_millis(450),
            frame_interval: Duration::from_millis(140),
        },
    }
}

pub(super) fn render_splash_into(
    kind: SplashKind,
    theme: &Theme,
    tick: u16,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    buf.set_style(area, theme.base_style());
    Clear.render(area, buf);

    let splash_height = match kind {
        SplashKind::Startup => area.height.min(9),
        SplashKind::Shutdown => area.height.min(5),
    };
    let splash_width = area.width.min(48);
    let splash_area = centered_rect(area, splash_width, splash_height);

    // Only the shutdown farewell carries the animated wave line.
    let lines = match kind {
        SplashKind::Startup => startup_lines(theme),
        SplashKind::Shutdown => shutdown_lines(theme, wave_line(splash_area.width, tick)),
    };

    let paragraph = Paragraph::new(lines)
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true })
    .block(Block::default().style(theme.base_style()));

    paragraph.render(splash_area, buf);
}

// Startup is a static logo card: NO `wave` parameter and NO wave line. Build the
// logo rows from the fixed-width letter cells (see logo_rows()), then add blank
// spacers around the label and message for breathing room.
fn startup_lines(theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    for row in logo_rows() {
        lines.push(Line::from(Span::styled(
            row,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        STARTUP_LABEL,
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        STARTUP_MESSAGE,
        Style::default().fg(theme.foreground),
    )));
    // Trailing blank: breathing room below the message (no wave line).
    lines.push(Line::from(""));
    lines
}

fn shutdown_lines(theme: &Theme, wave: String) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            SHUTDOWN_TITLE,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            SHUTDOWN_MESSAGE,
            Style::default().fg(theme.foreground),
        )),
        Line::from(Span::styled(
            wave,
            Style::default().fg(theme.playing),
        )),
    ]
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn wave_line(width: u16, tick: u16) -> String {
    if width == 0 {
        return String::new();
    }
    let visible = width.saturating_sub(4).max(1) as usize;
    (0..visible)
        .map(|i| WAVE_GLYPHS[(i + tick as usize) % WAVE_GLYPHS.len()])
        .collect()
}
```

Keep the tests from Step 1 at the bottom of the same file.

- [ ] **Step 4: Wire narrow UI wrappers in `src/ui.rs`**

Add the module declaration near `mod visualizer;`:

```rust
mod splash;
mod visualizer;
```

Add wrappers near `pub fn render`:

```rust
pub(crate) use splash::{SplashKind, SplashTiming};

pub(crate) fn splash_timing(kind: SplashKind, low_power: bool) -> SplashTiming {
    if low_power {
        splash::low_power_timing(kind)
    } else {
        splash::normal_timing(kind)
    }
}

pub(crate) fn render_splash(
    kind: SplashKind,
    theme: &Theme,
    tick: u16,
    frame: &mut Frame,
) {
    splash::render_splash_into(kind, theme, tick, frame.area(), frame.buffer_mut());
}
```

If Rust visibility complains about re-exporting `pub(super)` items, make `SplashKind` `pub(crate)` in `src/ui/splash.rs` while keeping helper functions `pub(super)`.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo fmt --check
cargo test ui::splash
cargo test ui
```

Expected: all pass.

---

### Task 2: Terminal Lifecycle Splash Runner

**Files:**

- Modify: `src/cli.rs`

**Interfaces:**

- Consumes: `crate::ui::{render_splash, splash_timing, SplashKind, SplashTiming}`.
- Produces:
  - `fn splash_frame_budget(timing: crate::ui::SplashTiming) -> u16` for testable timing math.
  - `fn run_splash(terminal: &mut Terminal<CrosstermBackend<Stdout>>, theme: ThemeName, kind: crate::ui::SplashKind, low_power: bool) -> Result<()>` for terminal lifecycle rendering.

- [ ] **Step 1: Add failing timing tests in `src/cli.rs`**

In the existing `#[cfg(test)] mod tests` in `src/cli.rs`, add:

```rust
#[test]
fn splash_frame_budget_covers_duration_at_least_once() {
    let timing = crate::ui::SplashTiming {
        duration: Duration::from_millis(250),
        frame_interval: Duration::from_millis(100),
    };
    assert_eq!(splash_frame_budget(timing), 3);
}

#[test]
fn low_power_splash_budget_is_no_larger_than_normal() {
    let normal = crate::ui::splash_timing(crate::ui::SplashKind::Startup, false);
    let low = crate::ui::splash_timing(crate::ui::SplashKind::Startup, true);
    assert!(splash_frame_budget(low) <= splash_frame_budget(normal));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test cli::tests::splash_frame_budget_covers_duration_at_least_once cli::tests::low_power_splash_budget_is_no_larger_than_normal
```

Expected: fail because `splash_frame_budget` does not exist.

- [ ] **Step 3: Add imports and timing helper in `src/cli.rs`**

Add `ThemeName` is already imported; use existing import. Add helper near the event-loop helpers:

```rust
fn splash_frame_budget(timing: crate::ui::SplashTiming) -> u16 {
    let interval_ms = timing.frame_interval.as_millis().max(1);
    let duration_ms = timing.duration.as_millis();
    let frames = duration_ms.div_ceil(interval_ms).max(1);
    frames.min(u16::MAX as u128) as u16
}
```

- [ ] **Step 4: Implement `run_splash` in `src/cli.rs`**

Add this function near `event_loop`:

```rust
fn run_splash(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: ThemeName,
    kind: crate::ui::SplashKind,
    low_power: bool,
) -> Result<()> {
    let timing = crate::ui::splash_timing(kind, low_power);
    let palette = theme.theme();
    let frames = splash_frame_budget(timing);

    for tick in 0..frames {
        terminal.draw(|frame| crate::ui::render_splash(kind, &palette, tick, frame))?;
        if event::poll(timing.frame_interval)? {
            if matches!(event::read()?, Event::Key(_)) {
                break;
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Call splash runner from `run_app`**

In `run_app`, after `let mut guard = TerminalGuard::new()?;` and before `event_loop(...)`, add:

```rust
run_splash(
    &mut guard.terminal,
    app.settings().theme,
    crate::ui::SplashKind::Startup,
    args.low_power,
)?;
```

Replace the existing event-loop/shutdown tail with logic that shows shutdown splash only after a successful event loop:

```rust
let loop_result = event_loop(
    &mut guard.terminal,
    &mut app,
    &runtime,
    &mut debounce,
    &mut persistence,
    args.low_power,
);

if loop_result.is_ok() {
    let _ = run_splash(
        &mut guard.terminal,
        app.settings().theme,
        crate::ui::SplashKind::Shutdown,
        args.low_power,
    );
}

let _ = audio.command_tx.send(AudioCommand::Shutdown);
let _ = request_tx.send(SearchRequest::Shutdown);
persistence.save(&app);
let _ = worker.join();

loop_result
```

This preserves the existing return value and cleanup ordering while adding the farewell before the terminal guard drops.

- [ ] **Step 6: Run focused CLI/UI tests**

Run:

```bash
cargo fmt --check
cargo test ui::splash
cargo test cli::tests::splash_frame_budget_covers_duration_at_least_once
cargo test cli::tests::low_power_splash_budget_is_no_larger_than_normal
cargo test ui
cargo test app
cargo check
```

Expected: all pass.

---

### Task 3: Final Validation and Issue Report

**Files:**

- Modify: `.mikan/active/MIK-037.md`

**Interfaces:**

- Consumes: completed Tasks 1 and 2.
- Produces: a final report in the mikan issue with implementation summary and validation evidence.

- [ ] **Step 1: Run complete validation**

Run:

```bash
cargo fmt --check
cargo test ui
cargo test app
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 2: Run lens diagnostics**

Run via parent tooling if available:

```text
lens_diagnostics mode=all severity=error
```

Expected: no blocking errors in edited files.

- [ ] **Step 3: Append mikan report**

Use mikan MCP or CLI, not direct file editing, to append a report to `MIK-037`:

```markdown
Implemented quiet startup/shutdown splash animation.

Summary:
- Added focused `src/ui/splash.rs` renderer with a pixel-art startup logo, startup/shutdown messages, and deterministic wave frames.
- Wired splash rendering through narrow `src/ui.rs` wrappers.
- Added lifecycle splash runner in `src/cli.rs` before the main UI loop and after clean quit.
- Low-power mode uses a shorter/lower-frame timing budget.

Validation:
- `cargo fmt --check`
- `cargo test ui`
- `cargo test app`
- `cargo check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `lens_diagnostics mode=all severity=error`
```

- [ ] **Step 4: Report to parent session**

If implementing in asem, report parent with:

```text
MIK-037 implementation complete.
Changed files:
- src/ui.rs
- src/ui/splash.rs
- src/cli.rs
- .mikan/active/MIK-037.md

Validation:
[exact command results]

Notes:
[any caveats, especially manual visual verification status]
```

Do not close the session yourself.

---

## Self-Review

- Spec coverage: startup splash, shutdown splash, skip behavior, low-power timing, theme-driven rendering, focused module boundary, and non-interference constraints are each mapped to tasks.
- Placeholder scan: no placeholder tokens or unspecified edge handling remains.
- Type consistency: `SplashKind`, `SplashTiming`, `render_splash`, `splash_timing`, and `splash_frame_budget` signatures match across tasks.
