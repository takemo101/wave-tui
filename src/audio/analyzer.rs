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

use crate::model::VizFrame;

/// Visualizer gain applied before soft compression. Chosen during the spike so
/// typical playback magnitudes spread across the band range.
const DEFAULT_GAIN: f32 = 3.0;
/// Lowest band edge in Hz; below this is mostly rumble/DC for BGM use.
const MIN_BAND_HZ: f32 = 60.0;
/// Highest band edge in Hz, capped well below Nyquist for a calmer spectrum.
const MAX_BAND_HZ: f32 = 12_000.0;

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

/// Analyze the most recent `n_fft` samples into a normalized [`VizFrame`].
///
/// Applies a Hann window, runs a forward FFT, averages magnitudes into
/// `band_count` log-spaced bands, normalizes each band into `0.0..=1.0`, and
/// pairs them with the windowed RMS. Returns a silent frame when there are
/// fewer than `n_fft` samples. The result is deterministic for a given input.
pub(crate) fn analyze(
    samples: &[f32],
    sample_rate: u32,
    n_fft: usize,
    band_count: usize,
) -> VizFrame {
    if n_fft == 0 || samples.len() < n_fft {
        return VizFrame::silent(band_count);
    }

    let frame = &samples[samples.len() - n_fft..];
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

    VizFrame::new(bands, rms)
}

/// Append `sample` to `history`, keeping only the most recent `n_fft * 4`
/// samples so the working set stays bounded during long playback.
fn push_history(history: &mut VecDeque<f32>, sample: f32, n_fft: usize) {
    history.push_back(sample);
    while history.len() > n_fft * 4 {
        history.pop_front();
    }
}

/// Consume mirrored played samples from `rx` and emit a normalized [`VizFrame`]
/// at most once per `interval`, until `stop` is set or the sender is dropped.
///
/// This is the streaming bridge between the realtime output callback (which
/// mirrors mono samples into `rx`) and the visualizer: it keeps a rolling
/// history, runs [`analyze`] over the newest `n_fft` samples on a cadence, and
/// hands each frame to `on_frame`. Keeping the callback a plain `FnMut` keeps
/// this module independent of the runtime's event type. A final frame is emitted
/// when the stream disconnects so the visualizer reflects the last audio played.
pub(crate) fn run_analyzer_loop(
    rx: Receiver<f32>,
    sample_rate: u32,
    band_count: usize,
    n_fft: usize,
    interval: Duration,
    stop: Arc<AtomicBool>,
    mut on_frame: impl FnMut(VizFrame),
) {
    let mut history: VecDeque<f32> = VecDeque::with_capacity(n_fft * 4);
    let mut buffer = vec![0.0_f32; n_fft];
    let mut emit = |history: &VecDeque<f32>, buffer: &mut [f32]| {
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
        let samples = sine(440.0, 44_100, n_fft * 2);
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
        let frame = analyze(&[0.1, 0.2, 0.3], 44_100, 1024, 16);
        assert_eq!(frame.bands, vec![0.0; 16]);
        assert_eq!(frame.rms, 0.0);
    }

    #[test]
    fn analyze_silence_is_all_zero_bands() {
        let n_fft = 256;
        let frame = analyze(&vec![0.0; n_fft], 44_100, n_fft, 8);
        assert_eq!(frame.bands, vec![0.0; 8]);
        assert_eq!(frame.rms, 0.0);
    }

    #[test]
    fn analyzer_loop_emits_a_frame_then_stops_on_disconnect() {
        // Drive the streaming loop without any real audio device: pre-fill a
        // channel with > n_fft samples, drop the sender, and run the loop in
        // this thread. With a zero interval it emits as soon as it has enough
        // history, then exits when the channel disconnects.
        let n_fft = 256;
        let band_count = 8;
        let (tx, rx) = sync_channel::<f32>(2048);
        for sample in sine(440.0, 44_100, n_fft * 2) {
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
        let (tx, rx) = sync_channel::<f32>(8);
        tx.try_send(0.1).unwrap();
        let stop = Arc::new(AtomicBool::new(true));
        let mut frames = Vec::new();
        run_analyzer_loop(rx, 44_100, 8, 256, Duration::ZERO, stop, |frame| {
            frames.push(frame)
        });
        assert!(frames.is_empty());
    }
}
