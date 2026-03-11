//! Rendering for OOB tag bucketed resolution modal.
//!
//! Uses full-area layout with:
//! - Info bar showing bucket tabs and full path of selected file
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::meta::views::ConflictBucket;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{
    render_file_path_list, FocusPane, PathEntry, PathField, ResolutionLayout, StyledCell,
    ThreeColTable,
};

use super::types::{OobConflictState, ResolutionButton};

pub fn render(f: &mut Frame, area: Rect, state: &mut OobConflictState) {
    // Use full area with 1-cell padding
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Custom layout for conflict modal: tab bar takes more space, 3-line buttons for hints
    let layout = ResolutionLayout::new(padded, 4, 3, 33);

    // Render each section
    render_info_bar(f, layout.info_bar, state);
    render_file_list(f, layout.list_pane, state);
    render_diff_details(f, layout.details_pane, state);
    render_buttons(f, layout.buttons, state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let bucket_state = state.active_bucket_state();

    // Title with filter indicator
    let filter_indicator = if bucket_state.filter_text.is_some() {
        let filtered_count = bucket_state.get_filtered_indices().len();
        let bucket_count = bucket_state.files.len();
        format!(" [filtered: {}/{}]", filtered_count, bucket_count)
    } else {
        String::new()
    };

    let title = format!(
        " OOB Tag Resolution — {} file{}{} ",
        state.total_files(),
        if state.total_files() == 1 { "" } else { "s" },
        filter_indicator,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = render_pane(f, area, block);

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
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
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
        let path_lines = PathField::new(
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            &file.path,
        )
        .style(Style::default().fg(Color::Cyan))
        .render_lines(chunks[1].width);
        f.render_widget(Paragraph::new(path_lines), chunks[1]);
    }
}

fn render_file_list(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let is_focused = state.focus_pane == FocusPane::List;

    let bucket_state = state.active_bucket_state();

    // Build title with selection count if active
    let title = if bucket_state.selection.is_active() {
        format!(
            "Files ({}, {} selected)",
            bucket_state.files.len(),
            bucket_state.selection.selection_count()
        )
    } else {
        format!("Files ({})", bucket_state.files.len())
    };

    let block = crate::ui::helpers::focused_block(&title, is_focused);
    let inner = render_pane(f, area, block);

    if bucket_state.files.is_empty() {
        let empty = Paragraph::new("No files in this bucket")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    // Show selection indicators by default for resolvable buckets
    let show_selection = state.active_bucket.is_resolvable() || bucket_state.selection.is_active();

    let entries: Vec<PathEntry> = bucket_state
        .files
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let mut prefix = Vec::new();
            if show_selection {
                let marker_style = if bucket_state.selection.is_selected(idx) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                prefix.push(Span::styled(
                    format!("{} ", bucket_state.selection.marker(idx)),
                    marker_style,
                ));
            }

            PathEntry {
                path: &file.path,
                prefix,
                suffix: Vec::new(),
            }
        })
        .collect();

    render_file_path_list(f, inner, &entries, bucket_state.cursor, bucket_state.scroll);
}

fn render_diff_details(f: &mut Frame, area: Rect, state: &OobConflictState) {
    let block = Block::default()
        .title("Tag Diff (DB vs Disk)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = render_pane(f, area, block);

    if state.active_bucket_state().files.is_empty() {
        return;
    }

    let mismatches = state.current_mismatches();
    if mismatches.is_empty() {
        let empty =
            Paragraph::new("No tag differences found").style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    }

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
        rows: mismatches
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
    table.render(f, inner);
}

fn render_buttons(f: &mut Frame, area: Rect, state: &mut OobConflictState) {
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

    let buttons_line = match state.active_bucket {
        ConflictBucket::DbOnly | ConflictBucket::DiskOnly | ConflictBucket::Conflict => {
            let apply_label = " Apply DB -> Files ";
            let assimilate_label = " Assimilate Files -> DB ";
            let cancel_label = " Cancel ";
            let has_files = !state.active_bucket_state().files.is_empty();

            // Calculate button positions for click detection
            let total_width =
                apply_label.len() + 3 + assimilate_label.len() + 3 + cancel_label.len();
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

            let apply_style =
                if state.selected_button == ResolutionButton::ApplyDb && is_focused && has_files {
                    Style::default().fg(Color::Black).bg(Color::Green)
                } else if has_files {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

            let assimilate_style = if state.selected_button == ResolutionButton::AssimilateDisk
                && is_focused
                && has_files
            {
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
        ConflictBucket::MtimeOnly => {
            let ack_label = " Acknowledge Mtime ";
            let cancel_label = " Cancel ";
            let has_files = !state.active_bucket_state().files.is_empty();

            // Calculate button positions for click detection
            let total_width = ack_label.len() + 3 + cancel_label.len();
            let start_x = inner.x + (inner.width.saturating_sub(total_width as u16)) / 2;

            let mut x = start_x;

            // Acknowledge button
            let ack_rect = Rect::new(x, inner.y, ack_label.len() as u16, 1);
            state.button_rects.set("acknowledge", ack_rect);
            x += ack_label.len() as u16 + 3;

            // Cancel button
            let cancel_rect = Rect::new(x, inner.y, cancel_label.len() as u16, 1);
            state.button_rects.set("cancel", cancel_rect);

            let ack_style = if is_focused && has_files {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else if has_files {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let cancel_style = Style::default().fg(Color::White);

            Line::from(vec![
                Span::raw("  "),
                Span::styled(ack_label, ack_style),
                Span::raw("   "),
                Span::styled(cancel_label, cancel_style),
            ])
        }
    };

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
