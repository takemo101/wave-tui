# wave-tui — quiet terminal radio for work sessions

`wave-tui` is a terminal-first internet radio player for work sessions.

The current codebase started as a small Rust/Ratatui radio prototype. The
replacement design keeps the lightweight TUI spirit while moving toward native
Rust playback, real FFT visualization, online Radio Browser search, persistence,
and responsive layouts.

## Current status

This repository is in design/spike phase.

Implemented now:

- Rust Ratatui + crossterm TUI prototype
- Radio Browser top-voted station loading
- external `ffplay`/`mpv` playback in the original prototype
- native audio spike binary proving `reqwest` + `symphonia` + `cpal` +
  `rustfft` is viable

Planned replacement:

- native Rust playback instead of external players
- real FFT-powered Spectrum Stack visualizer
- online search-as-you-type with debounce/cache
- curated Music and Spoken/News catalog
- persisted previous station, volume, favorites, and theme
- Minimal / Neon / CRT themes
- Wide Search Console and Medium/Compact Split Mini layouts

## Quick start

Build and run the current prototype:

```bash
cargo run --release
```

Install with `just`:

```bash
just install
wave-tui
```

Install directly with Cargo:

```bash
cargo install --path .
wave-tui
```

## Native audio spike

The spike validates the replacement playback architecture:

```text
HTTP stream -> Symphonia decode -> CPAL output -> played-sample mirror -> RustFFT
```

Run the spike:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Expected result: audio plays through the default output device and the terminal
prints changing `fft ...` bars.

See [`docs/audio-spike.md`](docs/audio-spike.md) for findings and caveats.

## Prototype controls

| Key           | Action                                  |
| ------------- | --------------------------------------- |
| `j` / `↓`     | next station                            |
| `k` / `↑`     | previous station                        |
| `Enter`       | play selected station                   |
| `s` / `Space` | stop playback                           |
| `+` / `-`     | volume up / down (restarts stream)      |
| `/`           | quick filter by name/tags/country       |
| `d`           | cycle discover genre filter             |
| `Tab`         | toggle Discover / Search tab            |
| `q` / `Esc`   | quit (stops player)                     |

These controls describe the current prototype. The replacement controls are
specified in [`docs/SPEC.md`](docs/SPEC.md).

## Documentation map

- [`AGENTS.md`](AGENTS.md) — workflow guidance for AI coding agents
- [`docs/SPEC.md`](docs/SPEC.md) — product specification and MVP scope
- [`docs/implementation-guidelines.md`](docs/implementation-guidelines.md) —
  implementation principles adapted from okite-ai skills
- [`docs/ui-design-decisions.md`](docs/ui-design-decisions.md) — design deck
  decisions
- [`docs/audio-spike.md`](docs/audio-spike.md) — native audio spike results
- [`docs/superpowers/plans/2026-06-27-radio-replacement.md`](docs/superpowers/plans/2026-06-27-radio-replacement.md)
  — implementation plan

## Verification

```bash
cargo fmt --check
cargo test
cargo check
```

For the audio spike:

```bash
cargo test --test audio_spike
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

## Credits

Radio station data: <https://www.radio-browser.info/>

Reference inspirations:

- [`cliamp`](https://github.com/bjarneo/cliamp) for terminal music-player visual
  direction
- [`late.sh` / `late-cli`](https://github.com/mpiorowski/late-sh) for native
  audio + FFT architecture reference
