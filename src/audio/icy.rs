//! ICY/Shoutcast metadata parsing.
//!
//! Only the deterministic `StreamTitle` field parser lives here; it is validated
//! by tests without any live stream. Splitting interleaved ICY metadata from
//! audio bytes (`icy-metaint` demuxing) belongs to the streaming runtime in a
//! later task (see `docs/audio-spike.md`).

/// Extract the `StreamTitle` value from an ICY metadata block.
///
/// ICY metadata looks like `StreamTitle='Artist - Track';StreamUrl='';`.
/// Returns `None` when the field is missing or its value is empty (after
/// trimming), so callers can treat "no current title" uniformly regardless of
/// whether the station omitted the field or sent an empty one.
pub(crate) fn parse_stream_title(metadata: &str) -> Option<String> {
    let marker = "StreamTitle='";
    let start = metadata.find(marker)? + marker.len();
    let rest = &metadata[start..];
    let end = rest.find("';")?;
    let title = rest[..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_stream_title() {
        assert_eq!(
            parse_stream_title("StreamTitle='Artist - Track';StreamUrl='';"),
            Some("Artist - Track".to_string())
        );
    }

    #[test]
    fn trims_surrounding_whitespace_in_title() {
        assert_eq!(
            parse_stream_title("StreamTitle='  Spaced Out  ';"),
            Some("Spaced Out".to_string())
        );
    }

    #[test]
    fn empty_title_is_none() {
        assert_eq!(parse_stream_title("StreamTitle='';StreamUrl='';"), None);
        assert_eq!(parse_stream_title("StreamTitle='   ';"), None);
    }

    #[test]
    fn missing_title_is_none() {
        assert_eq!(parse_stream_title("StreamUrl='https://example.com';"), None);
        assert_eq!(parse_stream_title(""), None);
        // Opening marker present but never terminated.
        assert_eq!(parse_stream_title("StreamTitle='unterminated"), None);
    }
}
