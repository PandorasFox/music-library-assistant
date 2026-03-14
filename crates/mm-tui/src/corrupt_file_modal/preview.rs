//! Corrupt File Resolution Rendering
//!
//! Only the ModalFrame (TUI rendering) trait remains here.
//! State struct, input handling, and ModalFrameCore are provided by
//! the generic ResolutionState.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::types::CorruptFileState;
use crate::helpers::truncate_right;
use crate::widgets::modal_frame::ModalFrame;
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

impl ModalFrame for CorruptFileState {
    fn accent_color(&self) -> Color {
        Color::Red
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let total = self.data.0.total_count();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Corrupt File Resolution ",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", total),
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
        let file = &self.data.0.files[idx];
        let path = truncate_right(&file.corpus_path, width as usize);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let description = Paragraph::new(vec![
            Line::from("These files have corrupt tags or unreadable audio data."),
            Line::from("Stashing will move them to stash/corrupt/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, area);
    }
}
