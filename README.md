# wave-tui — quiet terminal radio for work sessions

`wave-tui` is a terminal-first internet radio player for work sessions. It is
built to live in a terminal/herdr pane, resume your background audio quickly,
and stay calm enough to keep open for hours while you work.

It is a focused work-session BGM radio, not a music library or Spotify-like
player. See [`docs/SPEC.md`](docs/SPEC.md) for the full scope and non-goals.

## Features

- **Native Rust playback** — MP3/AAC-centered HTTP streams play through a
  `reqwest` + `symphonia` + `cpal` pipeline. No external `ffplay`/`mpv` process.
- **Real FFT visualizer** — the Spectrum Stack bars are driven by actual played
  audio samples via `rustfft`, not a simulation.
- **Auto-resume** — launching `wave-tui` replays your previous station; first
  launch (or a failed previous station) starts silently with curated
  recommendations.
- **Curated catalog + online search** — a small built-in Music and Spoken/News
  catalog, plus Radio Browser search-as-you-type with debounce and a result
  cache. Results are ranked toward likely-playable, popular stations.
- **Favorites and persistence** — previous station, volume, favorites, and the
  selected theme persist across restarts.
- **ICY/Shoutcast now-playing** — current track titles appear when a station
  provides them, falling back to station metadata otherwise.
- **Three responsive layouts** — Wide "Search Console", plus Medium and Compact
  "Split Mini" tiers that keep both the station list and Now Playing visible.
- **Three themes** — `Minimal` (calm default), `Neon`, and `CRT`.
- **Resilient offline/error handling** — a failed online search shows a clear
  offline state in every layout tier without crashing, and you can still retry
  the previous station and built-in candidates. Favorited stations remain
  marked and retryable when they are present in the current catalog/search list;
  a dedicated favorites-only browse view is not in the MVP yet. Stations that
  fail to play are dimmed and temporarily disabled for the session.

## Quick start

Build and run from source:

```bash
cargo run --release
```

Install with `just` (copies the release binary to `~/.local/bin` by default):

```bash
just install
wave-tui
```

Install directly with Cargo:

```bash
cargo install --path .
wave-tui
```

On first launch with no saved settings, the app starts silently and shows the
curated catalog. Press `/` and start typing to search Radio Browser, or select a
station and press `Enter` to play.

## Controls

Navigation uses focus-based panes. `Tab`/`Shift+Tab` move focus; the list and
search controls act on the focused pane.

| Key             | Action                                         |
| --------------- | ---------------------------------------------- |
| `Tab`           | focus next pane                                |
| `Shift+Tab`     | focus previous pane                            |
| `j` / `↓`       | select next station                            |
| `k` / `↑`       | select previous station                        |
| `g` / `Home`    | jump to first station                          |
| `G` / `End`     | jump to last station                           |
| `Enter`         | play the selected station                      |
| `Space`         | stop / play toggle for the current station     |
| `+` / `-`       | volume up / down                               |
| `f`             | toggle favorite for the selected station       |
| `t`             | cycle theme (Minimal → Neon → CRT)             |
| `/`             | focus search and type to search Radio Browser  |
| `Esc`           | while searching: clear search and return to catalog |
| `q` / `Esc`     | quit (`Esc` quits when not searching)          |
| `Ctrl+C`        | quit from any mode                              |

Search is online as you type, with a ~350ms debounce and a per-query cache;
stale in-flight searches are ignored.

## Command-line options

```text
wave-tui [OPTIONS] [SEARCH]

OPTIONS:
    --theme <name>                Theme for this run: minimal | neon | crt
    --volume <0-100>              Startup volume override
    --no-auto-play                Start silently even if a previous station exists
    --audio-output-device <name>  CPAL output device name
    --low-power                   Lower UI update cadence (audio unaffected)
    --search <query>              Start in search mode with this query
    -h, --help                    Print help
    -V, --version                 Print version

ARGS:
    [SEARCH]                      Optional positional search query (same as --search)
```

`--theme` and `--volume` are per-run overrides. A `--volume` override is not
written back to disk unless you change the volume with `+`/`-` during the run.

## Troubleshooting

**No audio output.** Confirm your default output device works, then try naming a
device explicitly with `--audio-output-device <name>`. Playback prefers an
output configuration matching the stream's sample rate; streams whose sample
rate the selected device cannot produce may fail. Verify the native pipeline in
isolation with the audio spike below.

**A station won't play.** Remote streams can be offline, geo-restricted, or use
an unsupported codec (MVP targets MP3/AAC HTTP streams). Failed stations are
marked, dimmed, and temporarily disabled for the session; selection moves to the
next viable candidate. Retrying a station that later succeeds clears the mark.

**Search shows "offline".** Radio Browser could not be reached. The previous
station and built-in catalog candidates remain available to retry; favorited
stations are retryable when they are present in the current visible list. There
is no favorites-only browse view yet. The offline state clears after the next
successful search.

## Native audio spike

`audio_spike` is a standalone manual-verification tool that drives the same
native [`AudioRuntime`](src/audio.rs) used by the app, printing the visualizer
frames it emits. It validates the playback architecture end to end:

```text
HTTP stream -> Symphonia decode -> CPAL output -> played-sample mirror -> RustFFT
```

Run it against a live stream:

```bash
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Expected result: audio plays through the default output device and the terminal
prints changing `fft ...` bars. See [`docs/audio-spike.md`](docs/audio-spike.md)
for findings and caveats.

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
cargo clippy --all-targets -- -D warnings
```

For the audio spike specifically:

```bash
cargo test --test audio_spike
cargo run --bin audio_spike -- https://dancewave.online/dance.mp3 5
```

Default tests do not require network, audio devices, or a real terminal; live
audio, real streams, ICY metadata, and terminal rendering are verified manually.

## Credits

Radio station data: <https://www.radio-browser.info/>

Reference inspirations:

- [`cliamp`](https://github.com/bjarneo/cliamp) for terminal music-player visual
  direction
- [`late.sh` / `late-cli`](https://github.com/mpiorowski/late-sh) for native
  audio + FFT architecture reference
</content>

</invoke>
