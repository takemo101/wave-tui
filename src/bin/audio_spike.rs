use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};
use rustfft::{num_complex::Complex, FftPlanner};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
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

const DEFAULT_STREAM: &str = "https://dancewave.online/dance.mp3";
const FFT_SIZE: usize = 1024;
const BAND_COUNT: usize = 16;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (url, seconds) = match args.as_slice() {
        [] => (DEFAULT_STREAM.to_string(), 8),
        [only] => match only.parse::<u64>() {
            Ok(seconds) => (DEFAULT_STREAM.to_string(), seconds),
            Err(_) => (only.to_string(), 8),
        },
        [url, seconds, ..] => (url.to_string(), seconds.parse::<u64>().unwrap_or(8)),
    };

    println!("audio spike: url={url}");
    println!("audio spike: duration={seconds}s");

    run(&url, Duration::from_secs(seconds))
}

fn run(url: &str, duration: Duration) -> Result<()> {
    let decoder = StreamDecoder::new_http(url)?;
    let sample_rate = decoder.sample_rate();
    let source_channels = decoder.channels();
    println!("decoded stream: {sample_rate} Hz, {source_channels} channel(s)");

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device")?;
    let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    let config = choose_output_config(&device, sample_rate)?;
    let output_channels = config.channels() as usize;
    let output_sample_rate = config.sample_rate().0;
    println!(
        "output device: {device_name}; {output_sample_rate} Hz, {output_channels} channel(s), {:?}",
        config.sample_format()
    );

    if output_sample_rate != sample_rate {
        anyhow::bail!(
            "spike does not resample yet: stream is {sample_rate} Hz but output selected {output_sample_rate} Hz"
        );
    }

    let queue_capacity = sample_rate as usize * source_channels * 2;
    let (mut queue_tx, queue_rx) = HeapRb::<f32>::new(queue_capacity).split();
    let (played_tx, played_rx) = mpsc::sync_channel::<f32>(sample_rate as usize / 2);
    let stop = Arc::new(AtomicBool::new(false));

    let decoder_stop = Arc::clone(&stop);
    let decoder_thread = thread::spawn(move || {
        for sample in decoder {
            if decoder_stop.load(Ordering::Relaxed) {
                break;
            }
            loop {
                match queue_tx.try_push(sample) {
                    Ok(()) => break,
                    Err(returned) => {
                        if decoder_stop.load(Ordering::Relaxed) {
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
    });

    let analyzer_stop = Arc::clone(&stop);
    let analyzer_thread =
        thread::spawn(move || analyzer_loop(played_rx, sample_rate, analyzer_stop));

    let stream = build_output_stream(
        &device,
        config,
        queue_rx,
        source_channels,
        output_channels,
        played_tx,
    )?;
    stream
        .play()
        .context("failed to start CPAL output stream")?;

    let started = Instant::now();
    while started.elapsed() < duration {
        thread::sleep(Duration::from_millis(100));
    }

    stop.store(true, Ordering::Relaxed);
    drop(stream);
    let _ = decoder_thread.join();
    let _ = analyzer_thread.join();
    println!("audio spike: complete");
    Ok(())
}

struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_buf: Vec<f32>,
    convert_buf: Option<SampleBuffer<f32>>,
    sample_pos: usize,
    sample_rate: u32,
    channels: usize,
}

impl StreamDecoder {
    fn new_http(url: &str) -> Result<Self> {
        let stream_url = wave_tui::audio_spike::resolve_stream_url(url);
        let resp = reqwest::blocking::get(&stream_url)
            .context("http get")?
            .error_for_status()
            .with_context(|| format!("stream request failed for {stream_url}"))?;
        let final_url = resp.url().clone();
        let source = ReadOnlySource::new(resp);
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = wave_tui::audio_spike::stream_extension(final_url.path()) {
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
        })
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn refill(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(_)) => return Ok(false),
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
                    eprintln!("decoder refill error: {err:#}");
                    return None;
                }
            }
        }
        let sample = self.sample_buf.get(self.sample_pos).copied();
        self.sample_pos += 1;
        sample
    }
}

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

fn choose_output_config(
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
        .context("no output config supports the stream sample rate")
}

fn build_output_stream(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    queue_rx: ringbuf::HeapCons<f32>,
    source_channels: usize,
    output_channels: usize,
    played_tx: mpsc::SyncSender<f32>,
) -> Result<cpal::Stream> {
    let stream_config = config.config();
    let err_fn = |err| eprintln!("audio output stream error: {err}");
    match config.sample_format() {
        cpal::SampleFormat::F32 => build_typed_output_stream::<f32>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            played_tx,
            err_fn,
        ),
        cpal::SampleFormat::I16 => build_typed_output_stream::<i16>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            played_tx,
            err_fn,
        ),
        cpal::SampleFormat::U16 => build_typed_output_stream::<u16>(
            device,
            &stream_config,
            queue_rx,
            source_channels,
            output_channels,
            played_tx,
            err_fn,
        ),
        other => anyhow::bail!("unsupported output sample format for spike: {other:?}"),
    }
}

fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut queue_rx: ringbuf::HeapCons<f32>,
    source_channels: usize,
    output_channels: usize,
    played_tx: mpsc::SyncSender<f32>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut source_frame = vec![0.0; source_channels.max(1)];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            for frame in data.chunks_mut(output_channels) {
                let has_frame = queue_rx.occupied_len() >= source_channels;
                if has_frame {
                    for slot in &mut source_frame {
                        *slot = queue_rx.try_pop().unwrap_or(0.0);
                    }
                } else {
                    source_frame.fill(0.0);
                }

                let analyzer_sample = mix_for_analyzer(&source_frame);
                if has_frame {
                    let _ = played_tx.try_send(analyzer_sample);
                }

                for (idx, out) in frame.iter_mut().enumerate() {
                    *out = T::from_sample(map_output_sample(&source_frame, idx, output_channels));
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

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

fn mix_for_analyzer(source_frame: &[f32]) -> f32 {
    if source_frame.is_empty() {
        0.0
    } else {
        source_frame.iter().copied().sum::<f32>() / source_frame.len() as f32
    }
}

fn analyzer_loop(rx: mpsc::Receiver<f32>, sample_rate: u32, stop: Arc<AtomicBool>) {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut samples = vec![0.0_f32; FFT_SIZE];
    let mut scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let bands = log_bands(sample_rate as f32, FFT_SIZE, BAND_COUNT);
    let mut history = std::collections::VecDeque::with_capacity(FFT_SIZE * 4);
    let mut last_print = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(sample) => {
                history.push_back(sample);
                while history.len() > FFT_SIZE * 4 {
                    history.pop_front();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        while let Ok(sample) = rx.try_recv() {
            history.push_back(sample);
            while history.len() > FFT_SIZE * 4 {
                history.pop_front();
            }
        }

        if history.len() >= FFT_SIZE && last_print.elapsed() >= Duration::from_millis(120) {
            let start = history.len() - FFT_SIZE;
            for (dst, sample) in samples.iter_mut().zip(history.iter().skip(start)) {
                *dst = *sample;
            }
            let values = analyze_frame(&samples, &*fft, &mut scratch, &bands);
            print_bars(&values);
            last_print = Instant::now();
        }
    }
}

fn log_bands(sample_rate: f32, n_fft: usize, band_count: usize) -> Vec<(usize, usize)> {
    let nyquist = sample_rate / 2.0;
    let min_hz: f32 = 60.0;
    let max_hz = nyquist.min(12_000.0);
    let log_min = min_hz.ln();
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

fn analyze_frame(
    samples: &[f32],
    fft: &dyn rustfft::Fft<f32>,
    scratch: &mut [Complex<f32>],
    bands: &[(usize, usize)],
) -> Vec<f32> {
    let n = samples.len();
    for (i, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).cos();
        scratch[i] = Complex::new(sample * window, 0.0);
    }

    fft.process(scratch);
    let mags: Vec<f32> = scratch
        .iter()
        .take(n / 2)
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .collect();

    bands
        .iter()
        .map(|(b0, b1)| {
            let start = (*b0).min(mags.len());
            let end = (*b1).min(mags.len());
            if end <= start {
                0.0
            } else {
                let avg = mags[start..end].iter().copied().sum::<f32>() / (end - start) as f32;
                wave_tui::audio_spike::normalize_value(avg, 3.0)
            }
        })
        .collect()
}

fn print_bars(values: &[f32]) {
    let blocks = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let line: String = values
        .iter()
        .map(|value| {
            let idx = ((value.clamp(0.0, 1.0) * (blocks.len() - 1) as f32).round() as usize)
                .min(blocks.len() - 1);
            blocks[idx]
        })
        .collect();
    println!("fft {line}");
}
