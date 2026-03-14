//! Resolution Layout Widget
//!
//! Provides a standard two-pane layout for resolution modals with:
//! - Info bar at top (full-width, shows untruncated path)
//! - Left pane (~33%) for file list
//! - Right pane (~67%) for details (informational, not focusable)
//! - Bottom bar for decision buttons (focusable via Shift+Up/Down)
//!
//! ```text
//! +------------------------------------------+
//! | Full path info bar (untruncated)         |  <- 3 lines
//! +------------------------------------------+
//! | ~33% List     | ~67% Details             |  <- Min(5)
//! |               |                          |
//! +------------------------------------------+
//! | Decision Buttons                         |  <- 2 lines
//! +------------------------------------------+
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Rect};

// Re-export from mm-ui
pub use mm_ui::geometry::{rect_contains, padded_rect, ButtonRects, FocusPane};

/// Layout areas for a resolution modal.
#[derive(Debug, Clone, Copy)]
pub struct ResolutionLayout {
    /// Area for the full-width info bar showing selected item's full path
    pub info_bar: Rect,
    /// Area for the left pane (file list)
    pub list_pane: Rect,
    /// Area for the right pane (details) - informational only, not focusable
    pub details_pane: Rect,
    /// Area for bottom decision buttons
    pub buttons: Rect,
}

impl ResolutionLayout {
    /// Build a resolution layout from the given area.
    ///
    /// # Arguments
    /// * `area` - The full area to divide
    /// * `info_height` - Height of the info bar (typically 3 for bordered)
    /// * `buttons_height` - Height of the buttons bar (typically 2)
    /// * `list_percent` - Percentage of horizontal space for list pane (default 33)
    pub fn new(area: Rect, info_height: u16, buttons_height: u16, list_percent: u16) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(info_height),    // Info bar
                Constraint::Min(5),                 // Content panes
                Constraint::Length(buttons_height), // Buttons
            ])
            .split(area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(list_percent),
                Constraint::Percentage(100 - list_percent),
            ])
            .split(vertical[1]);

        Self {
            info_bar: vertical[0],
            list_pane: horizontal[0],
            details_pane: horizontal[1],
            buttons: vertical[2],
        }
    }

    /// Apply 1-cell padding to an area (for full-area modals).
    pub fn padded(area: Rect) -> Rect {
        padded_rect(area)
    }
}
