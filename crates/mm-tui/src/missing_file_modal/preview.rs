//! Missing File Resolution Preview — TUI rendering only.
//!
//! State, input handling, and ModalFrameCore live in mm-ui.
//! This module provides the ratatui ModalFrame impl.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use mm_ui::resolutions::missing_file::MissingFilePreviewState;
use crate::helpers::{render_pane, truncate_right};
use crate::widgets::FocusPane;
use crate::widgets::modal_frame::{ModalFrame, ModalFrameCore};
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};
use crate::widgets::file_path_list::{render_file_path_list, PathEntry};

/// Entry point for rendering this modal.
pub fn render(f: &mut Frame, area: Rect, state: &mut MissingFilePreviewState) {
    state.render_frame(f, area);
}

impl ModalFrame for MissingFilePreviewState {
    fn accent_color(&self) -> Color {
        Color::Cyan
    }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
            Span::styled("Tab", s),
            Span::styled(" switch list  ", s),
            Span::styled("\u{2190}\u{2192}", s),
            Span::styled(" select  ", s),
            Span::styled("Enter", s),
            Span::styled(" confirm", s),
        ]
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        let total = self.cached_data.total_count();
        let restorable = self.cached_data.restorable.len();
        let non_restorable = self.cached_data.non_restorable.len();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing File Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " ({} total: {} restorable, {} lost)",
                    total, restorable, non_restorable
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    fn render_frame_list(&mut self, f: &mut Frame, area: Rect) {
        let count = self.list_len();
        let list_focused =
            self.frame.focus_pane == FocusPane::List && self.focused_list == 0;
        let accent = if list_focused { Color::Green } else { Color::DarkGray };

        let block = Block::default()
            .title(self.list_title())
            .title_style(Style::default().fg(if count > 0 {
                Color::Green
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));

        let inner = render_pane(f, area, block);
        let cursor = self.cursor;
        self.frame.click_targets.populate(inner, cursor, count);

        if count == 0 {
            f.render_widget(
                Paragraph::new(self.empty_message())
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, cursor, cursor);
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.cached_data.restorable[idx];
        let path = truncate_right(&file.corpus_path, width as usize);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let detail_focused =
            self.frame.focus_pane == FocusPane::List && self.focused_list == 1;
        let count = self.cached_data.non_restorable.len();
        let accent = if detail_focused { Color::Red } else { Color::DarkGray };

        let title = format!(" Non-Restorable / Data Lost ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));

        let inner = render_pane(f, area, block);

        self.right_click_targets.populate(
            inner,
            self.right_scroll,
            count,
        );

        if count == 0 {
            f.render_widget(
                Paragraph::new("No non-restorable files")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .non_restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.right_scroll, self.right_scroll);
    }
}
