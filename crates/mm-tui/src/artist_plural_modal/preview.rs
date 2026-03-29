//! Artist Plural Normalization Rendering
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

use super::types::ArtistPluralState;
use crate::helpers::truncate_right;
use crate::widgets::modal_frame::ModalFrame;
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

impl ModalFrame for ArtistPluralState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let total = self.data.0.files.len();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Artist Plural Normalization ",
                Style::default()
                    .fg(Color::Yellow)
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
        let suffix = match (file.data.needs_artist, file.data.needs_album_artist) {
            (true, true) => " [ARTIST+ALBUMARTIST]",
            (true, false) => " [ARTIST]",
            (false, true) => " [ALBUMARTIST]",
            (false, false) => "",
        };
        let max_path = width as usize - suffix.len().min(width as usize);
        let path = truncate_right(&file.corpus_path, max_path);
        let display = format!("{}{}", path, suffix);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };
        ListItem::new(display).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let lines = if let Some(file) = self.data.0.files.get(self.list.cursor) {
            let mut lines = vec![
                Line::from(Span::styled(
                    "Multi-valued tags will be normalized to Navidrome convention:",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
            ];
            if file.data.needs_artist {
                let joined = file.data.artist_values.join("; ");
                lines.push(Line::from(vec![
                    Span::styled("ARTIST: ", Style::default().fg(Color::Yellow)),
                    Span::raw(truncate_right(&joined, area.width.saturating_sub(10) as usize)),
                ]));
                lines.push(Line::from(Span::styled(
                    format!("  → ARTIST = \"{}\", ARTISTS × {}", joined, file.data.artist_values.len()),
                    Style::default().fg(Color::Green),
                )));
            }
            if file.data.needs_album_artist {
                let joined = file.data.album_artist_values.join("; ");
                lines.push(Line::from(vec![
                    Span::styled("ALBUMARTIST: ", Style::default().fg(Color::Yellow)),
                    Span::raw(truncate_right(&joined, area.width.saturating_sub(15) as usize)),
                ]));
                lines.push(Line::from(Span::styled(
                    format!("  → ALBUMARTIST = \"{}\", ALBUMARTISTS × {}", joined, file.data.album_artist_values.len()),
                    Style::default().fg(Color::Green),
                )));
            }
            lines
        } else {
            vec![
                Line::from("Semicolon-join singular tags, add individual plural tags."),
                Line::from("Per Navidrome multi-artist convention."),
            ]
        };
        let description = Paragraph::new(lines)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, area);
    }
}
