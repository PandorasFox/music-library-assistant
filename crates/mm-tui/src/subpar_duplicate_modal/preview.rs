//! Subpar Duplicate Resolution Rendering

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::types::SubparDuplicateState;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{ModalFrame, PathField, CURSOR_STYLE};

/// Build the detail lines for the currently selected pair.
fn build_detail_lines(state: &SubparDuplicateState, width: u16) -> Vec<Line<'static>> {
    match state.data.0.files.get(state.list.cursor) {
            Some(file) => {
                let mut lines = PathField::new(
                    Span::styled("Subpar: ", Style::default().fg(Color::Red)),
                    &file.corpus_path,
                )
                .style(Style::default().fg(Color::White))
                .render_lines(width);

                let mut better_lines = PathField::new(
                    Span::styled("Better: ", Style::default().fg(Color::Green)),
                    &file.superior_path,
                )
                .style(Style::default().fg(Color::White))
                .render_lines(width);
                if let Some(last) = better_lines.last_mut() {
                    last.spans.push(Span::styled(
                        format!("  [{:.1}% match]", file.similarity_score),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.extend(better_lines);
                lines
            }
            None => vec![Line::from(Span::styled(
                "No file selected",
                Style::default().fg(Color::DarkGray),
            ))],
        }
}

impl ModalFrame for SubparDuplicateState {
    fn frame_title(&self) -> Line<'static> {
        let total = self.data.0.total_count();
        Line::from(vec![
            Span::styled(
                " Subpar Duplicate Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", total),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.data.0.files[idx];

        let total_width = width as usize;
        let left_width = (total_width * 40) / 100;
        let mid_width = (total_width * 10) / 100;
        let score_width = (total_width * 8) / 100;
        let right_width = total_width.saturating_sub(left_width + mid_width + score_width);

        let style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(Color::White)
        };
        let reason_style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(Color::Yellow)
        };
        let score_style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let subpar_path = truncate_left(&file.corpus_path, left_width.saturating_sub(1));
        let superior_path = truncate_left(&file.superior_path, right_width.saturating_sub(1));
        let score_str = format!("{:.1}%", file.similarity_score);

        let line = Line::from(vec![
            Span::styled(
                format!("{:<width$}", subpar_path, width = left_width),
                style,
            ),
            Span::styled(
                format!("{:^width$}", file.reason, width = mid_width),
                reason_style,
            ),
            Span::styled(
                format!("{:>width$}", score_str, width = score_width),
                score_style,
            ),
            Span::styled(
                format!(
                    " {:<width$}",
                    superior_path,
                    width = right_width.saturating_sub(1)
                ),
                style,
            ),
        ]);
        ListItem::new(line)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let detail_inner_width = area.width.saturating_sub(2);
        let detail_lines = build_detail_lines(self,detail_inner_width);

        let detail_block = Block::default()
            .title(" Selected Pair ")
            .title_style(Style::default().fg(Color::DarkGray))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let detail_inner = render_pane(f, area, detail_block);
        let para = Paragraph::new(detail_lines);
        f.render_widget(para, detail_inner);
    }
}
