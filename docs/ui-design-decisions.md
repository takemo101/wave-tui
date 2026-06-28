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

The primary/default visualizer should be vertical FFT bars inspired by cliamp.

Implications:

- Keep `SpectrumStack` as the default mode across all layout tiers.
- Map FFT bands to theme colors: low, mid, high.
- Minimal theme should render restrained bars.
- Neon and CRT can use stronger contrast/glow-like color choices.
- Low-power mode should lower update cadence, not change the visual language.

Planned `MIK-015` polish expands the visualizer from a single renderer to a
small mode set: `SpectrumStack`, `PeakDots`, `WaveScope`, `MirrorWave`, and
`AmbientPulse`. This requires `VizFrame` to carry a low-resolution normalized
waveform alongside FFT bands and RMS. All modes remain real-audio-driven and
must stretch/interpolate their source data to fill the allocated visualizer pane
width; this means using the current pane fully, not turning Wide or Compact into
a full-width/full-screen visualizer layout.

The `v` key cycles the visualizer mode and the selected mode is persisted.

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

Planned `MIK-017` polish expands the set to six themes while preserving Minimal
as the default:

1. `Minimal`
2. `Neon`
3. `CRT`
4. `Solarized`
5. `Midnight`
6. `Sakura`

The `t` key remains a simple one-way cycle; no picker is planned for six themes.

### Browse and Favorites Polish

Planned `MIK-014`/`MIK-016` polish changes the Wide `Browse` pane from static
category context into a flat list-source picker. The active station list source
should be explicit: All Stations, Favorites, sections, categories, or Search.

Implications:

- Browse displays `All Stations`, `Favorites`, `Music`, all Music categories,
  `Spoken / News`, and all Spoken categories in one flat list.
- When Browse is focused, `j`/`k` and arrows move the Browse selection; `Enter`
  applies that source and moves focus to Stations.
- When Stations is focused, the same navigation keys continue to move station
  selection and `Enter` plays the selected station.
- Favorites is a real source built from persisted favorite station entries, not
  only a marker inside the current catalog/search list.
- Removing a favorite while the Favorites source is active removes it from that
  list immediately and clamps selection.
- Empty Favorites stays in the Favorites source and shows an explicit helpful
  empty state.
- Clearing Search restores the previous non-search source rather than always
  forcing All Stations.

### UX Design Deck Confirmation

A follow-up UI/UX design deck confirmed the concrete visual direction for
`MIK-014` through `MIK-017`:

- **Wide shell: Quiet Source Rail** — keep Browse as a quiet left rail, ranked
  Results as the central workspace, and Now Playing/visualizer on the right.
  This preserves the Search Console direction while making Browse actionable.
- **Browse/Favorites: Source Picker + Focus Handoff** — Browse is a source
  picker; applying a source with `Enter` moves focus to Stations so the next
  action can be station selection or playback. Favorites uses the same source
  model and shows an explicit empty state.
- **Visualizer language: Five-mode Calm Suite** — use the five-mode set
  (`SpectrumStack`, `PeakDots`, `WaveScope`, `MirrorWave`, `AmbientPulse`) with
  calm defaults and pane-width interpolation rather than a full-screen or
  full-width layout takeover.
- **Theme expansion: Calm Six-pack** — expand to Minimal, Neon, CRT, Solarized,
  Midnight, and Sakura. The added themes should broaden mood while staying
  suitable for long work sessions.

These deck choices are the implementation contract for the polish issues unless
superseded by a later design decision.

## Implementation Notes

- Store theme as a stable lowercase string. Current names are `minimal`, `neon`,
  and `crt`; planned names add `solarized`, `midnight`, and `sakura`.
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
