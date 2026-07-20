# Native Audio Spike Results

## Goal

Validate that the replacement can use a Rust-native pipeline for live radio:

1. Fetch an HTTP radio stream.
2. Decode it with Symphonia.
3. Play it through CPAL.
4. Capture played samples.
5. Generate FFT visualizer bands with RustFFT.

## Added Spike Artifacts

- `src/lib.rs`
- `src/audio_spike.rs`
  - small deterministic helpers covered by tests
- `src/bin/audio_spike.rs`
  - standalone spike binary
- `tests/audio_spike.rs`
  - helper tests

## How to Run

Default 8-second run:

```bash
cargo run --bin audio_spike
```

Short default run:

```bash
cargo run --bin audio_spike -- 3
```

Specific stream and duration:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

## Verified Result

Command run:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Observed output:

```text
audio spike: url=https://dancewave.online/dance.mp3
audio spike: duration=5s
decoded stream: 44100 Hz, 2 channel(s)
output device: Mac miniのスピーカー; 44100 Hz, 2 channel(s), F32
fft ████████████▇▇▇▆
fft ██████████▇▇█▇▇▇
...
audio spike: complete
```

This confirms:

- Symphonia can decode a live MP3 radio stream.
- CPAL can play decoded samples through the local output device.
- The output callback can mirror played samples to an analyzer path.
- RustFFT can produce visible changing band output from actual playback audio.

## Important Findings

### Direct mount URLs without extensions need careful handling

The first attempted stream was:

```text
https://stream.radioparadise.com/mp3-192
```

The late-cli-style `resolve_stream_url` helper appended `/stream`, causing:

```text
https://stream.radioparadise.com/mp3-192/stream
```

That URL returned 404.

For MVP implementation, station URL resolution should not blindly append
`/stream` for Radio Browser station URLs. The app should treat Radio Browser
`url_resolved` as a direct stream URL first, and only append `/stream` for known
base URLs or curated catalog entries that explicitly opt into that behavior.

### Resampling is still needed for robust MVP playback

The spike intentionally bails if the stream sample rate does not match a
supported output device sample rate. The successful test used 44.1 kHz output.

MVP should include either:

- output config selection that prefers the stream sample rate when available, and
- a resampler fallback when the device only supports another rate, commonly
  48 kHz.

The late-cli implementation uses this kind of device/sample-rate handling and is
still the right reference.

### ICY metadata is implemented after the spike

The original spike only helper-tested an ICY `StreamTitle` parser. MIK-011
completed the production follow-up: `src/audio/icy.rs` now splits interleaved
ICY metadata blocks from audio bytes, deduplicates unchanged titles, and exposes
an `IcyReader` that feeds Symphonia audio-only bytes. `src/audio/decoder.rs`
requests `Icy-MetaData: 1`, reads `icy-metaint`, and wraps streams only when
metadata framing is present.

This behavior is covered by pure synthetic-byte tests; live station metadata
still belongs in manual verification because remote ICY support varies by
station.

## Automated Checks

Commands run:

```bash
cargo test --test audio_spike
cargo check
```

Result:

- `tests/audio_spike.rs`: 6 passed
- `cargo check`: passed

## Recommendation

Proceed with the replacement architecture from `docs/SPEC.md`, with two
adjustments:

1. Do not use late-cli-style `/stream` URL appending for arbitrary Radio Browser
   station URLs.
2. Include resampling earlier than originally implied, or clearly restrict the
   MVP's first playback task to streams whose sample rate is supported by the
   selected CPAL device.
