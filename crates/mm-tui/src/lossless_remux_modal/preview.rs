//! Lossless Remux Resolution Preview — TUI rendering only.
//!
//! State, input handling, and ModalFrameCore live in mm-ui.
//! This module provides the ratatui ModalFrame impl.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use mm_meta::views::cluster_deploy::LosslessRemuxModalData;
use mm_ui::resolutions::lossless_remux::LosslessRemuxPreviewState;
use crate::helpers::{render_pane, truncate_right};
use crate::widgets::modal_frame::ModalFrame;
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

/// Entry point for rendering this modal.
pub fn render(f: &mut Frame, area: Rect, state: &mut LosslessRemuxPreviewState) {
    state.render_frame(f, area);
}

fn render_remux_section(data: &LosslessRemuxModalData, f: &mut Frame, area: Rect) {
    let has_files = data.has_files();
    let color = if has_files { Color::Green } else { Color::DarkGray };

    let block = Block::default()
        .title(" Lossless \u{2192} FLAC ")
        .title_style(Style::default().fg(color))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));

    let inner = render_pane(f, area, block);

    if !has_files {
        f.render_widget(
            Paragraph::new("No remux candidate files").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(2)])
        .split(inner);

    f.render_widget(
        Paragraph::new("Remux to FLAC (lossless)").style(Style::default().fg(Color::DarkGray)),
        chunks[0],
    );

    let breakdown = data.format_breakdown();
    let items: Vec<ListItem> = breakdown
        .iter()
        .map(|(ftype, count)| {
            ListItem::new(format!("  {}: {}", ftype, count))
                .style(Style::default().fg(Color::White))
        })
        .collect();
    f.render_widget(List::new(items), chunks[1]);
}

impl ModalFrame for LosslessRemuxPreviewState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
            Span::styled("\u{2190}\u{2192}", s),
            Span::styled(" select  ", s),
            Span::styled("Tab", s),
            Span::styled(" switch  ", s),
            Span::styled("Enter", s),
            Span::styled(" confirm", s),
        ]
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Lossless Remux ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", count),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.cached_data.files[idx];

        let type_tag = format!("[{}] ", file.file_type);
        let prefix_len = type_tag.chars().count();
        let path_budget = (width as usize).saturating_sub(prefix_len);
        let path = truncate_right(&file.corpus_path, path_budget);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };

        ListItem::new(Line::from(vec![
            Span::styled(
                type_tag,
                if is_cursor {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
            Span::styled(path, style),
        ]))
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        render_remux_section(&self.cached_data, f, area);
    }
}
