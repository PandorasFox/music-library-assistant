//! Shared packing category type for UI display across packing browser and tree browser.

use ratatui::style::Color;

/// Which category of packing results to display.
///
/// Used by the release packing browser for full-screen browsing and by the
/// tree browser for compact `[MB]` markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingCategory {
    Perfect,
    FullMatches,
    Singles,
    Incomplete,
    LowConfidence,
    Knots,
    /// Unsolved: had AcoustID match, was scored for releases, but lost conflict resolution.
    UnsolvedConflict,
    /// Unsolved: had AcoustID match but was never optimally scored for any release.
    UnsolvedNoRelease,
    /// Unsolved: fingerprinted but no AcoustID match at all.
    UnsolvedNoMatch,
}

impl PackingCategory {
    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Perfect => "Perfect Matches",
            Self::FullMatches => "Full Matches",
            Self::Singles => "Singles",
            Self::Incomplete => "Incomplete Releases",
            Self::LowConfidence => "Low Confidence",
            Self::Knots => "Packing Knots",
            Self::UnsolvedConflict => "Unsolved — Lost Conflict",
            Self::UnsolvedNoRelease => "Unsolved — No Viable Release",
            Self::UnsolvedNoMatch => "Unsolved — No AcoustID Match",
        }
    }

    /// Compact symbol for tree browser markers.
    pub fn marker_symbol(self) -> &'static str {
        match self {
            Self::Perfect => "✓",
            Self::FullMatches => "●",
            Self::Singles => "♪",
            Self::Incomplete => "◐",
            Self::LowConfidence => "?!",
            Self::Knots => "⚡",
            Self::UnsolvedConflict => "✗",
            Self::UnsolvedNoRelease => "?",
            Self::UnsolvedNoMatch => "∅",
        }
    }

    /// Color for this category.
    pub fn color(self) -> Color {
        match self {
            Self::Perfect => Color::Green,
            Self::FullMatches => Color::Cyan,
            Self::Singles => Color::Cyan,
            Self::Incomplete => Color::Yellow,
            Self::LowConfidence => Color::Yellow,
            Self::Knots => Color::Magenta,
            Self::UnsolvedConflict => Color::Red,
            Self::UnsolvedNoRelease => Color::Red,
            Self::UnsolvedNoMatch => Color::DarkGray,
        }
    }

    /// Parse from signal_packed_release key prefix.
    pub fn from_key_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "perfect" => Some(Self::Perfect),
            "full_match" => Some(Self::FullMatches),
            "single" => Some(Self::Singles),
            "incomplete" => Some(Self::Incomplete),
            "low_confidence" => Some(Self::LowConfidence),
            _ => None,
        }
    }
}
