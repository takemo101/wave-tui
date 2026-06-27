use wave_tui::audio_spike::{
    normalize_value, parse_icy_title, resolve_stream_url, stream_extension,
};

#[test]
fn resolves_bare_stream_base_to_stream_mount() {
    assert_eq!(
        resolve_stream_url("https://example.com/radio"),
        "https://example.com/radio/stream"
    );
}

#[test]
fn preserves_direct_mp3_and_aac_urls() {
    assert_eq!(
        resolve_stream_url("https://example.com/live.mp3"),
        "https://example.com/live.mp3"
    );
    assert_eq!(
        resolve_stream_url("https://example.com/live.aac?token=1"),
        "https://example.com/live.aac?token=1"
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
}

#[test]
fn parses_icy_stream_title() {
    assert_eq!(
        parse_icy_title("StreamTitle='Artist - Track';StreamUrl='';"),
        Some("Artist - Track".to_string())
    );
}

#[test]
fn ignores_empty_icy_stream_title() {
    assert_eq!(parse_icy_title("StreamTitle='';StreamUrl='';"), None);
}

#[test]
fn normalizes_fft_values_into_unit_range() {
    assert_eq!(normalize_value(0.0, 3.0), 0.0);
    assert!(normalize_value(0.1, 3.0) > 0.0);
    assert_eq!(normalize_value(100.0, 3.0), 1.0);
}
