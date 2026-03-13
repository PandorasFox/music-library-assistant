//! Rendering for OOB tag sync resolution modal.
//!
//! Uses full-area layout with:
//! - Info bar showing full untruncated path of selected file
//! - 33% list pane / 67% details pane
//! - Decision buttons bar (focusable via Shift+Up/Down)

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use mm_meta::views::OobSyncDirection;
use crate::helpers::render_pane;
use crate::widgets::{
    render_file_path_list, FocusPane, ModalFrame, PathEntry, PathField,
    StyledCell, ThreeColTable,
};
use crate::widgets::modal_frame::{ContentLayout, FrameState};

use super::types::{OobSyncButton, OobSyncButtonCtx, OobSyncAction, OobSyncState};

pub fn render(f: &mut Frame, area: Rect, state: &mut OobSyncState) {
    state.render_frame(f, area);
}

impl ModalFrame for OobSyncState {
    type Button = OobSyncButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 33, info_height: 3 }
    }

    fn list_title(&self) -> String { "Files".into() }
    fn accent_color(&self) -> Color { Color::Yellow }

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

    fn frame_state(&self) -> &FrameState<OobSyncButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<OobSyncButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.current_file }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.current_file }
    fn list_len(&self) -> usize { self.files.len() }
    fn button_ctx(&self) -> OobSyncButtonCtx { OobSyncState::button_ctx(self) }
    fn escape_action(&self) -> OobSyncAction { OobSyncAction::Cancel }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        render_info_bar(f, area, self);
    }

    fn render_frame_list(&mut self, f: &mut Frame, area: Rect) {
        render_file_list(f, area, self);
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        render_mismatch_details(f, area, self);
    }
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &OobSyncState) {
    let disk_count = state.disk_to_index_count();
    let db_count = state.index_to_disk_count();

    // Title with counts and filter indicator
    let filter_indicator = if state.filter_text.is_some() {
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
    let is_focused = state.frame.focus_pane == FocusPane::List;

    // Build title with selection count if active
    let title = if state.selection.is_active() {
        format!("Files ({} selected)", state.selection.selection_count())
    } else {
        "Files".to_string()
    };

    let block = crate::helpers::focused_block(&title, is_focused);
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
