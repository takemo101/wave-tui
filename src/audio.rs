//! Native playback facade.
//!
//! This is the public entry point for the audio module. Decoder, output,
//! analyzer, and ICY details are kept private behind this facade so callers
//! depend on the facade rather than on CPAL/Symphonia/RustFFT specifics. The
//! runtime, commands, and events are implemented in a later task.
//!
//! Deterministic helpers validated by the native audio spike live here and in
//! the private submodules: stream-URL resolution policy (this file), FFT
//! normalization and log-band mapping ([`analyzer`]), and ICY `StreamTitle`
//! parsing ([`icy`]). See `docs/audio-spike.md`.

pub(crate) mod analyzer;
mod decoder;
pub(crate) mod icy;
mod output;

/// How a raw station URL should be resolved into a concrete stream URL.
///
/// The spike showed that blindly appending `/stream` breaks real Radio Browser
/// mounts (`docs/audio-spike.md`), so resolution is an explicit policy rather
/// than a guess: Radio Browser `url_resolved` values are [`Direct`], and only
/// curated bases that opt in use [`CuratedStreamBase`].
///
/// [`Direct`]: StreamMount::Direct
/// [`CuratedStreamBase`]: StreamMount::CuratedStreamBase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamMount {
    /// Use the URL exactly as provided. Default for Radio Browser results and
    /// any already-direct stream URL.
    // Constructed by the Radio Browser playback path in a later task; covered by tests now.
    #[allow(dead_code)]
    Direct,
    /// Curated base URL that explicitly requires a `/stream` mount appended.
    CuratedStreamBase,
}

/// Recognized stream container extension, ignoring any query string.
///
/// Returns a `'static` codec hint (`"mp3"`, `"aac"`, `"m4a"`) usable as a
/// Symphonia probe hint, or `None` when the path has no known audio extension.
pub(crate) fn stream_extension(path_or_url: &str) -> Option<&'static str> {
    let path = path_or_url.split('?').next().unwrap_or(path_or_url);
    if path.ends_with(".mp3") {
        Some("mp3")
    } else if path.ends_with(".aac") {
        Some("aac")
    } else if path.ends_with(".m4a") {
        Some("m4a")
    } else {
        None
    }
}

/// Resolve a raw station URL into a concrete stream URL according to `mount`.
///
/// [`StreamMount::Direct`] never appends anything; the input is treated as an
/// authoritative stream URL (only surrounding whitespace is trimmed).
/// [`StreamMount::CuratedStreamBase`] appends `/stream` only when the base is
/// not already a stream mount or a direct media URL.
pub(crate) fn resolve_stream_url(raw: &str, mount: StreamMount) -> String {
    match mount {
        StreamMount::Direct => raw.trim().to_string(),
        StreamMount::CuratedStreamBase => {
            let trimmed = raw.trim().trim_end_matches('/');
            if trimmed.ends_with("/stream")
                || trimmed.contains("/stream/")
                || stream_extension(trimmed).is_some()
            {
                trimmed.to_string()
            } else {
                format!("{trimmed}/stream")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_policy_never_appends_stream() {
        // Radio Browser url_resolved values must be used verbatim.
        assert_eq!(
            resolve_stream_url(
                "https://stream.radioparadise.com/mp3-192",
                StreamMount::Direct
            ),
            "https://stream.radioparadise.com/mp3-192"
        );
        assert_eq!(
            resolve_stream_url("https://example.com/radio", StreamMount::Direct),
            "https://example.com/radio"
        );
    }

    #[test]
    fn direct_policy_only_trims_surrounding_whitespace() {
        assert_eq!(
            resolve_stream_url("  https://example.com/live.mp3  ", StreamMount::Direct),
            "https://example.com/live.mp3"
        );
        // A trailing slash is preserved; a direct URL is authoritative.
        assert_eq!(
            resolve_stream_url("https://example.com/radio/", StreamMount::Direct),
            "https://example.com/radio/"
        );
    }

    #[test]
    fn curated_base_appends_stream_mount_when_needed() {
        assert_eq!(
            resolve_stream_url("https://example.com/radio", StreamMount::CuratedStreamBase),
            "https://example.com/radio/stream"
        );
        assert_eq!(
            resolve_stream_url("https://example.com/radio/", StreamMount::CuratedStreamBase),
            "https://example.com/radio/stream"
        );
    }

    #[test]
    fn curated_base_preserves_direct_media_and_existing_mounts() {
        assert_eq!(
            resolve_stream_url(
                "https://example.com/live.mp3",
                StreamMount::CuratedStreamBase
            ),
            "https://example.com/live.mp3"
        );
        assert_eq!(
            resolve_stream_url(
                "https://example.com/live.aac?token=1",
                StreamMount::CuratedStreamBase
            ),
            "https://example.com/live.aac?token=1"
        );
        assert_eq!(
            resolve_stream_url(
                "https://example.com/x/stream",
                StreamMount::CuratedStreamBase
            ),
            "https://example.com/x/stream"
        );
    }

    #[test]
    fn detects_supported_stream_extensions_before_query_string() {
        assert_eq!(
            stream_extension("https://example.com/live.mp3?x=1"),
            Some("mp3")
        );
        assert_eq!(
            stream_extension("https://example.com/live.aac"),
            Some("aac")
        );
        assert_eq!(
            stream_extension("https://example.com/live.m4a"),
            Some("m4a")
        );
        assert_eq!(stream_extension("https://example.com/live.ogg"), None);
        assert_eq!(stream_extension("https://example.com/radio"), None);
    }
}
