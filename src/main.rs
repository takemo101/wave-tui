//! wave-tui primary binary entry point.
//!
//! The executable is intentionally thin: it delegates to [`wave_tui::cli::run`],
//! which parses CLI arguments, sets up and tears down the terminal, spawns the
//! native audio runtime and the blocking search worker, and drives the event
//! loop. Keeping the binary minimal preserves the module boundaries documented
//! in `AGENTS.md`: parsing, the event loop, and adapters live in `cli`, while
//! domain and rendering logic live in their own modules.
//!
//! For isolated manual native-audio verification, use the `audio_spike` binary
//! (see `docs/audio-spike.md`).

use anyhow::Result;

fn main() -> Result<()> {
    wave_tui::cli::run()
}
