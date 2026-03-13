//! Shared packing category type — re-exported from mm-meta with color extension.

pub use mm_meta::signals::packing_category::*;

use ratatui::style::Color;

/// Color extension for PackingCategory (ratatui is not in mm-meta).
pub trait PackingCategoryColor {
    fn color(self) -> Color;
}

impl PackingCategoryColor for PackingCategory {
    fn color(self) -> Color {
        match self {
            PackingCategory::Perfect => Color::Green,
            PackingCategory::FullMatches => Color::Cyan,
            PackingCategory::Singles => Color::Cyan,
            PackingCategory::Incomplete => Color::Yellow,
            PackingCategory::LowConfidence => Color::Yellow,
            PackingCategory::Knots => Color::Magenta,
            PackingCategory::UnsolvedConflict => Color::Red,
            PackingCategory::UnsolvedNoRelease => Color::Red,
            PackingCategory::UnsolvedNoMatch => Color::DarkGray,
        }
    }
}
