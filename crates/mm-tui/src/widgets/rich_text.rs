//! Rich text content model for width-aware structured wizard pane rendering.
//!
//! Provides `RichBlock` — a content block enum that can represent headings,
//! paragraphs with inline styled spans, separators, blank lines, and N-column
//! tables with per-cell wrapping. The `render_rich()` function flattens blocks
//! into `Vec<Line<'static>>` at a given width.
//!
//! Shared text utilities (`wrap_text`, `pad_to_width`, `dim_style`) live here
//! and are re-exported for use by `ThreeColTable` and other widgets.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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

    #[cfg(test)]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }
}

/// Width-aware content block for structured wizard pane rendering.
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

/// Render rich blocks into flat lines at a given width.
pub fn render_rich(blocks: &[RichBlock], width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut output = Vec::new();

    for block in blocks {
        match block {
            RichBlock::Heading(text) => {
                let style = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                let display = truncate_to_width(text, w);
                output.push(Line::from(Span::styled(display, style)));
            }

            RichBlock::Paragraph(spans) => {
                render_paragraph(&mut output, spans, w);
            }

            RichBlock::Separator => {
                let sep = "\u{2500}".repeat(w);
                output.push(Line::from(Span::styled(
                    sep,
                    Style::default().fg(Color::DarkGray),
                )));
            }

            RichBlock::Blank => {
                output.push(Line::raw(""));
            }

            RichBlock::Table {
                headers,
                rows,
                col_ratio,
            } => {
                render_table(&mut output, headers, rows, col_ratio, w);
            }
        }
    }

    output
}

// ============================================================================
// Paragraph rendering
// ============================================================================

fn render_paragraph(output: &mut Vec<Line<'static>>, spans: &[RichSpan], width: usize) {
    if width == 0 {
        output.push(Line::raw(""));
        return;
    }

    // Walk spans left-to-right, building lines. When adding chars from a span
    // would exceed width, break to the next line.
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        let mut remaining = span.text.as_str();
        let style = span.style;

        while !remaining.is_empty() {
            let (chunk, rest, chunk_width) = take_fitting(remaining, width - current_width);

            if chunk.is_empty() && current_width > 0 {
                // Can't fit anything on this line, wrap
                output.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
                continue;
            }

            if chunk.is_empty() && current_width == 0 {
                // Single character wider than width — take at least one char to avoid infinite loop
                let mut chars = remaining.chars();
                let ch = chars.next().unwrap();
                let s = ch.to_string();
                current_spans.push(Span::styled(s, style));
                current_width += ch.width().unwrap_or(0);
                remaining = chars.as_str();
                if !remaining.is_empty() {
                    output.push(Line::from(std::mem::take(&mut current_spans)));
                    current_width = 0;
                }
                continue;
            }

            current_spans.push(Span::styled(chunk.to_string(), style));
            current_width += chunk_width;
            remaining = rest;

            if current_width >= width && !remaining.is_empty() {
                output.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
            }
        }
    }

    // Flush remaining spans (always produce at least one line)
    if !current_spans.is_empty() || output.is_empty() {
        output.push(Line::from(current_spans));
    }
}

/// Take as many characters from `text` as fit within `avail` display columns.
/// Returns (taken_str, remaining_str, taken_width).
fn take_fitting(text: &str, avail: usize) -> (&str, &str, usize) {
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
fn truncate_to_width(text: &str, max_width: usize) -> String {
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

// ============================================================================
// Table rendering
// ============================================================================

fn render_table(
    output: &mut Vec<Line<'static>>,
    headers: &[RichSpan],
    rows: &[Vec<Vec<RichSpan>>],
    col_ratio: &[u16],
    total_width: usize,
) {
    let num_cols = headers.len();
    if num_cols == 0 || total_width < num_cols + 1 {
        return;
    }

    let col_widths = compute_col_widths(total_width, col_ratio);
    let sep_style = Style::default().fg(Color::DarkGray);

    // Header line
    let mut header_spans = Vec::new();
    for (i, header) in headers.iter().enumerate() {
        if i > 0 {
            header_spans.push(Span::styled("\u{2502}", sep_style));
        }
        header_spans.push(Span::styled(
            pad_to_width(&truncate_to_width(&header.text, col_widths[i]), col_widths[i]),
            header.style,
        ));
    }
    output.push(Line::from(header_spans));

    // Data rows
    for (row_idx, row) in rows.iter().enumerate() {
        let is_dim = row_idx % 2 == 1;

        // Wrap each cell
        let wrapped_cells: Vec<Vec<(String, Style)>> = row
            .iter()
            .enumerate()
            .map(|(col_idx, cell)| {
                let col_w = col_widths.get(col_idx).copied().unwrap_or(1);
                wrap_cell(cell, col_w, is_dim)
            })
            .collect();

        // Row height = max wrapped lines across cells
        let row_height = wrapped_cells.iter().map(|c| c.len().max(1)).max().unwrap_or(1);

        for line_idx in 0..row_height {
            let mut line_spans = Vec::new();
            for (col_idx, cell_lines) in wrapped_cells.iter().enumerate() {
                if col_idx > 0 {
                    line_spans.push(Span::styled("\u{2502}", sep_style));
                }
                let col_w = col_widths.get(col_idx).copied().unwrap_or(1);
                if let Some((text, style)) = cell_lines.get(line_idx) {
                    line_spans.push(Span::styled(pad_to_width(text, col_w), *style));
                } else {
                    line_spans.push(Span::styled(" ".repeat(col_w), Style::default()));
                }
            }
            output.push(Line::from(line_spans));
        }
    }
}

/// Compute column widths from ratios and total width.
fn compute_col_widths(total_width: usize, col_ratio: &[u16]) -> Vec<usize> {
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

/// Wrap a cell's spans into lines fitting within `col_width`.
/// Returns Vec of (text, style) per display line.
fn wrap_cell(spans: &[RichSpan], col_width: usize, is_dim: bool) -> Vec<(String, Style)> {
    // Concatenate all span texts and track style boundaries
    let mut full_text = String::new();
    let mut style_runs: Vec<(usize, usize, Style)> = Vec::new(); // (byte_start, byte_end, style)

    for span in spans {
        let start = full_text.len();
        full_text.push_str(&span.text);
        let end = full_text.len();
        let style = if is_dim {
            dim_style(span.style)
        } else {
            span.style
        };
        style_runs.push((start, end, style));
    }

    let wrapped_lines = wrap_text(&full_text, col_width);

    // Map wrapped lines back to styled output.
    // For simplicity, find the dominant style for each wrapped line segment.
    let mut result = Vec::new();
    let mut byte_offset = 0;

    for line_text in &wrapped_lines {
        let line_byte_end = byte_offset + line_text.len();
        // Find the style that covers the start of this line
        let style = style_runs
            .iter()
            .find(|&&(start, end, _)| start < line_byte_end && end > byte_offset)
            .map(|&(_, _, s)| s)
            .unwrap_or(Style::default());
        result.push((line_text.clone(), style));
        byte_offset = line_byte_end;
    }

    result
}

// ============================================================================
// Shared text utilities (used by ThreeColTable and rich_text)
// ============================================================================

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

/// Produce a dimmed variant of a style for alternating rows.
pub fn dim_style(style: Style) -> Style {
    match style.fg {
        Some(Color::White) => style.fg(Color::Gray),
        Some(Color::Green) => style.fg(Color::Green),
        Some(Color::Red) => style.fg(Color::Red),
        Some(Color::Cyan) => style.fg(Color::Cyan),
        _ => style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_renders_single_line() {
        let blocks = vec![RichBlock::Heading("Test Heading".into())];
        let lines = render_rich(&blocks, 80);
        assert_eq!(lines.len(), 1);
        // Should contain the heading text
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Test Heading");
    }

    #[test]
    fn paragraph_no_wrap() {
        let blocks = vec![RichBlock::Paragraph(vec![
            RichSpan::new("Hello ", Style::default().fg(Color::White)),
            RichSpan::new("World", Style::default().fg(Color::Green)),
        ])];
        let lines = render_rich(&blocks, 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 2);
    }

    #[test]
    fn paragraph_wraps_at_width() {
        let blocks = vec![RichBlock::Paragraph(vec![
            RichSpan::new("AAAA", Style::default().fg(Color::White)),
            RichSpan::new(" ", Style::default()),
            RichSpan::new("BBBB", Style::default().fg(Color::Green)),
        ])];
        // Width 5: "AAAA " fits on first line, "BBBB" wraps to second
        let lines = render_rich(&blocks, 5);
        assert!(lines.len() >= 2, "expected wrapping, got {} lines", lines.len());
    }

    #[test]
    fn paragraph_wraps_mid_span() {
        let blocks = vec![RichBlock::Paragraph(vec![RichSpan::new(
            "ABCDEFGHIJ",
            Style::default(),
        )])];
        let lines = render_rich(&blocks, 5);
        assert_eq!(lines.len(), 2);
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(first, "ABCDE");
        let second: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(second, "FGHIJ");
    }

    #[test]
    fn blank_and_separator() {
        let blocks = vec![RichBlock::Blank, RichBlock::Separator];
        let lines = render_rich(&blocks, 10);
        assert_eq!(lines.len(), 2);
        // First line is blank
        let blank_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(blank_text, "");
        // Second line is separator
        let sep_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(sep_text, "\u{2500}".repeat(10));
    }

    #[test]
    fn table_basic() {
        let blocks = vec![RichBlock::Table {
            headers: vec![
                RichSpan::new("A", Style::default()),
                RichSpan::new("B", Style::default()),
            ],
            rows: vec![vec![
                vec![RichSpan::plain("x")],
                vec![RichSpan::plain("y")],
            ]],
            col_ratio: vec![50, 50],
        }];
        let lines = render_rich(&blocks, 21); // 10 + 1 sep + 10
        assert_eq!(lines.len(), 2); // header + 1 data row
    }

    #[test]
    fn table_cell_wrapping() {
        let blocks = vec![RichBlock::Table {
            headers: vec![
                RichSpan::new("Col1", Style::default()),
                RichSpan::new("Col2", Style::default()),
            ],
            rows: vec![vec![
                vec![RichSpan::plain("Short")],
                vec![RichSpan::plain("This is a long cell that should wrap")],
            ]],
            col_ratio: vec![50, 50],
        }];
        // Width 21 → each col ~10 chars. "This is a long cell that should wrap" wraps.
        let lines = render_rich(&blocks, 21);
        assert!(lines.len() > 2, "expected cell wrapping, got {} lines", lines.len());
    }

    #[test]
    fn table_alternating_rows() {
        let blocks = vec![RichBlock::Table {
            headers: vec![RichSpan::new("H", Style::default())],
            rows: vec![
                vec![vec![RichSpan::new("r0", Style::default().fg(Color::White))]],
                vec![vec![RichSpan::new("r1", Style::default().fg(Color::White))]],
            ],
            col_ratio: vec![100],
        }];
        let lines = render_rich(&blocks, 20);
        assert_eq!(lines.len(), 3); // header + 2 rows
        // Row 1 (odd) should have dimmed style (White → Gray)
        let row1_style = lines[2].spans[0].style;
        assert_eq!(row1_style.fg, Some(Color::Gray));
    }

    #[test]
    fn table_empty_rows() {
        let blocks = vec![RichBlock::Table {
            headers: vec![
                RichSpan::new("A", Style::default()),
                RichSpan::new("B", Style::default()),
            ],
            rows: vec![],
            col_ratio: vec![50, 50],
        }];
        let lines = render_rich(&blocks, 21);
        assert_eq!(lines.len(), 1); // just header
    }

    #[test]
    fn render_rich_mixed() {
        let blocks = vec![
            RichBlock::Heading("Title".into()),
            RichBlock::Paragraph(vec![RichSpan::plain("Some text")]),
            RichBlock::Blank,
            RichBlock::Table {
                headers: vec![RichSpan::new("H", Style::default())],
                rows: vec![vec![vec![RichSpan::plain("data")]]],
                col_ratio: vec![100],
            },
        ];
        let lines = render_rich(&blocks, 40);
        // Heading(1) + Paragraph(1) + Blank(1) + Table(header+row=2) = 5
        assert_eq!(lines.len(), 5);
    }

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
