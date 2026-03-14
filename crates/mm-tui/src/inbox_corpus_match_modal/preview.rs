//! Inbox Corpus Match Resolution Rendering

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use mm_meta::views::MatchClassification;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{ModalFrame, PathField, CURSOR_STYLE};

use super::types::InboxCorpusMatchPreviewState;

impl ModalFrame for InboxCorpusMatchPreviewState {
    fn frame_title(&self) -> Line<'static> {
        let (better, equivalent, subpar) = self.data.0.count_by_class();
        Line::from(vec![
            Span::styled(
                " Inbox Corpus Match Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " {} better, {} equivalent, {} subpar",
                    better, equivalent, subpar
                ),
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
        let entry = &self.data.0.entries[idx];

        let total_width = width as usize;
        let icon_width = 3;
        let quality_width = 22;
        let path_width = total_width.saturating_sub(icon_width + quality_width);

        let style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(Color::White)
        };

        let (icon, icon_color) = match entry.classification {
            MatchClassification::Better => ("B", Color::Cyan),
            MatchClassification::Equivalent => ("=", Color::Green),
            MatchClassification::Subpar => ("v", Color::Yellow),
        };

        let icon_style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(icon_color)
        };

        let quality_style = if is_cursor && is_focused {
            CURSOR_STYLE
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let path_display = truncate_left(&entry.inbox_path, path_width.saturating_sub(1));

        let line = Line::from(vec![
            Span::styled(format!(" {} ", icon), icon_style),
            Span::styled(
                format!("{:<width$}", path_display, width = path_width),
                style,
            ),
            Span::styled(
                format!("{:>width$}", entry.inbox_quality, width = quality_width),
                quality_style,
            ),
        ]);
        ListItem::new(line)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Corpus Match Details ")
            .title_style(Style::default().fg(Color::DarkGray))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = render_pane(f, area, block);

        let current = self.data.0.entries.get(self.cursor);

        let lines = if let Some(entry) = current {
            let mut inbox_lines = PathField::new(
                Span::styled("Inbox: ", Style::default().fg(Color::Magenta)),
                &entry.inbox_path,
            )
            .style(Style::default().fg(Color::White))
            .render_lines(inner.width);
            if let Some(last) = inbox_lines.last_mut() {
                last.spans.push(Span::styled(
                    format!("  [{}]", entry.inbox_quality),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let mut lines = inbox_lines;

            for (i, cm) in entry.corpus_matches.iter().enumerate().take(3) {
                let mut match_lines = PathField::new(
                    Span::styled(format!("  #{}: ", i + 1), Style::default().fg(Color::Green)),
                    &cm.corpus_path,
                )
                .style(Style::default().fg(Color::White))
                .render_lines(inner.width);
                if let Some(last) = match_lines.last_mut() {
                    last.spans.push(Span::styled(
                        format!("  [{}] {:.1}%", cm.corpus_quality, cm.similarity),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.extend(match_lines);
            }

            lines
        } else {
            vec![Line::from(Span::styled(
                "No entry selected",
                Style::default().fg(Color::DarkGray),
            ))]
        };

        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}
