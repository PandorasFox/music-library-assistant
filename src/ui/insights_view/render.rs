//! Insights View Rendering
//!
//! Renders the insights view with TODO placeholders for main and details panes.
//! Dims content when the Witch is busy.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::InsightsViewState;

/// Render the full insights view
pub fn render_insights_view(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    // Layout: Title bar at top, content below
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()), // Title bar with borders
            Constraint::Min(5),                             // Content
        ])
        .split(area);

    // Render unified title bar
    let titlebar = UnifiedTitleBar::new(LateralView::Insights);
    titlebar.render(f, main_chunks[0]);

    // Layout: Main list on left (70%), details on right (30%)
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(main_chunks[1]);

    render_insights_list(f, content_chunks[0], state);
    render_insight_details(f, content_chunks[1], state);
}

/// Render the insights list (TODO placeholder)
fn render_insights_list(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Insights")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let text_color = if busy { Color::DarkGray } else { Color::Yellow };

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "TODO: Main insights pane",
            Style::default().fg(text_color),
        )),
    ];

    // Add busy indicator if the Witch is working
    if busy {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "(The Witch is busy - actions disabled)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

/// Render details pane (TODO placeholder)
fn render_insight_details(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let busy = state.is_witch_busy();
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .title("Details")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let text_color = if busy { Color::DarkGray } else { Color::Yellow };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "TODO: Details pane",
            Style::default().fg(text_color),
        )),
    ];

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}
