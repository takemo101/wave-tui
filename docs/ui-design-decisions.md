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

### Lifecycle Splash

Startup and shutdown transitions should feel like a quiet polish layer, not a
blocking loading screen.

Implications:

- Startup shows a short, skippable, left-to-right pixel-art `WAVE` logo reveal.
- The startup label includes the package version (`wave-tui v...`) and the line
  `settling into the signal`.
- Startup omits the wave-glyph line; its motion is limited to the logo reveal so
  it stays readable and calm.
- Shutdown may show the farewell copy `thanks for listening` /
  `see you next wave` with a small calm wave animation.
- Splash rendering is theme-driven and separate from the audio visualizer. It
  must not change playback, search, settings, layout tiers, or key mappings.

### Wide Layout: Search Console

Wide terminals should prioritize online search and result evaluation.

Implications:

- Search input, loading/cache state, result count, and offline/error state should
  be prominent.
- Ranked station results should occupy the largest region.
- Results should use a responsive table-like presentation on wide panes when
  width allows, then collapse metadata before falling back to compact list rows.
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

`MIK-015` polish expands the visualizer from a single renderer to a small mode
set; `MIK-031` adds a third FFT-band member so the set is now six modes:
`SpectrumStack`, `PeakDots`, `SkylinePeaks`, `WaveScope`, `MirrorWave`, and
`AmbientPulse`, all implemented. `SpectrumStack`/`PeakDots`/`SkylinePeaks` are
FFT-band driven (`PeakDots` draws the current peak with a five-frame real-audio
trail, while `SkylinePeaks` is a calm skyline of bright peak caps over a digital
0/1 binary tail, distinct from the solid `SpectrumStack` bars), `WaveScope`/
`MirrorWave` draw the time-domain waveform, and `AmbientPulse` is an RMS/band
ambient glow. This requires `VizFrame` to carry a low-resolution normalized
waveform alongside FFT bands and RMS. All modes remain real-audio-driven and
stretch/interpolate their source data to fill the allocated visualizer pane
width; this means using the current pane fully, not turning Wide or Compact into
a full-width/full-screen visualizer layout.

The `v` key cycles the visualizer mode and the selected mode is persisted.

### Signal View: Quiet Signal

Signal View is an opt-in visual-player surface entered with `z`, using the
**Quiet Signal** direction selected from the design deck: a center-stage view of
the current station with minimal chrome, generous negative space, a subdued
key-hint footer, and a large theme-driven visualizer.

Implications:

- Signal View is a temporary display mode, not a layout tier and not a
  navigation pane; it does not change the default Wide/Medium/Compact layouts and
  is not persisted across launches.
- It hides the Search, Browse, and Stations discovery UI and shows the current
  station (ICY title when available, otherwise station name) with an idle prompt
  when no station is selected.
- The visualizer reuses the currently selected mode and theme and takes the
  largest flexible region, so it is meaningfully larger than the normal Now
  Playing visualizer on medium and large panes. It does not introduce a Signal
  View-specific visualizer.
- Favorite state uses the same calm star language as station lists: `★` appears
  only when the current station is favorited, and non-favorites show no empty
  marker. Volume is shown as a thin, near-full-width bar without attaching the
  visualizer mode label to the volume control.
- Allowed keys are `z`/`Esc` (back), `q` (quit), `Space`, `+`/`-`, `v`, `t`, and
  `f`; `f` favorites the current station, not the hidden station-list selection.
  Discovery, navigation, and focus keys are ignored silently.
- Signal View adds no playlist, queue, search, or new selection behavior and does
  not turn compact panes into a default full-screen visualizer.

### Agent Pulse: Quiet Count + Bioluminescent Current

The optional Herdr Agent Pulse uses a one-line **quiet count** in normal
layouts and a full-screen, music-reactive **Bioluminescent Current** canvas
as its only rich surface. The current presentation decision is
`docs/superpowers/specs/2026-07-18-agent-pulse-bioluminescent-current-design.md`
(approved 2026-07-18), which supersedes the presentation decisions of the
original `docs/superpowers/specs/2026-07-16-herdr-agent-pulse-design.md`
(Quiet Companion summary + Status Constellation overlay) and the interim
`docs/superpowers/specs/2026-07-18-agent-pulse-beat-orbit-design.md` (Beat
Orbit ring canvas). The earlier modal/list/card/completed-history surfaces
are removed; the 2026-07-16 design's local-only and read-only privacy
boundaries remain in force. Agent Pulse presents agent activity as ambient
light inside a music visualizer, never as a work-management dashboard.

Implications:

- **Quiet count.** Wide and Medium add exactly one `● n active` line to Now
  Playing — a count of every agent on the session's socket, never names,
  output, or prompts — using theme colors only. Stale dims the count;
  unavailable removes the line.
- **Compact suppression.** The Compact tier shows no Agent Pulse line to
  preserve its Split Mini station and playback context; while the
  integration is active, `a` still opens the canvas there. Signal View keeps
  its restricted key contract: it never shows Agent Pulse and ignores `a`.
- **Standalone invisibility.** Ineligible and standalone launches render
  byte-identical to the pre-integration UI: no reserved rows, no empty slots,
  no "not in Herdr" hints, and mouse capture stays off.
- **Bioluminescent Current canvas.** `a` opens a single full-screen view that
  replaces the whole player surface. A continuous current derived from the
  played-sample FFT bands flows across the screen — per-band magnitude sets
  its height and glyph weight (`·`/`~`/`≈`/`≋`) — and every agent is one
  state-glyph light (`●` working, `◆` blocked, `○` idle, `✓` done, `?`
  unknown) at a stable, identity-derived position along the flow. Dense
  terminals shrink spacing rather than omitting lights.
- **Music-driven, not timer-driven.** Light glow, halo size, and a short
  upstream trail react to the current RMS and the light's assigned FFT band;
  trails are drawn from real recent visualizer frames. Silence leaves the
  current and lights dim and still by construction; nothing animates on a
  clock. Low-power mode freezes flow, light positions, and trails flat while
  state colors and minimal brightness still update.
- **Restrained signal color.** Only working (playing color) and blocked
  (error color) get strong color; idle, done, and unknown stay muted, and
  done lights fade until their snapshot removes them. Stale freezes the last
  live field dimmed under a single `stale · reconnecting` banner; unavailable
  hides every light behind one calm `agents · unavailable · retrying` line.
- **Selected-name-only privacy.** Selecting a light (`Tab`/`Shift+Tab`/
  arrows/`j`/`k`, or a click on its cells) shows only `name · status` when
  the agent has an explicit Herdr `name`; an unnamed selection shows no label
  at all. Pane ids, workspace ids, working directories, and agent types never
  render.
- **Player-first input.** The canvas consumes search and station
  navigation/selection keys, but the documented global player shortcuts —
  `Space`, `+`/`-`, `f`, `t`, `v`, and `z` (Signal View) — fall through with
  their exact normal semantics. Mouse clicks only select lights and resolve
  only while the connection is live; keyboard selection over the last known
  lights keeps working during stale/unavailable states — pointer input
  should not act on possibly outdated data.

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

`MIK-017` polish expands the set to six themes while preserving Minimal as the
default. The `t` key cycles them in this order:

1. `Minimal`
2. `Neon`
3. `CRT`
4. `Solarized`
5. `Midnight`
6. `Sakura`

`Solarized`, `Midnight`, and `Sakura` were introduced as named placeholder
palettes (`MIK-028`) and now carry their own distinct colors (`MIK-029`):
Solarized is a muted teal base with blue/cyan/yellow accents, Midnight a deep
navy base with blue/violet accents, and Sakura a warm dark base with rose/pink
accents — each readable on a dark terminal canvas with a meaningful low/mid/high
spectrum split. The `t` key remains a simple one-way cycle; no picker is planned
for six themes.

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

Browse sources act as filters over the current Radio Browser search results when
a successful search result population exists; otherwise they fall back to curated
catalog sources. `All Stations` shows all current results, while section and
category sources filter the full result population (category membership inferred
from a conservative tag/name alias dictionary). Browse rail labels stay stable —
`All Stations` is never renamed to "Search Results" — and the search/status strip
carries the active filter context (for example `filter: Jazz`). A genre filter
with zero matches in the current search shows a specific empty state such as
`No Jazz results in current search` rather than silently reverting to curated
stations. Clearing search preserves the active Browse source but rebuilds it from
the curated catalog. `Favorites` is never a search filter; it always shows saved
favorites.

### UX Design Deck Confirmation

A follow-up UI/UX design deck confirmed the concrete visual direction for
`MIK-014` through `MIK-017`:

- **Wide shell: Quiet Source Rail** — keep Browse as a quiet left rail, ranked
  Results as the central workspace, and Now Playing/visualizer on the right.
  Results may render as a station-comparison table at wide widths, collapsing
  metadata as space narrows. This preserves the Search Console direction while
  making Browse actionable.
- **Browse/Favorites: Source Picker + Focus Handoff** — Browse is a source
  picker; applying a source with `Enter` moves focus to Stations so the next
  action can be station selection or playback. Favorites uses the same source
  model and shows an explicit empty state.
- **Visualizer language: Six-mode Calm Suite** — use the six-mode set
  (`SpectrumStack`, `PeakDots`, `SkylinePeaks`, `WaveScope`, `MirrorWave`,
  `AmbientPulse`) with calm defaults and pane-width interpolation rather than a
  full-screen or full-width layout takeover.
- **Theme expansion: Calm Six-pack** — expand to Minimal, Neon, CRT, Solarized,
  Midnight, and Sakura. The added themes should broaden mood while staying
  suitable for long work sessions.

These deck choices are the implementation contract for the polish issues unless
superseded by a later design decision. Later splash polish (`MIK-037` through
`MIK-039`) adds the quiet lifecycle splash described above while preserving the
same Quiet Focus Pane constraints.

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
