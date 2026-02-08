//! Status Bar Widget
//!
//! A minimal 2-line status bar replacing the old 10-line footer.
//!
//! - Line 1: Selected item path OR status/error message
//! - Line 2: Transaction status when active

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::helpers::truncate_left;

/// Summary of the current pending transaction for display.
#[derive(Debug, Clone)]
pub struct TransactionSummary {
    /// Transaction label (e.g., "Deploy", "Tag edits")
    pub label: String,
    /// Number of decisions staged
    pub decision_count: usize,
    /// Total mutations across all decisions
    pub mutation_count: usize,
}

/// Render the 2-line status bar.
///
/// # Arguments
///
/// - `selected_path`: The path of the currently selected item (if any)
/// - `status_message`: An ephemeral status/error message to display
/// - `transaction`: Active transaction summary (if any)
pub fn render(
    f: &mut Frame,
    area: Rect,
    selected_path: Option<&str>,
    status_message: Option<&str>,
    transaction: Option<&TransactionSummary>,
) {
    // Line 1: Status message takes priority, then selected path
    let line1 = if let Some(msg) = status_message {
        Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(Color::Yellow),
        ))
    } else if let Some(path) = selected_path {
        // Truncate path from left if needed
        let max_width = area.width.saturating_sub(2) as usize;
        let display_path = truncate_left(path, max_width);
        Line::from(Span::styled(
            display_path,
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from("")
    };

    // Line 2: Transaction status
    let line2 = if let Some(txn) = transaction {
        let plural_d = if txn.decision_count == 1 { "" } else { "s" };
        let plural_m = if txn.mutation_count == 1 { "" } else { "s" };
        Line::from(vec![
            Span::styled("Transaction ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("\"{}\"", txn.label),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                txn.decision_count.to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!(" decision{}", plural_d),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(", ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                txn.mutation_count.to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!(" mutation{} staged", plural_m),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from("")
    };

    let para = Paragraph::new(vec![line1, line2]);
    f.render_widget(para, area);
}
