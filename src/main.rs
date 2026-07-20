//! wave-tui primary binary entry point.
//!
//! The executable is intentionally thin: it delegates to
//! [`wave_tui::runtime::run`], the composition root that turns parsed CLI
//! arguments into a live session — terminal setup and teardown, the native
//! audio runtime, the blocking search worker, and the event loop. Keeping the
//! binary minimal preserves the module boundaries documented in `AGENTS.md`:
//! argument parsing and key mapping live in `cli`, runtime orchestration lives
//! in `runtime`, and domain and rendering logic live in their own modules.
//!
//! For isolated manual native-audio verification, use the `audio_spike` binary
//! (see `docs/audio-spike.md`).

use anyhow::Result;

fn main() -> Result<()> {
    wave_tui::runtime::run()
}
