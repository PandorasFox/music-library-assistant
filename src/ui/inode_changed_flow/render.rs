//! Rendering for inode changed acknowledgement flow.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path of selected file
//! - File list showing path + old inode → new inode
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::truncate_left;
use crate::ui::widgets::FocusPane;

use super::types::{InodeChangedButton, InodeChangedState};

pub fn render(f: &mut Frame, area: Rect, state: &mut InodeChangedState) {
    // Use full area with 1-cell padding
    let padded = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    f.render_widget(Clear, padded);

    // Layout: info bar + file list + buttons
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Info bar
            Constraint::Min(5),     // File list
            Constraint::Length(2),  // Buttons
        ])
        .split(padded);

    render_info_bar(f, chunks[0], state);
    render_file_list(f, chunks[1], state);
    render_buttons(f, chunks[2], state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &InodeChangedState) {
    let title = format!(
        " Inode Changed — {} file{} ",
        state.files.len(),
        if state.files.len() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Show full untruncated path of selected file
    if let Some(file) = state.files.get(state.current_file) {
        let path_line = Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&file.path, Style::default().fg(Color::Cyan)),
        ]);
        let para = Paragraph::new(path_line);
        f.render_widget(para, inner);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &InodeChangedState) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .title("Files (inode changed)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible_height = inner.height as usize;
    let max_width = inner.width as usize;

    // Calculate scroll to keep current file visible
    let scroll = if state.current_file >= state.scroll + visible_height {
        state.current_file - visible_height + 1
    } else if state.current_file < state.scroll {
        state.current_file
    } else {
        state.scroll
    };

    let items: Vec<ListItem> = state.files
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(idx, file)| {
            let is_cursor = idx == state.current_file;

            let cursor_prefix = if is_cursor { "> " } else { "  " };

            // Format inode change indicator
            let inode_indicator = format!("{} → {}", file.old_inode, file.new_inode);
            let suffix_len = inode_indicator.len() + 2;  // " [inode]"
            let path_max = max_width.saturating_sub(cursor_prefix.len() + suffix_len);
            let truncated_path = truncate_left(&file.path, path_max);

            let spans = vec![
                Span::raw(cursor_prefix.to_string()),
                Span::styled(
                    truncated_path,
                    if is_cursor {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::raw(" "),
                Span::styled(
                    inode_indicator,
                    Style::default().fg(Color::DarkGray),
                ),
            ];

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);

    // Show hint at bottom if there are files
    if !state.files.is_empty() && inner.height > 2 {
        let hint = Line::from(vec![
            Span::styled(
                "  Files were replaced (same path, new inode). Acknowledge to update the index.",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let hint_para = Paragraph::new(hint);
        f.render_widget(hint_para, hint_area);
    }
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut InodeChangedState) {
    let is_focused = state.focus_pane == FocusPane::Buttons;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let ack_label = format!(" Acknowledge ({}) ", state.files.len());
    let cancel_label = " Cancel ";

    // Style buttons
    let ack_style = if state.selected_button == InodeChangedButton::Acknowledge && is_focused {
        if !state.files.is_empty() {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray).bg(Color::Black)
        }
    } else if !state.files.is_empty() {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let cancel_style = if state.selected_button == InodeChangedButton::Cancel && is_focused {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default().fg(Color::White)
    };

    let buttons = Line::from(vec![
        Span::raw("  "),
        Span::styled(ack_label, ack_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
        Span::raw("  [Y] quick acknowledge  [Esc] cancel"),
    ]);

    let para = Paragraph::new(buttons).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
