//! Status Bar Widget
//!
//! A minimal 2-line status bar. Callers provide the content for each line;
//! the widget just renders them.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::helpers::truncate_left;

/// Render the 2-line status bar.
///
/// Both lines are optional. Line 1 is left-truncated to fit the available
/// width (useful for long paths). Line 2 is rendered as-is.
pub fn render(f: &mut Frame, area: Rect, line1: Option<&str>, line2: Option<&str>) {
    let style = Style::default().fg(Color::DarkGray);

    let line1 = if let Some(text) = line1 {
        let max_width = area.width.saturating_sub(2) as usize;
        let display = truncate_left(text, max_width);
        Line::from(Span::styled(display, style))
    } else {
        Line::from("")
    };

    let line2 = if let Some(text) = line2 {
        Line::from(Span::styled(text.to_string(), style))
    } else {
        Line::from("")
    };

    let para = Paragraph::new(vec![line1, line2]);
    f.render_widget(para, area);
}
