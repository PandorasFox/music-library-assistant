//! Rendering for moved file acknowledgement modal.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::types::{MovedFileButton, MovedFileState};
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{FocusPane, PathField, CURSOR_STYLE, LIST_ITEM_STYLE};

/// Render the moved file acknowledgement modal.
pub fn render(state: &MovedFileState, f: &mut Frame, area: Rect) {
    // Main layout: header, list, details, buttons
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(5),    // File list
            Constraint::Length(6), // Details for selected file (extra line for zone)
            Constraint::Length(3), // Buttons
        ])
        .split(area);

    render_header(state, f, chunks[0]);
    render_file_list(state, f, chunks[1]);
    render_details(state, f, chunks[2]);
    render_buttons(state, f, chunks[3]);
}

fn render_header(state: &MovedFileState, f: &mut Frame, area: Rect) {
    let count = state.files.len();
    let text = format!(
        "Moved Files: {} file{} detected with path changes",
        count,
        if count == 1 { "" } else { "s" }
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Acknowledge Moved Files")
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_file_list(state: &MovedFileState, f: &mut Frame, area: Rect) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Files")
        .border_style(border_style);

    let inner = render_pane(f, area, block);

    if state.files.is_empty() {
        let empty = Paragraph::new("No moved files to acknowledge");
        f.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = state
        .files
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let is_selected = idx == state.current_file;
            let indicator = if is_selected { "▶ " } else { "  " };
            let style = if is_selected { CURSOR_STYLE } else { LIST_ITEM_STYLE };

            // Show the new path (current location)
            ListItem::new(Line::styled(format!("{}{}", indicator, file.new_path), style))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn render_details(state: &MovedFileState, f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Details")
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = render_pane(f, area, block);

    if state.files.is_empty() {
        return;
    }

    let file = &state.files[state.current_file];

    let mut lines = Vec::new();
    lines.extend(
        PathField::new(
            Span::styled("Old path: ", Style::default().fg(Color::DarkGray)),
            &file.old_path,
        )
        .style(Style::default().fg(Color::Red))
        .render_lines(inner.width),
    );
    lines.extend(
        PathField::new(
            Span::styled("New path: ", Style::default().fg(Color::DarkGray)),
            &file.new_path,
        )
        .style(Style::default().fg(Color::Green))
        .render_lines(inner.width),
    );
    lines.push(Line::from(vec![
        Span::styled("Inode: ", Style::default().fg(Color::DarkGray)),
        Span::raw(file.inode.to_string()),
    ]));

    // Show zone transition for cross-zone moves
    if file.old_zone != file.new_zone {
        lines.push(Line::from(vec![
            Span::styled("Zone:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(&file.old_zone, Style::default().fg(Color::Red)),
            Span::styled(" → ", Style::default().fg(Color::DarkGray)),
            Span::styled(&file.new_zone, Style::default().fg(Color::Green)),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_buttons(state: &MovedFileState, f: &mut Frame, area: Rect) {
    let is_focused = state.focus_pane == FocusPane::Buttons;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let inner = render_pane(f, area, block);

    // Button layout
    let button_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    // Acknowledge button
    let ack_selected = state.selected_button == MovedFileButton::Acknowledge && is_focused;
    let ack_style = if ack_selected {
        Style::default()
            .bg(Color::Green)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    let ack_text = if state.files.is_empty() {
        "[ Acknowledge (disabled) ]"
    } else {
        "[ Acknowledge ]"
    };
    let ack = Paragraph::new(ack_text)
        .style(ack_style)
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(ack, button_chunks[0]);

    // Cancel button
    let cancel_selected = state.selected_button == MovedFileButton::Cancel && is_focused;
    let cancel_style = if cancel_selected {
        Style::default()
            .bg(Color::Red)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    };
    let cancel = Paragraph::new("[ Cancel ]")
        .style(cancel_style)
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(cancel, button_chunks[1]);
}
