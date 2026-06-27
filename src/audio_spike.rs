//! Thin compatibility shim for the native audio spike.
//!
//! The deterministic helpers validated by the spike now live in the production
//! audio module (`crate::audio`). These wrappers preserve the spike binary and
//! `tests/audio_spike.rs` API while delegating to that production logic, so
//! there is a single source of truth. New code should call `crate::audio`
//! directly rather than going through this shim.

use crate::audio::{self, StreamMount};

/// See [`crate::audio::stream_extension`].
pub fn stream_extension(path_or_url: &str) -> Option<&'static str> {
    audio::stream_extension(path_or_url)
}

/// Resolve a curated-style base URL by appending a `/stream` mount when needed.
///
/// The spike always treated its input as a curated base, so this maps to
/// [`StreamMount::CuratedStreamBase`]. Production Radio Browser URLs must use
/// [`StreamMount::Direct`] instead; see `docs/audio-spike.md`.
pub fn resolve_stream_url(audio_base_url: &str) -> String {
    audio::resolve_stream_url(audio_base_url, StreamMount::CuratedStreamBase)
}

/// See [`crate::audio::icy::parse_stream_title`].
pub fn parse_icy_title(metadata: &str) -> Option<String> {
    audio::icy::parse_stream_title(metadata)
}

/// See [`crate::audio::analyzer::normalize_value`].
pub fn normalize_value(x: f32, gain: f32) -> f32 {
    audio::analyzer::normalize_value(x, gain)
}
