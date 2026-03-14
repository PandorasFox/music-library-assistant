//! Shared pure utility functions.

use crate::input::InputAction;

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

/// Handle standard scroll input (NavUp/NavDown/PageUp/PageDown) for a single list.
///
/// Updates `scroll` in-place. Returns `true` if the action was handled.
pub fn handle_scroll_input(
    scroll: &mut usize,
    action: &InputAction,
    item_count: usize,
) -> bool {
    let max = item_count.saturating_sub(1);
    match action {
        InputAction::NavUp => {
            *scroll = scroll.saturating_sub(1);
            true
        }
        InputAction::NavDown => {
            if *scroll < max {
                *scroll += 1;
            }
            true
        }
        InputAction::PageUp => {
            *scroll = scroll.saturating_sub(10);
            true
        }
        InputAction::PageDown => {
            *scroll = (*scroll + 10).min(max);
            true
        }
        _ => false,
    }
}

// ============================================================================
// Modal Button State
// ============================================================================

/// Two-button state for resolution modals with a single action + cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StashCancelButton {
    StashAll,
    #[default]
    Cancel,
}

impl StashCancelButton {
    /// Move selection left (toward action button if available).
    pub fn left(&mut self, has_items: bool) {
        *self = match *self {
            Self::Cancel if has_items => Self::StashAll,
            other => other,
        };
    }

    /// Move selection right (toward cancel).
    pub fn right(&mut self) {
        *self = Self::Cancel;
    }
}

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
        let path = "/home/用户/音乐/歌曲.mp3";
        let truncated = truncate_left(path, 15);
        assert!(truncated.starts_with("..."));
        assert_eq!(truncated.chars().count(), 15);
    }
}
