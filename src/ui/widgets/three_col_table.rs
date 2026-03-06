//! Reusable three-column wrapped table with alternating row tinting.
//!
//! Used by transaction review (Mutation/Before/After) and OOB tag resolution
//! modals (Field/DB Value/Disk Value). Supports per-cell styling, text wrapping
//! within columns, and a 20/40/40 (or configurable) column ratio.

use ratatui::layout::Rect;

/// A wrapped row: three columns of wrapped text lines, plus the computed row height.
type WrappedRow = (Vec<String>, Vec<String>, Vec<String>, usize);
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A single cell in the table with text and a style.
pub struct StyledCell {
    pub text: String,
    pub style: Style,
}

impl StyledCell {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// Configuration for a three-column wrapped table.
pub struct ThreeColTable {
    /// Column headers: (label, style) for each of the 3 columns.
    pub headers: [(String, Style); 3],
    /// Data rows. Each row has exactly 3 styled cells.
    pub rows: Vec<[StyledCell; 3]>,
    /// Column width ratio (must sum to 100). Default: [20, 40, 40].
    pub col_ratio: [usize; 3],
    /// Current scroll offset in lines.
    pub scroll: usize,
    /// Style applied to the `│` column separators.
    pub separator_style: Style,
    /// If true, alternate rows use dimmed style variants.
    pub alternate_rows: bool,
}

impl ThreeColTable {
    /// Render into the given area.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let total_width = area.width as usize;
        if total_width < 6 {
            return;
        }

        let (col_widths, _) = self.compute_col_widths(total_width);
        let wrapped_rows = self.compute_wrapped_rows(col_widths);

        // Header
        let header_line = Line::from(vec![
            Span::styled(
                pad_to_width(&self.headers[0].0, col_widths[0]),
                self.headers[0].1,
            ),
            Span::styled("\u{2502}", self.separator_style),
            Span::styled(
                pad_to_width(&self.headers[1].0, col_widths[1]),
                self.headers[1].1,
            ),
            Span::styled("\u{2502}", self.separator_style),
            Span::styled(
                pad_to_width(&self.headers[2].0, col_widths[2]),
                self.headers[2].1,
            ),
        ]);

        let header_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
        f.render_widget(Paragraph::new(header_line), header_area);

        // Body
        let body_y = area.y + 1;
        let body_height = area.height.saturating_sub(1) as usize;
        if body_height == 0 {
            return;
        }

        let total_lines: usize = wrapped_rows.iter().map(|(_, _, _, h)| *h).sum();
        let max_scroll = total_lines.saturating_sub(body_height);
        let scroll = self.scroll.min(max_scroll);

        // Find starting row/line for scroll offset
        let mut lines_skipped = 0usize;
        let mut start_row = 0usize;
        let mut start_line_in_row = 0usize;

        for (i, (_, _, _, row_h)) in wrapped_rows.iter().enumerate() {
            if lines_skipped + row_h > scroll {
                start_row = i;
                start_line_in_row = scroll - lines_skipped;
                break;
            }
            lines_skipped += row_h;
            if i == wrapped_rows.len() - 1 {
                start_row = wrapped_rows.len();
            }
        }

        let mut y_offset = 0usize;

        for (row_idx, (label_lines, col1_lines, col2_lines, row_height)) in
            wrapped_rows.iter().enumerate().skip(start_row)
        {
            if y_offset >= body_height {
                break;
            }

            let first_line = if row_idx == start_row { start_line_in_row } else { 0 };

            for line_idx in first_line..*row_height {
                if y_offset >= body_height {
                    break;
                }

                let label_text = label_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");
                let c1_text = col1_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");
                let c2_text = col2_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");

                // Get base styles from the row data
                let (ls, c1s, c2s) = if self.alternate_rows && row_idx % 2 == 1 {
                    (
                        dim_style(self.rows[row_idx][0].style),
                        dim_style(self.rows[row_idx][1].style),
                        dim_style(self.rows[row_idx][2].style),
                    )
                } else {
                    (
                        self.rows[row_idx][0].style,
                        self.rows[row_idx][1].style,
                        self.rows[row_idx][2].style,
                    )
                };

                let line = Line::from(vec![
                    Span::styled(pad_to_width(label_text, col_widths[0]), ls),
                    Span::styled("\u{2502}", self.separator_style),
                    Span::styled(pad_to_width(c1_text, col_widths[1]), c1s),
                    Span::styled("\u{2502}", self.separator_style),
                    Span::styled(pad_to_width(c2_text, col_widths[2]), c2s),
                ]);

                let line_area = Rect {
                    x: area.x,
                    y: body_y + y_offset as u16,
                    width: area.width,
                    height: 1,
                };
                f.render_widget(Paragraph::new(line), line_area);
                y_offset += 1;
            }
        }

        // Scroll indicators
        if total_lines > body_height {
            let indicator = if scroll > 0 && scroll < max_scroll {
                "^v"
            } else if scroll > 0 {
                "^"
            } else {
                "v"
            };
            let indicator_area = Rect {
                x: area.x + area.width.saturating_sub(3),
                y: body_y,
                width: 2,
                height: 1,
            };
            let indicator_widget =
                Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
            f.render_widget(indicator_widget, indicator_area);
        }
    }

    fn compute_col_widths(&self, total_width: usize) -> ([usize; 3], usize) {
        let col0_w = total_width * self.col_ratio[0] / 100;
        let remaining = total_width.saturating_sub(col0_w + 2); // 2 separator chars
        let col1_w = remaining * self.col_ratio[1] / (self.col_ratio[1] + self.col_ratio[2]);
        let col2_w = remaining.saturating_sub(col1_w);
        let widths = [col0_w.max(1), col1_w.max(1), col2_w.max(1)];
        let used = widths[0] + widths[1] + widths[2] + 2;
        (widths, used)
    }

    fn compute_wrapped_rows(
        &self,
        col_widths: [usize; 3],
    ) -> Vec<WrappedRow> {
        self.rows
            .iter()
            .map(|row| {
                let c0 = wrap_text(&row[0].text, col_widths[0]);
                let c1 = wrap_text(&row[1].text, col_widths[1]);
                let c2 = wrap_text(&row[2].text, col_widths[2]);
                let height = c0.len().max(c1.len()).max(c2.len());
                (c0, c1, c2, height)
            })
            .collect()
    }
}

/// Wrap text to fit within `width` display columns, respecting wide (CJK) characters.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
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
fn pad_to_width(text: &str, target_width: usize) -> String {
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
fn dim_style(style: Style) -> Style {
    // Swap bright colors to their dimmer equivalents
    match style.fg {
        Some(Color::White) => style.fg(Color::Gray),
        Some(Color::Green) => style.fg(Color::Green), // keep green distinguishable
        Some(Color::Red) => style.fg(Color::Red),     // keep red distinguishable
        Some(Color::Cyan) => style.fg(Color::Cyan),   // keep cyan distinguishable
        _ => style,
    }
}
