//! HTTP stream decoding (Symphonia).
//!
//! [`StreamDecoder`] fetches an HTTP(S) audio stream with `reqwest`, probes the
//! container with Symphonia, and yields interleaved `f32` samples one at a time
//! via [`Iterator`]. It performs no URL resolution: the caller passes an
//! already-authoritative stream URL (the audio spike showed that blindly
//! appending `/stream` breaks real Radio Browser mounts; see
//! `docs/audio-spike.md`). All failures are recoverable [`anyhow`] errors so the
//! runtime can turn them into `AudioEvent::Failed` rather than panicking.

use std::time::Duration;

use anyhow::{Context, Result};
use symphonia::{
    core::{
        audio::{AudioBufferRef, SampleBuffer},
        codecs::{Decoder, DecoderOptions},
        formats::{FormatOptions, FormatReader},
        io::{MediaSourceStream, ReadOnlySource},
        meta::MetadataOptions,
        probe::Hint,
    },
    default::{get_codecs, get_probe},
};

/// Pull-based decoder over a live HTTP audio stream.
///
/// Construct with [`StreamDecoder::new_http`], read [`sample_rate`] and
/// [`channels`] for output configuration, then iterate to drain interleaved
/// `f32` samples. The iterator ends (`None`) on end-of-stream, a network read
/// error, or an unrecoverable decode error; transient mid-stream issues are
/// surfaced on stderr and terminate the iterator rather than panicking.
///
/// [`sample_rate`]: StreamDecoder::sample_rate
/// [`channels`]: StreamDecoder::channels
pub(crate) struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_buf: Vec<f32>,
    convert_buf: Option<SampleBuffer<f32>>,
    sample_pos: usize,
    sample_rate: u32,
    channels: usize,
    /// Reason the stream ended, set when iteration stops abnormally (network
    /// read error / disconnect or an unrecoverable decode error). The runtime
    /// reads this to turn a mid-stream stop into an `AudioEvent::Failed`.
    last_error: Option<String>,
}

/// How long to wait for the initial TCP/TLS connect before failing.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-read timeout. For reqwest's *blocking* client `Client::timeout` is applied
/// to each individual socket read (not as a total request deadline), so this
/// bounds how long a single read may stall without killing an actively-flowing
/// stream. It turns a silently-wedged station into a recoverable failure and
/// bounds teardown latency: a blocked read returns instead of hanging forever.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

impl StreamDecoder {
    /// Open `url` as a live stream and prepare a Symphonia decoder for it.
    ///
    /// The URL is used verbatim (it must already be a direct stream URL). A
    /// non-success HTTP status, missing track, or unsupported codec is returned
    /// as an error.
    pub(crate) fn new_http(url: &str) -> Result<Self> {
        // Bounded connect and per-read timeouts. The blocking client applies
        // `timeout` per socket read, so this does not impose a total deadline on
        // the unbounded stream; it only bounds a single stalled read.
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(READ_TIMEOUT)
            .build()
            .context("failed to build http client")?;
        let resp = client
            .get(url)
            .send()
            .context("http get")?
            .error_for_status()
            .with_context(|| format!("stream request failed for {url}"))?;
        let final_url = resp.url().clone();
        let source = ReadOnlySource::new(resp);
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new();
        // Hint the probe with the container extension when the resolved URL has
        // one; default to MP3, the dominant format for these streams.
        if let Some(ext) = super::stream_extension(final_url.path()) {
            hint.with_extension(ext);
        } else {
            hint.with_extension("mp3");
        }

        let probed = get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?;
        let format = probed.format;
        let (track_id, sample_rate, channels, decoder) = {
            let track = format.default_track().context("no default track")?;
            let sample_rate = track.codec_params.sample_rate.context("no sample rate")?;
            let channels = track
                .codec_params
                .channels
                .context("no channel layout")?
                .count();
            let decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
            (track.id, sample_rate, channels, decoder)
        };

        Ok(Self {
            format,
            decoder,
            track_id,
            sample_buf: Vec::new(),
            convert_buf: None,
            sample_pos: 0,
            sample_rate,
            channels,
            last_error: None,
        })
    }

    /// Stream sample rate in Hz, as reported by the decoded track.
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Number of interleaved channels per frame.
    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    /// Take the reason iteration stopped, if it stopped abnormally.
    ///
    /// Returns `None` on a clean end (which, for a live radio stream, does not
    /// normally happen) and `Some(message)` after a network read error or an
    /// unrecoverable decode error.
    pub(crate) fn take_last_error(&mut self) -> Option<String> {
        self.last_error.take()
    }

    /// Decode the next packet into the internal interleaved sample buffer.
    ///
    /// Returns `Ok(false)` on end-of-stream / network read error (treated as a
    /// clean stop), `Ok(true)` when fresh samples were produced, and `Err` only
    /// for unrecoverable decode errors.
    fn refill(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(err)) => {
                    // A read error / disconnect / read-timeout ends the stream.
                    // For a live stream this is a failure, not a clean EOF.
                    self.last_error = Some(format!("stream read ended: {err}"));
                    return Ok(false);
                }
                Err(err) => return Err(err.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = self.decoder.decode(&packet)?;
            self.sample_buf.clear();
            self.sample_pos = 0;
            push_interleaved_samples(&mut self.sample_buf, &mut self.convert_buf, decoded)?;
            return Ok(true);
        }
    }
}

impl Iterator for StreamDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.sample_pos >= self.sample_buf.len() {
            match self.refill() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(err) => {
                    self.last_error = Some(format!("decode error: {err:#}"));
                    return None;
                }
            }
        }
        let sample = self.sample_buf.get(self.sample_pos).copied();
        self.sample_pos += 1;
        sample
    }
}

/// Append a decoded Symphonia buffer to `out` as interleaved `f32` samples,
/// reusing `convert_buf` across calls when it is large enough.
fn push_interleaved_samples(
    out: &mut Vec<f32>,
    convert_buf: &mut Option<SampleBuffer<f32>>,
    decoded: AudioBufferRef<'_>,
) -> Result<()> {
    let spec = *decoded.spec();
    let required_samples = decoded.frames() * spec.channels.count();
    let needs_new = convert_buf
        .as_ref()
        .map(|buf| buf.capacity() < required_samples)
        .unwrap_or(true);
    if needs_new {
        *convert_buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
    }
    let buf = convert_buf
        .as_mut()
        .context("sample conversion buffer missing")?;
    buf.clear();
    buf.copy_interleaved_ref(decoded);
    out.extend_from_slice(buf.samples());
    Ok(())
}
