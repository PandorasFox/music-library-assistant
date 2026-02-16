//! Inbox View Rendering
//!
//! Renders the inbox view as an aggregate signal overview:
//! - Corpus matches (magenta) — count of fingerprint matches
//! - Unindexed (yellow) — count of files pending indexing
//! - Files in inbox (gray) — total file count

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::{InboxInsightAction, InboxViewState};

/// Render the full inbox view.
pub fn render_inbox_view(f: &mut Frame, area: Rect, state: &InboxViewState) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(5),
        ])
        .split(area);

    // Render unified title bar
    let titlebar = UnifiedTitleBar::new(LateralView::Inbox);
    titlebar.render(f, main_chunks[0]);

    render_inbox_content(f, main_chunks[1], state);
}

fn render_inbox_content(f: &mut Frame, area: Rect, state: &InboxViewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Inbox Overview ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.entries.is_empty() {
        let empty = Paragraph::new("No files in inbox")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected;

            let count_str = format!("{:>6}", entry.count);
            let label_style = if is_selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            let action_indicator = match entry.action {
                InboxInsightAction::LaunchIntake
                | InboxInsightAction::LaunchCorpusMatchResolution
                | InboxInsightAction::LaunchInboxTagCanonicity => " \u{23CE}",
                InboxInsightAction::Informational => "",
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("  {} ", if is_selected { "▸" } else { " " }),
                    Style::default().fg(entry.color),
                ),
                Span::styled(
                    format!("{} ", count_str),
                    Style::default().fg(entry.color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(&entry.label, label_style),
                Span::styled(
                    action_indicator,
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

