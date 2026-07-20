//! Native playback facade.
//!
//! This is the public entry point for the audio module. Decoder, output,
//! analyzer, and ICY details are kept private behind this facade so callers
//! depend on the facade rather than on CPAL/Symphonia/RustFFT specifics.
//!
//! The runtime ([`AudioRuntime::spawn`]) owns a background control thread that
//! turns [`AudioCommand`]s into playback and reports progress as
//! [`AudioEvent`]s, including [`AudioEvent::Viz`] visualizer frames. Callers
//! interact only through the returned [`AudioHandle`]'s channels and never touch
//! CPAL/Symphonia/RustFFT directly.
//!
//! Deterministic helpers validated by the native audio spike live here and in
//! the private submodules: stream-URL resolution policy (this file), FFT
//! normalization and log-band mapping ([`analyzer`]), and ICY `StreamTitle`
//! parsing ([`icy`]). See `docs/audio-spike.md`.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ringbuf::{
    traits::{Observer, Producer, Split},
    HeapRb,
};

use crate::model::{
    PlaybackRequestId, PlaybackRequestSeq, Station, StationId, VizFrame, VolumePercent,
};

use self::decoder::StreamDecoder;
use self::output::SharedVolume;
use self::played_sample::PlayedSample;

pub(crate) mod analyzer;
mod decoder;
pub(crate) mod icy;
mod output;
mod played_sample;

/// FFT window size used by the streaming analyzer. Matches the spike.
const FFT_SIZE: usize = 1024;
/// Number of visualizer bands emitted per [`VizFrame`].
const BAND_COUNT: usize = 16;
/// Visualizer frame cadence under normal operation.
const VIZ_INTERVAL: Duration = Duration::from_millis(120);
/// Slower visualizer cadence when [`AudioRuntimeConfig::low_power`] is set.
const VIZ_INTERVAL_LOW_POWER: Duration = Duration::from_millis(250);

/// A command sent to the audio runtime's control thread.
#[derive(Debug, Clone)]
pub enum AudioCommand {
    /// Stop any current playback and start `station` at `volume`.
    ///
    /// `request` identifies this play attempt. It is echoed on every
    /// station-scoped event the attempt produces, so the app can tell the
    /// current attempt's events apart from a superseded one's — including a
    /// replay of the same station.
    ///
    /// The station is boxed to keep the command enum small (a `Station` carries
    /// several owned fields), so cloning/queuing commands stays cheap.
    Play {
        request: PlaybackRequestId,
        station: Box<Station>,
        volume: VolumePercent,
    },
    /// Stop current playback.
    Stop,
    /// Change the live output volume without restarting playback.
    SetVolume(VolumePercent),
    /// Stop playback and shut the runtime down.
    Shutdown,
}

/// A progress event emitted by the audio runtime.
///
/// Event scoping is deliberately split in two:
///
/// - *Station-scoped* events (`Connecting`, `Playing`, `Failed`, `Viz`,
///   `IcyTitle`) carry the [`PlaybackRequestId`] of the [`AudioCommand::Play`]
///   that produced them. These are the events that can arrive late: `Failed`,
///   `Viz`, and `IcyTitle` are emitted from the decoder/analyzer/output worker
///   threads, which outlive the command that spawned them and may still be
///   draining while a newer request is already connecting. The app rejects any
///   of them whose request is not the currently expected one.
/// - *Global* events (`Stopped`, `VolumeChanged`) carry no request and are
///   applied unconditionally. Both are emitted only from the control thread, in
///   command order, on the single event channel — so they cannot overtake a
///   later request's events the way a worker thread's can. They also describe
///   runtime-wide state (nothing is playing; the live output volume) rather
///   than the fate of one attempt, so scoping them to a request would be wrong
///   even if they could race: a `Stop` issued between two plays must still stop
///   the UI, and volume persists across stations.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// Connecting to the station's stream (emitted before the network attempt).
    Connecting {
        request: PlaybackRequestId,
        station: StationId,
    },
    /// Playback has started for the station.
    Playing {
        request: PlaybackRequestId,
        station: StationId,
    },
    /// Playback stopped (in response to `Stop`). Global; see the type docs.
    Stopped,
    /// Playback could not start or failed; the station is recoverable-failed.
    Failed {
        request: PlaybackRequestId,
        station: StationId,
        message: String,
    },
    /// The live volume changed. Global; see the type docs.
    VolumeChanged(VolumePercent),
    /// A visualizer frame derived from the most recent played samples.
    Viz {
        request: PlaybackRequestId,
        frame: VizFrame,
    },
    /// A new ICY/Shoutcast `StreamTitle` was demuxed for `station`. Emitted only
    /// when the title changes, so the app is not flooded with repeats.
    IcyTitle {
        request: PlaybackRequestId,
        station: StationId,
        title: String,
    },
}

/// Configuration for [`AudioRuntime::spawn`].
#[derive(Debug, Clone, Default)]
pub struct AudioRuntimeConfig {
    /// Preferred output device name; `None` uses the host default device.
    pub output_device: Option<String>,
    /// Reduce visualizer cadence to lower CPU use.
    pub low_power: bool,
}

/// Handle to a running audio runtime: send [`AudioCommand`]s and receive
/// [`AudioEvent`]s. Dropping the handle (and thus `command_tx`) shuts the
/// runtime down.
///
/// The handle also owns the process's [`PlaybackRequestSeq`]. Allocation lives
/// on the controller side of the boundary — not inside the reducer and not on
/// the audio thread — so every play path (`Enter`, `Space`, Signal View,
/// startup auto-play) draws from one sequence and the app is only ever *told*
/// which request to expect.
pub struct AudioHandle {
    pub command_tx: Sender<AudioCommand>,
    pub event_rx: Receiver<AudioEvent>,
    requests: PlaybackRequestSeq,
}

impl AudioHandle {
    /// Build a handle around an existing command/event pair.
    pub fn new(command_tx: Sender<AudioCommand>, event_rx: Receiver<AudioEvent>) -> Self {
        Self {
            command_tx,
            event_rx,
            requests: PlaybackRequestSeq::new(),
        }
    }

    /// Allocate the id for the next play request.
    ///
    /// The caller must record it with the app (so the app expects that
    /// request's events) and send it on the matching [`AudioCommand::Play`].
    pub fn next_playback_request(&self) -> PlaybackRequestId {
        self.requests.next_id()
    }
}

/// The native audio runtime. Use [`AudioRuntime::spawn`] to start it.
pub struct AudioRuntime;

impl AudioRuntime {
    /// Spawn the control thread and return a handle to drive it.
    ///
    /// The thread lives until it receives [`AudioCommand::Shutdown`] or the
    /// command channel is closed (handle dropped).
    pub fn spawn(config: AudioRuntimeConfig) -> AudioHandle {
        let (command_tx, command_rx) = mpsc::channel::<AudioCommand>();
        let (event_tx, event_rx) = mpsc::channel::<AudioEvent>();
        thread::spawn(move || run_control_loop(config, command_rx, event_tx));
        AudioHandle::new(command_tx, event_rx)
    }
}

/// An active playback session: the CPAL stream plus its worker threads.
///
/// Held only on the control thread, so the non-`Send` CPAL stream never crosses
/// threads. Dropping it tears playback down: the stop flag is raised and both
/// worker threads are joined (the analyzer wakes within its recv timeout; the
/// decoder thread observes the flag and exits once the queue stops draining).
struct Playback {
    // `stream` is dropped after `Drop::drop` returns; declaration order keeps it
    // alive while the worker threads are joined.
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    decoder_thread: Option<JoinHandle<()>>,
    analyzer_thread: Option<JoinHandle<()>>,
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.decoder_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.analyzer_thread.take() {
            let _ = handle.join();
        }
    }
}

/// The control thread body: own current playback and the live volume, and
/// translate commands into playback actions and events.
fn run_control_loop(
    config: AudioRuntimeConfig,
    command_rx: Receiver<AudioCommand>,
    event_tx: Sender<AudioEvent>,
) {
    // Volume persists across stations; `Play` overrides it with its own value.
    let volume = SharedVolume::new(VolumePercent::clamped(100));
    let mut current: Option<Playback> = None;

    while let Ok(command) = command_rx.recv() {
        match command {
            AudioCommand::Play {
                request,
                station,
                volume: v,
            } => {
                // Stop the previous stream before announcing the new one. The
                // old session's workers may still emit events after this point;
                // they carry the old request and the app drops them.
                drop(current.take());
                let station_id = station.id.clone();
                let _ = event_tx.send(AudioEvent::Connecting {
                    request,
                    station: station_id.clone(),
                });
                volume.set(v);
                match start_playback(request, &station, &config, &volume, &event_tx) {
                    Ok(playback) => {
                        current = Some(playback);
                        let _ = event_tx.send(AudioEvent::Playing {
                            request,
                            station: station_id,
                        });
                    }
                    Err(err) => {
                        let _ = event_tx.send(AudioEvent::Failed {
                            request,
                            station: station_id,
                            message: format!("{err:#}"),
                        });
                    }
                }
            }
            AudioCommand::Stop => {
                drop(current.take());
                let _ = event_tx.send(AudioEvent::Stopped);
            }
            AudioCommand::SetVolume(v) => {
                volume.set(v);
                let _ = event_tx.send(AudioEvent::VolumeChanged(v));
            }
            AudioCommand::Shutdown => {
                drop(current.take());
                break;
            }
        }
    }
    // Dropping `current` on the way out tears down any active playback.
}

/// Start decoding, output, and analysis for `station`, returning a live
/// [`Playback`]. Any failure (network, device, unsupported rate, decode) is
/// returned as an error for the caller to surface as [`AudioEvent::Failed`].
fn start_playback(
    request: PlaybackRequestId,
    station: &Station,
    config: &AudioRuntimeConfig,
    volume: &SharedVolume,
    event_tx: &Sender<AudioEvent>,
) -> anyhow::Result<Playback> {
    // The station URL is treated as a direct, authoritative stream URL; mount
    // resolution (`/stream` for curated bases) is a catalog concern applied
    // upstream, per the audio spike findings.
    //
    // ICY titles are demuxed inside the decoder and surfaced as events tagged
    // with this request, so the app can ignore titles from an attempt it has
    // since left — including an earlier attempt at this same station.
    let icy_event_tx = event_tx.clone();
    let icy_station = station.id.clone();
    let on_title: Box<dyn FnMut(String) + Send + Sync> = Box::new(move |title| {
        let _ = icy_event_tx.send(AudioEvent::IcyTitle {
            request,
            station: icy_station.clone(),
            title,
        });
    });
    let decoder = StreamDecoder::new_http(station.url.as_str(), on_title)?;
    let sample_rate = decoder.sample_rate();
    let source_channels = decoder.channels();

    let device = output::select_output_device(config.output_device.as_deref())?;
    let output_config = output::choose_output_config(&device, sample_rate)?;

    let queue_capacity = sample_rate as usize * source_channels.max(1) * 2;
    let (queue_tx, queue_rx) = HeapRb::<f32>::new(queue_capacity).split();
    // Bounded mirror channel: a slow analyzer drops samples rather than stalling
    // the realtime output callback. One typed sample per played source frame.
    let (played_tx, played_rx) = mpsc::sync_channel::<PlayedSample>(sample_rate as usize / 2);
    let stop = Arc::new(AtomicBool::new(false));

    // Build the output stream (the last fallible step) *before* spawning any
    // worker threads, so an output-setup failure cannot leak threads. A CPAL
    // device error after this point is surfaced as a recoverable failure.
    let station_id = station.id.clone();
    let output_err_tx = event_tx.clone();
    let output_err_station = station_id.clone();
    let stream = output::build_output_stream(
        &device,
        output_config,
        queue_rx,
        source_channels,
        volume.clone(),
        played_tx,
        move |message| {
            let _ = output_err_tx.send(AudioEvent::Failed {
                request,
                station: output_err_station.clone(),
                message,
            });
        },
    )?;

    // From here on, threads are spawned; any later failure must tear them down.
    let decoder_stop = Arc::clone(&stop);
    let decoder_event_tx = event_tx.clone();
    let decoder_thread = thread::spawn(move || {
        pump_decoder(
            decoder,
            queue_tx,
            decoder_stop,
            decoder_event_tx,
            request,
            station_id,
        )
    });

    let analyzer_stop = Arc::clone(&stop);
    let viz_tx = event_tx.clone();
    let interval = if config.low_power {
        VIZ_INTERVAL_LOW_POWER
    } else {
        VIZ_INTERVAL
    };
    let analyzer_thread = thread::spawn(move || {
        analyzer::run_analyzer_loop(
            played_rx,
            sample_rate,
            BAND_COUNT,
            FFT_SIZE,
            interval,
            analyzer_stop,
            |frame| {
                let _ = viz_tx.send(AudioEvent::Viz { request, frame });
            },
        );
    });

    use cpal::traits::StreamTrait;
    if let Err(err) = stream.play() {
        // Tear the just-spawned workers down rather than leaking them.
        stop.store(true, Ordering::Relaxed);
        let _ = decoder_thread.join();
        let _ = analyzer_thread.join();
        return Err(anyhow::anyhow!("failed to start output stream: {err}"));
    }

    Ok(Playback {
        _stream: stream,
        stop,
        decoder_thread: Some(decoder_thread),
        analyzer_thread: Some(analyzer_thread),
    })
}

/// Drain the decoder into the output queue until the stream ends or `stop`.
///
/// Back-pressure: when the queue is full it briefly sleeps and retries, checking
/// `stop` so teardown is prompt even while the consumer is paused. If the stream
/// ends on its own (network read error / decode error) while `stop` is unset —
/// abnormal for a live radio stream — it emits [`AudioEvent::Failed`] so the
/// failure is surfaced instead of silently draining the output to silence.
fn pump_decoder(
    mut decoder: StreamDecoder,
    mut queue_tx: ringbuf::HeapProd<f32>,
    stop: Arc<AtomicBool>,
    event_tx: Sender<AudioEvent>,
    request: PlaybackRequestId,
    station: StationId,
) {
    // `by_ref` keeps `decoder` alive after the loop so we can read why it ended.
    for sample in decoder.by_ref() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        loop {
            match queue_tx.try_push(sample) {
                Ok(()) => break,
                Err(returned) => {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(2));
                    if queue_tx.vacant_len() > 0 {
                        let _ = queue_tx.try_push(returned);
                        break;
                    }
                }
            }
        }
    }

    // Only an unsolicited end is a failure; a stop request is a clean teardown.
    if !stop.load(Ordering::Relaxed) {
        let message = decoder
            .take_last_error()
            .unwrap_or_else(|| "stream ended unexpectedly".to_string());
        let _ = event_tx.send(AudioEvent::Failed {
            request,
            station,
            message,
        });
    }
}

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

    /// A handle with no runtime behind it: enough to exercise the
    /// controller-side request allocation without a device or network.
    fn detached_handle() -> AudioHandle {
        let (command_tx, _command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        AudioHandle::new(command_tx, event_rx)
    }

    #[test]
    fn the_handle_allocates_a_distinct_request_per_play() {
        let handle = detached_handle();
        let first = handle.next_playback_request();
        let second = handle.next_playback_request();
        let third = handle.next_playback_request();
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }

    #[test]
    fn station_scoped_events_are_distinguished_by_request_not_station() {
        // The whole point of MIK-065: two attempts at the *same* station are
        // different events, so the app can reject the older one.
        let handle = detached_handle();
        let station = crate::model::StationId::new("a").unwrap();
        let first = AudioEvent::Playing {
            request: handle.next_playback_request(),
            station: station.clone(),
        };
        let second = AudioEvent::Playing {
            request: handle.next_playback_request(),
            station,
        };
        assert_ne!(first, second);
    }

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
