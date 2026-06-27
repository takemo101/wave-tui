//! Terminal-size to layout-tier policy.
//!
//! This module owns the single source of truth for breakpoint policy: it maps a
//! terminal's `(width, height)` to one of three named responsive tiers from the
//! design deck. Tier selection is deterministic and pure so it can be tested
//! without a real terminal. Pane geometry within a tier belongs to `ui`.

/// Minimum width/height required before a terminal is treated as the Wide tier.
const WIDE_MIN_WIDTH: u16 = 100;
const WIDE_MIN_HEIGHT: u16 = 28;

/// Minimum width/height required before a terminal is treated as the Medium
/// tier; anything smaller in either dimension collapses to Compact.
const MEDIUM_MIN_WIDTH: u16 = 72;
const MEDIUM_MIN_HEIGHT: u16 = 18;

/// One of the three responsive layout tiers.
///
/// Variants are ordered from most constrained to least constrained so the
/// derived `Ord` lets [`LayoutTier::from_size`] pick the more constrained tier
/// when width and height disagree (`Compact < Medium < Wide`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayoutTier {
    /// Split Mini, reduced: herdr-style small terminals.
    Compact,
    /// Split Mini: balanced list + player layout.
    Medium,
    /// Search Console: search-first layout for large terminals.
    Wide,
}

impl LayoutTier {
    /// Select the layout tier for a terminal of `width` x `height` cells.
    ///
    /// Each dimension is classified independently and the more constrained
    /// result wins, so a wide-but-short terminal does not get a layout taller
    /// than it can show. Selection is total and deterministic.
    pub fn from_size(width: u16, height: u16) -> Self {
        let by_width = if width >= WIDE_MIN_WIDTH {
            LayoutTier::Wide
        } else if width >= MEDIUM_MIN_WIDTH {
            LayoutTier::Medium
        } else {
            LayoutTier::Compact
        };
        let by_height = if height >= WIDE_MIN_HEIGHT {
            LayoutTier::Wide
        } else if height >= MEDIUM_MIN_HEIGHT {
            LayoutTier::Medium
        } else {
            LayoutTier::Compact
        };
        by_width.min(by_height)
    }

    /// Stable lowercase identifier for the tier.
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutTier::Wide => "wide",
            LayoutTier::Medium => "medium",
            LayoutTier::Compact => "compact",
        }
    }

    /// The named design contract this tier renders, per the UI design deck.
    pub fn contract(self) -> &'static str {
        match self {
            LayoutTier::Wide => "Search Console",
            LayoutTier::Medium => "Split Mini",
            LayoutTier::Compact => "Split Mini reduced",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_terminal_selects_wide_search_console() {
        let tier = LayoutTier::from_size(140, 40);
        assert_eq!(tier, LayoutTier::Wide);
        assert_eq!(tier.contract(), "Search Console");
    }

    #[test]
    fn medium_terminal_selects_split_mini() {
        let tier = LayoutTier::from_size(80, 22);
        assert_eq!(tier, LayoutTier::Medium);
        assert_eq!(tier.contract(), "Split Mini");
    }

    #[test]
    fn small_terminal_selects_compact_split_mini_reduced() {
        let tier = LayoutTier::from_size(60, 14);
        assert_eq!(tier, LayoutTier::Compact);
        assert_eq!(tier.contract(), "Split Mini reduced");
    }

    #[test]
    fn boundaries_are_inclusive_lower_bounds() {
        assert_eq!(
            LayoutTier::from_size(WIDE_MIN_WIDTH, WIDE_MIN_HEIGHT),
            LayoutTier::Wide
        );
        assert_eq!(
            LayoutTier::from_size(WIDE_MIN_WIDTH - 1, WIDE_MIN_HEIGHT),
            LayoutTier::Medium
        );
        assert_eq!(
            LayoutTier::from_size(MEDIUM_MIN_WIDTH, MEDIUM_MIN_HEIGHT),
            LayoutTier::Medium
        );
        assert_eq!(
            LayoutTier::from_size(MEDIUM_MIN_WIDTH - 1, MEDIUM_MIN_HEIGHT),
            LayoutTier::Compact
        );
    }

    #[test]
    fn the_more_constrained_dimension_wins() {
        // Wide width but short height must not claim the Wide tier.
        assert_eq!(LayoutTier::from_size(160, 16), LayoutTier::Compact);
        assert_eq!(LayoutTier::from_size(160, 20), LayoutTier::Medium);
        // Tall but narrow is likewise capped by width.
        assert_eq!(LayoutTier::from_size(50, 60), LayoutTier::Compact);
    }

    #[test]
    fn selection_is_deterministic_for_a_given_size() {
        for width in [0u16, 72, 100, 200] {
            for height in [0u16, 18, 28, 50] {
                let first = LayoutTier::from_size(width, height);
                let second = LayoutTier::from_size(width, height);
                assert_eq!(first, second);
            }
        }
    }

    #[test]
    fn tier_ordering_is_compact_lt_medium_lt_wide() {
        assert!(LayoutTier::Compact < LayoutTier::Medium);
        assert!(LayoutTier::Medium < LayoutTier::Wide);
    }
}
