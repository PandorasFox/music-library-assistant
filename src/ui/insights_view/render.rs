//! Insights View Rendering
//!
//! Uses existing widgets for consistent styling:
//! - SelectableList for the insights list
//! - HealthStatus for status coloring
//! - StatusIndicator for the selected insight details

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::corpus::health::insights::{Insight, InsightSeverity};
use crate::ui::widgets::{
    HealthStatus, LateralView, SelectableItem, SelectableList, SelectableListStyle,
    StatusIndicator, UnifiedTitleBar,
};

use super::InsightsViewState;

/// Map InsightSeverity to widget HealthStatus
fn severity_to_health_status(severity: InsightSeverity) -> HealthStatus {
    match severity {
        InsightSeverity::Healthy => HealthStatus::Healthy,
        InsightSeverity::Info => HealthStatus::Info,
        InsightSeverity::Warning => HealthStatus::Warning,
        InsightSeverity::Critical => HealthStatus::Critical,
    }
}

/// Render the full insights view
pub fn render_insights_view(f: &mut Frame, area: Rect, state: &mut InsightsViewState) {
    // Layout: Title bar at top (3 rows for borders), content below
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

/// Render the insights list
fn render_insights_list(f: &mut Frame, area: Rect, state: &mut InsightsViewState) {
    // Convert insights to selectable items with severity coloring
    let items: Vec<SelectableItem> = state
        .insights
        .iter()
        .map(|insight| {
            let status = severity_to_health_status(insight.severity());
            let icon = match insight.severity() {
                InsightSeverity::Healthy => "[OK]",
                InsightSeverity::Info => "[i]",
                InsightSeverity::Warning => "[!]",
                InsightSeverity::Critical => "[X]",
            };

            // Build label with icon
            let label = format!("{} {}", icon, insight.label());

            // Add suffix with item count if actionable
            let suffix = if insight.is_actionable() {
                Some(vec![Span::styled(
                    " →",
                    Style::default().fg(Color::DarkGray),
                )])
            } else {
                None
            };

            let mut item = SelectableItem::new(label).with_style(status.style());
            if let Some(s) = suffix {
                item = item.with_suffix(s);
            }
            item
        })
        .collect();

    // Create list with title
    let list = SelectableList::new(items)
        .title("Corpus Insights")
        .style(SelectableListStyle::arrow());

    list.render(f, area, &mut state.list_state);
}

/// Render details for the selected insight
fn render_insight_details(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let block = Block::default()
        .title("Details")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(insight) = state.selected_insight() {
        let status = severity_to_health_status(insight.severity());

        let mut lines = vec![
            // Title line with severity color
            Line::from(vec![
                Span::styled("Status: ", Style::default()),
                Span::styled(
                    format!("{:?}", insight.severity()),
                    status.emphasized(),
                ),
            ]),
            Line::from(""),
            // Description
            Line::from(vec![Span::styled(
                insight.description(),
                Style::default().fg(Color::White),
            )]),
            Line::from(""),
        ];

        // Add item count if relevant
        let count = insight.item_count();
        if count > 0 {
            lines.push(Line::from(vec![
                Span::raw("Items: "),
                Span::styled(
                    count.to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Add actionable hint
        if insight.is_actionable() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Press Enter to resolve",
                Style::default().fg(Color::Green),
            )]));
        } else if matches!(insight, Insight::Computing { .. }) {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Computing in background...",
                Style::default().fg(Color::Cyan),
            )]));
        }

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);
    } else {
        let paragraph = Paragraph::new("No insight selected")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(paragraph, inner);
    }
}

/// Render a minimal status bar showing computation progress
pub fn render_insights_status(f: &mut Frame, area: Rect, state: &InsightsViewState) {
    let status = if state.has_pending_computation() {
        "Computing insights..."
    } else {
        "Ready"
    };

    let line = Line::from(vec![
        Span::raw("Insights: "),
        Span::styled(
            format!("{} items", state.insights.len()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" | "),
        Span::raw(status),
    ]);

    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}
