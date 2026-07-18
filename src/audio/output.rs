//! CPAL output stream, device/config selection, and live volume control.
//!
//! This module owns the CPAL boundary: selecting an output device and a config
//! that matches the stream sample rate, building the typed output callback, and
//! applying a shared, lock-free volume. The callback also mirrors each played
//! source frame into a channel as a typed [`PlayedSample`] so the analyzer can
//! compute visualizer frames and phase traces.
//!
//! Unsupported sample rates are an explicit, recoverable error: the MVP does not
//! resample, so [`choose_output_config`] fails when the device cannot output the
//! stream's rate rather than silently retiming audio (see `docs/audio-spike.md`).

use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc, Arc,
};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use ringbuf::traits::{Consumer, Observer};

use crate::model::VolumePercent;

use super::played_sample::PlayedSample;

/// Lock-free playback volume shared between the control thread and the realtime
/// output callback.
///
/// The gain (`0.0..=1.0`) is stored as the bit pattern of an `f32` in an atomic
/// so [`SharedVolume::set`] can update it from the control thread while the
/// audio callback reads it per frame via [`SharedVolume::gain`] without locking.
#[derive(Clone)]
pub(crate) struct SharedVolume {
    gain_bits: Arc<AtomicU32>,
}

impl SharedVolume {
    /// Create a control initialized to `volume`.
    pub(crate) fn new(volume: VolumePercent) -> Self {
        let control = Self {
            gain_bits: Arc::new(AtomicU32::new(0)),
        };
        control.set(volume);
        control
    }

    /// Update the live gain. Takes effect on the next audio callback frame
    /// without restarting the stream.
    pub(crate) fn set(&self, volume: VolumePercent) {
        let gain = volume.get() as f32 / VolumePercent::MAX as f32;
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Current linear gain in `0.0..=1.0`.
    pub(crate) fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }
}

/// Select an output device by name, or the host default when `name` is `None`.
///
/// A requested-but-missing device is a recoverable error rather than a silent
/// fallback, so the runtime can report exactly what went wrong.
pub(crate) fn select_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        Some(want) => {
            let mut devices = host
                .output_devices()
                .context("failed to enumerate output devices")?;
            devices
                .find(|device| device.name().map(|n| n == want).unwrap_or(false))
                .with_context(|| format!("requested output device not found: {want}"))
        }
        None => host
            .default_output_device()
            .context("no default output device"),
    }
}

/// Choose an output config whose sample rate matches the stream's `sample_rate`.
///
/// Prefers `F32` then 16-bit formats and the fewest channels. Because the MVP
/// does not resample, this returns a clear error when no config covers the
/// stream rate instead of picking a mismatched rate.
pub(crate) fn choose_output_config(
    device: &cpal::Device,
    sample_rate: u32,
) -> Result<cpal::SupportedStreamConfig> {
    let supported: Vec<_> = device
        .supported_output_configs()
        .context("failed to inspect output configs")?
        .collect();
    supported
        .iter()
        .filter(|config| {
            config.min_sample_rate().0 <= sample_rate && sample_rate <= config.max_sample_rate().0
        })
        .min_by_key(|config| {
            let format_rank = match config.sample_format() {
                cpal::SampleFormat::F32 => 0,
                cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => 1,
                _ => 2,
            };
            (format_rank, config.channels())
        })
        .map(|config| config.with_sample_rate(cpal::SampleRate(sample_rate)))
        .with_context(|| {
            format!(
                "output device does not support stream sample rate {sample_rate} Hz \
                 (resampling is not implemented in the MVP)"
            )
        })
}

/// Build and return the CPAL output stream feeding from `queue_rx`.
///
/// Each output frame pulls one source frame from the queue, mirrors it to
/// `played_tx` as a pre-volume [`PlayedSample`] for analysis, then writes the
/// volume-scaled samples to the device. The returned stream is paused until
/// [`cpal::traits::StreamTrait::play`] is called.
///
/// `on_error` is invoked (off the realtime thread, by CPAL) with a human-readable
/// message when the device reports a stream error, so the caller can surface it
/// as a recoverable failure instead of losing it. Keeping it a plain `FnMut`
/// rather than an event type keeps this module independent of the runtime facade.
pub(crate) fn build_output_stream(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    queue_rx: ringbuf::HeapCons<f32>,
    source_channels: usize,
    volume: SharedVolume,
    played_tx: mpsc::SyncSender<PlayedSample>,
    mut on_error: impl FnMut(String) + Send + 'static,
) -> Result<cpal::Stream> {
    let stream_config = config.config();
    let output_channels = config.channels() as usize;
    let err_fn = move |err: cpal::StreamError| on_error(format!("output stream error: {err}"));
    match config.sample_format() {
        cpal::SampleFormat::F32 => build_typed_output_stream::<f32>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            volume,
            played_tx,
            err_fn,
        ),
        cpal::SampleFormat::I16 => build_typed_output_stream::<i16>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            volume,
            played_tx,
            err_fn,
        ),
        cpal::SampleFormat::U16 => build_typed_output_stream::<u16>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            volume,
            played_tx,
            err_fn,
        ),
        other => anyhow::bail!("unsupported output sample format: {other:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut queue_rx: ringbuf::HeapCons<f32>,
    source_channels: usize,
    output_channels: usize,
    volume: SharedVolume,
    played_tx: mpsc::SyncSender<PlayedSample>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut source_frame = vec![0.0; source_channels.max(1)];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let gain = volume.gain();
            for frame in data.chunks_mut(output_channels) {
                let has_frame = queue_rx.occupied_len() >= source_channels;
                if has_frame {
                    for slot in &mut source_frame {
                        *slot = queue_rx.try_pop().unwrap_or(0.0);
                    }
                } else {
                    source_frame.fill(0.0);
                }

                // Mirror the pre-volume played frame so the visualizer reflects
                // the stream content regardless of listening level.
                if has_frame {
                    if let Some(sample) = PlayedSample::from_source_frame(&source_frame) {
                        let _ = played_tx.try_send(sample);
                    }
                }

                for (idx, out) in frame.iter_mut().enumerate() {
                    let sample = map_output_sample(&source_frame, idx, output_channels) * gain;
                    *out = T::from_sample(sample);
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Map a decoded source frame onto output channel `output_idx`, up/down-mixing
/// between common channel layouts (mono, stereo, and matched counts).
fn map_output_sample(source_frame: &[f32], output_idx: usize, output_channels: usize) -> f32 {
    match (source_frame.len(), output_channels) {
        (_, 0) | (0, _) => 0.0,
        (1, _) => source_frame[0],
        (2, 1) => (source_frame[0] + source_frame[1]) * 0.5,
        (2, _) => source_frame[output_idx % 2],
        (src, n) if src == n => source_frame[output_idx],
        (_, 1) => mix_for_analyzer(source_frame),
        (src, _) if src > output_channels => source_frame[output_idx],
        (src, _) if output_idx < src => source_frame[output_idx],
        _ => *source_frame.last().unwrap_or(&0.0),
    }
}

/// Average a source frame to a single mono sample for the analyzer mirror.
fn mix_for_analyzer(source_frame: &[f32]) -> f32 {
    if source_frame.is_empty() {
        0.0
    } else {
        source_frame.iter().copied().sum::<f32>() / source_frame.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_volume_maps_percent_to_unit_gain() {
        let volume = SharedVolume::new(VolumePercent::new(100).unwrap());
        assert_eq!(volume.gain(), 1.0);
        volume.set(VolumePercent::new(0).unwrap());
        assert_eq!(volume.gain(), 0.0);
        volume.set(VolumePercent::new(50).unwrap());
        assert!((volume.gain() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shared_volume_is_shared_across_clones() {
        let a = SharedVolume::new(VolumePercent::new(100).unwrap());
        let b = a.clone();
        a.set(VolumePercent::new(25).unwrap());
        // The clone observes the update: both point at the same atomic.
        assert!((b.gain() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn mono_source_feeds_every_output_channel() {
        let frame = [0.7];
        assert_eq!(map_output_sample(&frame, 0, 2), 0.7);
        assert_eq!(map_output_sample(&frame, 1, 2), 0.7);
    }

    #[test]
    fn stereo_to_mono_averages_channels() {
        let frame = [1.0, 0.0];
        assert_eq!(map_output_sample(&frame, 0, 1), 0.5);
        assert_eq!(mix_for_analyzer(&frame), 0.5);
    }

    #[test]
    fn stereo_to_stereo_passes_channels_through() {
        let frame = [0.2, -0.4];
        assert_eq!(map_output_sample(&frame, 0, 2), 0.2);
        assert_eq!(map_output_sample(&frame, 1, 2), -0.4);
    }

    #[test]
    fn empty_or_zero_channel_layouts_are_silent() {
        assert_eq!(map_output_sample(&[], 0, 2), 0.0);
        assert_eq!(map_output_sample(&[0.5], 0, 0), 0.0);
        assert_eq!(mix_for_analyzer(&[]), 0.0);
    }

    #[test]
    fn played_sample_keeps_stereo_channels_and_pre_volume_mono_mix() {
        let sample = PlayedSample::from_source_frame(&[0.8, -0.2]).unwrap();
        assert_eq!(sample.left, 0.8);
        assert_eq!(sample.right, -0.2);
        assert!((sample.mono - 0.3).abs() < f32::EPSILON);
        assert!(sample.is_stereo);
    }

    #[test]
    fn played_sample_mono_duplicates_the_channel_for_analyzer_fallback() {
        let sample = PlayedSample::from_source_frame(&[0.4]).unwrap();
        assert_eq!(sample.left, 0.4);
        assert_eq!(sample.right, 0.4);
        assert!(!sample.is_stereo);
    }

    #[test]
    fn played_sample_rejects_empty_frames_and_clamps_hot_samples() {
        assert_eq!(PlayedSample::from_source_frame(&[]), None);
        let sample = PlayedSample::from_source_frame(&[2.0, -3.0]).unwrap();
        assert_eq!(sample.left, 1.0);
        assert_eq!(sample.right, -1.0);
        assert!((-1.0..=1.0).contains(&sample.mono));
    }
}
