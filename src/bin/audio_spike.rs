//! Manual smoke test for the native audio runtime.
//!
//! Drives [`wave_tui::audio::AudioRuntime`] with a single `Play` command and
//! prints the visualizer frames it emits, so the runtime can be exercised
//! against a real CPAL device and live HTTP stream before it is wired into the
//! TUI. Manual command behavior is preserved: `audio_spike [URL] [SECONDS]`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use wave_tui::audio::{AudioCommand, AudioEvent, AudioRuntime, AudioRuntimeConfig};
use wave_tui::model::{
    CodecKind, Station, StationId, StationName, StationSource, StreamUrl, VolumePercent,
};

const DEFAULT_STREAM: &str = "https://dancewave.online/dance.mp3";

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
    let station = spike_station(url)?;
    let handle = AudioRuntime::spawn(AudioRuntimeConfig::default());

    handle
        .command_tx
        .send(AudioCommand::Play {
            station: Box::new(station),
            volume: VolumePercent::clamped(100),
        })
        .context("audio runtime stopped before play")?;

    let started = Instant::now();
    let mut failed = false;
    while started.elapsed() < duration {
        match handle.event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioEvent::Connecting { .. }) => println!("audio spike: connecting"),
            Ok(AudioEvent::Playing { .. }) => println!("audio spike: playing"),
            Ok(AudioEvent::Failed { message, .. }) => {
                eprintln!("audio spike: failed: {message}");
                failed = true;
                break;
            }
            Ok(AudioEvent::Viz(frame)) => print_bars(frame.bands()),
            Ok(AudioEvent::IcyTitle { title, .. }) => println!("audio spike: title: {title}"),
            Ok(AudioEvent::Stopped) | Ok(AudioEvent::VolumeChanged(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = handle.command_tx.send(AudioCommand::Shutdown);
    if failed {
        anyhow::bail!("playback failed");
    }
    println!("audio spike: complete");
    Ok(())
}

/// Build a minimal [`Station`] for the spike from a raw stream URL.
fn spike_station(url: &str) -> Result<Station> {
    Ok(Station {
        id: StationId::new("spike").expect("static id is non-empty"),
        name: StationName::new("spike").expect("static name is non-empty"),
        url: StreamUrl::parse(url).context("invalid stream url")?,
        homepage: None,
        country: None,
        language: None,
        tags: Vec::new(),
        codec: CodecKind::Unknown,
        bitrate: None,
        votes: None,
        click_count: None,
        source: StationSource::BuiltIn,
    })
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
