//! OOB Resolution — TUI rendering only.
//!
//! State, input handling, and ModalFrameCore live in mm-ui.
//! This module provides the ratatui ModalFrame impl.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mm_meta::views::ConflictBucket;
use crate::helpers::render_pane;
use crate::widgets::{
    render_file_path_list, FocusPane, ModalFrame, PathEntry, PathField,
    StyledCell, ThreeColTable,
};

use super::types::OobResolutionState;

pub fn render(f: &mut Frame, area: Rect, state: &mut OobResolutionState) {
    state.render_frame(f, area);
}

impl ModalFrame for OobResolutionState {
    fn accent_color(&self) -> Color { Color::Red }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        vec![
            Span::styled("Shift+\u{2191}\u{2193}", s), Span::styled(" focus", s),
            Span::styled("  \u{00b7}  ", s),
            Span::styled("^A", s), Span::styled(" select all", s),
            Span::styled("  \u{00b7}  ", s),
            Span::styled("Space", s), Span::styled(" toggle", s),
            Span::styled("  \u{00b7}  ", s),
            Span::styled("Enter", s), Span::styled(" confirm", s),
        ]
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        render_info_bar(f, area, self);
    }

    fn render_frame_list(&mut self, f: &mut Frame, area: Rect) {
        render_file_list(f, area, self);
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        render_diff_details(f, area, self);
    }
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &OobResolutionState) {
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

    let inner = render_pane(f, area, block);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
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

fn render_file_list(f: &mut Frame, area: Rect, state: &OobResolutionState) {
    let is_focused = state.frame.focus_pane == FocusPane::List;

    let bucket_state = state.active_bucket_state();

    let title = if !bucket_state.list.selected.is_empty() {
        format!(
            "Files ({}, {} selected)",
            bucket_state.files.len(),
            bucket_state.list.selected.len()
        )
    } else {
        format!("Files ({})", bucket_state.files.len())
    };

    let block = crate::helpers::focused_block(&title, is_focused);
    let inner = render_pane(f, area, block);

    if bucket_state.files.is_empty() {
        let empty = Paragraph::new("No files in this bucket")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    let show_selection = state.active_bucket.is_resolvable() || !bucket_state.list.selected.is_empty();

    let entries: Vec<PathEntry> = bucket_state
        .files
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let mut prefix = Vec::new();
            if show_selection {
                let is_sel = bucket_state.list.selected.contains(&idx);
                let marker_style = if is_sel {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let marker = if is_sel { "[x]" } else { "[ ]" };
                prefix.push(Span::styled(
                    format!("{} ", marker),
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

    render_file_path_list(f, inner, &entries, bucket_state.list.cursor, bucket_state.list.scroll);
}

fn render_diff_details(f: &mut Frame, area: Rect, state: &OobResolutionState) {
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
