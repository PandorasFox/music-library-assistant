//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

// ============================================================================
// Pane Rendering Utilities
// ============================================================================

/// Render a block and return its inner area, properly cleared.
///
/// This is the standard way to render panes in MM. It:
/// 1. Computes the inner area from the block
/// 2. Renders the block (borders, title, etc.)
/// 3. Clears the inner area to prevent leftover content from showing through
/// 4. Returns the inner Rect for content rendering
///
/// Without clearing, list widgets that don't fill their area will show
/// "ghost" content from previous renders.
pub fn render_pane(f: &mut Frame, area: Rect, block: Block) -> Rect {
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Clear, inner);
    inner
}

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

/// Compute scroll offset to keep cursor visible (edge-pinning).
///
/// The cursor scrolls the viewport only when it reaches the edge.
/// This is distinct from center-biased scrolling (used by `StandardList`)
/// where the cursor stays near the middle of the viewport.
pub fn clamp_scroll(cursor: usize, scroll: usize, visible_height: usize) -> usize {
    if visible_height == 0 {
        return 0;
    }
    if cursor >= scroll + visible_height {
        cursor.saturating_sub(visible_height - 1)
    } else if cursor < scroll {
        cursor
    } else {
        scroll
    }
}

/// Create a bordered block with focus-aware border color.
///
/// Yellow border when focused, DarkGray when not. This is the standard
/// pane border pattern used across all multi-pane views.
pub fn focused_block(title: &str, is_focused: bool) -> Block<'_> {
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
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
    if max_chars <= 3 {
        return s.chars().take(max_chars).collect();
    }
    let take = max_chars - 3;
    format!("{}...", s.chars().take(take).collect::<String>())
}

/// Format a number with SI suffix, always 3 significant digits.
///
/// - `< 1000`: raw number ("847")
/// - `1000..10000`: X.XXk ("2.54k")
/// - `10000..100000`: XX.Xk ("25.4k")
/// - `100000..1000000`: XXXk ("254k")
/// - Same pattern for M, G, T...
pub fn format_si(n: usize) -> String {
    if n < 1000 {
        return n.to_string();
    }

    let suffixes = ['k', 'M', 'G', 'T', 'P'];
    let mut value = n as f64;
    for suffix in &suffixes {
        value /= 1000.0;
        if value < 10.0 {
            return format!("{:.2}{}", value, suffix);
        } else if value < 100.0 {
            return format!("{:.1}{}", value, suffix);
        } else if value < 1000.0 {
            return format!("{:.0}{}", value, suffix);
        }
    }
    // Fallback for astronomically large numbers
    format!("{:.0}P", value)
}

/// Format a duration in milliseconds as "M:SS".
pub fn format_duration_ms(ms: i64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}

/// Format a byte count with appropriate unit (bytes, KB, MB, GB).
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Format a sample rate as kHz or Hz.
pub fn format_sample_rate(sr: i32) -> String {
    if sr >= 1000 && sr % 1000 == 0 {
        format!("{} kHz", sr / 1000)
    } else if sr >= 1000 {
        format!("{:.1} kHz", sr as f64 / 1000.0)
    } else {
        format!("{} Hz", sr)
    }
}

/// Format a bitrate in kbps.
pub fn format_kbps(br: i32) -> String {
    format!("{} kbps", br)
}

// ============================================================================
// Mutation Introspection
// ============================================================================

/// Extract pending tag edits from a list of mutations.
///
/// Returns a map of inode → [(tag_name, old_value, new_value)] for display
/// in health modal right panes when a tag editor decision has been staged.
///
/// - `old_value` is empty for new tags
/// - `new_value` is empty for deleted tags
pub fn pending_edits_from_mutations(
    mutations: &[crate::meta::mutations::Mutation],
) -> std::collections::HashMap<i64, Vec<(String, String, String)>> {
    use crate::meta::mutations::Mutation;
    let mut result: std::collections::HashMap<i64, Vec<(String, String, String)>> =
        std::collections::HashMap::new();

    for mutation in mutations {
        if let Mutation::ApplyTagOps(ref ops) = mutation {
            for op in &ops.ops {
                let old = op.old_value.clone().unwrap_or_default();
                let new = op.new_value.clone().unwrap_or_default();
                result
                    .entry(op.inode)
                    .or_default()
                    .push((op.tag_name.clone(), old, new));
            }
        }
    }

    result
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
