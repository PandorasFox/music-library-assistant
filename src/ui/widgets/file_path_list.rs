//! File Path List rendering helper.
//!
//! Provides a common "scrollable list of paths" pattern with:
//! - Right-truncation via `truncate_right()` from `ui/helpers.rs`
//! - Standard cursor highlighting via `CURSOR_STYLE`
//! - Optional prefix/suffix `Span`s per entry (for `[WAV]` tags, `> ` cursors, etc.)
//! - Multi-line balloon expansion showing the hidden tail when a cursor path exceeds available width

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};
use ratatui::Frame;

use super::path_display::wrap_path;
use super::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};
use crate::ui::helpers::truncate_right;

/// A single entry in a file path list.
pub struct PathEntry<'a> {
    /// The file path to display.
    pub path: &'a str,
    /// Spans rendered before the path (e.g., `[WAV] `, `> `, `[✓] `).
    pub prefix: Vec<Span<'a>>,
    /// Spans rendered after the path (e.g., ` →I`, ` →D`).
    pub suffix: Vec<Span<'a>>,
}

impl<'a> PathEntry<'a> {
    /// Create a plain entry with no prefix or suffix.
    pub fn plain(path: &'a str) -> Self {
        Self {
            path,
            prefix: Vec::new(),
            suffix: Vec::new(),
        }
    }
}

/// Balloon continuation line style: DarkGray fg so the directory portion
/// appears as a muted continuation of the cursor highlight.
const BALLOON_STYLE: Style = Style::new().fg(Color::DarkGray);
const BALLOON_PREFIX: &str = "  \u{21b3} ";

/// Render a scrollable list of file paths with cursor highlighting and balloon.
///
/// Returns the full untruncated path of the cursor item (useful for info bars).
///
/// **Balloon behavior:** When the cursor item's path exceeds the available width
/// (after accounting for prefix/suffix), the main line shows the start of the path
/// (right-truncated) and one or more continuation lines with a `↳` prefix show
/// the hidden tail, wrapped at `/` boundaries via `wrap_path`. The visible height
/// is reduced by the balloon line count when active.
pub fn render_file_path_list(
    f: &mut Frame,
    area: Rect,
    entries: &[PathEntry],
    cursor: usize,
    scroll: usize,
) -> Option<String> {
    if entries.is_empty() {
        return None;
    }

    let total_width = area.width as usize;
    let total_height = area.height as usize;

    if total_height == 0 || total_width == 0 {
        return None;
    }

    // Check if the cursor item's path needs a balloon; compute how many lines
    let cursor_entry = entries.get(cursor);
    let balloon_line_count = cursor_entry
        .map(|entry| {
            let prefix_width = span_char_width(&entry.prefix);
            let suffix_width = span_char_width(&entry.suffix);
            let path_budget = total_width.saturating_sub(prefix_width + suffix_width);
            if entry.path.chars().count() <= path_budget {
                return 0;
            }
            // Hidden portion = tail that truncate_right replaced with "..."
            let visible_chars = path_budget.saturating_sub(3);
            let hidden_part: String = entry.path.chars().skip(visible_chars).collect();
            let balloon_prefix_width = BALLOON_PREFIX.chars().count();
            let balloon_budget = total_width.saturating_sub(balloon_prefix_width);
            wrap_path(&hidden_part, balloon_budget).len()
        })
        .unwrap_or(0);

    // If balloon is active and cursor is visible, we lose display lines
    let visible_height = total_height.saturating_sub(balloon_line_count);

    // Clamp scroll so cursor is visible
    let scroll = clamp_scroll(scroll, cursor, visible_height, entries.len());

    // Determine if cursor is in the visible window
    let cursor_visible = cursor >= scroll && cursor < scroll + visible_height;
    let balloon_active = balloon_line_count > 0 && cursor_visible;

    // The cursor's position within the visible window
    let cursor_visual_row = if cursor_visible {
        Some(cursor - scroll)
    } else {
        None
    };

    // Build list items
    let mut items: Vec<ListItem> = Vec::with_capacity(
        visible_height + balloon_line_count,
    );

    for (visible_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
        let Some(entry) = entries.get(entry_idx) else {
            break;
        };

        let is_cursor = entry_idx == cursor;
        let prefix_width = span_char_width(&entry.prefix);
        let suffix_width = span_char_width(&entry.suffix);
        let path_budget = total_width.saturating_sub(prefix_width + suffix_width);

        let truncated = truncate_right(entry.path, path_budget);
        let path_style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };

        let mut spans = Vec::with_capacity(entry.prefix.len() + 1 + entry.suffix.len());

        // Prefix spans: inherit cursor style when on cursor line
        for span in &entry.prefix {
            if is_cursor {
                spans.push(Span::styled(span.content.clone(), CURSOR_STYLE));
            } else {
                spans.push(span.clone());
            }
        }

        spans.push(Span::styled(truncated, path_style));

        // Suffix spans: inherit cursor style when on cursor line
        for span in &entry.suffix {
            if is_cursor {
                spans.push(Span::styled(span.content.clone(), CURSOR_STYLE));
            } else {
                spans.push(span.clone());
            }
        }

        // Pad remaining width with cursor style background when on cursor line
        if is_cursor {
            let used_width = span_char_width_from_spans(&spans);
            if used_width < total_width {
                let pad = " ".repeat(total_width - used_width);
                spans.push(Span::styled(pad, CURSOR_STYLE));
            }
        }

        items.push(ListItem::new(Line::from(spans)));

        // Insert balloon lines immediately after cursor
        if balloon_active && Some(visible_idx) == cursor_visual_row {
            for line in build_balloon_lines(entry.path, path_budget, total_width) {
                items.push(ListItem::new(line));
            }
        }
    }

    let list = List::new(items);
    f.render_widget(list, area);

    cursor_entry.map(|e| e.path.to_string())
}

/// Build balloon continuation lines showing the truncated-away tail portion.
///
/// Uses `wrap_path` to split the hidden portion at `/` boundaries so the
/// full path is always visible across multiple balloon lines.
fn build_balloon_lines(full_path: &str, path_budget: usize, total_width: usize) -> Vec<Line<'static>> {
    // The part that was truncated away (the tail that got replaced by "...")
    let visible_chars = path_budget.saturating_sub(3); // "..." takes 3
    let hidden_part: String = full_path.chars().skip(visible_chars).collect();

    let prefix_width = BALLOON_PREFIX.chars().count();
    let balloon_budget = total_width.saturating_sub(prefix_width);

    wrap_path(&hidden_part, balloon_budget)
        .into_iter()
        .map(|seg| {
            Line::from(vec![
                Span::styled(BALLOON_PREFIX.to_string(), BALLOON_STYLE),
                Span::styled(seg, BALLOON_STYLE),
            ])
        })
        .collect()
}

/// Compute total character width of prefix/suffix Span slices.
fn span_char_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// Compute total character width from a Vec of Spans (for padding calculation).
fn span_char_width_from_spans(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// Clamp scroll offset so the cursor item is always visible.
fn clamp_scroll(scroll: usize, cursor: usize, visible_height: usize, total: usize) -> usize {
    if total == 0 || visible_height == 0 {
        return 0;
    }
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = scroll.min(max_scroll);

    if cursor < scroll {
        cursor
    } else if cursor >= scroll + visible_height {
        cursor.saturating_sub(visible_height - 1)
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_char_width_empty() {
        assert_eq!(span_char_width(&[]), 0);
    }

    #[test]
    fn test_span_char_width_with_spans() {
        let spans = vec![
            Span::raw("[WAV] "),
            Span::raw(" ->I"),
        ];
        assert_eq!(span_char_width(&spans), 10);
    }

    #[test]
    fn test_clamp_scroll_cursor_below() {
        // Cursor at 15, scroll at 0, visible 10 items => scroll should move to 6
        assert_eq!(clamp_scroll(0, 15, 10, 20), 6);
    }

    #[test]
    fn test_clamp_scroll_cursor_above() {
        // Cursor at 2, scroll at 5 => scroll should move to 2
        assert_eq!(clamp_scroll(5, 2, 10, 20), 2);
    }

    #[test]
    fn test_clamp_scroll_cursor_visible() {
        // Cursor at 5, scroll at 3, visible 10 => no change needed
        assert_eq!(clamp_scroll(3, 5, 10, 20), 3);
    }

    #[test]
    fn test_clamp_scroll_max_bound() {
        // scroll at 15 but only 20 items with visible_height 10 => max scroll is 10
        assert_eq!(clamp_scroll(15, 12, 10, 20), 10);
    }

    #[test]
    fn test_balloon_lines_content() {
        let lines = build_balloon_lines(
            "/very/long/path/to/some/deeply/nested/directory/file.flac",
            30,
            60,
        );
        assert!(!lines.is_empty());
        // Each line should start with balloon prefix
        for line in &lines {
            let first_span = &line.spans[0];
            assert!(first_span.content.contains('\u{21b3}'));
        }
    }

    #[test]
    fn test_path_entry_plain() {
        let entry = PathEntry::plain("/some/path.flac");
        assert!(entry.prefix.is_empty());
        assert!(entry.suffix.is_empty());
        assert_eq!(entry.path, "/some/path.flac");
    }
}
