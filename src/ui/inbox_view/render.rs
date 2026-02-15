//! Inbox View Rendering
//!
//! Renders the inbox view as a file list grouped by status:
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
        ])
        .split(area);

    // Render unified title bar
    let titlebar = UnifiedTitleBar::new(LateralView::Inbox);
    titlebar.render(f, main_chunks[0]);

    render_inbox_content(f, main_chunks[1], state);
}

fn render_inbox_content(f: &mut Frame, area: Rect, state: &mut InboxViewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Inbox Files ");
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
            let (status_label, status_color) = match entry.status {
                InboxEntryStatus::Unindexed => ("UNINDEXED", Color::Yellow),
                InboxEntryStatus::Healthy => ("READY", Color::Green),
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
                    format!(" {:>9} ", status_label),
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
