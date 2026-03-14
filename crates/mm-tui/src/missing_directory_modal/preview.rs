//! Missing Directory Resolution Preview — TUI rendering only.
//!
//! Data wrapper, buttons, and actions live in `mm_ui::resolutions::missing_directory`.
//! This module provides the ratatui-specific `ModalFrame` impl.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use crate::helpers::truncate_left;
use crate::widgets::modal_frame::ModalFrame;

// Re-export the mm-ui types that callers need.
pub use mm_ui::resolutions::missing_directory::{
    MissingDirectoryAction, MissingDirectoryButton, MissingDirectoryButtonCtx,
    MissingDirectoryData, MissingDirectoryState,
};

// ============================================================================
// ModalFrame Rendering
// ============================================================================

impl ModalFrame for MissingDirectoryState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let count = self.data.0.count();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing Directory Acknowledgment ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} directories)", count),
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
        let dir = &self.data.0.directories[idx];
        let path = truncate_left(dir, width.saturating_sub(2) as usize);
        let style = if is_cursor {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let desc = Paragraph::new(
            "These directories were deleted externally. \
             Dropping will remove them and their files from the index.",
        )
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, area);
    }
}
