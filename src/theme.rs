//! Theme names and palette definitions.
//!
//! This module owns the [`ThemeName`] domain primitive, its boundary parser,
//! and the centralized [`Theme`] palette. Per the High Contrast Trio design
//! decision, all UI colors live here so rendering code never hard-codes palette
//! values. Themes are stored as stable lowercase strings (`minimal`, `neon`,
//! `crt`); unknown names fall back to `Minimal` at the boundary.

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::DomainError;

/// One of the built-in High Contrast Trio themes. Always valid by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ThemeName {
    #[default]
    Minimal,
    Neon,
    Crt,
}

impl ThemeName {
    /// Stable lowercase identifier used for persistence and CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeName::Minimal => "minimal",
            ThemeName::Neon => "neon",
            ThemeName::Crt => "crt",
        }
    }

    /// Strict boundary parser; rejects unknown names with a [`DomainError`].
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(ThemeName::Minimal),
            "neon" => Ok(ThemeName::Neon),
            "crt" => Ok(ThemeName::Crt),
            other => Err(DomainError::UnknownTheme(other.to_string())),
        }
    }

    /// Lenient boundary parser; unknown names fall back to the default theme.
    pub fn parse_or_default(raw: &str) -> Self {
        Self::parse(raw).unwrap_or(ThemeName::Minimal)
    }

    /// Next theme in the cycling order `Minimal -> Neon -> CRT -> Minimal`.
    ///
    /// Bound to the `t` key in the keyboard model; the order is stable so
    /// repeated presses are predictable.
    pub fn next(self) -> Self {
        match self {
            ThemeName::Minimal => ThemeName::Neon,
            ThemeName::Neon => ThemeName::Crt,
            ThemeName::Crt => ThemeName::Minimal,
        }
    }

    /// Resolve this name to its centralized [`Theme`] palette.
    pub fn theme(self) -> Theme {
        Theme::for_name(self)
    }
}

/// Themes persist as their stable lowercase string identifier.
impl Serialize for ThemeName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Strict deserialization: unknown names are a parse error so corrupt persisted
/// settings fail at the boundary rather than silently coercing. Lenient
/// fallback to the default theme belongs to [`ThemeName::parse_or_default`].
impl<'de> Deserialize<'de> for ThemeName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        ThemeName::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Centralized color palette for one theme.
///
/// All UI-facing colors are owned here; rendering code asks the active `Theme`
/// for colors/styles instead of hard-coding palette values. Spectrum bars map
/// to the `spectrum_low`/`spectrum_mid`/`spectrum_high` triplet, as required by
/// the shared `SpectrumStack` renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Which theme this palette belongs to.
    pub name: ThemeName,
    /// Primary surface background.
    pub background: Color,
    /// Primary readable foreground text.
    pub foreground: Color,
    /// Low-saturation secondary text (hints, inactive metadata).
    pub muted: Color,
    /// Primary accent for headings, focus, and active controls.
    pub accent: Color,
    /// Pane borders and separators.
    pub border: Color,
    /// Foreground of the selected/highlighted row.
    pub selection_fg: Color,
    /// Background of the selected/highlighted row.
    pub selection_bg: Color,
    /// Signal color for active playback.
    pub playing: Color,
    /// Error / offline / failed-stream signal color.
    pub error: Color,
    /// Spectrum band color for the low third of the band range.
    pub spectrum_low: Color,
    /// Spectrum band color for the mid third of the band range.
    pub spectrum_mid: Color,
    /// Spectrum band color for the high third of the band range.
    pub spectrum_high: Color,
}

impl Theme {
    /// Build the palette for a theme name.
    pub fn for_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Minimal => Self::minimal(),
            ThemeName::Neon => Self::neon(),
            ThemeName::Crt => Self::crt(),
        }
    }

    /// Quiet dark work-session theme with restrained, grayscale spectrum bars.
    fn minimal() -> Self {
        Self {
            name: ThemeName::Minimal,
            background: Color::Black,
            foreground: Color::Gray,
            muted: Color::DarkGray,
            accent: Color::LightBlue,
            border: Color::DarkGray,
            selection_fg: Color::Black,
            selection_bg: Color::Gray,
            playing: Color::Green,
            error: Color::Red,
            spectrum_low: Color::DarkGray,
            spectrum_mid: Color::Gray,
            spectrum_high: Color::White,
        }
    }

    /// High-energy cliamp-like theme with vivid cyan/magenta/yellow spectrum.
    fn neon() -> Self {
        Self {
            name: ThemeName::Neon,
            background: Color::Black,
            foreground: Color::White,
            muted: Color::Gray,
            accent: Color::Magenta,
            border: Color::Cyan,
            selection_fg: Color::Black,
            selection_bg: Color::Magenta,
            playing: Color::LightGreen,
            error: Color::LightRed,
            spectrum_low: Color::Cyan,
            spectrum_mid: Color::Magenta,
            spectrum_high: Color::Yellow,
        }
    }

    /// Retro green/amber phosphor terminal theme.
    fn crt() -> Self {
        let phosphor = Color::Rgb(0, 200, 70);
        let amber = Color::Rgb(255, 176, 0);
        Self {
            name: ThemeName::Crt,
            background: Color::Black,
            foreground: phosphor,
            muted: Color::Rgb(0, 110, 40),
            accent: amber,
            border: Color::Rgb(0, 140, 50),
            selection_fg: Color::Black,
            selection_bg: phosphor,
            playing: amber,
            error: Color::Rgb(255, 80, 0),
            spectrum_low: Color::Rgb(0, 110, 40),
            spectrum_mid: phosphor,
            spectrum_high: amber,
        }
    }

    /// Map a normalized band position in `0.0..=1.0` to a spectrum color.
    ///
    /// The low/mid/high split lets the shared `SpectrumStack` renderer color
    /// bars by frequency band without knowing the concrete palette.
    pub fn spectrum_color(&self, position: f32) -> Color {
        let position = position.clamp(0.0, 1.0);
        if position < 1.0 / 3.0 {
            self.spectrum_low
        } else if position < 2.0 / 3.0 {
            self.spectrum_mid
        } else {
            self.spectrum_high
        }
    }

    /// Base text style: foreground on the theme background.
    pub fn base_style(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }

    /// Style for the selected/highlighted row.
    pub fn selection_style(&self) -> Style {
        Style::default()
            .fg(self.selection_fg)
            .bg(self.selection_bg)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for accented headings and active controls.
    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::for_name(ThemeName::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_theme_names_case_insensitively() {
        assert_eq!(ThemeName::parse("Minimal").unwrap(), ThemeName::Minimal);
        assert_eq!(ThemeName::parse(" NEON ").unwrap(), ThemeName::Neon);
        assert_eq!(ThemeName::parse("crt").unwrap(), ThemeName::Crt);
    }

    #[test]
    fn unknown_theme_name_is_rejected_but_falls_back_leniently() {
        assert!(matches!(
            ThemeName::parse("solarized"),
            Err(DomainError::UnknownTheme(_))
        ));
        assert_eq!(ThemeName::parse_or_default("solarized"), ThemeName::Minimal);
    }

    #[test]
    fn theme_name_roundtrips_through_str() {
        for theme in [ThemeName::Minimal, ThemeName::Neon, ThemeName::Crt] {
            assert_eq!(ThemeName::parse(theme.as_str()).unwrap(), theme);
        }
    }

    #[test]
    fn theme_name_serializes_as_lowercase_string_and_roundtrips() {
        let json = serde_json::to_string(&ThemeName::Neon).unwrap();
        assert_eq!(json, "\"neon\"");
        let decoded: ThemeName = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ThemeName::Neon);
    }

    #[test]
    fn theme_name_deserialization_rejects_unknown_strictly() {
        assert!(serde_json::from_str::<ThemeName>("\"solarized\"").is_err());
    }

    #[test]
    fn default_theme_name_is_minimal() {
        assert_eq!(ThemeName::default(), ThemeName::Minimal);
    }

    #[test]
    fn theme_cycling_order_is_minimal_neon_crt() {
        assert_eq!(ThemeName::Minimal.next(), ThemeName::Neon);
        assert_eq!(ThemeName::Neon.next(), ThemeName::Crt);
        assert_eq!(ThemeName::Crt.next(), ThemeName::Minimal);
    }

    #[test]
    fn theme_cycling_returns_to_start_after_three_steps() {
        let start = ThemeName::Minimal;
        assert_eq!(start.next().next().next(), start);
    }

    #[test]
    fn for_name_centralizes_each_palette() {
        for name in [ThemeName::Minimal, ThemeName::Neon, ThemeName::Crt] {
            assert_eq!(Theme::for_name(name).name, name);
            assert_eq!(name.theme(), Theme::for_name(name));
        }
    }

    #[test]
    fn default_theme_is_minimal_palette() {
        assert_eq!(Theme::default(), Theme::for_name(ThemeName::Minimal));
    }

    #[test]
    fn themes_differ_so_switching_is_meaningful() {
        let minimal = Theme::for_name(ThemeName::Minimal);
        let neon = Theme::for_name(ThemeName::Neon);
        let crt = Theme::for_name(ThemeName::Crt);
        assert_ne!(minimal.spectrum_high, neon.spectrum_high);
        assert_ne!(neon.accent, crt.accent);
        assert_ne!(minimal.accent, crt.accent);
    }

    #[test]
    fn spectrum_color_maps_low_mid_high_bands() {
        let theme = Theme::for_name(ThemeName::Neon);
        assert_eq!(theme.spectrum_color(0.0), theme.spectrum_low);
        assert_eq!(theme.spectrum_color(0.5), theme.spectrum_mid);
        assert_eq!(theme.spectrum_color(1.0), theme.spectrum_high);
    }

    #[test]
    fn spectrum_color_clamps_out_of_range_positions() {
        let theme = Theme::for_name(ThemeName::Minimal);
        assert_eq!(theme.spectrum_color(-1.0), theme.spectrum_low);
        assert_eq!(theme.spectrum_color(2.0), theme.spectrum_high);
    }
}
