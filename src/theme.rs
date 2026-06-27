//! Theme names and palette definitions.
//!
//! For MIK-001 this module owns only the [`ThemeName`] domain primitive and its
//! boundary parser. Palette/`Theme` structures and cycling are added in a later
//! task. Themes are stored as stable lowercase strings (`minimal`, `neon`,
//! `crt`); unknown names fall back to `Minimal` at the boundary.

use crate::model::DomainError;

/// One of the built-in High Contrast Trio themes. Always valid by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeName {
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
}

impl Default for ThemeName {
    fn default() -> Self {
        ThemeName::Minimal
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
}
