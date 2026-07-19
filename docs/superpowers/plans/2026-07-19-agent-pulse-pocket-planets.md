# Agent Pulse Pocket Planets Implementation Plan

> **Historical note (2026-07-19):** this plan completed (rendering slice
> committed as `feat: refine Agent Pulse pocket planets`, docs pass as
> `docs: document Agent Pulse pocket planets`). Its stage layout, shadow,
> and selected-only callout presentation is superseded by
> [`2026-07-19-agent-planets-stage.md`](2026-07-19-agent-planets-stage.md),
> and the ring/satellite status language it preserved is superseded by
> [`2026-07-19-agent-planets-orbiting-particles-focus.md`](2026-07-19-agent-planets-orbiting-particles-focus.md)
> as revised (thin status atmospheres, no orbiting particles);
> the completed task record below is preserved unchanged.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refine Ringed Planets into small, theme-colored Banded Worlds while leaving the shipped Dual Phase Scope and selected-agent callout behavior intact.

**Architecture:** Restrict code work to `ui::agent_pulse`: cap the planet body rectangle before existing ellipse/ring geometry, derive a stable private-seed `PlanetSurface` plus two theme spectrum colors, and paint surface cells inside the existing hit-tested body. Reuse `PlanetGeometry`, status rings, callouts, phase layers, App capture, and input paths unchanged; add direct candidate-branch tests for the existing callout policy.

**Tech Stack:** Rust 2018, Ratatui `Buffer`, existing `Theme` spectrum palette, existing `PlanetGeometry`, pure UI tests, GitButler `but`.

## Global Constraints

- Preserve Dual Phase Scope phase data, two traces, Comet-free current rendering, persistence, silence threshold, audio timing, stale capture, and audible-first low-power capture exactly.
- Do not modify audio, model, App, CLI behavior, Herdr protocol/socket, persistence, controls, or add dependencies.
- Cap normal planet bodies at 9 terminal cells wide and 5 cells tall; rings may extend only one cell beyond the body. Dense reduction is body+ring+surface → body+ring → body → one body cell; never omit an agent.
- Use only active-theme colors (`Theme::spectrum_color`, foreground, muted, selection, playing, error); never hard-code RGB values.
- Surface palette/family is stable private identity language, never a status, audio, time, or selection signal. Status remains on the ring/satellite.
- Never render cross/cross-like blocked state. Blocked remains a broken `theme.error` orbit.
- Preserve planet-only hit testing; scope/persistence/shadow/callout/empty cells do not select.
- Preserve selected named `name · status` callout privacy, top-layer draw order, and unnamed no-label behavior.
- Before final commit run `cargo fmt --check`, focused tests, `cargo test`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo build --release`; live checks remain unchecked.

---

### Task 1: Cap geometry, paint Banded Worlds, and harden callout tests

**Files:**

- Modify: `src/ui/agent_pulse.rs:340-645, 920-1097, 1518-2618`

**Interfaces:**

- Consumes: existing `CollageTile`, `PlanetGeometry`, `Theme`, `AgentStatus`, `VizFrame`, and `selection_callout`.
- Produces: private `PlanetSurface::{BandedGas, IceCap, CrateredRock}`, `PlanetPalette { base, accent }`, `pocket_rect`, `planet_surface`, `planet_palette`, and `surface_cells`.
- Preserves: `phase_layers`, `planet_geometry` ring/hit-cell semantics, `render_canvas` draw order, status ring behavior, and existing input actions.

- [x] **Step 1: Add failing scale/surface/callout-branch tests.**

In `src/ui/agent_pulse.rs` tests, add fixtures for three identities and these tests:

```rust
#[test]
fn pocket_planet_caps_normal_body_and_keeps_dense_body_cells() {
    let area = Rect::new(0, 0, 120, 36);
    let tile = oversized_tile(area);
    let rect = pocket_rect(tile.rect, area);
    assert!(rect.width <= 9);
    assert!(rect.height <= 5);
    assert!(!planet_geometry(&tile, area, AgentStatus::Idle, &phase_frame()).body.is_empty());
    assert_eq!(dense_planet_body_count(80, Rect::new(0, 0, 50, 15)), 80);
}

#[test]
fn seed_stably_selects_each_banded_world_surface_and_palette() {
    assert_eq!(planet_surface(0), PlanetSurface::BandedGas);
    assert_eq!(planet_surface(1), PlanetSurface::IceCap);
    assert_eq!(planet_surface(2), PlanetSurface::CrateredRock);
    assert_eq!(planet_palette(17).base_position, planet_palette(17).base_position);
    assert_ne!(planet_palette(0).base_position, planet_palette(1).base_position);
}

#[test]
fn surface_cells_are_stable_across_audio_status_and_time() {
    let first = rendered_surface("gas", AgentStatus::Working, phase_frame_with_offset(0.1));
    let later = rendered_surface("gas", AgentStatus::Blocked, phase_frame_with_offset(0.8));
    assert_eq!(surface_geometry(&first), surface_geometry(&later));
}

#[test]
fn selection_callout_exercises_left_below_above_and_all_collide_fallback() {
    assert_eq!(forced_callout(PlacementCase::Left).rect, expected_left_rect());
    assert_eq!(forced_callout(PlacementCase::Below).rect, expected_below_rect());
    assert_eq!(forced_callout(PlacementCase::Above).rect, expected_above_rect());
    assert_eq!(forced_callout(PlacementCase::AllCollide).rect, expected_right_fallback_rect());
}
```

The callout test must build synthetic `PlanetGeometry` values whose `hit_cells` deliberately occupy each preceding candidate row. It must not derive the expected answer by calling `selection_callout` through the render helper.

- [x] **Step 2: Run focused tests and verify failure.**

Run:

```bash
cargo test ui::agent_pulse::tests::pocket_planet_caps
cargo test ui::agent_pulse::tests::seed_stably_selects
cargo test ui::agent_pulse::tests::surface_cells_are_stable
cargo test ui::agent_pulse::tests::selection_callout_exercises
```

Expected: FAIL because pocket geometry, surface types/palettes, and synthetic candidate fixtures do not exist.

- [x] **Step 3: Cap body geometry before ellipse/ring calculation.**

Add constants and a helper:

```rust
const POCKET_MAX_W: u16 = 9;
const POCKET_MAX_H: u16 = 5;

fn pocket_rect(rect: Rect, area: Rect) -> Rect {
    let width = rect.width.min(POCKET_MAX_W).max(1);
    let height = rect.height.min(POCKET_MAX_H).max(1);
    let x = rect.x + rect.width.saturating_sub(width) / 2;
    let y = rect.y + rect.height.saturating_sub(height) / 2;
    clamp_rect(x as i32, y as i32, width, height, area)
}
```

In `planet_geometry`, call `pocket_rect(tile.rect, area)` before `body_cells` and `ring_cells`. Keep the current one/two-cell dense body fallback and current one-cell ring overhang limit. Do not change `collage_layout`, tile audio transforms, phase layers, or status ring functions.

- [x] **Step 4: Derive stable Banded Worlds surface and active-theme palette.**

Add:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanetSurface { BandedGas, IceCap, CrateredRock }

#[derive(Clone, Copy)]
struct PlanetPalette { base_position: f32, accent_position: f32 }

fn planet_surface(seed: u64) -> PlanetSurface {
    match seed % 3 {
        0 => PlanetSurface::BandedGas,
        1 => PlanetSurface::IceCap,
        _ => PlanetSurface::CrateredRock,
    }
}

fn planet_palette(seed: u64) -> PlanetPalette {
    const POSITIONS: [f32; 4] = [0.16, 0.38, 0.62, 0.84];
    let base = seed as usize % POSITIONS.len();
    PlanetPalette {
        base_position: POSITIONS[base],
        accent_position: POSITIONS[(base + 1 + ((seed >> 4) as usize % 2)) % POSITIONS.len()],
    }
}
```

Inside `render_planet`, obtain `base = theme.spectrum_color(palette.base_position)` and `accent = theme.spectrum_color(palette.accent_position)`. Paint only cells already in `geometry.body`:

- BandedGas: short horizontal rows selected by `(y - body_top + seed) % 3 == 0` use accent.
- IceCap: body cells in the top third use accent.
- CrateredRock: the existing stable `geometry.craters` use accent while all other body cells use base.

The body must use `base`; never use `status_color` for surface color. Keep ring/satellite/Working arc style code as current. Apply existing silent, stale, Done, and Unknown dimming to both surface styles. No fixed `Color::Rgb`/hex/color literals may appear.

- [x] **Step 5: Verify scope/status/hit parity and direct callout branches.**

Keep existing phase-scope, state ring, stale/low-power, selected callout, and planet-only hit tests. Add assertions that a ring/body cell selected from the smaller `PlanetGeometry` still resolves, while a former oversized-rectangle-only cell resolves nothing. Run:

```bash
cargo fmt --check
cargo test ui::agent_pulse::tests
cargo test cli::tests::collage_
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: all commands exit 0.

- [x] **Step 6: Commit the Pocket Planets rendering slice.**

```bash
but commit agent-pulse-ringed-planets -m "feat: refine Agent Pulse pocket planets"
```

### Task 2: Synchronize current docs and final verification

**Files:**

- Modify: `README.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/specs/2026-07-19-agent-pulse-ringed-planets-design.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-ringed-planets.md`
- Modify: `docs/superpowers/plans/2026-07-19-agent-pulse-pocket-planets.md` only to check completed steps after implementation

**Interfaces:**

- Consumes: completed Pocket Planets behavior and `2026-07-19-agent-pulse-pocket-planets-design.md` as the current presentation decision.
- Produces: truthful current docs, historical Ringed Planets presentation record, and an explicitly unchecked manual verification list.

- [x] **Step 1: Update current documentation with final Pocket Planets behavior.**

Describe the shipped canvas as the unchanged real-audio Dual Phase Scope behind Pocket Planets. Include these exact behavioral facts:

```text
Planets are capped at a 9×5 body with an optional one-cell ring overhang.
Each private identity owns a stable Banded gas, Ice-cap, or Cratered-rock surface using active-theme colors.
State remains on the ring: Working arc, Idle ring, Blocked broken error orbit, Done satellite; no cross glyph.
A selected named planet shows only `name · status` in a top-layer callout.
```

Preserve accurate mono/stereo lags, no-scrolling-waveform, stale/unavailable, and audible-first low-power capture wording. Clarify that low-power freezes positions while fresh snapshots may update status ring treatment. Point `AGENTS.md` to the Pocket Planets spec/plan as the current Agent Pulse presentation references.

- [x] **Step 2: Mark prior planet presentation documents historical.**

Add a dated note to `2026-07-19-agent-pulse-ringed-planets-design.md`: its local-only/read-only/privacy/selection/recovery contracts remain current, while its larger grey planet scale/surface presentation is superseded by Pocket Planets. Preserve its body.

At the top of the earlier Ringed Planets plan, add a brief historical completion/supersession note; retain its checked implementation record and do not rewrite completed task details.

- [x] **Step 3: Record manual checks and run the final gate.**

In `docs/SPEC.md`, leave every live item unchecked: mono/stereo streams, all six themes, resize and dense planet field, mouse/keyboard callout readability and collision fallback, reconnect, low-power, standalone, and disabled launch. Inspect the docs diff and run:

```bash
but diff agent-pulse-ringed-planets
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: every command exits 0; automated commands do not check manual boxes.

- [x] **Step 4: Commit final documentation.**

```bash
but commit agent-pulse-ringed-planets -m "docs: document Agent Pulse pocket planets"
```

## Plan self-review

- **Spec coverage:** Task 1 caps planets, adds stable theme-derived Banded Worlds, preserves scope/status/hit behavior, and closes the direct callout-branch test gap. Task 2 documents the current presentation, retains historical records, and records honest manual work.
- **Placeholder scan:** No unresolved markers, omitted tests, or deferred implementation details remain. Manual checks are named, intentionally unchecked live verification.
- **Type consistency:** `pocket_rect` feeds `PlanetGeometry`; `planet_surface`/`planet_palette` feed `render_planet`; the existing `PlanetGeometry::hit_cells` feeds `hit_test`; `selection_callout` retains its existing production signature and receives direct synthetic tests.
