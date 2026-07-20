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

### Blocking I/O drove the control-thread design

The spike's blocking pipeline (`reqwest` blocking client + Symphonia pull
decoding) has a consequence the spike itself did not exercise: any work done on
the audio control thread inherits the stream's read timeout. MIK-066 found two
such places and moved both off that thread.

- **Connecting.** `StreamDecoder::new_http` connects and probes the container,
  both of which read from the socket. A station that accepts the connection and
  then goes silent stalls there for the configured read timeout. It now runs on a
  per-attempt connect worker; the control thread only polls for the outcome.
- **Teardown.** Tearing a session down used to join its decoder thread inline,
  and that thread is usually parked in exactly such a read. Retirement now
  raises the stop flag and hands the workers to a background reaper.

`src/audio/session.rs` owns this lifecycle and is deliberately free of CPAL,
Symphonia, and HTTP: the device/network wiring is injected as a `PlaybackEngine`,
which is what lets the concurrency behavior be tested with a fake blocking engine
instead of a real device or network.

#### Bounded, not instant, cleanup

Neither a `reqwest` blocking read nor a Symphonia packet read is cancellable
once entered, so cancellation is cooperative and bounded rather than immediate:

- A cancelled worker observes the stop flag only after its current read returns,
  which the client's connect/read timeouts bound.
- The reaper joins retirements in arrival order, so a retirement queued behind a
  still-blocked worker is reclaimed later. It has already been cancelled and
  exits on its own, so this defers the `join`, it does not leak.
- On shutdown the reaper is detached rather than joined, so `Shutdown` returns
  without waiting on a blocked read. Outstanding work is dropped, which closes
  the sockets it holds. Note the honest limit: workers still inside a bounded
  read at that moment are cancelled but *not* joined, and the process exits
  without them. The join guarantee covers the life of the runtime, not shutdown.
- Dropping the CPAL stream stays on the control thread. It is a bounded device
  call, not a network read, and doing it first frees the output device
  immediately.

#### Capping uncancellable connects

Bounded cleanup alone is not enough when input is faster than the bound. Each
connect occupies a thread and a socket for up to its timeout and cannot be
cancelled, so one connect per keypress would let a held-down Enter against a
wedged station pile up threads far faster than they retire.

The runtime therefore caps concurrent connect workers (`MAX_CONNECT_WORKERS`)
and coalesces rather than queues. Only the newest request ever waits for a slot,
because a superseded request has no one left to play it; when a slot frees, the
request the user settled on is the one that connects. The cap is deliberately
above one, so changing station once or twice while a station is slow still
connects immediately instead of waiting out a timeout.

The cap never delays command receipt: a `Play` is accepted and announced as
`Connecting` whether or not a worker can start for it yet.

#### Worker panics are recoverable failures

The control loop accounts for every started connect worker by its outcome, so a
worker that died without reporting would strand its request as permanently
pending *and* leak a connect slot — the request would never fail, never play, and
never free capacity. Connect workers therefore catch panics and report them as
ordinary failed outcomes, which reach the app as a recoverable
`AudioEvent::Failed` and release the slot.

Late completions are made harmless rather than prevented: a connect outcome is
adopted only while its MIK-065 playback request id still matches the pending
request, so a connect that finishes after a `Stop`, a replacement `Play`, or a
`Shutdown` is discarded instead of starting playback.

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
