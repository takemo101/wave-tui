//! Visualizer rendering for the Now Playing pane.
//!
//! This module owns the six-mode "Calm Suite" visualizer: the mode dispatch
//! ([`render_visualizer`]), each per-mode renderer, and the pure
//! sampling/cell-writing helpers they share. [`crate::ui`] keeps layout and pane
//! orchestration and calls only [`render_visualizer`]; every other item here is
//! private to this module.
//!
//! Like the rest of [`crate::ui`], rendering is read-only: each renderer is
//! driven by real [`VizFrame`] data (FFT bands, RMS, or the time-domain waveform),
//! the pane area, and the active [`Theme`]. No RNG, wall-clock time, or fake
//! animation is used; PeakDots may read the app's short VizFrame history to draw
//! real frame trails. All colors come from the active [`Theme`]; this module
//! hard-codes no palette values.

use ratatui::{buffer::Buffer, layout::Rect};

use crate::app::App;
use crate::model::VisualizerMode;
use crate::theme::Theme;

/// Draw the visualizer pane using the app's selected [`VisualizerMode`].
///
/// Every mode in the six-mode Calm Suite has a dedicated renderer, each driven
/// from the real [`VizFrame`] (FFT bands, RMS, or the time-domain waveform) and
/// each stretched to the full pane width. The match is exhaustive on purpose
/// (MIK-026): adding a future mode is a compile error here until it is wired,
/// rather than silently falling back to the Spectrum Stack.
pub(super) fn render_visualizer(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    match app.visualizer_mode() {
        VisualizerMode::SpectrumStack => render_spectrum(theme, app, area, buf),
        VisualizerMode::PeakDots => render_peak_dots(theme, app, area, buf),
        VisualizerMode::SkylinePeaks => render_skyline_peaks(theme, app, area, buf),
        VisualizerMode::WaveScope => render_wave_scope(theme, app, area, buf),
        VisualizerMode::MirrorWave => render_mirror_wave(theme, app, area, buf),
        VisualizerMode::AmbientPulse => render_ambient_pulse(theme, app, area, buf),
    }
}

/// Spectrum-column particle glyphs, ordered faint → heavy. A cell's depth within
/// its column (heavy at the base, fine toward the top) selects the grain, so a
/// column fills like a textured analyzer bar made of particles instead of a solid
/// block. All are calm one-cell dot glyphs — no `*`, no `█` bars, no mosaic
/// block/shade glyphs, no stars/diamonds; no color lives here (colors come from
/// [`Theme`]).
const DUST_GLYPHS: [char; 3] = ['·', '∙', '•'];

/// The shared "Spectrum Stack": a traditional bottom-up **spectrum analyzer**
/// whose columns are filled with **particles** instead of solid bars (MIK-035).
///
/// This single renderer is used by every layout tier. Each band maps to a column
/// (stretched to the full pane width via [`spectrum_columns`]); the column's height
/// is `round(magnitude * pane_height)`, so the **silhouette is the real spectrum
/// shape**. Every cell from the floor up to that height is filled: the grain
/// ([`DUST_GLYPHS`] `·∙•`) is chosen by the cell's *depth* in the column — heavy
/// `•` at the base fading to fine `·` near the top — themed via
/// [`Theme::spectrum_color`] (the faint top is muted). The renderer is a pure,
/// static function of `(frame, area, theme)` — no RNG, no time/frame/tick state —
/// so the same frame always renders identically. A silent column draws nothing.
/// All colors come from the active [`Theme`].
fn render_spectrum(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = spectrum_columns(app.viz().bands(), area.width as usize);
    let top = DUST_GLYPHS.len() - 1;
    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        // The column height is the real spectrum shape (the analyzer silhouette).
        let fill = (magnitude * area.height as f32).round() as u16;
        if fill == 0 {
            continue; // silent/too-quiet column: draw nothing
        }
        let column_color = theme.spectrum_color(position);
        let x = area.x + i as u16;
        for row in 0..fill {
            let y = area.y + area.height - 1 - row;
            // Grain by depth in the column: heavy `•` at the base, fine `·` on top.
            let depth = 1.0 - row as f32 / fill as f32;
            let level = ((depth * DUST_GLYPHS.len() as f32) as usize).min(top);
            let color = if level == 0 {
                theme.muted
            } else {
                column_color
            };
            set_cell(buf, x, y, DUST_GLYPHS[level], color);
        }
    }
}

/// Write a single glyph in `fg` into the buffer if `(x, y)` is in bounds.
fn set_cell(buf: &mut Buffer, x: u16, y: u16, ch: char, fg: ratatui::style::Color) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch).set_fg(fg);
    }
}

/// PeakDots glyphs from current frame to oldest retained trail frame.
const PEAK_DOT_TRAIL_GLYPHS: [char; 6] = ['●', '•', '∙', '·', '·', '·'];

/// The "Peak Dots" visualizer: one themed current dot per pane column, plus a
/// short five-frame trail of quieter dots from recent real audio frames.
///
/// A distinct renderer from [`render_spectrum`]: it shares the full-pane-width
/// [`spectrum_columns`] sampling and the theme's low→mid→high spectrum gradient,
/// but draws only peak cells so the visualizer reads as a quiet peak band. Older
/// frames are drawn first with smaller/fainter glyphs, then the current frame is
/// drawn last as `●`, so overlap never dims the current peak. Columns whose
/// magnitude rounds to zero (silent/empty frames) draw nothing. The trail is
/// driven only by recent [`VizFrame`] history, never by fake animation.
fn render_peak_dots(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let history: Vec<_> = app.viz_history().enumerate().collect();
    for (age, frame) in history.into_iter().rev() {
        let glyph = PEAK_DOT_TRAIL_GLYPHS
            .get(age)
            .copied()
            .unwrap_or(*PEAK_DOT_TRAIL_GLYPHS.last().unwrap());
        let columns = spectrum_columns(frame.bands(), area.width as usize);
        for (i, (magnitude, position)) in columns.into_iter().enumerate() {
            let filled = (magnitude * area.height as f32).round() as u16;
            if filled == 0 {
                continue;
            }
            let color = if age >= 3 {
                theme.muted
            } else {
                theme.spectrum_color(position)
            };
            let x = area.x + i as u16;
            // The peak sits at the top of where a filled bar would reach.
            let y = area.y + area.height - filled;
            set_cell(buf, x, y, glyph, color);
        }
    }
}

/// The "Skyline Peaks" visualizer: a calm FFT skyline of bright peak caps over a
/// digital binary tail.
///
/// A third FFT-band Spectrum-family renderer alongside [`render_spectrum`] and
/// [`render_peak_dots`]. It shares the full-pane-width [`spectrum_columns`]
/// sampling and the theme's low→mid→high spectrum gradient, but draws each column
/// as a distinct silhouette: a bright cap glyph (`▀`) marks the peak and a
/// deterministic pseudo-random binary tail (`0`/`1`) fills the body below it down
/// to the floor. This reads quieter than the solid [`render_spectrum`] bars yet
/// carries a digital Matrix-like presence beyond the single [`render_peak_dots`]
/// dot, so the three FFT modes stay visibly distinct. Columns whose magnitude
/// rounds to zero (silent/empty frames) draw nothing. Pure function of the current
/// [`VizFrame`]; it carries no animation, RNG, wall-clock time, or mode-specific
/// state.
fn render_skyline_peaks(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = spectrum_columns(app.viz().bands(), area.width as usize);
    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        let filled = (magnitude * area.height as f32).round() as u16;
        if filled == 0 {
            continue;
        }
        let color = theme.spectrum_color(position);
        let x = area.x + i as u16;
        // The cap sits at the top of where a filled bar would reach; the tail is
        // the calm digital body beneath it, down to the floor.
        let cap_y = area.y + area.height - filled;
        for row in 0..filled.saturating_sub(1) {
            let y = area.y + area.height - 1 - row;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(skyline_binary_tail_digit(i, row, filled, magnitude))
                    .set_fg(color);
            }
        }
        if let Some(cell) = buf.cell_mut((x, cap_y)) {
            cell.set_char('▀').set_fg(color);
        }
    }
}

/// Stable pseudo-random binary digit for the SkylinePeaks tail.
///
/// This keeps the Matrix-like texture deterministic for tests and screenshots:
/// the same audio frame and pane always produce the same cells, while nearby
/// columns/rows still alternate enough to avoid a mechanical checkerboard.
fn skyline_binary_tail_digit(
    column: usize,
    row_from_floor: u16,
    filled: u16,
    magnitude: f32,
) -> char {
    let magnitude_bucket = (magnitude.clamp(0.0, 1.0) * 255.0).round() as u32;
    let mut hash = (column as u32).wrapping_mul(0x045d_9f3b)
        ^ (row_from_floor as u32).wrapping_mul(0x27d4_eb2d)
        ^ (filled as u32).wrapping_mul(0x1656_67b1)
        ^ magnitude_bucket.wrapping_mul(0x9e37_79b9);
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7feb_352d);
    hash ^= hash >> 15;
    if hash & 1 == 0 {
        '0'
    } else {
        '1'
    }
}

/// The "WaveScope" visualizer: an oscilloscope trace of [`VizFrame::waveform`].
///
/// One trace point per pane column, sampled from the full-width
/// [`waveform_columns`] interpolation so the scope spans the whole pane. A
/// sample of `0.0` sits on the vertical center, `+1.0` reaches the top, and
/// `-1.0` the bottom; the point's color comes from the theme's spectrum gradient
/// keyed on the signal's amplitude so louder excursions read brighter. Empty and
/// all-zero waveforms both render as a flat baseline (stable silence), per the
/// MIK-024 reviewer note. Pure function of the current [`VizFrame`].
fn render_wave_scope(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = waveform_columns(app.viz().waveform(), area.width as usize);
    let center = (area.height - 1) / 2;
    let half = (area.height - 1) as f32 / 2.0;
    let top = area.y;
    let bottom = area.y + area.height - 1;
    for (i, (sample, _position)) in columns.into_iter().enumerate() {
        let offset = (sample * half).round() as i32;
        let y = ((area.y + center) as i32 - offset).clamp(top as i32, bottom as i32) as u16;
        let color = theme.spectrum_color(sample.abs());
        let x = area.x + i as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char('•').set_fg(color);
        }
    }
}

/// The "MirrorWave" visualizer: a symmetrical oscilloscope around the center.
///
/// For each pane column the waveform sample's magnitude is mirrored above and
/// below the vertical center, giving a calmer, balanced scope than the raw
/// [`render_wave_scope`] trace. The center cell is always drawn so the baseline
/// stays visible, and louder samples extend the symmetric bars further out.
/// Color follows the theme's spectrum gradient keyed on amplitude. Empty and
/// all-zero waveforms render as the flat center baseline (silence). Pure
/// function of the current [`VizFrame`].
fn render_mirror_wave(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = waveform_columns(app.viz().waveform(), area.width as usize);
    let center = area.y + (area.height - 1) / 2;
    let reach_max = (area.height - 1) / 2;
    let bottom = area.y + area.height - 1;
    for (i, (sample, _position)) in columns.into_iter().enumerate() {
        let amplitude = sample.abs();
        let reach = (amplitude * reach_max as f32).round() as u16;
        let color = theme.spectrum_color(amplitude);
        let x = area.x + i as u16;
        for r in 0..=reach {
            let up = center as i32 - r as i32;
            if up >= area.y as i32 {
                if let Some(cell) = buf.cell_mut((x, up as u16)) {
                    cell.set_char('┃').set_fg(color);
                }
            }
            let down = center + r;
            if down <= bottom {
                if let Some(cell) = buf.cell_mut((x, down)) {
                    cell.set_char('┃').set_fg(color);
                }
            }
        }
    }
}

/// The "AmbientPulse" visualizer: a low-noise glow driven by RMS and bands.
///
/// Each column blends the interpolated FFT band magnitude with the frame RMS
/// into a calm level, drawn as a vertically centered shaded band whose height
/// and shade density (`░`/`▒`/`▓`) grow with that level. When the frame carries
/// no bands the RMS alone pulses uniformly across the pane, so the mode stays
/// real-data-driven rather than animating on its own. A silent frame draws
/// nothing. Bands are stretched to the full pane width via [`spectrum_columns`].
fn render_ambient_pulse(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let frame = app.viz();
    let rms = frame.rms();
    // Prefer the band shape for the per-column glow; with no bands the RMS still
    // pulses the whole pane so the mode reflects real playback level.
    let columns = if frame.bands().is_empty() {
        let last_col = area.width.saturating_sub(1) as usize;
        (0..area.width as usize)
            .map(|col| {
                let position = if last_col == 0 {
                    0.0
                } else {
                    col as f32 / last_col as f32
                };
                (rms, position)
            })
            .collect::<Vec<_>>()
    } else {
        spectrum_columns(frame.bands(), area.width as usize)
    };

    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        let level = (magnitude * 0.6 + rms * 0.4).clamp(0.0, 1.0);
        let shade = if level < 0.15 {
            None
        } else if level < 0.45 {
            Some('░')
        } else if level < 0.75 {
            Some('▒')
        } else {
            Some('▓')
        };
        let Some(shade) = shade else { continue };

        let fill = (level * area.height as f32).round() as u16;
        if fill == 0 {
            continue;
        }
        let start = area.y + (area.height - fill) / 2;
        let color = theme.spectrum_color(position);
        let x = area.x + i as u16;
        for y in start..(start + fill).min(area.y + area.height) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(shade).set_fg(color);
            }
        }
    }
}

/// Resample the time-domain `waveform` into one `(sample, position)` column per
/// pane cell so the waveform modes fill the full pane width.
///
/// `sample` is the interpolated signed amplitude in `-1.0..=1.0`, linearly
/// interpolated between the two nearest waveform points; `position` is the
/// column's normalized `0.0..=1.0` location, used for the theme color gradient.
/// An empty waveform is treated as flat silence: every column samples `0.0`, so an
/// empty and an all-zero waveform render identically (a flat baseline), per the
/// MIK-024 reviewer note. Returns an empty vector only for zero width.
pub(super) fn waveform_columns(waveform: &[f32], width: usize) -> Vec<(f32, f32)> {
    if width == 0 {
        return Vec::new();
    }
    let last_col = width - 1;
    let position_of = |col: usize| {
        if last_col == 0 {
            0.0
        } else {
            col as f32 / last_col as f32
        }
    };
    if waveform.is_empty() {
        return (0..width).map(|col| (0.0, position_of(col))).collect();
    }
    let last_sample = waveform.len() - 1;
    (0..width)
        .map(|col| {
            let position = position_of(col);
            let point = position * last_sample as f32;
            let lo = point.floor() as usize;
            let hi = (lo + 1).min(last_sample);
            let frac = point - lo as f32;
            let sample = waveform[lo] + (waveform[hi] - waveform[lo]) * frac;
            (sample.clamp(-1.0, 1.0), position)
        })
        .collect()
}

/// Resample the FFT `bands` into one `(magnitude, position)` column per pane
/// cell so the Spectrum Stack fills the full pane width.
///
/// `magnitude` is the bar height in `0.0..=1.0`, linearly interpolated between
/// the two nearest bands so columns stay smooth when the pane is wider than the
/// band count. `position` is the column's normalized `0.0..=1.0` location, used
/// by [`Theme::spectrum_color`] so the low→mid→high gradient stretches across
/// the whole pane rather than only the first `bands.len()` columns.
///
/// Pure and deterministic; returns an empty vector for empty bands or zero
/// width, so callers stay safe for tiny panes and silent/empty frames.
pub(super) fn spectrum_columns(bands: &[f32], width: usize) -> Vec<(f32, f32)> {
    if bands.is_empty() || width == 0 {
        return Vec::new();
    }
    let last_band = bands.len() - 1;
    let last_col = width - 1;
    (0..width)
        .map(|col| {
            let position = if last_col == 0 {
                0.0
            } else {
                col as f32 / last_col as f32
            };
            // Map the column onto the band range and interpolate between the two
            // nearest bands. Endpoints land exactly on the first/last band.
            let sample = position * last_band as f32;
            let lo = sample.floor() as usize;
            let hi = (lo + 1).min(last_band);
            let frac = sample - lo as f32;
            let magnitude = bands[lo] + (bands[hi] - bands[lo]) * frac;
            (magnitude.clamp(0.0, 1.0), position)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, App};
    use crate::audio::AudioEvent;
    use crate::catalog::Catalog;
    use crate::model::{VisualizerMode, VizFrame};
    use crate::settings::Settings;
    use crate::theme::{Theme, ThemeName};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn base_app() -> App {
        App::new(Settings::default(), Catalog::curated())
    }

    /// Flatten a buffer's cell symbols into newline-separated text.
    fn buffer_text(buf: &Buffer) -> String {
        let area = *buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        out
    }

    /// True if any cell in the buffer carries `fg`.
    fn has_fg(buf: &Buffer, fg: ratatui::style::Color) -> bool {
        let area = *buf.area();
        (0..area.height).any(|y| (0..area.width).any(|x| buf.cell((x, y)).unwrap().fg == fg))
    }

    /// Render only the active visualizer into a standalone buffer, the routed
    /// path used by Now Playing (mode-aware) rather than a fixed renderer.
    fn render_viz_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_visualizer(&theme, app, area, &mut buf);
        buf
    }

    /// Render only the Spectrum Stack into a standalone buffer so artifact and
    /// shape assertions are isolated from the surrounding panes (whose volume
    /// gauge also uses `░`).
    fn render_spectrum_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_spectrum(&theme, app, area, &mut buf);
        buf
    }

    /// Apply a steady viz frame whose bands are all `level`.
    fn with_flat_frame(level: f32, bands: usize) -> App {
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![level; bands],
            level,
            vec![level; bands],
        ))));
        app
    }

    /// Cycle a fresh app to `mode` via the public `v`-key action.
    fn app_in_mode(mode: VisualizerMode) -> App {
        let mut app = base_app();
        while app.visualizer_mode() != mode {
            app.apply(Action::CycleVisualizerMode);
        }
        app
    }

    /// Mosaic/block glyphs the particle base must NOT use as its visual language.
    const MOSAIC_BLOCKS: [char; 5] = ['░', '▒', '▓', '▄', '▀'];

    /// True for any glyph the particle spectrum may draw: a dust grain.
    fn is_particle(c: char) -> bool {
        DUST_GLYPHS.contains(&c)
    }

    /// The set of buffer rows (relative y) whose line contains `glyph`.
    fn rows_with(text: &str, glyph: char) -> std::collections::BTreeSet<usize> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| line.contains(glyph))
            .map(|(y, _)| y)
            .collect()
    }

    /// True if the cell at `(x, y)` carries `glyph`.
    fn cell_is(buf: &Buffer, x: u16, y: u16, glyph: &str) -> bool {
        buf.cell((x, y)).unwrap().symbol() == glyph
    }

    /// True if `text` contains any ambient shade glyph.
    fn has_ambient_shade(text: &str) -> bool {
        text.chars().any(|c| matches!(c, '░' | '▒' | '▓'))
    }

    #[test]
    fn spectrum_particles_use_interpolated_gradient_not_discrete_band_cutoffs() {
        // MIK-032: the spectrum gradient means intermediate columns blend between
        // the low/mid/high anchors. Prove at least one rendered grain carries a
        // color that is none of the three discrete anchors — i.e. a real gradient,
        // not the old hard low/mid/high cutoffs. All color knowledge still comes
        // from the theme; the visualizer introduces no palette constants.
        let app = with_flat_frame(1.0, 16);
        let theme = app.theme().theme();
        let discrete = [theme.spectrum_low, theme.spectrum_mid, theme.spectrum_high];
        let buf = render_spectrum_buffer(&app, 130, 32);
        let area = *buf.area();
        // Restrict to the mid/heavy grains (`∙`/`•`): those carry the spectrum
        // gradient color, whereas faint `·` is muted, so it would not prove
        // interpolation.
        let mut blended = false;
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = buf.cell((x, y)).unwrap();
                let sym = cell.symbol().chars().next().unwrap_or(' ');
                if matches!(sym, '∙' | '•') && !discrete.contains(&cell.fg) {
                    blended = true;
                }
            }
        }
        assert!(
            blended,
            "spectrum used only the three discrete anchors, not a gradient"
        );
    }

    #[test]
    fn spectrum_stack_renders_particles_not_mosaic_or_bars() {
        // The columns must read as a particle/dot fill: only dust grains (`·∙•`)
        // are drawn — no `*`, no solid `█` bars, and no mosaic block/shade glyphs
        // (`░▒▓▄▀`).
        let app = with_flat_frame(1.0, 16);
        let text = buffer_text(&render_spectrum_buffer(&app, 48, 16));
        let particles = text.chars().filter(|c| is_particle(*c)).count();
        assert!(particles > 0, "no particles drawn: {text}");
        assert!(
            !text.contains('*'),
            "SpectrumStack must not use `*`: {text}"
        );
        assert!(
            !text.contains('+'),
            "static spectrum must not draw `+`: {text}"
        );
        assert!(
            !text.contains('█'),
            "particles must not be solid bars: {text}"
        );
        for block in MOSAIC_BLOCKS {
            assert!(
                !text.contains(block),
                "mosaic block glyph {block} leaked into the particle fill"
            );
        }
        // Every drawn cell is a particle grain.
        let drawn = text.chars().filter(|&c| c != ' ' && c != '\n').count();
        assert_eq!(particles, drawn, "non-particle glyphs leaked into the fill");
    }

    #[test]
    fn spectrum_stack_is_static_and_deterministic_for_same_frame_and_area() {
        // Pure, static function of frame + area + theme: there is no animation tick,
        // and rendering the same frame twice yields an identical buffer (symbols
        // and colors), with no RNG/time/tick state.
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.3, 0.6, 0.9, 0.2, 0.7, 1.0, 0.4, 0.5],
            0.6,
            vec![0.0; 8],
        ))));
        let first = render_spectrum_buffer(&app, 40, 12);
        let second = render_spectrum_buffer(&app, 40, 12);
        assert_eq!(first, second, "SpectrumStack render is not deterministic");
    }

    #[test]
    fn spectrum_stack_column_height_tracks_band_magnitude() {
        // Traditional analyzer responsiveness: a loud band's column is taller than
        // a quiet band's, and a louder overall frame fills more cells.
        let mut bands = vec![0.1_f32; 16];
        bands[0] = 1.0; // loud band
        bands[15] = 0.25; // quiet band
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            bands,
            0.5,
            vec![0.0; 16],
        ))));
        let h = 16u16;
        let buf = render_spectrum_buffer(&app, 16, h);
        let col_height = |x: u16| {
            (0..h)
                .filter(|&y| buf.cell((x, y)).unwrap().symbol() != " ")
                .count()
        };
        assert_eq!(col_height(0), 16, "loud band should fill its whole column");
        assert!(
            col_height(0) > col_height(15),
            "loud column ({}) not taller than quiet column ({})",
            col_height(0),
            col_height(15)
        );

        let count = |level: f32| {
            buffer_text(&render_spectrum_buffer(&with_flat_frame(level, 16), 32, 16))
                .chars()
                .filter(|c| is_particle(*c))
                .count()
        };
        assert!(count(1.0) > count(0.3), "louder frame not denser");
    }

    #[test]
    fn spectrum_stack_is_a_traditional_bottom_up_silhouette() {
        // Each column is a contiguous bottom-up fill to `round(magnitude*height)`:
        // filled from the floor with no gaps, and empty above. This is the normal
        // analyzer silhouette, not a free particle field.
        let h = 16u16;
        let app = with_flat_frame(0.5, 16);
        let expected = (0.5 * h as f32).round() as u16; // 8
        let buf = render_spectrum_buffer(&app, 20, h);
        for x in 0..20u16 {
            for row in 0..h {
                let y = h - 1 - row; // row counts up from the floor
                let filled = buf.cell((x, y)).unwrap().symbol() != " ";
                assert_eq!(
                    filled,
                    row < expected,
                    "column {x} row {row}: expected filled={}",
                    row < expected
                );
            }
        }
    }

    #[test]
    fn spectrum_stack_draws_nothing_for_silent_or_zero_frame() {
        // Silent/empty frames stay calm: no particles at all. Also safe for tiny
        // and zero-sized panes.
        let mut silent = base_app();
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        let mut empty = base_app();
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));
        for app in [&silent, &empty] {
            for (w, h) in [(0, 0), (1, 1), (2, 4), (48, 16)] {
                let text = buffer_text(&render_spectrum_buffer(app, w, h));
                assert!(
                    !text.chars().any(is_particle),
                    "silent/empty frame drew particles at {w}x{h}: {text}"
                );
            }
        }
    }

    #[test]
    fn peak_dots_single_frame_is_unaffected_by_spectrum_particles() {
        // The animated particle upgrade is scoped to SpectrumStack: a fresh
        // single-frame PeakDots render still draws peak dots without a particle
        // field. Trail glyphs appear only after previous VizFrames exist.
        let mut app = with_flat_frame(1.0, 16);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);

        let area = Rect::new(0, 0, 48, 16);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_peak_dots(&theme, &app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains('●'), "PeakDots lost its dots: {text}");
        assert!(
            !text.chars().any(is_particle),
            "single-frame PeakDots should not draw a particle field: {text}"
        );
    }

    // --- Spectrum pane-width usage (MIK-025) ----------------------------

    #[test]
    fn spectrum_columns_resamples_to_full_width_preserving_endpoints() {
        // The helper produces exactly one column per pane cell, so the bars use
        // the full pane width instead of the band count.
        let bands = [0.2_f32, 0.4, 0.6, 0.8];
        let cols = spectrum_columns(&bands, 16);
        assert_eq!(cols.len(), 16, "one column per pane cell");

        // Endpoints map to the first/last band magnitude exactly.
        assert!((cols.first().unwrap().0 - 0.2).abs() < 1e-6);
        assert!((cols.last().unwrap().0 - 0.8).abs() < 1e-6);

        // Positions span the full 0.0..=1.0 range so the low/mid/high color
        // split stretches across the whole pane.
        assert_eq!(cols.first().unwrap().1, 0.0);
        assert!((cols.last().unwrap().1 - 1.0).abs() < 1e-6);

        // Monotonic bands resample to non-decreasing magnitudes (smooth fill).
        for pair in cols.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0 - 1e-6,
                "interpolated magnitudes should not jitter for monotonic bands"
            );
        }
    }

    #[test]
    fn spectrum_columns_is_deterministic() {
        let bands = [0.1_f32, 0.5, 0.9];
        assert_eq!(spectrum_columns(&bands, 24), spectrum_columns(&bands, 24));
    }

    #[test]
    fn spectrum_columns_is_safe_for_empty_bands_and_zero_width() {
        assert!(spectrum_columns(&[], 40).is_empty());
        assert!(spectrum_columns(&[0.5_f32; 8], 0).is_empty());
        // A single band still produces a full-width run without panicking.
        assert_eq!(spectrum_columns(&[0.7_f32], 5).len(), 5);
    }

    #[test]
    fn spectrum_fills_full_pane_width_when_wider_than_bands() {
        // Regression: the old renderer drew only min(bands.len(), width)
        // columns, leaving the right side of a wide pane blank. With far more
        // pane cells than bands, every column must now carry dust. With a loud
        // flat frame every column lights at least one particle near the floor, so
        // assert per-column coverage (each column carries at least one dust cell).
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render_spectrum(&theme, &app, area, &mut buf);

        for x in 0..area.width {
            let filled = (0..area.height).any(|y| {
                is_particle(
                    buf.cell((x, y))
                        .unwrap()
                        .symbol()
                        .chars()
                        .next()
                        .unwrap_or(' '),
                )
            });
            assert!(filled, "column {x} not filled across the full pane width");
        }
    }

    #[test]
    fn spectrum_is_safe_for_tiny_panes_and_silent_frames() {
        let theme = Theme::for_name(ThemeName::Minimal);
        // Silent frame: bands present but zero, so no dust is drawn.
        let mut silent = base_app();
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        // Empty frame: no bands at all.
        let mut empty = base_app();
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));

        for app in [&silent, &empty] {
            for (w, h) in [(1, 1), (2, 1), (1, 4), (40, 1)] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_spectrum(&theme, app, area, &mut buf);
                assert!(
                    !buffer_text(&buf).chars().any(is_particle),
                    "silent/empty frame must draw no mosaic at {w}x{h}"
                );
            }
        }
    }

    // --- PeakDots visualizer mode (MIK-026) -----------------------------

    #[test]
    fn selecting_peak_dots_changes_the_rendered_visualizer() {
        // The default SpectrumStack draws a particle field; cycling to PeakDots
        // routes to a distinct renderer that emphasizes the per-column peak.
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);

        let stack_text = buffer_text(&render_viz_buffer(&app, 32, 8));
        assert!(
            stack_text.chars().any(is_particle),
            "spectrum stack particles missing: {stack_text}"
        );
        assert!(
            !stack_text.contains('●'),
            "spectrum stack must not draw peak dots"
        );

        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);
        let dots_text = buffer_text(&render_viz_buffer(&app, 32, 8));
        assert!(
            dots_text.contains('●'),
            "peak dots must draw dot markers: {dots_text}"
        );
        assert_ne!(
            stack_text, dots_text,
            "the rendered visualizer must change with the selected mode"
        );
    }

    #[test]
    fn peak_dots_fills_full_pane_width_with_real_bands() {
        // Far more pane cells than bands: every column carries a peak dot, drawn
        // from the shared full-width sampling helper, not just bands.len() cells.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::CycleVisualizerMode); // SpectrumStack -> PeakDots
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render_peak_dots(&theme, &app, area, &mut buf);

        // Full magnitude lands the peak dot on the top row of every column.
        for x in 0..area.width {
            assert_eq!(
                buf.cell((x, 0)).unwrap().symbol(),
                "●",
                "column {x} not capped with a peak dot across the full pane width"
            );
        }
    }

    #[test]
    fn peak_dots_uses_theme_spectrum_colors() {
        // Dots are colored by the low/mid/high spectrum split, not an ad hoc
        // palette, so they stay theme-driven like the shared Spectrum Stack.
        let mut app = base_app();
        app.apply(Action::CycleTheme); // Minimal -> Neon
        app.apply(Action::CycleVisualizerMode); // -> PeakDots
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        let theme = app.theme().theme();
        let buf = render_viz_buffer(&app, 30, 8);
        assert!(
            has_fg(&buf, theme.spectrum_low)
                || has_fg(&buf, theme.spectrum_mid)
                || has_fg(&buf, theme.spectrum_high),
            "peak dot colors not themed"
        );
    }

    #[test]
    fn peak_dots_renders_trail_from_previous_audio_frames() {
        // PeakDots keeps a short real-frame trail: the current peak remains a
        // strong dot, while previous frame peaks render as quieter glyphs at
        // their old heights.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::CycleVisualizerMode); // SpectrumStack -> PeakDots
        for magnitude in [0.2_f32, 0.4, 0.6] {
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                [magnitude],
                magnitude,
                [],
            ))));
        }

        let area = Rect::new(0, 0, 1, 10);
        let mut buf = Buffer::empty(area);
        render_peak_dots(&theme, &app, area, &mut buf);

        assert_eq!(buf.cell((0, 4)).unwrap().symbol(), "●", "current peak");
        assert_eq!(buf.cell((0, 6)).unwrap().symbol(), "•", "one-frame trail");
        assert_eq!(buf.cell((0, 8)).unwrap().symbol(), "∙", "two-frame trail");
    }

    #[test]
    fn peak_dots_current_dot_wins_when_trail_overlaps() {
        // Drawing older frames first must never dim the current peak when the
        // current and trailing frames land on the same cell.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::CycleVisualizerMode); // SpectrumStack -> PeakDots
        for _ in 0..3 {
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                [1.0_f32],
                1.0,
                [],
            ))));
        }

        let area = Rect::new(0, 0, 1, 6);
        let mut buf = Buffer::empty(area);
        render_peak_dots(&theme, &app, area, &mut buf);

        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "●");
    }

    #[test]
    fn peak_dots_is_safe_for_tiny_panes_and_silent_frames() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut silent = base_app();
        silent.apply(Action::CycleVisualizerMode);
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        let mut empty = base_app();
        empty.apply(Action::CycleVisualizerMode);
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));

        for app in [&silent, &empty] {
            for (w, h) in [(1, 1), (2, 1), (1, 4), (40, 1)] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_peak_dots(&theme, app, area, &mut buf);
                assert!(
                    !buffer_text(&buf).contains('●'),
                    "silent/empty frame must draw no peak dots at {w}x{h}"
                );
            }
        }
    }

    // --- SkylinePeaks visualizer mode (MIK-031) -------------------------

    #[test]
    fn selecting_skyline_peaks_changes_the_rendered_visualizer() {
        // SkylinePeaks is reachable from PeakDots via the `v` cycle and routes to
        // its own renderer: a bright cap glyph over a digital binary tail,
        // distinct from both the filled SpectrumStack bars and the single
        // PeakDots dot.
        let mut app = app_in_mode(VisualizerMode::SkylinePeaks);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));

        let skyline_text = buffer_text(&render_viz_buffer(&app, 32, 8));
        assert!(
            skyline_text.contains('▀'),
            "skyline peaks must draw a cap glyph: {skyline_text}"
        );
        assert!(
            skyline_text.contains('0') || skyline_text.contains('1'),
            "skyline peaks must draw a digital binary tail: {skyline_text}"
        );
        assert!(
            !skyline_text.contains('╎'),
            "skyline peaks must replace the dashed tail with binary digits: {skyline_text}"
        );
        // Distinct from the sibling spectrum modes' glyph languages.
        assert!(
            !skyline_text.contains('●'),
            "skyline peaks must not draw peak dots"
        );

        let mut stack = base_app();
        stack.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        let stack_text = buffer_text(&render_viz_buffer(&stack, 32, 8));
        assert_ne!(
            stack_text, skyline_text,
            "skyline peaks must render differently from the spectrum stack"
        );
    }

    #[test]
    fn skyline_peaks_caps_every_column_across_the_full_pane_width() {
        // Far more pane cells than bands: each column gets a cap drawn from the
        // shared full-width sampling helper, not just bands.len() cells.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = app_in_mode(VisualizerMode::SkylinePeaks);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render_skyline_peaks(&theme, &app, area, &mut buf);

        // Full magnitude lands the cap on the top row of every column.
        for x in 0..area.width {
            assert_eq!(
                buf.cell((x, 0)).unwrap().symbol(),
                "▀",
                "column {x} not capped across the full pane width"
            );
        }
    }

    #[test]
    fn skyline_peaks_draws_a_digital_binary_tail_below_the_cap() {
        // A tall column shows the bright cap on top and a calm 0/1 tail beneath
        // it, so the silhouette reads as a digital skyline rather than a solid bar.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = app_in_mode(VisualizerMode::SkylinePeaks);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 8, 6);
        let mut buf = Buffer::empty(area);
        render_skyline_peaks(&theme, &app, area, &mut buf);

        let x = 0;
        assert_eq!(buf.cell((x, 0)).unwrap().symbol(), "▀", "cap on top row");
        for y in 1..area.height {
            let glyph = buf.cell((x, y)).unwrap().symbol();
            assert!(
                glyph == "0" || glyph == "1",
                "tail below the cap must be a binary digit at row {y}, got {glyph:?}"
            );
        }
        let text = buffer_text(&buf);
        assert!(
            text.contains('0'),
            "binary tail should include zeros: {text}"
        );
        assert!(
            text.contains('1'),
            "binary tail should include ones: {text}"
        );
        assert!(
            !text.contains('╎'),
            "skyline tail must not use the old dashed glyph"
        );
        // The tail is not a solid block (distinct from the SpectrumStack fill).
        assert!(!text.contains('█'), "skyline tail must not be solid bars");
    }

    #[test]
    fn skyline_peaks_uses_theme_spectrum_colors() {
        // Caps and tails are colored by the low/mid/high spectrum split, not an ad
        // hoc palette, so they stay theme-driven like the shared Spectrum Stack.
        let mut app = base_app();
        app.apply(Action::CycleTheme); // Minimal -> Neon
        while app.visualizer_mode() != VisualizerMode::SkylinePeaks {
            app.apply(Action::CycleVisualizerMode);
        }
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        let theme = app.theme().theme();
        let buf = render_viz_buffer(&app, 30, 8);
        assert!(
            has_fg(&buf, theme.spectrum_low)
                || has_fg(&buf, theme.spectrum_mid)
                || has_fg(&buf, theme.spectrum_high),
            "skyline peak colors not themed"
        );
    }

    #[test]
    fn skyline_peaks_is_safe_for_tiny_panes_and_silent_frames() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut silent = app_in_mode(VisualizerMode::SkylinePeaks);
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        let mut empty = app_in_mode(VisualizerMode::SkylinePeaks);
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));

        for app in [&silent, &empty] {
            for (w, h) in [(0u16, 0u16), (1, 1), (2, 1), (1, 4), (40, 1)] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_skyline_peaks(&theme, app, area, &mut buf);
                assert!(
                    !buffer_text(&buf).contains('▀'),
                    "silent/empty frame must draw no skyline caps at {w}x{h}"
                );
            }
        }
    }

    // --- Waveform / RMS visualizer modes (MIK-027) ----------------------

    #[test]
    fn wave_scope_renders_a_waveform_trace() {
        // WaveScope draws one trace point per column from VizFrame::waveform; a
        // non-flat waveform produces a trace that varies in height.
        let mut app = app_in_mode(VisualizerMode::WaveScope);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![-1.0, -0.5, 0.0, 0.5, 1.0],
        ))));
        let text = buffer_text(&render_viz_buffer(&app, 16, 7));
        assert!(text.contains('•'), "wave scope trace missing: {text}");
        assert!(
            rows_with(&text, '•').len() > 1,
            "wave scope trace is flat for a non-flat waveform: {text}"
        );
    }

    #[test]
    fn wave_scope_treats_empty_and_zeroed_waveform_as_flat_silence() {
        // MIK-024 reviewer note: empty and all-zero waveforms are both flat
        // silence and must render identically — a single baseline row.
        let render = |wf: Vec<f32>| {
            let mut app = app_in_mode(VisualizerMode::WaveScope);
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                vec![],
                0.0,
                wf,
            ))));
            buffer_text(&render_viz_buffer(&app, 16, 7))
        };
        let empty = render(vec![]);
        let zeroed = render(vec![0.0; 8]);
        assert_eq!(
            empty, zeroed,
            "empty and zeroed waveform must render identically"
        );
        assert_eq!(
            rows_with(&empty, '•').len(),
            1,
            "silence must be a single flat baseline: {empty}"
        );
    }

    #[test]
    fn wave_scope_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::WaveScope);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.2, -0.2, 0.6, -0.6],
        ))));
        let (w, h) = (40u16, 8u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            assert!(
                (0..h).any(|y| cell_is(&buf, x, y, "•")),
                "wave scope column {x} empty (not full width)"
            );
        }
    }

    #[test]
    fn mirror_wave_is_symmetric_around_center() {
        let mut app = app_in_mode(VisualizerMode::MirrorWave);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.8; 8],
        ))));
        let h = 7u16;
        let buf = render_viz_buffer(&app, 20, h);
        assert!(buffer_text(&buf).contains('┃'), "mirror wave bars missing");
        let center = (h - 1) / 2;
        let x = 5u16;
        for r in 0..=center {
            assert_eq!(
                cell_is(&buf, x, center - r, "┃"),
                cell_is(&buf, x, center + r, "┃"),
                "mirror wave not symmetric at offset {r}"
            );
        }
    }

    #[test]
    fn mirror_wave_reflects_waveform_and_is_flat_for_silence() {
        let render = |wf: Vec<f32>| {
            let mut app = app_in_mode(VisualizerMode::MirrorWave);
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                vec![],
                0.0,
                wf,
            ))));
            buffer_text(&render_viz_buffer(&app, 20, 7))
        };
        let loud = render(vec![0.9; 8]);
        let silent = render(vec![]);
        assert!(
            rows_with(&loud, '┃').len() > rows_with(&silent, '┃').len(),
            "louder waveform must reach further from center: {loud}"
        );
        assert_eq!(
            rows_with(&silent, '┃').len(),
            1,
            "silence must be a single baseline row: {silent}"
        );
        assert_eq!(
            silent,
            render(vec![0.0; 8]),
            "empty and zeroed waveform must render identically"
        );
    }

    #[test]
    fn mirror_wave_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::MirrorWave);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.3, -0.3, 0.7],
        ))));
        let (w, h) = (40u16, 8u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            assert!(
                (0..h).any(|y| cell_is(&buf, x, y, "┃")),
                "mirror wave column {x} empty (not full width)"
            );
        }
    }

    #[test]
    fn ambient_pulse_is_rms_driven_not_fake_animation() {
        // Real RMS + bands produce ambient shading; a silent frame draws nothing
        // (proving the mode reacts to data instead of animating on its own).
        let mut active = app_in_mode(VisualizerMode::AmbientPulse);
        active.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9; 8],
            0.8,
            vec![],
        ))));
        let active_text = buffer_text(&render_viz_buffer(&active, 24, 7));
        assert!(
            has_ambient_shade(&active_text),
            "ambient pulse drew nothing for real data: {active_text}"
        );

        let mut silent = app_in_mode(VisualizerMode::AmbientPulse);
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(8))));
        let silent_text = buffer_text(&render_viz_buffer(&silent, 24, 7));
        assert!(
            !has_ambient_shade(&silent_text),
            "ambient pulse must be silent for a silent frame: {silent_text}"
        );
    }

    #[test]
    fn ambient_pulse_pulses_from_rms_without_bands() {
        // RMS alone (no bands) still drives the ambient display.
        let mut app = app_in_mode(VisualizerMode::AmbientPulse);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.9,
            vec![],
        ))));
        let text = buffer_text(&render_viz_buffer(&app, 24, 7));
        assert!(has_ambient_shade(&text), "rms-only ambient missing: {text}");
    }

    #[test]
    fn ambient_pulse_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::AmbientPulse);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0; 8],
            1.0,
            vec![],
        ))));
        let (w, h) = (40u16, 7u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            let filled = (0..h).any(|y| {
                let s = buf.cell((x, y)).unwrap().symbol();
                s == "░" || s == "▒" || s == "▓"
            });
            assert!(filled, "ambient column {x} empty (not full width)");
        }
    }

    #[test]
    fn each_visualizer_mode_renders_distinctly() {
        // Cycling through every mode with the same frame yields five distinct
        // renders, so each mode has its own visual language.
        let frame = VizFrame::new(
            vec![1.0, 0.2, 0.8, 0.4, 0.9, 0.1, 0.6, 0.3],
            0.7,
            vec![-0.9, -0.3, 0.2, 0.8, 0.5, -0.6, 0.1, 0.7],
        );
        let mut app = base_app();
        let mut texts = Vec::new();
        for _ in 0..VisualizerMode::ALL.len() {
            app.apply(Action::Audio(AudioEvent::Viz(frame.clone())));
            texts.push(buffer_text(&render_viz_buffer(&app, 32, 9)));
            app.apply(Action::CycleVisualizerMode);
        }
        for i in 0..texts.len() {
            for j in (i + 1)..texts.len() {
                assert_ne!(texts[i], texts[j], "modes {i} and {j} render identically");
            }
        }
    }

    #[test]
    fn waveform_and_ambient_modes_are_safe_for_tiny_panes_and_silent_frames() {
        for mode in [
            VisualizerMode::WaveScope,
            VisualizerMode::MirrorWave,
            VisualizerMode::AmbientPulse,
        ] {
            let mut silent = app_in_mode(mode);
            silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
            let mut empty = app_in_mode(mode);
            empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                Vec::<f32>::new(),
                0.0,
                Vec::<f32>::new(),
            ))));
            for app in [&silent, &empty] {
                for (w, h) in [(0u16, 0u16), (1, 1), (2, 1), (1, 4), (40, 1)] {
                    let _ = render_viz_buffer(app, w, h);
                }
            }
        }
    }
}
