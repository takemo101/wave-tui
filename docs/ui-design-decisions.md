# UI Design Decisions

This document records the selected visual direction from the design deck for the
`wave-tui` replacement.

## Selected Direction

### Overall Personality: Quiet Focus Pane

The app should feel calm enough to keep open during work.

Implications:

- Default visual density should be moderate, not flashy.
- Minimal should be the recommended first-run/default theme.
- The UI should reserve strong color and motion for useful signal:
  playback state, selected row, errors, and audio-reactive bars.
- The app can still support vivid modes through Neon and CRT themes.

### Wide Layout: Search Console

Wide terminals should prioritize online search and result evaluation.

Implications:

- Search input, loading/cache state, result count, and offline/error state should
  be prominent.
- Ranked station results should occupy the largest region.
- Music and Spoken/News categories should remain visible as shortcuts.
- Now Playing and the FFT visualizer should remain visible while searching.

### Medium and Compact Layout: Split Mini

Constrained panes should keep both station context and playback context visible.

Implications:

- Do not default to a full-screen visualizer in compact mode.
- Do not hide the station list behind a drawer for MVP.
- Reduce metadata detail and visualizer height before hiding either the list or
  player region.
- Compact mode should show at least a few station/search rows when possible.

### Visualizer: Spectrum Stack

The primary visualizer should be vertical FFT bars inspired by cliamp.

Implications:

- Use one shared `SpectrumStack` component across all layout tiers.
- Map FFT bands to theme colors: low, mid, high.
- Minimal theme should render restrained bars.
- Neon and CRT can use stronger contrast/glow-like color choices.
- Low-power mode should lower update cadence, not change the visual language.

### Theme Set: High Contrast Trio

Initial themes:

1. `Minimal`
   - quiet dark work-session theme
   - recommended default
2. `Neon`
   - high-energy cliamp-like theme
   - cyan/magenta/green/yellow/red accents
3. `CRT`
   - retro green/amber terminal theme
   - nostalgic but still readable

Themes should differ clearly enough that switching themes feels meaningful.

## Implementation Notes

- Store theme as a stable lowercase string: `minimal`, `neon`, `crt`.
- Unknown theme names should fall back to `minimal`.
- Theme structs should centralize all UI colors; rendering code should not
  hard-code palette values.
- Layout tier selection remains width/height based, but each tier has a named
  design contract:
  - Wide: Search Console
  - Medium: Split Mini
  - Compact: Split Mini reduced

## Updated Documents

These decisions are also reflected in:

- `docs/SPEC.md`
- `docs/superpowers/plans/2026-06-27-radio-replacement.md`
