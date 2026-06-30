# wave-tui — quiet terminal radio for work sessions

`wave-tui` is a terminal-first internet radio player for work sessions. It is
built to live in a terminal/herdr pane, resume your background audio quickly,
and stay calm enough to keep open for hours while you work.

It is a focused work-session BGM radio, not a music library or Spotify-like
player. See [`docs/SPEC.md`](docs/SPEC.md) for the full scope and non-goals.

Project site: [takemo101.github.io/wave-tui](https://takemo101.github.io/wave-tui/)

## Features

- **Native Rust playback** — MP3/AAC-centered HTTP streams play through a
  `reqwest` + `symphonia` + `cpal` pipeline. No external `ffplay`/`mpv` process.
- **Real FFT visualizer** — the Spectrum Stack's particle-filled analyzer is
  driven by actual played audio samples via `rustfft`, not a simulation. Six
  calm modes are selectable with `v` (see [Visualizer modes](#visualizer-modes));
  all are real-audio-driven and stretch to fill the visualizer pane.
- **Auto-resume** — launching `wave-tui` replays your previous station; first
  launch (or a failed previous station) starts silently with curated
  recommendations.
- **Quiet lifecycle splash** — startup shows a short, skippable pixel-art
  `WAVE` logo reveal with the `wave-tui v...` version label and
  `settling into the signal`; shutdown shows `thanks for listening` /
  `see you next wave` with a small calm wave animation.
- **Curated catalog + online search** — a small built-in Music and Spoken/News
  catalog, plus Radio Browser search-as-you-type with debounce and a result
  cache. Results are ranked toward likely-playable, popular stations.
- **Favorites and persistence** — previous station, volume, favorites, and the
  selected theme persist across restarts.
- **ICY/Shoutcast now-playing** — current track titles appear when a station
  provides them, falling back to station metadata otherwise.
- **Three responsive layouts** — Wide "Search Console", plus Medium and Compact
  "Split Mini" tiers that keep both the station list and Now Playing visible.
- **Signal View** — press `z` for a quiet, opt-in visual-player mode that hides
  the discovery UI and shows the current station center-stage with a large
  visualizer; `z`/`Esc` return and `q` quits. It is not persisted and has no CLI
  flag.
- **Six themes** — `Minimal` (calm default), `Neon`, `CRT`, `Solarized`,
  `Midnight`, and `Sakura`. Each carries a distinct palette tuned to stay
  readable on a dark terminal during long work sessions.
- **Resilient offline/error handling** — a failed online search shows a clear
  offline state in every layout tier without crashing, and you can still retry
  the previous station and built-in candidates. Favorited stations are saved
  across restarts and reachable from a dedicated Favorites view in the Browse
  rail, even when they are absent from the current catalog or search results;
  saved favorites are retryable stream entries, not stations guaranteed to play
  offline. Stations that fail to play are dimmed and temporarily disabled for
  the session.

## Quick start

### Install a prebuilt binary on macOS

Install the latest GitHub Release to `~/.local/bin/wave-tui`:

```bash
INSTALL_URL=https://raw.githubusercontent.com/takemo101/wave-tui/main/install.sh
curl -fsSL "$INSTALL_URL" | sh
```

Choose a different install directory:

```bash
INSTALL_URL=https://raw.githubusercontent.com/takemo101/wave-tui/main/install.sh
curl -fsSL "$INSTALL_URL" | INSTALL_DIR=/usr/local/bin sh
```

Install a specific release tag:

```bash
INSTALL_URL=https://raw.githubusercontent.com/takemo101/wave-tui/main/install.sh
curl -fsSL "$INSTALL_URL" | VERSION=v0.1.2 sh
```

The installer currently publishes macOS prebuilt assets:

| OS | Architecture | Asset target |
| --- | --- | --- |
| macOS | Apple Silicon | `aarch64-apple-darwin` |
| macOS | Intel | `x86_64-apple-darwin` |

Linux users should install from source for now. `wave-tui` uses native audio
output through `cpal`, so Linux binary packaging needs distribution-level audio
library verification before prebuilt assets are advertised as supported.

### Build and run from source

Run directly from a clone:

```bash
cargo run --release
```

Install with `just` (copies the release binary to `~/.local/bin` by default):

```bash
just install
wave-tui
```

Install directly with Cargo from a clone:

```bash
cargo install --path .
wave-tui
```

Install directly from GitHub with Cargo:

```bash
cargo install --git https://github.com/takemo101/wave-tui
wave-tui
```

On first launch with no saved settings, the app starts with a short skippable
startup card: a pixel-art `WAVE` logo reveals left-to-right above the
`wave-tui v...` version label and `settling into the signal`. The startup card
uses no `~≈∿` wave line; that calmer wave animation is reserved for the shutdown
farewell (`thanks for listening` / `see you next wave`). After the splash, the
app starts silently and shows the curated catalog. Press `/` and start typing to
search Radio Browser, or select a station and press `Enter` to play.

## Controls

Navigation uses focus-based panes. `Tab`/`Shift+Tab` move focus; the list and
search controls act on the focused pane.

| Key         | Action                       |
| ----------- | ---------------------------- |
| `Tab`       | focus next pane              |
| `Shift+Tab` | focus previous pane          |
| `j` / `↓`   | select next station          |
| `k` / `↑`   | select previous station      |
| `g` / `Home`| jump to first station        |
| `G` / `End` | jump to last station         |
| `Enter`     | play selected station        |
| `Space`     | stop / play toggle           |
| `+` / `-`   | volume up / down             |
| `f`         | toggle favorite              |
| `t`         | cycle theme                  |
| `v`         | cycle visualizer mode        |
| `z`         | toggle Signal View           |
| `/`         | search Radio Browser         |
| `Esc`       | clear search / return        |
| `q` / `Esc` | quit when not searching      |
| `Ctrl+C`    | quit from any mode           |

Search is online as you type, with a ~350ms debounce and a per-query cache;
stale in-flight searches are ignored.

### Browse and Favorites

The Browse rail is a source picker: `All Stations`, `Favorites`, and each
section and category. Focus it with `Tab`, move the cursor with `j`/`k` or the
arrows, and press `Enter` to apply a source — that replaces the station list and
hands focus back to it. The active source is marked with a filled dot, distinct
from the Browse cursor.

When you have search results, Browse sources filter those results: `All
Stations` shows every result, and a section or category narrows them to matching
stations (the search strip shows the active filter, e.g. `filter: Jazz`). With no
search results, the same sources fall back to the curated catalog. Clearing
search keeps your Browse selection but rebuilds it from the curated catalog. A
genre with no matches in the current search shows a note like `No Jazz results in
current search`. `Favorites` is never filtered by search — it always lists your
saved stations.

`Favorites` lists the stations you've saved with `f`, in the order you saved
them, and stays reachable even when those stations are absent from the current
catalog or search results. Removing a favorite with `f` while the Favorites view
is active drops it from the list immediately. An empty Favorites view shows an
explicit hint to save a station with `f`. Saved favorites are retryable stream
entries, not stations guaranteed to play while offline.

### Visualizer modes

The `v` key cycles a six-mode "Calm Suite" of visualizers, and the selected
mode is persisted across restarts. Every mode is driven by real played audio and
stretches its source data to fill the visualizer pane width; none turns the
layout into a full-screen visualizer.

| Mode            | Source      | Look                      |
| --------------- | ----------- | ------------------------- |
| `SpectrumStack` | FFT bands   | Particle analyzer columns |
| `PeakDots`      | FFT bands   | One peak dot per column   |
| `SkylinePeaks`  | FFT bands   | Peak cap and dashed tail  |
| `WaveScope`     | Waveform    | Oscilloscope trace        |
| `MirrorWave`    | Waveform    | Mirrored waveform         |
| `AmbientPulse`  | RMS + bands | Low-noise centered glow   |

The waveform modes treat both an empty and an all-zero waveform as a flat
silence baseline, and `AmbientPulse` draws nothing for a silent frame, so a
stopped or quiet stream stays calm rather than showing fake motion.

### Signal View

Press `z` to enter Signal View, a quiet visual-player mode for the current
station. It hides the Search, Browse, and Stations UI and presents the current
station center-stage with a large visualizer that fills the largest region of
the screen. Press `z` or `Esc` to return to the normal UI; `q` still quits.

While Signal View is active, only `Space` (play/stop), `+`/`-` (volume), `v`
(visualizer mode), `t` (theme), and `f` (favorite) stay active — `f` favorites
the station shown on screen. A favorited current station uses the same `★` marker
as the station list; non-favorites show no marker. Discovery, navigation, and
focus keys are ignored. Signal View is a temporary view: it is not saved across
launches and has no command-line flag. With no current station it shows a short
`Select a station, then press z` prompt, and it stays put across stopped,
connecting, playing, and failed states instead of dropping you back to the
normal UI. The title area includes a thin, near-full-width volume bar with the
active visualizer mode label.

## Command-line options

```text
wave-tui [OPTIONS] [SEARCH]

OPTIONS:
    --theme <name>                Theme for this run: minimal | neon | crt |
                                  solarized | midnight | sakura
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

## Uninstall

If you installed with `install.sh`, remove the installed binary:

```bash
rm -f ~/.local/bin/wave-tui
```

If you used a custom `INSTALL_DIR`, delete the binary from that directory instead:

```bash
rm -f /usr/local/bin/wave-tui
```

If you installed from a repository clone with `just install`, use the same
`INSTALL_DIR` with `just uninstall`:

```bash
just uninstall
# or
INSTALL_DIR=/usr/local/bin just uninstall
```

## Troubleshooting

**Linux source builds.** Linux users should currently build from source. Depending
on your distribution and audio stack, native `cpal` output may require system
audio libraries or development packages to be present before `cargo build` can
link successfully. Prebuilt Linux assets will be documented only after CI builds
and real-device audio playback are verified on representative distributions.

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
station, built-in catalog candidates, and saved favorites remain available to
retry: open the Favorites view from the Browse rail to reach saved stations even
while offline. Favorites are retryable stream entries, not stations guaranteed
to play offline. The offline state clears after the next successful search.

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
- [`docs/index.html`](docs/index.html) — GitHub Pages landing and setup guide
- [`install.sh`](install.sh) — GitHub Releases installer for supported
  prebuilt binaries
- [`.github/workflows/pages.yml`](.github/workflows/pages.yml) — GitHub Pages
  deployment workflow
- [`.github/workflows/release.yml`](.github/workflows/release.yml) — tagged
  release asset workflow
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
