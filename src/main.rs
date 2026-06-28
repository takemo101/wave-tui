//! wave-tui primary binary entry point.
//!
//! The pre-replacement prototype that lived here (external command-line
//! players, hard-coded colors, a raw Radio Browser station struct, and a fake
//! visualizer) has been retired in MIK-013 so the active binary path no longer
//! contradicts the documented native-audio replacement architecture.
//!
//! This is intentionally a small placeholder. Wiring the replacement modules
//! (`app`, `audio`, `catalog`, `cli`, `layout`, `model`, `search`, `settings`,
//! `theme`, `ui`) into the real terminal event loop, key handling, debounce,
//! persistence, and native audio runtime is MIK-010's responsibility.
//!
//! For manual native-audio verification, use the `audio_spike` binary
//! (see `docs/audio-spike.md`).

use anyhow::Result;

fn main() -> Result<()> {
    println!("wave-tui: native-audio replacement integration is pending (MIK-010).");
    println!("This placeholder entry point does not start the player yet.");
    println!("For manual native-audio verification, run the audio_spike binary:");
    println!("  cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5");
    Ok(())
}
