//! Rendering for OOB tag sync resolution flow.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path of selected file
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::corpus::db::types::OobSyncDirection;
use crate::ui::helpers::{render_pane, truncate_left};
use crate::ui::widgets::{FocusPane, ResolutionLayout};

use super::types::{OobSyncButton, OobSyncState};

pub fn render(f: &mut Frame, area: Rect, state: &mut OobSyncState) {
    // Use full area with 1-cell padding
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Get layout areas
    let layout = ResolutionLayout::default_split(padded);

    // Render each section
    render_info_bar(f, layout.info_bar, state);
    render_file_list(f, layout.list_pane, state);
    render_mismatch_details(f, layout.details_pane, state);
    render_buttons(f, layout.buttons, state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &OobSyncState) {
    let disk_count = state.disk_to_index_count();
    let db_count = state.index_to_disk_count();

    // Title with counts and filter indicator
    let filter_indicator = if state.filter.is_some() {
        let filtered_count = state.get_filtered_indices().len();
        format!(" [filtered: {}/{}]", filtered_count, state.files.len())
    } else {
        String::new()
    };

    let title = format!(
        " OOB Tag Sync — {} file{} ({} disk→index, {} index→disk){} ",
        state.files.len(),
        if state.files.len() == 1 { "" } else { "s" },
        disk_count,
        db_count,
        filter_indicator,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = render_pane(f, area, block);

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

fn render_file_list(f: &mut Frame, area: Rect, state: &OobSyncState) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    // Build title with selection count if active
    let title = if state.selection.is_active() {
        format!("Files ({} selected)", state.selection.selection_count())
    } else {
        "Files".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = render_pane(f, area, block);

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

    let selection_active = state.selection.is_active();

    let items: Vec<ListItem> = state.files
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(idx, file)| {
            let is_cursor = idx == state.current_file;
            let dir_indicator = match file.direction {
                OobSyncDirection::DiskToIndex => "→I",
                OobSyncDirection::IndexToDisk => "→D",
            };
            let dir_color = match file.direction {
                OobSyncDirection::DiskToIndex => Color::Cyan,
                OobSyncDirection::IndexToDisk => Color::Magenta,
            };

            // Build prefix: cursor indicator + optional selection marker
            let cursor_prefix = if is_cursor { "> " } else { "  " };
            let selection_marker = if selection_active {
                state.selection.marker(idx)
            } else {
                ""
            };
            let prefix = format!("{}{} ", cursor_prefix, selection_marker);

            let suffix_len = dir_indicator.len() + 1; // " →I"
            let path_max = max_width.saturating_sub(prefix.len() + suffix_len);
            let truncated_path = truncate_left(&file.path, path_max);

            let marker_style = if state.selection.is_selected(idx) {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let mut spans = vec![
                Span::raw(cursor_prefix.to_string()),
            ];

            if selection_active {
                spans.push(Span::styled(selection_marker.to_string(), marker_style));
                spans.push(Span::raw(" "));
            }

            spans.push(Span::styled(
                truncated_path,
                if is_cursor {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(dir_indicator, Style::default().fg(dir_color)));

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn render_mismatch_details(f: &mut Frame, area: Rect, state: &OobSyncState) {
    let block = Block::default()
        .title("Tag Mismatches")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = render_pane(f, area, block);

    let Some(file) = state.files.get(state.current_file) else {
        return;
    };

    let mut lines = Vec::new();

    // File info
    let dir_label = match file.direction {
        OobSyncDirection::DiskToIndex => "Disk → Index",
        OobSyncDirection::IndexToDisk => "Index → Disk",
    };
    let dir_color = match file.direction {
        OobSyncDirection::DiskToIndex => Color::Cyan,
        OobSyncDirection::IndexToDisk => Color::Magenta,
    };

    lines.push(Line::from(vec![
        Span::raw("Direction: "),
        Span::styled(dir_label, Style::default().fg(dir_color).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));

    // Column headers
    let col_width = inner.width as usize;
    let field_w = 18.min(col_width / 3);

    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<width$}", "Field", width = field_w),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "DB Value",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Disk Value",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Mismatch rows
    for mismatch in &file.mismatches {
        let db_display = mismatch.db_value.as_deref().unwrap_or("—");
        let disk_display = mismatch.disk_value.as_deref().unwrap_or("—");

        let db_color = if mismatch.db_value.is_some() { Color::Green } else { Color::DarkGray };
        let disk_color = if mismatch.disk_value.is_some() { Color::Cyan } else { Color::DarkGray };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<width$}", mismatch.field, width = field_w),
                Style::default().fg(Color::White),
            ),
            Span::styled(db_display.to_string(), Style::default().fg(db_color)),
            Span::raw("  "),
            Span::styled(disk_display.to_string(), Style::default().fg(disk_color)),
        ]));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut OobSyncState) {
    let is_focused = state.focus_pane == FocusPane::Buttons;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = render_pane(f, area, block);

    // Clear stored button rects
    state.button_rects.clear();

    let disk_count = state.disk_to_index_count();
    let db_count = state.index_to_disk_count();

    let disk_label = format!(" Accept Disk ({}) ", disk_count);
    let db_label = format!(" Accept DB ({}) ", db_count);
    let cancel_label = " Cancel ";

    // Calculate button positions for click detection
    // Buttons are centered, so we need to compute their positions
    let total_width = disk_label.len() + 2 + db_label.len() + 2 + cancel_label.len();
    let start_x = inner.x + (inner.width.saturating_sub(total_width as u16)) / 2;

    let mut x = start_x;

    // Accept Disk button
    let disk_rect = Rect::new(x, inner.y, disk_label.len() as u16, 1);
    state.button_rects.set("accept_disk", disk_rect);
    x += disk_label.len() as u16 + 2;

    // Accept DB button
    let db_rect = Rect::new(x, inner.y, db_label.len() as u16, 1);
    state.button_rects.set("accept_db", db_rect);
    x += db_label.len() as u16 + 2;

    // Cancel button
    let cancel_rect = Rect::new(x, inner.y, cancel_label.len() as u16, 1);
    state.button_rects.set("cancel", cancel_rect);

    // Style buttons
    let disk_style = if state.selected_button == OobSyncButton::AcceptDisk && is_focused {
        if disk_count > 0 {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray).bg(Color::Black)
        }
    } else if disk_count > 0 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let db_style = if state.selected_button == OobSyncButton::AcceptDb && is_focused {
        if db_count > 0 {
            Style::default().fg(Color::Black).bg(Color::Magenta)
        } else {
            Style::default().fg(Color::DarkGray).bg(Color::Black)
        }
    } else if db_count > 0 {
        Style::default().fg(Color::Magenta)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let cancel_style = if state.selected_button == OobSyncButton::Cancel && is_focused {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default().fg(Color::White)
    };

    let buttons = Line::from(vec![
        Span::raw("  "),
        Span::styled(disk_label, disk_style),
        Span::raw("  "),
        Span::styled(db_label, db_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
        Span::raw("  "),
    ]);

    let para = Paragraph::new(buttons).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
