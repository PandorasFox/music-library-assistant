//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.

// Re-export centered_rect from widgets module
pub use super::widgets::centered_rect;

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Compute a centered rectangle with fixed dimensions within an area.
///
/// If the fixed dimensions exceed the area, the modal is clamped to fit.
pub fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let actual_width = width.min(area.width);
    let actual_height = height.min(area.height);

    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(actual_height)) / 2),
            Constraint::Length(actual_height),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(actual_width)) / 2),
            Constraint::Length(actual_width),
            Constraint::Min(0),
        ])
        .split(popup_layout[1])[1]
}

// ============================================================================
// Formatting Utilities
// ============================================================================

/// Truncate a string from the left, UTF-8 safe. Result: `...visible_end`
///
/// Keeps the rightmost `max_chars` characters. Useful for paths where
/// the filename/end is most relevant.
pub fn truncate_left(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    let skip = char_count.saturating_sub(max_chars - 3);
    format!("...{}", s.chars().skip(skip).collect::<String>())
}

/// Truncate a string from the right, UTF-8 safe. Result: `visible_start...`
///
/// Keeps the leftmost `max_chars` characters. Useful for tag values and labels
/// where the beginning is most relevant.
pub fn truncate_right(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(3);
    format!("{}...", s.chars().take(take).collect::<String>())
}

/// Format a Duration as a human-readable string.
pub fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if total_secs == 0 {
        format!("{}ms", millis)
    } else if total_secs < 60 {
        format!("{}.{}s", total_secs, millis / 100)
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{}m {}s", mins, secs)
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_left_short() {
        let path = "/home/user/file.mp3";
        assert_eq!(truncate_left(path, 50), path);
    }

    #[test]
    fn test_truncate_left_exact() {
        let path = "exactly20chars!!!"; // 17 chars
        assert_eq!(truncate_left(path, 17), path);
    }

    #[test]
    fn test_truncate_left_long() {
        let path = "/very/long/path/to/some/deeply/nested/file.mp3";
        let truncated = truncate_left(path, 20);
        assert!(truncated.starts_with("..."));
        assert_eq!(truncated.chars().count(), 20);
    }

    #[test]
    fn test_truncate_left_unicode() {
        // Unicode characters should be handled correctly
        let path = "/home/用户/音乐/歌曲.mp3";
        let truncated = truncate_left(path, 15);
        assert!(truncated.starts_with("..."));
        assert_eq!(truncated.chars().count(), 15);
    }
}
