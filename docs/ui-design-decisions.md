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

### Agent Pulse: Quiet Count + Agent Planets Stage

The optional Herdr Agent Pulse uses a one-line **quiet count** in normal
layouts and a full-screen **Agent Planets** stage as its only rich
surface. The current agent presentation decisions are
`docs/superpowers/specs/2026-07-19-agent-planets-stage-design.md`
(approved 2026-07-19) for the centered stage and disc-mask planets,
`docs/superpowers/specs/2026-07-19-agent-planets-details-modal-design.md`
for the on-demand Agent details record that replaced the stage's
permanent Side Tags,
`docs/superpowers/specs/2026-07-19-agent-planets-surface-status-design.md`
for the interior-only surface status,
`docs/superpowers/specs/2026-07-19-agent-planets-solar-orbit-design.md`
for the static central sun and Working-only invisible orbits that
replaced audio-driven planet body motion, and
`docs/superpowers/specs/2026-07-19-agent-planets-orbiting-particles-focus-design.md`
as revised (the approved revision removed that design's orbiting
particles) for the selection focus brackets. That revision's thin status
atmospheres — which had replaced the status rings, Working arcs, and Done
satellites — are now historical, superseded by the surface-status
design's interior treatments. The
stage design supersedes the stage layout, shadowed planet
geometry, and selected-only callout of
`docs/superpowers/specs/2026-07-19-agent-pulse-pocket-planets-design.md`
(whose Banded Worlds surface palette and privacy contracts remain
current). Pocket Planets superseded the planet scale/surface presentation
of
`docs/superpowers/specs/2026-07-19-agent-pulse-ringed-planets-design.md`
(whose privacy/selection contracts remain current while its ring state
language is now historical), which in turn superseded the square
agent-frame presentation of
the still-current Dual Phase Scope decision
`docs/superpowers/specs/2026-07-19-agent-pulse-lissajous-scope-design.md`
(approved 2026-07-19). The Lissajous Scope design superseded the
presentation decisions of the
interim `docs/superpowers/specs/2026-07-18-agent-pulse-kinetic-collage-design.md`
(Kinetic Collage album-art tiles over a scrolling waveform/FFT trace), which
in turn superseded the original
`docs/superpowers/specs/2026-07-16-herdr-agent-pulse-design.md`
(Quiet Companion summary + Status Constellation overlay), the interim
`docs/superpowers/specs/2026-07-18-agent-pulse-beat-orbit-design.md` (Beat
Orbit ring canvas), and the interim
`docs/superpowers/specs/2026-07-18-agent-pulse-bioluminescent-current-design.md`
(Bioluminescent Current flow canvas). The earlier
modal/list/card/completed-history surfaces remain removed; the 2026-07-16
design's local-only and read-only privacy boundaries remain in force. Agent
Pulse opens a centered Agent Planets stage: station title and volume
context around the unchanged Dual Phase Scope's two real-audio Lissajous
traces, behind a quiet solar system of small round disc-mask planets
slowly orbiting a static central sun with status held inside each disc —
an oscilloscope, never a work-management dashboard.

Implications:

- **Quiet count and discovery.** Wide and Medium add exactly one
  `● n active` line to Now Playing — a count of every agent on the session's
  socket, never names, output, or prompts — using theme colors only. Stale
  dims the count; unavailable removes the line. While the summary is
  visible (live or stale), the Wide/Medium footer appends exactly one
  `a Agent Planets` hint; Compact, standalone, disabled, ineligible, and
  unavailable states append nothing, so those footers stay byte-identical.
- **Compact suppression.** The Compact tier shows no Agent Pulse line to
  preserve its Split Mini station and playback context; while the
  integration is active, `a` still opens the stage there. Signal View keeps
  its restricted key contract: it never shows Agent Pulse and ignores `a`.
- **Standalone invisibility.** Ineligible and standalone launches render
  byte-identical to the pre-integration UI: no reserved rows, no empty slots,
  no "not in Herdr" hints, and mouse capture stays off.
- **Centered Agent Planets stage.** `a` opens a single full-screen view
  that replaces the whole player surface with the same centered hierarchy
  as Single View: a Title Case `Agent Planets · n active` heading, the
  current ICY title (falling back to the station name, then calm
  no-station copy), the exact Single View volume line directly beneath
  that title as the lowest title-metadata row, the scope/planet field, and
  a compact footer with selection/player/close hints that never advertises
  `z`. There is no separate status/context line and no dedicated volume
  gauge row; the title and volume reuse existing player state and expose
  no agent data.
- **Unchanged Dual Phase Scope field.** The field behind the planets is two
  overlapping, centered, low-contrast phase portraits of paired played
  samples — the primary in the theme's main visualizer color, the secondary
  in its complementary color — plus up to two dim phosphor-persistence
  layers from recent real visualizer frames. No trace is a scrolling
  amplitude-over-time waveform. Stereo output pairs the played left/right
  samples for the primary trace; mono output pairs the played mono mix with
  itself at a documented 29-sample lag, and the secondary trace always uses
  a distinct 97-sample mono lag, so every supported stream draws a real
  Lissajous figure.
- **Round disc-mask planets with Banded Worlds surfaces.** Every agent is
  one small, stable planet whose orbit — and therefore position at any
  orbit phase — derives deterministically from the agent's private
  identity. Planet bodies use one of four explicit
  round disc masks — 7×5, 5×3, 3×3, or a single cell — never a calculated
  rectangle/ellipse silhouette that could read as a cross, and never a
  full-tile shadow: disc masks replaced the earlier rectangle shadows and
  calculated planet silhouettes, so the scope stays readable around and
  between discs. Each private identity owns a stable Banded gas, Ice-cap,
  or Cratered-rock surface painted only on mask cells with two stable
  active-theme spectrum colors; the surface is identity language only and
  never signals status, audio, time, or selection. Dense terminals fall
  through the masks 7×5 → 5×3 → 3×3 → one selectable body cell, scaling
  orbit radii to the field, rather than grouping or omitting planets; only
  a one-cell body that cannot keep the required gap off the sun is dropped
  — never the sun.
- **A quiet solar system: Working-only clock orbits, audio-still bodies.**
  One small static, theme-derived sun sits at the field center —
  decoration, never a hit target, hidden only while unavailable. Every
  planet owns a seed-derived invisible concentric circular orbit around
  it: radius, initial angle, and a deliberately slow bounded angular speed
  all derive from the private identity, with no orbit guide line ever
  rendered. Only Working planets move, advancing from elapsed monotonic
  Working time; a Working→non-Working transition freezes the planet at
  its captured angle and a later Working stretch resumes from it. Audio
  never scales, offsets, or moves a planet body: RMS drives only the
  breathing theme-phosphor vignette and gentle trace brightness. Identical
  visualizer data at identical orbit phases renders identical cells;
  silence leaves the scope dim and still. Low-power mode
  freezes trace, persistence, and planet disc/orbit-phase/bracket
  geometry — using the first audible visualizer frame captured after
  startup (until audio becomes audible, the live frame renders) — while
  fresh agent snapshots may still update the per-status interior
  treatment and colors.
- **State inside the surface, never a cross.** Status never draws outside
  a planet's disc mask: it reuses existing body/surface cells in
  active-theme colors, with no exterior atmosphere, glow, ring, particle,
  shadow, or orbit line — interior status replaced the earlier status
  atmospheres, orbits, Working arcs, Done satellites, and orbiting
  particles. Working advances a narrow bright identity-surface band
  through the body cells, only on newly played audio data; Idle stays
  still and muted; Blocked weakly pulses one existing crater/surface cell
  in the error color with a deterministic, irregular pulse — never a cross
  glyph, blink timer, or broken orbit; Done keeps its whole body dim until
  its snapshot removes it; Unknown stays muted and nearly still. Silence
  rests every interior treatment; one-cell discs keep their body but omit
  status detail. The body palette never encodes status.
  Stale freezes the last live composition — traces, the sun, discs at
  frozen orbit positions, interior status, and brackets — dimmed under a
  quiet `· reconnecting` note on the stage heading; unavailable closes
  details and hides the sun and planets behind one calm
  `agents · unavailable · retrying` line while the stage chrome stays.
- **On-demand details, no permanent labels.** The stage field renders no
  agent text. Selecting a planet (`Tab`/`↓`/`j` for the next planet,
  wrapping last → first, and `Shift+Tab`/`↑`/`k` for the previous,
  wrapping first → last — cyclic only while the stage selection is
  interactive — or a click on its disc body cells) marks it with four
  theme-colored corner brackets around its tile: a foreground-only focus
  treatment with no painted selection background, decorative and never a
  hit target, and the identity surface is never restyled by selection. `Enter`
  opens a centered read-only Agent details record for the selected live
  planet showing only non-empty `name`, `agent`, normalized `status`, and
  `activity` (`terminal_title`) rows; `Enter`/`Esc` close only the record
  and `a` closes the record and the stage. Pane ids, workspace ids,
  working directories, terminal/session ids, and raw status never render.
- **Player-first input.** The stage consumes search and station
  navigation/selection keys, but the documented global player shortcuts —
  `Space`, `+`/`-`, `f`, `t`, and `v` — fall through with their exact
  normal semantics while details are closed (an open details record
  consumes them). `z` is consumed as a no-op while the stage is open and
  never enters Single View from it; outside the stage `z` keeps its normal
  Signal View toggle. Mouse clicks only select planets (their disc body
  cells), and
  selection — mouse and keyboard alike — resolves only while the connection
  is live; during stale/unavailable states the frozen composition's
  selection cannot change (`a`/`Esc` still close the stage) — selection
  input should not act on possibly outdated data.

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
