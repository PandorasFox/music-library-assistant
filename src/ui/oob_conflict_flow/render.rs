//! Rendering for OOB tag bucketed resolution flow.
//!
//! Uses full-area layout with:
//! - Info bar showing bucket tabs and full path of selected file
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Ctrl+Tab)

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::corpus::db::types::ConflictBucket;
use crate::ui::helpers::truncate_left;
use crate::ui::widgets::{FocusPane, ResolutionLayout};

use super::types::{OobConflictState, ResolutionButton};

pub fn render(f: &mut Frame, area: Rect, state: &mut OobConflictState) {
    // Use full area with 1-cell padding
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Custom layout for conflict flow: tab bar takes more space
    let layout = ResolutionLayout::new(padded, 4, 2, 33);

    // Render each section
    render_info_bar(f, layout.info_bar, state);
    render_file_list(f, layout.list_pane, state);
    render_diff_details(f, layout.details_pane, state);
    render_buttons(f, layout.buttons, state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let title = format!(
        " OOB Tag Resolution — {} file{} ",
        state.total_files(),
        if state.total_files() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner: tab bar + path line
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Tab bar
            Constraint::Length(1), // Path
        ])
        .split(inner);

    // Tab bar
    let mut tab_spans = Vec::new();
    tab_spans.push(Span::raw(" "));

    for bucket in &ConflictBucket::ALL {
        let count = state.bucket_counts[bucket.index()];
        let is_active = *bucket == state.active_bucket;
        let label = format!(" {} ({}) ", bucket.label(), count);

        let style = if is_active {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else if count > 0 {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        tab_spans.push(Span::styled(label, style));
        tab_spans.push(Span::raw(" "));
    }

    let tab_line = Line::from(tab_spans);
    f.render_widget(Paragraph::new(tab_line), chunks[0]);

    // Full path of selected file
    if let Some(file) = state.active_bucket_state().current_file() {
        let path_line = Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&file.path, Style::default().fg(Color::Cyan)),
        ]);
        f.render_widget(Paragraph::new(path_line), chunks[1]);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let is_focused = state.focus_pane == FocusPane::List;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let bucket_state = state.active_bucket_state();

    // Build title with selection count if active
    let title = if state.selection.is_active() {
        format!("Files ({}, {} selected)", bucket_state.files.len(), state.selection.selection_count())
    } else {
        format!("Files ({})", bucket_state.files.len())
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if bucket_state.files.is_empty() {
        let empty = Paragraph::new("No files in this bucket")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    let visible_height = inner.height as usize;
    let max_width = inner.width as usize;

    // Calculate scroll to keep cursor visible
    let scroll = if bucket_state.cursor >= bucket_state.scroll + visible_height {
        bucket_state.cursor - visible_height + 1
    } else if bucket_state.cursor < bucket_state.scroll {
        bucket_state.cursor
    } else {
        bucket_state.scroll
    };

    let selection_active = state.selection.is_active();

    let items: Vec<ListItem> = bucket_state.files
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(idx, file)| {
            let is_cursor = idx == bucket_state.cursor;
            let cursor_prefix = if is_cursor { "> " } else { "  " };
            let selection_marker = if selection_active {
                state.selection.marker(idx)
            } else {
                ""
            };

            // Calculate available path width
            let prefix_len = cursor_prefix.len() + if selection_active { selection_marker.len() + 1 } else { 0 };
            let path_max = max_width.saturating_sub(prefix_len);
            let truncated_path = truncate_left(&file.path, path_max);

            let marker_style = if state.selection.is_selected(idx) {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let path_style = if is_cursor {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![Span::raw(cursor_prefix.to_string())];

            if selection_active {
                spans.push(Span::styled(selection_marker.to_string(), marker_style));
                spans.push(Span::raw(" "));
            }

            spans.push(Span::styled(truncated_path, path_style));

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn render_diff_details(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let block = Block::default()
        .title("Tag Diff (DB vs Disk)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.active_bucket_state().files.is_empty() {
        return;
    }

    let mut lines = Vec::new();

    if state.current_diff.is_empty() {
        lines.push(Line::from(Span::styled(
            "No tag differences found",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let col_width = inner.width as usize;
        let field_w = 18.min(col_width / 3);

        // Column headers
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

        for mismatch in &state.current_diff {
            let db_display = mismatch.db_value.as_deref().unwrap_or("\u{2014}");
            let disk_display = mismatch.disk_value.as_deref().unwrap_or("\u{2014}");

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
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut OobConflictState) {
    let is_focused = state.focus_pane == FocusPane::Buttons;
    let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Clear stored button rects
    state.button_rects.clear();

    let content = match state.active_bucket {
        ConflictBucket::DbOnly | ConflictBucket::DiskOnly => {
            let apply_label = " Apply DB -> Files ";
            let assimilate_label = " Assimilate Files -> DB ";
            let cancel_label = " Cancel ";
            let has_files = !state.active_bucket_state().files.is_empty();

            // Calculate button positions for click detection
            let total_width = apply_label.len() + 3 + assimilate_label.len() + 3 + cancel_label.len();
            let start_x = inner.x + (inner.width.saturating_sub(total_width as u16)) / 2;

            let mut x = start_x;

            // Apply DB button
            let apply_rect = Rect::new(x, inner.y, apply_label.len() as u16, 1);
            state.button_rects.set("apply_db", apply_rect);
            x += apply_label.len() as u16 + 3;

            // Assimilate Disk button
            let assimilate_rect = Rect::new(x, inner.y, assimilate_label.len() as u16, 1);
            state.button_rects.set("assimilate_disk", assimilate_rect);
            x += assimilate_label.len() as u16 + 3;

            // Cancel button
            let cancel_rect = Rect::new(x, inner.y, cancel_label.len() as u16, 1);
            state.button_rects.set("cancel", cancel_rect);

            let apply_style = if state.selected_button == ResolutionButton::ApplyDb && is_focused && has_files {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else if has_files {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let assimilate_style = if state.selected_button == ResolutionButton::AssimilateDisk && is_focused && has_files {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else if has_files {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let cancel_style = Style::default().fg(Color::White);

            Line::from(vec![
                Span::raw("  "),
                Span::styled(apply_label, apply_style),
                Span::raw("   "),
                Span::styled(assimilate_label, assimilate_style),
                Span::raw("   "),
                Span::styled(cancel_label, cancel_style),
            ])
        }
        ConflictBucket::NoChanges => {
            Line::from(Span::styled(
                "No resolution needed — tags match (or awaiting classification)",
                Style::default().fg(Color::DarkGray),
            ))
        }
        ConflictBucket::Conflict => {
            Line::from(Span::styled(
                "Manual resolution required — tag editor integration pending",
                Style::default().fg(Color::DarkGray),
            ))
        }
    };

    let para = Paragraph::new(content).alignment(Alignment::Center);
    f.render_widget(para, inner);
}
