//! Rich text content model for width-aware structured rendering.
//!
//! Provides `RichBlock` — a content block enum that can represent headings,
//! paragraphs with inline styled spans, separators, blank lines, and N-column
//! tables with per-cell wrapping.
//!
//! Type definitions and pure text utilities live here (mm-ui).
//! Rendering to ratatui `Line`/`Span` output lives in mm-tui.

use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A styled text span (owned, 'static).
#[derive(Debug, Clone)]
pub struct RichSpan {
    pub text: String,
    pub style: Style,
}

impl RichSpan {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }
}

/// Width-aware content block for structured rendering.
#[derive(Debug, Clone)]
pub enum RichBlock {
    /// Bold heading line (yellow, bold by default).
    Heading(String),
    /// Inline styled spans that wrap at available width.
    Paragraph(Vec<RichSpan>),
    /// Horizontal separator (─── across full width).
    Separator,
    /// Empty line.
    Blank,
    /// N-column table with styled span cells and per-cell wrapping.
    Table {
        /// Column headers (one RichSpan per column).
        headers: Vec<RichSpan>,
        /// Data rows. Each row is a vec of cells; each cell is a vec of spans.
        rows: Vec<Vec<Vec<RichSpan>>>,
        /// Relative column widths (e.g. [30, 35, 35]). Must match column count.
        col_ratio: Vec<u16>,
    },
}

// ============================================================================
// Pure text utilities
// ============================================================================

/// Take as many characters from `text` as fit within `avail` display columns.
/// Returns (taken_str, remaining_str, taken_width).
pub fn take_fitting(text: &str, avail: usize) -> (&str, &str, usize) {
    let mut width = 0;
    let mut byte_end = 0;

    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > avail {
            break;
        }
        width += cw;
        byte_end += ch.len_utf8();
    }

    (&text[..byte_end], &text[byte_end..], width)
}

/// Truncate a string to fit within `max_width` display columns.
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > max_width {
            break;
        }
        result.push(ch);
        width += cw;
    }
    result
}

/// Wrap text to fit within `width` display columns, respecting wide (CJK) characters.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Pad `text` with trailing spaces so its display width reaches `target_width`.
pub fn pad_to_width(text: &str, target_width: usize) -> String {
    let text_width = UnicodeWidthStr::width(text);
    let padding = target_width.saturating_sub(text_width);
    let mut s = String::with_capacity(text.len() + padding);
    s.push_str(text);
    for _ in 0..padding {
        s.push(' ');
    }
    s
}

/// Compute column widths from ratios and total width.
pub fn compute_col_widths(total_width: usize, col_ratio: &[u16]) -> Vec<usize> {
    let num_cols = col_ratio.len();
    if num_cols == 0 {
        return vec![];
    }

    let separators = num_cols.saturating_sub(1);
    let available = total_width.saturating_sub(separators);
    let ratio_sum: u16 = col_ratio.iter().sum();

    if ratio_sum == 0 {
        return vec![1; num_cols];
    }

    let mut widths: Vec<usize> = col_ratio
        .iter()
        .map(|&r| (available * r as usize / ratio_sum as usize).max(1))
        .collect();

    // Distribute rounding remainder to last column
    let used: usize = widths.iter().sum();
    if used < available {
        if let Some(last) = widths.last_mut() {
            *last += available - used;
        }
    }

    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_basic() {
        assert_eq!(wrap_text("hello", 10), vec!["hello"]);
        assert_eq!(wrap_text("hello world!", 5), vec!["hello", " worl", "d!"]);
        assert_eq!(wrap_text("", 5), vec![""]);
    }

    #[test]
    fn pad_to_width_basic() {
        assert_eq!(pad_to_width("hi", 5), "hi   ");
        assert_eq!(pad_to_width("hello", 3), "hello"); // no truncation
    }
}
