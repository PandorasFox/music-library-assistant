//! Inbox View Rendering
//!
//! Renders the inbox view as an aggregate signal overview:
//! - Corpus matches (magenta) — count of fingerprint matches
//! - Unindexed (yellow) — count of files pending indexing
//! - Files in inbox (gray) — total file count

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::{InboxInsightAction, InboxViewState};

/// Render the full inbox view (titlebar is rendered by render_app).
pub fn render_inbox_view(f: &mut Frame, area: Rect, state: &InboxViewState) {
    render_inbox_content(f, area, state);
}

fn render_inbox_content(f: &mut Frame, area: Rect, state: &InboxViewState) {
    let busy = state.busy;
    let border_color = if busy { Color::DarkGray } else { Color::Gray };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Inbox Overview ")
        .border_style(Style::default().fg(border_color));
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

            let (entry_color, label_style, arrow) = if busy {
                (
                    Color::DarkGray,
                    Style::default().fg(Color::DarkGray),
                    " ",
                )
            } else {
                (
                    entry.color,
                    if is_selected {
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                    if is_selected { "▸" } else { " " },
                )
            };

            let action_indicator = if busy {
                ""
            } else {
                match entry.action {
                    InboxInsightAction::LaunchIntake
                    | InboxInsightAction::LaunchCorpusMatchResolution
                    | InboxInsightAction::LaunchInboxTagCanonicity
                    | InboxInsightAction::LaunchOrganize
                    | InboxInsightAction::LaunchInboxCompoundSplit => " \u{23CE}",
                    InboxInsightAction::Informational => "",
                }
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("  {} ", arrow),
                    Style::default().fg(entry_color),
                ),
                Span::styled(
                    format!("{} ", count_str),
                    Style::default().fg(entry_color).add_modifier(Modifier::BOLD),
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

