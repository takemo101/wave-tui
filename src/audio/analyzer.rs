//! FFT analyzer: deterministic helpers that turn played samples into normalized
//! visualizer bands.
//!
//! These helpers were validated by the native audio spike (`docs/audio-spike.md`)
//! and are pure/deterministic so they can be tested without live audio or a real
//! output device. The streaming runtime that feeds samples into [`analyze`] is
//! implemented in a later task.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustfft::{num_complex::Complex, FftPlanner};

use crate::model::{PhaseTrace, VizFrame};

use super::played_sample::PlayedSample;

/// Visualizer gain applied before soft compression. Chosen during the spike so
/// typical playback magnitudes spread across the band range.
const DEFAULT_GAIN: f32 = 3.0;
/// Lowest band edge in Hz; below this is mostly rumble/DC for BGM use.
const MIN_BAND_HZ: f32 = 60.0;
/// Highest band edge in Hz, capped well below Nyquist for a calmer spectrum.
const MAX_BAND_HZ: f32 = 12_000.0;
/// Number of low-resolution time-domain points in each frame's waveform.
///
/// Renderers resample these to their pane width, so a small fixed count keeps
/// the waveform cheap while staying smooth enough for a scope display.
pub(crate) const WAVEFORM_POINTS: usize = 64;
/// Number of paired points sampled into each phase trace.
pub(crate) const PHASE_POINTS: usize = 128;
/// Sample lag pairing each point with a later mono sample when no stereo pair
/// exists (mono primary fallback). Non-zero so a mono stream still opens into a
/// Lissajous figure instead of collapsing onto the x = y diagonal; prime so the
/// lag does not lock onto common periodicities.
pub(crate) const PRIMARY_PHASE_LAG: usize = 29;
/// Distinct non-zero (and prime) sample lag for the secondary trace, so the two
/// traces always differ.
pub(crate) const SECONDARY_PHASE_LAG: usize = 97;

/// Soft-knee compressor mapping `[0, inf)` magnitudes toward `[0, 1)`.
fn soft_compress(x: f32) -> f32 {
    let k = 2.0;
    (k * x) / (1.0 + k * x)
}

/// Normalize a raw band magnitude into the `0.0..=1.0` visualizer range.
///
/// Applies `gain`, then soft compression, and clamps. Very large inputs
/// saturate to `1.0`. Magnitudes are non-negative in practice, but the input is
/// floored at `0.0` first so any spurious negative value maps to `0.0` (and
/// never hits the soft-compressor's pole), keeping output strictly in range.
pub(crate) fn normalize_value(x: f32, gain: f32) -> f32 {
    let amplified = (x * gain).max(0.0);
    if amplified >= 100.0 {
        1.0
    } else {
        soft_compress(amplified).clamp(0.0, 1.0)
    }
}

/// Downsample raw time-domain `samples` into exactly `points` normalized
/// waveform values for scope-style rendering.
///
/// Each output point is the mean of one contiguous bucket of the input, which
/// keeps the result bounded by the input magnitude and free of resampling
/// spikes. Output is clamped to `-1.0..=1.0` so callers can draw it directly.
/// Returns an empty vector for empty input or a zero point request.
pub(crate) fn waveform_points(samples: &[f32], points: usize) -> Vec<f32> {
    if points == 0 || samples.is_empty() {
        return Vec::new();
    }

    (0..points)
        .map(|i| {
            let start = i * samples.len() / points;
            let end = ((i + 1) * samples.len() / points).max(start + 1);
            let end = end.min(samples.len());
            let slice = &samples[start..end];
            let avg = slice.iter().copied().sum::<f32>() / slice.len() as f32;
            avg.clamp(-1.0, 1.0)
        })
        .collect()
}

/// Map `band_count` logarithmically spaced frequency bands to FFT bin ranges.
///
/// Each returned `(start, end)` is a half-open bin range into the lower half of
/// the spectrum. Ranges are non-empty (`end > start`) and clamped to `1..=n/2`.
pub(crate) fn log_bands(sample_rate: f32, n_fft: usize, band_count: usize) -> Vec<(usize, usize)> {
    let nyquist = sample_rate / 2.0;
    let max_hz = nyquist.min(MAX_BAND_HZ);
    let log_min = MIN_BAND_HZ.ln();
    let log_max = max_hz.ln();

    (0..band_count)
        .map(|i| {
            let t0 = i as f32 / band_count as f32;
            let t1 = (i + 1) as f32 / band_count as f32;
            let f0 = (log_min + (log_max - log_min) * t0).exp();
            let f1 = (log_min + (log_max - log_min) * t1).exp();
            let b0 = ((f0 / nyquist) * (n_fft as f32 / 2.0)).floor().max(1.0) as usize;
            let b1 = ((f1 / nyquist) * (n_fft as f32 / 2.0))
                .ceil()
                .max(b0 as f32 + 1.0) as usize;
            (b0, b1)
        })
        .collect()
}

/// Downsample the last analyzer window into a phase-portrait trace of paired
/// played samples.
///
/// Each point pairs values from the same source-frame timeline: for a stereo
/// window (`use_stereo`) the pair is the played left/right channels of one
/// frame; otherwise it pairs each frame's mono mix with the mono mix `lag`
/// frames later. The final `lag` frames are skipped rather than wrapped, so
/// every pair is chronological and no synthetic waveform is introduced. Returns
/// an empty trace when no pairs fit.
pub(crate) fn phase_trace(
    samples: &[PlayedSample],
    points: usize,
    lag: usize,
    use_stereo: bool,
) -> PhaseTrace {
    let usable = if use_stereo {
        samples.len()
    } else {
        samples.len().saturating_sub(lag)
    };
    if points == 0 || usable == 0 {
        return PhaseTrace::empty();
    }

    let count = points.min(usable);
    let mut x = Vec::with_capacity(count);
    let mut y = Vec::with_capacity(count);
    for i in 0..count {
        let idx = i * usable / count;
        let sample = samples[idx];
        if use_stereo {
            x.push(sample.left);
            y.push(sample.right);
        } else {
            x.push(sample.mono);
            y.push(samples[idx + lag].mono);
        }
    }
    PhaseTrace::new(x, y)
}

/// Analyze the most recent `n_fft` played samples into a normalized
/// [`VizFrame`].
///
/// Applies a Hann window to the mono mix, runs a forward FFT, averages
/// magnitudes into `band_count` log-spaced bands, normalizes each band into
/// `0.0..=1.0`, and pairs them with the windowed RMS, waveform, and two phase
/// traces built from the same played window ([`phase_trace`]). Returns a silent
/// frame when there are fewer than `n_fft` samples. The result is deterministic
/// for a given input.
pub(crate) fn analyze(
    samples: &[PlayedSample],
    sample_rate: u32,
    n_fft: usize,
    band_count: usize,
) -> VizFrame {
    if n_fft == 0 || samples.len() < n_fft {
        return VizFrame::silent(band_count);
    }

    let window_samples = &samples[samples.len() - n_fft..];
    let frame: Vec<f32> = window_samples.iter().map(|sample| sample.mono).collect();
    let mut buffer: Vec<Complex<f32>> = frame
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let window =
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n_fft as f32 - 1.0)).cos();
            Complex::new(sample * window, 0.0)
        })
        .collect();

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n_fft);
    fft.process(&mut buffer);

    let mags: Vec<f32> = buffer
        .iter()
        .take(n_fft / 2)
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .collect();

    let bands = log_bands(sample_rate as f32, n_fft, band_count)
        .into_iter()
        .map(|(b0, b1)| {
            let start = b0.min(mags.len());
            let end = b1.min(mags.len());
            if end <= start {
                0.0
            } else {
                let avg = mags[start..end].iter().copied().sum::<f32>() / (end - start) as f32;
                normalize_value(avg, DEFAULT_GAIN)
            }
        });

    let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
    let rms = (sum_sq / n_fft as f32).sqrt();

    // Derive the waveform from the raw (un-windowed) samples so the scope shows
    // the actual signal shape rather than the FFT's tapered window.
    let waveform = waveform_points(&frame, WAVEFORM_POINTS);

    // The primary trace prefers the played stereo pair; a mono window falls
    // back to lagged mono pairs. The secondary trace always uses a distinct
    // lagged mono pairing so the two traces differ for every source.
    let use_stereo = window_samples.iter().all(|sample| sample.is_stereo);
    let primary_phase = phase_trace(window_samples, PHASE_POINTS, PRIMARY_PHASE_LAG, use_stereo);
    let secondary_phase = phase_trace(window_samples, PHASE_POINTS, SECONDARY_PHASE_LAG, false);

    VizFrame::with_phase(bands, rms, waveform, primary_phase, secondary_phase)
}

/// Append `sample` to `history`, keeping only the most recent `n_fft * 4`
/// samples so the working set stays bounded during long playback.
fn push_history(history: &mut VecDeque<PlayedSample>, sample: PlayedSample, n_fft: usize) {
    history.push_back(sample);
    while history.len() > n_fft * 4 {
        history.pop_front();
    }
}

/// Consume mirrored played samples from `rx` and emit a normalized [`VizFrame`]
/// at most once per `interval`, until `stop` is set or the sender is dropped.
///
/// This is the streaming bridge between the realtime output callback (which
/// mirrors typed played samples into `rx`) and the visualizer: it keeps a
/// rolling history, runs [`analyze`] over the newest `n_fft` samples on a
/// cadence, and hands each frame to `on_frame`. Keeping the callback a plain
/// `FnMut` keeps this module independent of the runtime's event type. A final
/// frame is emitted when the stream disconnects so the visualizer reflects the
/// last audio played.
pub(crate) fn run_analyzer_loop(
    rx: Receiver<PlayedSample>,
    sample_rate: u32,
    band_count: usize,
    n_fft: usize,
    interval: Duration,
    stop: Arc<AtomicBool>,
    mut on_frame: impl FnMut(VizFrame),
) {
    let mut history: VecDeque<PlayedSample> = VecDeque::with_capacity(n_fft * 4);
    // Reused window buffer, pre-filled with silent frames until history covers it.
    let mut buffer = vec![
        PlayedSample {
            mono: 0.0,
            left: 0.0,
            right: 0.0,
            is_stereo: false,
        };
        n_fft
    ];
    let mut emit = |history: &VecDeque<PlayedSample>, buffer: &mut [PlayedSample]| {
        if history.len() < n_fft {
            return;
        }
        let start = history.len() - n_fft;
        for (dst, src) in buffer.iter_mut().zip(history.iter().skip(start)) {
            *dst = *src;
        }
        on_frame(analyze(buffer, sample_rate, n_fft, band_count));
    };

    let mut last_emit = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(sample) => push_history(&mut history, sample, n_fft),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                emit(&history, &mut buffer);
                break;
            }
        }
        while let Ok(sample) = rx.try_recv() {
            push_history(&mut history, sample, n_fft);
        }
        if history.len() >= n_fft && last_emit.elapsed() >= interval {
            emit(&history, &mut buffer);
            last_emit = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use super::*;

    /// Deterministic sine wave for FFT tests; no RNG, no live audio.
    fn sine(freq_hz: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect()
    }

    /// Typed mono played-sample window derived from a deterministic sine.
    fn mono_sine_frames(freq_hz: f32, sample_rate: u32, len: usize) -> Vec<PlayedSample> {
        sine(freq_hz, sample_rate, len)
            .into_iter()
            .map(|value| PlayedSample::from_source_frame(&[value]).unwrap())
            .collect()
    }

    /// Typed stereo played-sample window with distinct left/right sines.
    fn stereo_sine_frames(
        left_hz: f32,
        right_hz: f32,
        sample_rate: u32,
        len: usize,
    ) -> Vec<PlayedSample> {
        let left = sine(left_hz, sample_rate, len);
        let right = sine(right_hz, sample_rate, len);
        left.into_iter()
            .zip(right)
            .map(|(l, r)| PlayedSample::from_source_frame(&[l, r]).unwrap())
            .collect()
    }

    #[test]
    fn analyze_preserves_stereo_primary_phase_and_derives_a_second_trace() {
        let samples = stereo_sine_frames(440.0, 660.0, 44_100, 1_024);
        let frame = analyze(&samples, 44_100, 1_024, 16);
        assert!(!frame.primary_phase.x.is_empty());
        assert_ne!(frame.primary_phase.x, frame.primary_phase.y);
        assert_ne!(frame.primary_phase, frame.secondary_phase);
    }

    #[test]
    fn analyze_uses_distinct_lags_for_mono_phase_traces() {
        let samples = mono_sine_frames(440.0, 44_100, 1_024);
        let frame = analyze(&samples, 44_100, 1_024, 16);
        assert!(!frame.primary_phase.x.is_empty());
        assert_ne!(frame.primary_phase.x, frame.primary_phase.y);
        assert_ne!(frame.primary_phase, frame.secondary_phase);
    }

    #[test]
    fn normalize_value_stays_in_unit_range() {
        assert_eq!(normalize_value(0.0, 3.0), 0.0);
        assert!(normalize_value(0.1, 3.0) > 0.0);
        assert!(normalize_value(0.1, 3.0) < 1.0);
        assert_eq!(normalize_value(100.0, 3.0), 1.0);
        assert_eq!(normalize_value(f32::MAX, 3.0), 1.0);
        // Negative/garbage magnitudes never produce out-of-range output.
        assert_eq!(normalize_value(-5.0, 3.0), 0.0);
    }

    #[test]
    fn log_bands_are_ordered_non_empty_and_bounded() {
        let n_fft = 1024;
        let bands = log_bands(44_100.0, n_fft, 16);
        assert_eq!(bands.len(), 16);
        for (b0, b1) in &bands {
            assert!(b1 > b0, "band range must be non-empty: {b0}..{b1}");
            assert!(*b0 >= 1);
            assert!(*b1 <= n_fft / 2 + 1);
        }
        // Lower bands should start below higher bands.
        assert!(bands[0].0 <= bands[15].0);
    }

    #[test]
    fn analyze_keeps_every_band_and_rms_in_unit_range() {
        let n_fft = 1024;
        let band_count = 16;
        let samples = mono_sine_frames(440.0, 44_100, n_fft * 2);
        let frame = analyze(&samples, 44_100, n_fft, band_count);

        assert_eq!(frame.bands.len(), band_count);
        for band in &frame.bands {
            assert!((0.0..=1.0).contains(band), "band out of range: {band}");
        }
        assert!(
            (0.0..=1.0).contains(&frame.rms),
            "rms out of range: {}",
            frame.rms
        );
        // A loud tone should light up at least one band.
        assert!(frame.bands.iter().any(|b| *b > 0.0));
    }

    #[test]
    fn analyze_returns_silent_frame_without_enough_samples() {
        let samples = mono_sine_frames(440.0, 44_100, 3);
        let frame = analyze(&samples, 44_100, 1024, 16);
        assert_eq!(frame.bands, vec![0.0; 16]);
        assert_eq!(frame.rms, 0.0);
        assert!(frame.waveform.is_empty());
        assert!(frame.primary_phase.x.is_empty());
        assert!(frame.secondary_phase.x.is_empty());
    }

    #[test]
    fn analyze_silence_is_all_zero_bands() {
        let n_fft = 256;
        let samples: Vec<PlayedSample> =
            vec![PlayedSample::from_source_frame(&[0.0]).unwrap(); n_fft];
        let frame = analyze(&samples, 44_100, n_fft, 8);
        assert_eq!(frame.bands, vec![0.0; 8]);
        assert_eq!(frame.rms, 0.0);
        // Silent audio yields a flat, zeroed waveform — stable silence.
        assert_eq!(frame.waveform, vec![0.0; WAVEFORM_POINTS]);
        // Phase traces of silence are still: every pair sits at the center.
        assert!(frame.primary_phase.x.iter().all(|value| *value == 0.0));
        assert!(frame.primary_phase.y.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn waveform_points_downsamples_and_stays_in_range() {
        let samples = sine(440.0, 44_100, 1024);
        let points = waveform_points(&samples, 64);
        assert_eq!(points.len(), 64);
        for p in &points {
            assert!((-1.0..=1.0).contains(p), "waveform point out of range: {p}");
        }
        // A real tone must produce both positive and negative excursions.
        assert!(points.iter().any(|p| *p > 0.0));
        assert!(points.iter().any(|p| *p < 0.0));
    }

    #[test]
    fn waveform_points_clamps_out_of_range_input() {
        let points = waveform_points(&[-2.0, 2.0, -3.0, 3.0], 4);
        assert_eq!(points, vec![-1.0, 1.0, -1.0, 1.0]);
    }

    #[test]
    fn waveform_points_handles_degenerate_input() {
        assert!(waveform_points(&[], 64).is_empty());
        assert!(waveform_points(&[0.1, 0.2], 0).is_empty());
    }

    #[test]
    fn analyze_emits_waveform_from_real_samples() {
        let n_fft = 1024;
        let samples = mono_sine_frames(440.0, 44_100, n_fft * 2);
        let frame = analyze(&samples, 44_100, n_fft, 16);
        assert_eq!(frame.waveform.len(), WAVEFORM_POINTS);
        for p in &frame.waveform {
            assert!((-1.0..=1.0).contains(p), "waveform point out of range: {p}");
        }
        assert!(frame.waveform.iter().any(|p| *p != 0.0));
    }

    #[test]
    fn analyzer_loop_emits_a_frame_then_stops_on_disconnect() {
        // Drive the streaming loop without any real audio device: pre-fill a
        // channel with > n_fft samples, drop the sender, and run the loop in
        // this thread. With a zero interval it emits as soon as it has enough
        // history, then exits when the channel disconnects.
        let n_fft = 256;
        let band_count = 8;
        let (tx, rx) = sync_channel::<PlayedSample>(2048);
        for sample in mono_sine_frames(440.0, 44_100, n_fft * 2) {
            tx.try_send(sample).unwrap();
        }
        drop(tx);

        let stop = Arc::new(AtomicBool::new(false));
        let mut frames = Vec::new();
        run_analyzer_loop(
            rx,
            44_100,
            band_count,
            n_fft,
            Duration::ZERO,
            stop,
            |frame| frames.push(frame),
        );

        assert!(!frames.is_empty(), "expected at least one visualizer frame");
        assert_eq!(frames[0].bands.len(), band_count);
        assert!(frames[0].bands.iter().any(|b| *b > 0.0));
    }

    #[test]
    fn analyzer_loop_exits_immediately_when_stop_is_set() {
        // A pre-set stop flag must short-circuit the loop with no frames, even
        // when the channel still has a live sender.
        let (tx, rx) = sync_channel::<PlayedSample>(8);
        tx.try_send(PlayedSample::from_source_frame(&[0.1]).unwrap())
            .unwrap();
        let stop = Arc::new(AtomicBool::new(true));
        let mut frames = Vec::new();
        run_analyzer_loop(rx, 44_100, 8, 256, Duration::ZERO, stop, |frame| {
            frames.push(frame)
        });
        assert!(frames.is_empty());
    }
}
