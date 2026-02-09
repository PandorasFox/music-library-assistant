//! Centralized cursor/selection style constants.
//!
//! Single source of truth for list item and cursor highlighting across all
//! flows in MLA's TUI. Import these instead of defining inline styles.

use ratatui::style::{Color, Modifier, Style};

/// Style for the cursor (selected) item in any navigable list.
///
/// White text on dark gray background, bold.
pub const CURSOR_STYLE: Style = Style::new()
    .fg(Color::White)
    .bg(Color::DarkGray)
    .add_modifier(Modifier::BOLD);

/// Default style for non-selected list items.
pub const LIST_ITEM_STYLE: Style = Style::new().fg(Color::White);
