//! ICY/Shoutcast metadata parsing and demuxing.
//!
//! Two deterministic pieces live here, both validated without a live stream:
//!
//! - [`parse_stream_title`] extracts the `StreamTitle` field from a metadata
//!   block.
//! - [`IcyDemux`] is the `icy-metaint` demuxer state machine: it separates audio
//!   bytes from interleaved metadata blocks and reports `StreamTitle` changes.
//!
//! [`IcyReader`] adapts an arbitrary byte reader (the HTTP response) into an
//! audio-only [`Read`] by running [`IcyDemux`] and forwarding title changes to a
//! callback, so Symphonia never sees metadata bytes as audio.

use std::io::{self, Read};

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

/// Position within the `icy-metaint` framing.
#[derive(Debug)]
enum DemuxState {
    /// Passing through audio bytes; `usize` audio bytes remain before the next
    /// metadata length byte.
    Audio(usize),
    /// The next single byte is the metadata length (in 16-byte units).
    Length,
    /// Collecting a metadata block; `usize` bytes of it remain.
    Metadata(usize),
}

/// `icy-metaint` demuxer.
///
/// An ICY stream interleaves a metadata block after every `metaint` bytes of
/// audio: `metaint` audio bytes, one length byte (block size / 16), then that
/// many bytes of metadata (null-padded), repeating. [`push`](IcyDemux::push)
/// feeds raw stream bytes through this state machine, appends the audio bytes to
/// an output buffer, and returns any *changed* `StreamTitle`s so unchanged
/// repeats do not flood the caller.
#[derive(Debug)]
pub(crate) struct IcyDemux {
    metaint: usize,
    state: DemuxState,
    meta_buf: Vec<u8>,
    last_title: Option<String>,
}

impl IcyDemux {
    /// Create a demuxer for a stream whose `icy-metaint` is `metaint` bytes.
    pub(crate) fn new(metaint: usize) -> Self {
        Self {
            metaint,
            state: DemuxState::Audio(metaint),
            meta_buf: Vec::new(),
            last_title: None,
        }
    }

    /// Feed raw stream bytes, appending demuxed audio bytes to `audio_out` and
    /// returning any newly-changed `StreamTitle`s in order.
    pub(crate) fn push(&mut self, mut input: &[u8], audio_out: &mut Vec<u8>) -> Vec<String> {
        let mut titles = Vec::new();
        while !input.is_empty() {
            match self.state {
                DemuxState::Audio(remaining) => {
                    let take = remaining.min(input.len());
                    audio_out.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    let left = remaining - take;
                    self.state = if left == 0 {
                        DemuxState::Length
                    } else {
                        DemuxState::Audio(left)
                    };
                }
                DemuxState::Length => {
                    let len = input[0] as usize * 16;
                    input = &input[1..];
                    if len == 0 {
                        // No metadata this interval; resume audio immediately.
                        self.state = DemuxState::Audio(self.metaint);
                    } else {
                        self.meta_buf.clear();
                        self.state = DemuxState::Metadata(len);
                    }
                }
                DemuxState::Metadata(remaining) => {
                    let take = remaining.min(input.len());
                    self.meta_buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    let left = remaining - take;
                    if left == 0 {
                        if let Some(title) = self.finish_metadata() {
                            titles.push(title);
                        }
                        self.state = DemuxState::Audio(self.metaint);
                    } else {
                        self.state = DemuxState::Metadata(left);
                    }
                }
            }
        }
        titles
    }

    /// Parse the just-collected metadata block, returning the title only when it
    /// differs from the last one emitted (so unchanged repeats are dropped).
    fn finish_metadata(&mut self) -> Option<String> {
        // ICY metadata is text, null-padded to a 16-byte boundary.
        let text = String::from_utf8_lossy(&self.meta_buf);
        let title = parse_stream_title(text.trim_end_matches('\0'))?;
        if self.last_title.as_deref() == Some(title.as_str()) {
            None
        } else {
            self.last_title = Some(title.clone());
            Some(title)
        }
    }
}

/// A [`Read`] adapter that demuxes ICY metadata out of an inner reader.
///
/// It yields only audio bytes (so Symphonia never decodes metadata as audio) and
/// invokes `on_title` for each changed `StreamTitle`. The callback is the seam
/// the runtime uses to turn a title into an `AudioEvent`.
pub(crate) struct IcyReader<R> {
    inner: R,
    demux: IcyDemux,
    on_title: Box<dyn FnMut(String) + Send + Sync>,
    audio: Vec<u8>,
    audio_pos: usize,
    scratch: Vec<u8>,
}

/// Size of the per-read scratch buffer pulled from the inner reader.
const READ_CHUNK: usize = 8192;

impl<R: Read> IcyReader<R> {
    /// Wrap `inner`, demuxing a stream whose `icy-metaint` is `metaint` bytes and
    /// forwarding changed titles to `on_title`.
    pub(crate) fn new(
        inner: R,
        metaint: usize,
        on_title: Box<dyn FnMut(String) + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            demux: IcyDemux::new(metaint),
            on_title,
            audio: Vec::new(),
            audio_pos: 0,
            scratch: vec![0u8; READ_CHUNK],
        }
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            // Hand out any audio left over from a previous inner read first.
            if self.audio_pos < self.audio.len() {
                let n = (self.audio.len() - self.audio_pos).min(out.len());
                out[..n].copy_from_slice(&self.audio[self.audio_pos..self.audio_pos + n]);
                self.audio_pos += n;
                return Ok(n);
            }
            // Refill from the inner reader and demux. A chunk can be entirely
            // metadata, yielding no audio, so loop until we have audio or EOF
            // rather than reporting a premature end-of-stream.
            self.audio.clear();
            self.audio_pos = 0;
            let read = self.inner.read(&mut self.scratch)?;
            if read == 0 {
                return Ok(0);
            }
            for title in self.demux.push(&self.scratch[..read], &mut self.audio) {
                (self.on_title)(title);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

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

    /// Build one ICY metadata segment: a length byte (block size / 16) followed
    /// by the null-padded block, exactly as a Shoutcast server frames it.
    fn meta_segment(text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        let blocks = bytes.len().div_ceil(16);
        let mut out = vec![blocks as u8];
        out.extend_from_slice(bytes);
        out.resize(1 + blocks * 16, 0);
        out
    }

    /// Build a synthetic ICY stream: `metaint` audio bytes, a metadata segment,
    /// repeating for each `(audio_byte, title)` pair.
    fn icy_stream(metaint: usize, segments: &[(u8, &str)]) -> (Vec<u8>, Vec<u8>) {
        let mut stream = Vec::new();
        let mut audio = Vec::new();
        for (fill, title) in segments {
            let chunk = vec![*fill; metaint];
            stream.extend_from_slice(&chunk);
            audio.extend_from_slice(&chunk);
            stream.extend_from_slice(&meta_segment(&format!("StreamTitle='{title}';")));
        }
        (stream, audio)
    }

    #[test]
    fn demux_splits_audio_from_metadata_blocks() {
        let (stream, expected_audio) = icy_stream(8, &[(0xAA, "Song One"), (0xBB, "Song Two")]);
        let mut demux = IcyDemux::new(8);
        let mut audio = Vec::new();
        let titles = demux.push(&stream, &mut audio);

        assert_eq!(audio, expected_audio, "metadata bytes leaked into audio");
        assert_eq!(titles, vec!["Song One".to_string(), "Song Two".to_string()]);
    }

    #[test]
    fn demux_dedupes_unchanged_titles() {
        let (stream, _) = icy_stream(4, &[(1, "Same"), (1, "Same"), (1, "New")]);
        let mut demux = IcyDemux::new(4);
        let mut audio = Vec::new();
        let titles = demux.push(&stream, &mut audio);

        // The repeated "Same" block must not be re-emitted.
        assert_eq!(titles, vec!["Same".to_string(), "New".to_string()]);
    }

    #[test]
    fn demux_handles_zero_length_metadata_blocks() {
        // metaint advertised but each block is empty (length byte 0): all audio,
        // no titles.
        let metaint = 4;
        let mut stream = Vec::new();
        stream.extend_from_slice(&[1u8; 4]);
        stream.push(0); // zero-length metadata
        stream.extend_from_slice(&[2u8; 4]);
        stream.push(0);

        let mut demux = IcyDemux::new(metaint);
        let mut audio = Vec::new();
        let titles = demux.push(&stream, &mut audio);

        assert_eq!(audio, vec![1, 1, 1, 1, 2, 2, 2, 2]);
        assert!(titles.is_empty());
    }

    #[test]
    fn demux_is_chunk_boundary_independent() {
        let (stream, expected_audio) = icy_stream(8, &[(0xAA, "Song One"), (0xBB, "Song Two")]);
        let mut demux = IcyDemux::new(8);
        let mut audio = Vec::new();
        let mut titles = Vec::new();
        // Feed one byte at a time: framing must not depend on read boundaries.
        for byte in &stream {
            titles.extend(demux.push(&[*byte], &mut audio));
        }

        assert_eq!(audio, expected_audio);
        assert_eq!(titles, vec!["Song One".to_string(), "Song Two".to_string()]);
    }

    #[test]
    fn reader_strips_metadata_and_reports_title_changes() {
        let (stream, expected_audio) =
            icy_stream(8, &[(0xAA, "First"), (0xAA, "First"), (0xBB, "Second")]);
        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let mut reader = IcyReader::new(
            Cursor::new(stream),
            8,
            Box::new(move |title| sink.lock().unwrap().push(title)),
        );

        let mut audio = Vec::new();
        reader.read_to_end(&mut audio).unwrap();

        assert_eq!(audio, expected_audio, "metadata leaked into decoded audio");
        assert_eq!(
            *collected.lock().unwrap(),
            vec!["First".to_string(), "Second".to_string()]
        );
    }
}
