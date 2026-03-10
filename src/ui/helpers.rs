//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.

use std::path::PathBuf;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

use crate::corpus::paths;
use crate::meta::mutations::file_ops::StashFromZoneMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::meta::mutations::Mutation;

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
// Rendering Helpers
// ============================================================================

/// Render a label-value line with DarkGray label (padded to 11 chars) and White value.
pub fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<11}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

/// Render a score bar: label + numeric value + filled/empty block bar.
pub fn render_score_bar(label: &str, value: f64) -> Line<'static> {
    const BAR_WIDTH: usize = 20;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let bar = format!("{}{}", "\u{2588}".repeat(filled), " ".repeat(empty));

    Line::from(vec![
        Span::styled(format!("  {}", label), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" {:.2}  ", value),
            Style::default().fg(Color::White),
        ),
        Span::styled(bar, Style::default().fg(Color::Yellow)),
    ])
}

/// Render audio metadata lines (Format/Duration/Bitrate/SampleRate/Size/AlbumArt)
/// from a `FileMetaSummary`. Returns lines to extend into caller's buffer.
pub fn render_audio_metadata_lines(
    meta: &crate::ui::manual_review_modal::types::FileMetaSummary,
    label_style: Style,
    value_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Format: ", label_style),
        Span::styled(meta.file_type.to_uppercase(), value_style),
    ]));

    if let Some(dur) = meta.duration_ms {
        lines.push(Line::from(vec![
            Span::styled("Duration: ", label_style),
            Span::styled(format_duration_ms(dur), value_style),
        ]));
    }

    if let Some(br) = meta.bitrate_kbps {
        lines.push(Line::from(vec![
            Span::styled("Bitrate: ", label_style),
            Span::styled(format_kbps(br), value_style),
        ]));
    }

    if let Some(sr) = meta.sample_rate {
        lines.push(Line::from(vec![
            Span::styled("Sample rate: ", label_style),
            Span::styled(format_sample_rate(sr), value_style),
        ]));
    }

    if meta.file_size > 0 {
        lines.push(Line::from(vec![
            Span::styled("Size: ", label_style),
            Span::styled(format_bytes(meta.file_size as u64), value_style),
        ]));
    }

    let art_label = if meta.has_pictures { "Yes" } else { "No" };
    let art_style = if meta.has_pictures {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(vec![
        Span::styled("Album art: ", label_style),
        Span::styled(art_label, art_style),
    ]));

    lines
}

/// Render "── Pending edits ──" separator and change lines for staged tag edits.
///
/// Returns lines to append. Caller decides how to wrap them (ListItem, Paragraph, etc.).
pub fn render_pending_edit_lines(
    edits: &[(String, String, String)],
    max_width: usize,
    remaining_height: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if edits.is_empty() || remaining_height == 0 {
        return lines;
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "\u{2500}\u{2500} Pending edits \u{2500}\u{2500}",
        Style::default().fg(Color::Magenta),
    )));
    for (tag, old, new) in edits {
        if lines.len() >= remaining_height {
            break;
        }
        let change = if old.is_empty() {
            format!("{}: +\"{}\"", tag, new)
        } else if new.is_empty() {
            format!("{}: -\"{}\"", tag, old)
        } else {
            format!("{}: \"{}\" \u{2192} \"{}\"", tag, old, new)
        };
        let display = truncate_right(&change, max_width);
        lines.push(Line::from(Span::styled(
            display,
            Style::default().fg(Color::Magenta),
        )));
    }
    lines
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
// Scroll Input Helpers
// ============================================================================

/// Handle standard scroll input (NavUp/NavDown/PageUp/PageDown) for a single list.
///
/// Updates `scroll` in-place. Returns `true` if the action was handled,
/// `false` if the caller should continue matching other actions.
/// `item_count` is the total number of items in the list.
pub fn handle_scroll_input(
    scroll: &mut usize,
    action: &crate::ui::input::InputAction,
    item_count: usize,
) -> bool {
    use crate::ui::input::InputAction;
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
///
/// Used by corrupt file, subpar duplicate, and similar stash-all modals.
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

// ============================================================================
// Mutation Helpers
// ============================================================================

/// Generate StashFromZone + DropFromIndex mutations for a single corpus file.
///
/// Used by resolution modals that stash problematic files (corrupt, subpar, etc.).
pub fn stash_file_mutations(corpus_path: &str, inode: i64, stash_name: &str) -> Vec<Mutation> {
    let resolver = paths::get_resolver();
    let abs_path = resolver.resolve(std::path::Path::new(corpus_path));

    vec![
        Mutation::StashFromZone(StashFromZoneMutation {
            path: abs_path,
            stash_name: stash_name.to_string(),
        }),
        Mutation::DropFromIndex(DropFromIndexMutation {
            path: PathBuf::from(corpus_path),
            inode: Some(inode),
            zone: Some("corpus".to_string()),
        }),
    ]
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
