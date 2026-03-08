//! Progressive Worker Rendering
//!
//! Renders the progress bar modal for bulk operations.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

use super::ProgressiveWorkerState;

/// Render the progressive worker modal.
///
/// Shows a centered modal with:
/// - Title bar with operation label
/// - Progress bar with item count
/// - Current item label (if available)
pub fn render(f: &mut Frame, area: Rect, state: &ProgressiveWorkerState) {
    // Calculate modal dimensions - centered, modest size
    let modal_width = 60u16.min(area.width.saturating_sub(4));
    let modal_height = 8u16.min(area.height.saturating_sub(4));

    let modal_x = (area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect::new(
        area.x + modal_x,
        area.y + modal_y,
        modal_width,
        modal_height,
    );

    // Clear the modal area with a block
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" {} ", state.label),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(block.clone(), modal_area);

    // Inner area for content
    let inner = block.inner(modal_area);

    // Layout: progress bar + status text
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Progress bar
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Counter text
            Constraint::Length(1), // Current item label
            Constraint::Min(0),    // Remaining space
        ])
        .split(inner);

    // Progress bar
    let progress_ratio = state.progress_ratio();
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
        .ratio(progress_ratio);

    f.render_widget(gauge, chunks[1]);

    // Counter text: "45 / 100"
    let counter_text = format!("{} / {}", state.processed, state.total);
    let counter = Paragraph::new(counter_text)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center);

    f.render_widget(counter, chunks[3]);

    // Current item label (if available)
    if let Some(ref label) = state.current_label {
        let truncated = if label.len() > (modal_width as usize).saturating_sub(4) {
            format!(
                "{}...",
                &label[..label.len().min((modal_width as usize).saturating_sub(7))]
            )
        } else {
            label.clone()
        };

        let item_label = Paragraph::new(Line::from(vec![Span::styled(
            truncated,
            Style::default().fg(Color::DarkGray),
        )]))
        .alignment(Alignment::Center);

        f.render_widget(item_label, chunks[4]);
    }
}
