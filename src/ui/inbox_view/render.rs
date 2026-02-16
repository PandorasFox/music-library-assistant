//! Inbox View Rendering
//!
//! Renders the inbox view as a file list grouped by status:
//! - Corpus Match (red/magenta) — fingerprint match against corpus, stash candidate
//! - Unindexed files (yellow) — pending indexing
//! - Healthy files (green) — indexed, ready for operations

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::{InboxEntryStatus, InboxViewState};

/// Render the full inbox view.
pub fn render_inbox_view(f: &mut Frame, area: Rect, state: &mut InboxViewState) {
    // Layout: Title bar at top, content below
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(5),
            Constraint::Length(1), // Controls hint
        ])
        .split(area);

    // Render unified title bar
    let titlebar = UnifiedTitleBar::new(LateralView::Inbox);
    titlebar.render(f, main_chunks[0]);

    render_inbox_content(f, main_chunks[1], state);
    render_controls_hint(f, main_chunks[2], state);
}

fn render_inbox_content(f: &mut Frame, area: Rect, state: &mut InboxViewState) {
    // Compute status counts from entries
    let mut n_match = 0usize;
    let mut n_unindexed = 0usize;
    let mut n_ready = 0usize;
    for entry in &state.entries {
        match entry.status {
            InboxEntryStatus::CorpusMatch(_) => n_match += 1,
            InboxEntryStatus::Unindexed => n_unindexed += 1,
            InboxEntryStatus::Healthy => n_ready += 1,
        }
    }

    let title = if state.entries.is_empty() {
        " Inbox Files ".to_string()
    } else {
        let mut parts = Vec::new();
        if n_match > 0 { parts.push(format!("{} matched", n_match)); }
        if n_unindexed > 0 { parts.push(format!("{} unindexed", n_unindexed)); }
        if n_ready > 0 { parts.push(format!("{} ready", n_ready)); }
        format!(" Inbox Files — {} ", parts.join(", "))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.entries.is_empty() {
        let empty = Paragraph::new("No files in inbox")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    }

    // Adjust scroll for visible area
    let visible_height = inner.height as usize;
    if state.selected >= state.scroll + visible_height {
        state.scroll = state.selected - visible_height + 1;
    }
    if state.selected < state.scroll {
        state.scroll = state.selected;
    }

    let items: Vec<ListItem> = state
        .entries
        .iter()
        .enumerate()
        .skip(state.scroll)
        .take(visible_height)
        .map(|(i, entry)| {
            let (status_label, status_color) = match &entry.status {
                InboxEntryStatus::CorpusMatch(data) => {
                    let n = data.corpus_matches.len();
                    let best_sim = data.corpus_matches.iter()
                        .map(|m| m.similarity)
                        .fold(0.0_f64, f64::max);
                    (
                        format!("MATCH({}) {:.0}%", n, best_sim),
                        Color::Magenta,
                    )
                }
                InboxEntryStatus::Unindexed => ("UNINDEXED".to_string(), Color::Yellow),
                InboxEntryStatus::Healthy => ("READY".to_string(), Color::Green),
            };

            let is_selected = i == state.selected;

            let style = if is_selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            // Strip "inbox/" prefix for display
            let display_path = entry.path.strip_prefix("inbox/").unwrap_or(&entry.path);

            let line = Line::from(vec![
                Span::styled(
                    format!(" {:>14} ", status_label),
                    Style::default().fg(status_color),
                ),
                Span::styled(display_path, style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn render_controls_hint(f: &mut Frame, area: Rect, state: &InboxViewState) {
    let hints = match state.selected_entry().map(|e| &e.status) {
        Some(InboxEntryStatus::CorpusMatch(_)) => {
            vec![
                Span::styled(" Enter", Style::default().fg(Color::Cyan)),
                Span::styled(" Stash  ", Style::default().fg(Color::DarkGray)),
                Span::styled("T", Style::default().fg(Color::Cyan)),
                Span::styled(" Tags  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Tab", Style::default().fg(Color::Cyan)),
                Span::styled(" Cycle View", Style::default().fg(Color::DarkGray)),
            ]
        }
        Some(InboxEntryStatus::Healthy) => {
            vec![
                Span::styled(" Enter/T", Style::default().fg(Color::Cyan)),
                Span::styled(" Tags  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Tab", Style::default().fg(Color::Cyan)),
                Span::styled(" Cycle View", Style::default().fg(Color::DarkGray)),
            ]
        }
        Some(InboxEntryStatus::Unindexed) | None => {
            vec![
                Span::styled(" Tab", Style::default().fg(Color::Cyan)),
                Span::styled(" Cycle View", Style::default().fg(Color::DarkGray)),
            ]
        }
    };

    let line = Line::from(hints);
    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}
