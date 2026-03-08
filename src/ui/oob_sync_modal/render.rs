//! Rendering for OOB tag sync resolution modal.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path of selected file
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::meta::views::OobSyncDirection;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{
    render_file_path_list, FocusPane, PathEntry, PathField, ResolutionLayout, StyledCell,
    ThreeColTable,
};

use super::types::{OobSyncButton, OobSyncState};

pub fn render(f: &mut Frame, area: Rect, state: &mut OobSyncState) {
    // Use full area with 1-cell padding
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Get layout areas (3-line buttons area: border + buttons + hints)
    let layout = ResolutionLayout::new(padded, 3, 3, 33);

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
        let path_lines = PathField::new(
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            &file.path,
        )
        .style(Style::default().fg(Color::Cyan))
        .render_lines(inner.width);
        let para = Paragraph::new(path_lines);
        f.render_widget(para, inner);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &OobSyncState) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

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

    let selection_active = state.selection.is_active();

    let entries: Vec<PathEntry> = state
        .files
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let dir_indicator = match file.direction {
                OobSyncDirection::DiskToIndex => " →I",
                OobSyncDirection::IndexToDisk => " →D",
            };
            let dir_color = match file.direction {
                OobSyncDirection::DiskToIndex => Color::Cyan,
                OobSyncDirection::IndexToDisk => Color::Magenta,
            };

            let mut prefix = Vec::new();
            if selection_active {
                let marker_style = if state.selection.is_selected(idx) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                prefix.push(Span::styled(
                    format!("{} ", state.selection.marker(idx)),
                    marker_style,
                ));
            }

            PathEntry {
                path: &file.path,
                prefix,
                suffix: vec![Span::styled(dir_indicator, Style::default().fg(dir_color))],
            }
        })
        .collect();

    render_file_path_list(f, inner, &entries, state.current_file, state.scroll);
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

    // Direction label takes 2 lines (label + blank separator)
    let dir_label = match file.direction {
        OobSyncDirection::DiskToIndex => "Disk \u{2192} Index",
        OobSyncDirection::IndexToDisk => "Index \u{2192} Disk",
    };
    let dir_color = match file.direction {
        OobSyncDirection::DiskToIndex => Color::Cyan,
        OobSyncDirection::IndexToDisk => Color::Magenta,
    };

    let dir_line = Line::from(vec![
        Span::raw("Direction: "),
        Span::styled(
            dir_label,
            Style::default().fg(dir_color).add_modifier(Modifier::BOLD),
        ),
    ]);

    // Render direction line at top
    if inner.height < 3 {
        f.render_widget(Paragraph::new(dir_line), inner);
        return;
    }
    let dir_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    f.render_widget(Paragraph::new(dir_line), dir_area);

    // Table area below direction + blank line
    let table_area = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };

    let bold = Modifier::BOLD;
    let table = ThreeColTable {
        headers: [
            (
                "Field".into(),
                Style::default().fg(Color::DarkGray).add_modifier(bold),
            ),
            (
                "DB Value".into(),
                Style::default().fg(Color::Green).add_modifier(bold),
            ),
            (
                "Disk Value".into(),
                Style::default().fg(Color::Cyan).add_modifier(bold),
            ),
        ],
        rows: file
            .mismatches
            .iter()
            .map(|m| {
                let db_text = m.db_value.as_deref().unwrap_or("\u{2014}");
                let disk_text = m.disk_value.as_deref().unwrap_or("\u{2014}");
                let db_color = if m.db_value.is_some() {
                    Color::Green
                } else {
                    Color::DarkGray
                };
                let disk_color = if m.disk_value.is_some() {
                    Color::Cyan
                } else {
                    Color::DarkGray
                };
                [
                    StyledCell::new(&m.field, Style::default().fg(Color::White)),
                    StyledCell::new(db_text, Style::default().fg(db_color)),
                    StyledCell::new(disk_text, Style::default().fg(disk_color)),
                ]
            })
            .collect(),
        col_ratio: [20, 40, 40],
        scroll: 0,
        separator_style: Style::default().fg(Color::DarkGray),
        alternate_rows: true,
    };
    table.render(f, table_area);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut OobSyncState) {
    let is_focused = state.focus_pane == FocusPane::Buttons;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

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

    let buttons_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(disk_label, disk_style),
        Span::raw("  "),
        Span::styled(db_label, db_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
        Span::raw("  "),
    ]);

    let hint_style = Style::default().fg(Color::DarkGray);
    let hint_line = Line::from(vec![
        Span::styled("Shift+\u{2191}\u{2193}", hint_style),
        Span::styled(" focus", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("^A", hint_style),
        Span::styled(" select all", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("Space", hint_style),
        Span::styled(" toggle", hint_style),
        Span::styled("  \u{00b7}  ", hint_style),
        Span::styled("Enter", hint_style),
        Span::styled(" confirm", hint_style),
    ]);

    let para = Paragraph::new(vec![buttons_line, hint_line]).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
