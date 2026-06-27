//! Native playback facade.
//!
//! This is the public entry point for the audio module. Decoder, output,
//! analyzer, and ICY details are kept private behind this facade so callers
//! depend on the facade rather than on CPAL/Symphonia/RustFFT specifics. The
//! runtime, commands, and events are implemented in a later task.

mod analyzer;
mod decoder;
mod icy;
mod output;
