# Agent Pulse Lissajous Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Agent Pulse's scrolling-wave background and album-art interiors with real-audio Dual Phase Scope traces and status-driven spinner cores.

**Architecture:** Preserve the current local Herdr monitor, App-owned visualizer history/stale capture, deterministic agent-frame layout, and input routing. Extend the played-sample mirror and `VizFrame` with normalized paired phase traces; the pure Agent Pulse renderer maps those pairs to two centered Lissajous polylines, while stable frame edges and audio-driven frame transforms remain. Working cores derive their orientation from the current played-audio phase data, never a clock.

**Tech Stack:** Rust 2018, CPAL output callback, RustFFT analyzer, Ratatui `Buffer`, existing `VizFrame`/App history, pure module tests, GitButler `but`.

## Global Constraints

- Keep the current local Herdr control socket only; call only `agent.list`, never read pane output, discover remote sessions, or control panes.
- Add no dependencies, persistence format, user controls, playback commands, or new Herdr protocol methods.
- Preserve the normal `● n active` summary, `a`/`Esc` canvas contract, Signal View suppression, standalone invisibility, player-shortcut fallthrough, search/station-nav suppression, and Connected-only selection.
- Keep agent frame positions deterministic by private `AgentId`, preserve every frame at dense counts, and retain state-colored frame edges.
- Draw two real sample-pair phase portraits, not an amplitude-over-time waveform and not a synthetic FFT ripple. Stereo uses played left/right samples; mono uses two non-zero documented sample lags.
- Keep output clock-free: identical `VizFrame` data at different `Instant`s yields identical cells; silence is calm and still.
- Working uses an audio-derived rotating core; Idle, Blocked, Done, and Unknown use stationary cores. Blocked uses `theme.error`; Done remains dim until omitted.
- Show only `explicit name · status` for a selected named agent. Never show pane/workspace/cwd/agent type/fallback identifiers.
- Stale freezes and dims the captured final phase/frame/core composition; unavailable hides all graphics; low power freezes trace, frame, shadow, and spinner geometry while colors may refresh.
- Run `cargo fmt --check`, focused tests, `cargo test`, `cargo check`, and `cargo clippy --all-targets -- -D warnings` before the final commit. Live mono/stereo/theme/resize/reconnect checks remain explicitly manual.

---

## File structure and responsibility map

- `src/model.rs` — normalized, renderer-safe phase-pair domain value and `VizFrame` fields/constructors.
- `src/audio/played_sample.rs` (new) — private typed boundary between the realtime output callback and analyzer; preserves the pre-volume mono mix and source-channel pair without exposing CPAL details upward.
- `src/audio/output.rs` — builds one `PlayedSample` per actually played decoded frame before volume scaling.
- `src/audio/analyzer.rs` — calculates FFT/RMS/waveform plus the two bounded phase traces from real played samples.
- `src/audio.rs` — wires the typed bounded mirror channel from output to analyzer.
- `src/ui/agent_pulse.rs` — pure Dual Phase Scope geometry, phosphor persistence, status cores, selected label placement, and buffer tests.
- `src/app.rs` — owns an optional low-power visual capture so static low-power geometry is stable while current state colors refresh.
- `src/cli.rs` — configures the App's low-power visual policy once and retains the existing input path; verifies canvas keys remain unchanged.
- `README.md`, `docs/SPEC.md`, `docs/ui-design-decisions.md`, `AGENTS.md` — user-facing/current design pointers and manual checklist.
- `docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md` — historical/supersession header only; preserve its integration/privacy record.

---

### Task 1: Carry real phase-pair data from playback into `VizFrame`

**Files:**

- Create: `src/audio/played_sample.rs`
- Modify: `src/audio.rs:22-32, 250-305`
- Modify: `src/audio/output.rs:1-20, 120-285`
- Modify: `src/audio/analyzer.rs:1-223, 232-384`
- Modify: `src/model.rs:480-523, 723-744`

**Interfaces:**

- Consumes: decoded `source_frame: &[f32]` from the existing CPAL callback.
- Produces: `audio::played_sample::PlayedSample { mono, left, right, is_stereo }` on the existing bounded mirror channel.
- Produces: `model::PhaseTrace { x: Vec<f32>, y: Vec<f32> }` and `VizFrame::{primary_phase, secondary_phase}`.
- Existing callers continue using `VizFrame::new(bands, rms, waveform)`; it initializes empty phase traces. The analyzer alone calls `VizFrame::with_phase(...)`.

- [ ] **Step 1: Add failing model and analyzer tests.**

In `src/model.rs` tests, add the normalized phase-pair contract:

```rust
#[test]
fn phase_trace_clamps_coordinates_and_truncates_to_paired_length() {
    let trace = PhaseTrace::new([-2.0, -0.25, 2.0], [-0.5, 0.5]);
    assert_eq!(trace.x, vec![-1.0, -0.25]);
    assert_eq!(trace.y, vec![-0.5, 0.5]);
}

#[test]
fn legacy_viz_frame_constructor_has_empty_phase_traces() {
    let frame = VizFrame::new([0.2], 0.4, [0.1]);
    assert!(frame.primary_phase.x.is_empty());
    assert!(frame.secondary_phase.y.is_empty());
}
```

In `src/audio/analyzer.rs` tests, add typed sample fixtures and assert that a stereo source preserves left/right in the primary trace while mono produces non-diagonal lagged traces:

```rust
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
```

- [ ] **Step 2: Run the focused model/analyzer tests and confirm they fail.**

Run:

```bash
cargo test model::tests::phase_trace
cargo test audio::analyzer::tests::analyze_
```

Expected: FAIL because `PhaseTrace`, `primary_phase`, `secondary_phase`, and typed played samples do not yet exist.

- [ ] **Step 3: Define the narrow typed played-sample and phase-trace boundaries.**

Create `src/audio/played_sample.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayedSample {
    pub(crate) mono: f32,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) is_stereo: bool,
}

impl PlayedSample {
    pub(crate) fn from_source_frame(source: &[f32]) -> Option<Self> {
        let &left = source.first()?;
        let right = source.get(1).copied().unwrap_or(left);
        let mono = source.iter().copied().sum::<f32>() / source.len() as f32;
        Some(Self {
            mono: mono.clamp(-1.0, 1.0),
            left: left.clamp(-1.0, 1.0),
            right: right.clamp(-1.0, 1.0),
            is_stereo: source.len() >= 2,
        })
    }
}
```

Declare it privately in `src/audio.rs` (`mod played_sample;`). In `src/model.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseTrace {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

impl PhaseTrace {
    pub fn new(x: impl IntoIterator<Item = f32>, y: impl IntoIterator<Item = f32>) -> Self {
        let x: Vec<f32> = x.into_iter().map(|value| value.clamp(-1.0, 1.0)).collect();
        let y: Vec<f32> = y.into_iter().map(|value| value.clamp(-1.0, 1.0)).collect();
        let len = x.len().min(y.len());
        Self { x: x[..len].to_vec(), y: y[..len].to_vec() }
    }

    pub fn empty() -> Self { Self { x: Vec::new(), y: Vec::new() } }
}
```

Extend `VizFrame` with public `primary_phase` and `secondary_phase`; retain `new` as a compatibility constructor that supplies `PhaseTrace::empty()`. Add `with_phase(bands, rms, waveform, primary_phase, secondary_phase)` as the analyzer constructor.

- [ ] **Step 4: Replace the untyped mirror channel and derive phase pairs in the analyzer.**

Change `played_tx`/`played_rx` in `start_playback`, `build_output_stream`, and `run_analyzer_loop` from `f32` to `PlayedSample`. In the output callback, keep pre-volume behavior but send only when a decoded source frame exists:

```rust
if has_frame {
    if let Some(sample) = PlayedSample::from_source_frame(&source_frame) {
        let _ = played_tx.try_send(sample);
    }
}
```

In `analyzer.rs`, retain `VecDeque<PlayedSample>`. Build the existing FFT/RMS/waveform input from `sample.mono`. Add `phase_trace(samples, points, lag, use_stereo)` that downsamples the last analyzer window to matching x/y vectors:

```rust
let x = if use_stereo { sample.left } else { sample.mono };
let y = if use_stereo { sample.right } else { lagged.mono };
```

Use the first non-zero lag constant for a stereo primary or mono primary fallback; use a distinct non-zero lag for the secondary trace. Skip the final `lag` source frames rather than wrapping, so every pair is chronological and no synthetic waveform is introduced. `analyze` calls `VizFrame::with_phase` with the existing bands/RMS/waveform plus both traces.

- [ ] **Step 5: Add output boundary tests and run Task 1 verification.**

Add tests in `src/audio/output.rs`:

```rust
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
```

Run:

```bash
cargo fmt --check
cargo test model::tests::phase_trace
cargo test audio::output::tests::played_sample
cargo test audio::analyzer::tests
cargo check
```

Expected: every command exits 0.

- [ ] **Step 6: Commit the typed phase-data slice.**

```bash
but commit agent-pulse-lissajous-scope -m "feat: expose played-audio phase traces"
```

### Task 2: Render Dual Phase Scope and status spinner cores

**Files:**

- Modify: `src/ui/agent_pulse.rs:115-852, 875-1521`
- Modify: `src/app.rs:455-533, 802-822, 1105-1121, 1603-1625, 2417-2469`
- Modify: `src/cli.rs:657-848, 1887-2495`

**Interfaces:**

- Consumes: `VizFrame::{primary_phase, secondary_phase}`, `App::viz_history()`, `App::stale_viz()`, `AgentView`, `Theme`, and `low_power`.
- Produces: private `PhaseCell`, `PhaseLayer`, `spinner_glyph`, and `selection_label_rect`; `CollageLayout` retains `tiles` but replaces its waveform `trace` with phase layers.
- Produces: `App::configure_low_power_visuals(bool)` and `App::low_power_viz()` for frozen low-power geometry; this setting is configured exactly once by `run_app` before the event loop.

- [ ] **Step 1: Replace waveform/album-motif tests with failing Scope/Core contract tests.**

In `src/ui/agent_pulse.rs` tests, remove expectations that assert `AlbumMotif`, waveform columns, or motif interior glyphs. Add:

```rust
#[test]
fn dual_phase_scope_draws_two_centered_non_scrolling_traces() {
    let buf = render_collage(2, phase_frame(), vec![older_phase_frame()], false);
    assert!(count_primary_phase_cells(&buf) > 0);
    assert!(count_secondary_phase_cells(&buf) > 0);
    assert!(!buffer_text(&buf).contains("▁"));
}

#[test]
fn phase_scope_uses_audio_pairs_not_elapsed_time() {
    let mut app = collage_app(agents(2));
    push_frame(&mut app, phase_frame());
    assert_eq!(render_collage_for(&app, false, Instant::now()),
               render_collage_for(&app, false, Instant::now() + Duration::from_secs(9)));
}

#[test]
fn working_core_changes_with_new_audio_while_idle_and_blocked_stay_still() {
    let first = render_status_frames(phase_frame_with_offset(0.1));
    let next = render_status_frames(phase_frame_with_offset(0.7));
    assert_ne!(working_core_cells(&first), working_core_cells(&next));
    assert_eq!(idle_core_cells(&first), idle_core_cells(&next));
    assert_eq!(blocked_core_cells(&first), blocked_core_cells(&next));
}

#[test]
fn selected_named_frame_shows_only_name_and_status_near_its_frame() {
    let mut app = app_with_named_and_unnamed_agents();
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
    assert!(text.contains("research · working"));
    assert!(!text.contains("workspace-1"));
    assert!(!text.contains("pane-1"));
}
```

- [ ] **Step 2: Run focused UI tests and confirm they fail.**

Run:

```bash
cargo test ui::agent_pulse::tests::dual_phase_scope
cargo test ui::agent_pulse::tests::working_core
cargo test ui::agent_pulse::tests::selected_named_frame
```

Expected: FAIL because the renderer still owns `TraceCell`, `trace_cells`, and `AlbumMotif`.

- [ ] **Step 3: Implement pure phase geometry and short phosphor persistence.**

Delete `AlbumMotif`, `motif_palette`, `motif_cell`, `TraceCell`, `trace_y`, `trace_cells`, and `trace_glyph`. Keep `CollageTile::{index, seed, base_rect, rect, energy, shadows}` so dense deterministic layout, selection, hit testing, and real RMS/FFT transforms remain unchanged.

Add a paired-coordinate mapper and layer data:

```rust
struct PhaseCell { x: u16, y: u16 }
struct PhaseLayer { cells: Vec<PhaseCell>, color_position: f32, dim: bool }

fn phase_cells(trace: &PhaseTrace, area: Rect) -> Vec<PhaseCell> {
    trace.x.iter().zip(&trace.y).map(|(&x, &y)| PhaseCell {
        x: phase_x(area, x),
        y: phase_y(area, y),
    }).collect()
}
```

`phase_x` maps `-1.0..=1.0` to the centered canvas width and `phase_y` maps it inversely to height, clamped to `area`. `collage_layout` creates a main layer from `frame.primary_phase`, a complementary layer from `frame.secondary_phase`, then at most two dim persistence layers from the newest `history` frames. Empty phase traces render no cells; do not substitute FFT ripples or waveform columns. Preserve the existing theme vignette.

- [ ] **Step 4: Render frame edges and status cores instead of album-art interiors.**

Keep the current edge rendering and state styles. Fill each frame interior with the base style, then place one one-cell core at the drawn-rectangle center. Define:

```rust
fn spinner_glyph(seed: u64, frame: &VizFrame) -> &'static str {
    const FRAMES: [&str; 4] = ["◜", "◝", "◞", "◟"];
    let phase = phase_signature(&frame.primary_phase);
    FRAMES[((phase as u64).wrapping_add(seed) % FRAMES.len() as u64) as usize]
}

fn status_core(status: AgentStatus, seed: u64, frame: &VizFrame) -> (&'static str, bool) {
    match status {
        AgentStatus::Working => (spinner_glyph(seed, frame), true),
        AgentStatus::Idle => ("◌", false),
        AgentStatus::Blocked => ("×", false),
        AgentStatus::Done => ("·", false),
        AgentStatus::Unknown => ("·", false),
    }
}
```

`phase_signature` must be a deterministic quantization of the real primary-phase coordinates; it returns the same result for identical frames and is not time-derived. Apply the existing stale dimmer to core/edge styles. `Blocked` core uses `theme.error`; Done/Unknown are dim.

Place the existing selected `name · status` string on the nearest in-bounds row below the selected frame (or above it when the footer would collide). Do not alter its explicit-name-only gate.

- [ ] **Step 5: Add App-owned low-power capture and preserve stale precedence.**

Add `low_power_viz: Option<(VizFrame, Vec<VizFrame>)>` and `low_power_visuals: bool` to App's private state. `configure_low_power_visuals(true)` sets the policy once at startup. On the first `set_viz_frame`, clone current frame/history into `low_power_viz`; later frames still update `viz`/`viz_history` for color/edge calculations but do not replace the geometry capture. `configure_low_power_visuals(false)` clears the capture.

Expose `pub(crate) fn low_power_viz(&self) -> Option<(&VizFrame, &[VizFrame])>`. In `render_canvas`, bind the live history before selecting geometry so no branch borrows a temporary; use this exact precedence:

```rust
let live_history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
let fallback = (app.viz(), live_history.as_slice());
let (frame, history) = if stale {
    app.stale_viz().unwrap_or(fallback)
} else if low_power {
    app.low_power_viz().unwrap_or(fallback)
} else {
    fallback
};
```

Stale always wins; unavailable renders before either capture. Add reducer tests showing first low-power visual frame remains the geometry source after a later `AudioEvent::Viz`, while `app.viz()` still updates.

In `src/cli.rs`, call `app.configure_low_power_visuals(low_power)` immediately after `App::new` and before the first audio event. Add a controller test that canvas key routing is unchanged by the configuration.

- [ ] **Step 6: Add recovery/dense/hit-test regression tests and run UI/controller verification.**

Keep and adapt dense, hit-test, selection, stale, unavailable, and global-shortcut tests. Add:

```rust
#[test]
fn stale_and_low_power_freeze_phase_and_spinner_geometry() {
    let live = render_phase_and_cores(&connected_app_with_phase(0.3), false);
    let stale = render_phase_and_cores(&stale_app_captured_from(0.3, 0.9), false);
    let low_power = render_phase_and_cores(&low_power_app_captured_from(0.3, 0.9), true);
    assert_eq!(phase_and_core_geometry(&stale), phase_and_core_geometry(&live));
    assert_eq!(phase_and_core_geometry(&low_power), phase_and_core_geometry(&live));
}
```

Ensure the helper compares geometry rather than colors because stale dimming and state colors intentionally differ. Then run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo test app::tests::low_power
cargo test cli::tests::collage_
cargo check
```

Expected: every command exits 0.

- [ ] **Step 7: Commit the renderer and App-policy slice.**

```bash
but commit agent-pulse-lissajous-scope -m "feat: render Agent Pulse Lissajous scope"
```

### Task 3: Synchronize current docs and run the release-quality gate

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-lissajous-scope.md` only to check completed steps/record manual checks after implementation

**Interfaces:**

- Consumes: completed Dual Phase Scope behavior and `2026-07-19-agent-pulse-lissajous-scope-design.md` as the authoritative presentation decision.
- Produces: consistent user docs and a historical Kinetic Collage spec that preserves its integration/privacy context.

- [ ] **Step 1: Write documentation assertions as exact copy targets.**

Update each current user-facing document to say all of the following:

```text
Agent Pulse opens a full-screen Dual Phase Scope with two real-audio Lissajous traces.
Agent frames keep state-colored edges; Working has an audio-driven spinner core,
while Idle/Blocked/Done remain stationary.
A selected named agent shows only `name · status`.
```

Record that phase data uses played stereo pairs when available and distinct real-sample lags for mono, that no trace is a scrolling waveform, and that low power/stale freeze scope/core geometry. Keep the manual checks explicitly unchecked until a human runs live mono and stereo streams, six themes, resize/dense agents, click/keyboard selection, reconnect, low power, standalone, and disabled launch.

- [ ] **Step 2: Mark the Kinetic Collage presentation historical without rewriting its integration record.**

At the top of `docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`, add a dated note stating that its local-only/read-only/privacy/recovery contracts remain historical context, while its waveform/FFT background and album-art tile presentation are superseded by the 2026-07-19 Lissajous Scope design. Link to the new design. Do not alter the old body or erase the design history.

Update `AGENTS.md` pointers so a future agent reads the Lissajous design/plan before changing Agent Pulse presentation.

- [ ] **Step 3: Inspect the documentation diff and run the full automated gate.**

Run:

```bash
but diff agent-pulse-lissajous-scope
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: documentation is accurate and every command exits 0. Do not mark live manual checks as complete based on these commands.

- [ ] **Step 4: Commit documentation and manual-verification record.**

```bash
but commit agent-pulse-lissajous-scope -m "docs: document Agent Pulse Lissajous scope"
```

## Plan self-review

- **Spec coverage:** Task 1 implements the required real stereo/mono paired-audio source and safe `VizFrame` representation. Task 2 replaces only the waveform/motif presentation with dual phase traces and status cores while preserving selection, privacy, dense layout, stale, unavailable, low-power, and input contracts. Task 3 records the new authoritative presentation and retains historical records.
- **Placeholder scan:** No task uses TBD/TODO, deferred test bodies, or unspecified error handling. The only manual items are explicitly named live checks that cannot be truthfully automated.
- **Type consistency:** `PlayedSample` feeds analyzer history; analyzer produces `PhaseTrace` fields through `VizFrame::with_phase`; UI consumes those fields via `PhaseCell`/`PhaseLayer`; App owns low-power/stale visual captures; CLI configures low-power policy once.
