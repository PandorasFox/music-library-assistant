//! Rendering for moved file acknowledgement modal.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::types::MovedFileState;
use crate::helpers::render_pane;
use crate::widgets::{ModalFrame, PathField, CURSOR_STYLE, LIST_ITEM_STYLE};
use crate::widgets::modal_frame::{ContentLayout, FrameState};

/// Render the moved file acknowledgement modal.
pub fn render(state: &mut MovedFileState, f: &mut Frame, area: Rect) {
    state.render_frame(f, area);
}

impl ModalFrame for MovedFileState {
    type Button = super::types::MovedFileButton;

    fn frame_title(&self) -> Line<'static> {
        // Not used — FourSection uses render_header instead
        Line::default()
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 6,
        }
    }

    fn list_title(&self) -> String {
        "Files".to_string()
    }

    fn empty_message(&self) -> &'static str {
        "No moved files to acknowledge"
    }

    fn accent_color(&self) -> Color {
        Color::Cyan
    }

    fn frame_state(&self) -> &FrameState<super::types::MovedFileButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<super::types::MovedFileButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.current_file }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.current_file }
    fn list_len(&self) -> usize { self.files.len() }
    fn button_ctx(&self) -> super::types::MovedFileButtonCtx { MovedFileState::button_ctx(self) }
    fn escape_action(&self) -> super::types::MovedFileAction { super::types::MovedFileAction::Cancel }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let count = self.files.len();
        let text = format!(
            "Moved Files: {} file{} detected with path changes",
            count,
            if count == 1 { "" } else { "s" }
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Acknowledge Moved Files")
            .border_style(Style::default().fg(Color::Yellow));

        let paragraph = Paragraph::new(text).block(block);
        f.render_widget(paragraph, area);
    }

    fn render_list_item(
        &self,
        idx: usize,
        _width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.files[idx];
        let indicator = if is_cursor { "\u{25b6} " } else { "  " };
        let style = if is_cursor {
            CURSOR_STYLE
        } else {
            LIST_ITEM_STYLE
        };

        ListItem::new(Line::styled(
            format!("{}{}", indicator, file.new_path),
            style,
        ))
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Details")
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = render_pane(f, area, block);

        if self.files.is_empty() {
            return;
        }

        let file = &self.files[self.current_file];

        let mut lines = Vec::new();
        lines.extend(
            PathField::new(
                Span::styled("Old path: ", Style::default().fg(Color::DarkGray)),
                &file.old_path,
            )
            .style(Style::default().fg(Color::Red))
            .render_lines(inner.width),
        );
        lines.extend(
            PathField::new(
                Span::styled("New path: ", Style::default().fg(Color::DarkGray)),
                &file.new_path,
            )
            .style(Style::default().fg(Color::Green))
            .render_lines(inner.width),
        );
        lines.push(Line::from(vec![
            Span::styled("Inode: ", Style::default().fg(Color::DarkGray)),
            Span::raw(file.inode.to_string()),
        ]));

        if file.old_zone != file.new_zone {
            lines.push(Line::from(vec![
                Span::styled("Zone:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(&file.old_zone, Style::default().fg(Color::Red)),
                Span::styled(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                Span::styled(&file.new_zone, Style::default().fg(Color::Green)),
            ]));
        }

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);
    }
}
