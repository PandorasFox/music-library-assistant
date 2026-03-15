//! Shit Format Resolution Preview — TUI rendering only.
//!
//! State, input handling, and ModalFrameCore live in mm-ui.
//! This module provides the ratatui ModalFrame impl.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
    Frame,
};

use mm_meta::views::cluster_deploy::ShitFormatModalData;
use mm_ui::resolutions::shit_format::{ShitFormatButton, ShitFormatPreviewState};
use crate::helpers::{render_pane, truncate_right};
use crate::widgets::modal_frame::ModalFrame;
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

/// Entry point for rendering this modal.
pub fn render(f: &mut Frame, area: Rect, state: &mut ShitFormatPreviewState) {
    state.render_frame(f, area);
}

fn render_lossless_section(data: &ShitFormatModalData, f: &mut Frame, area: Rect) {
    let has_files = data.has_lossless();
    let color = if has_files { Color::Green } else { Color::DarkGray };

    let block = Block::default()
        .title(" Lossless \u{2192} FLAC ")
        .title_style(Style::default().fg(color))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));

    let inner = render_pane(f, area, block);

    if !has_files {
        f.render_widget(
            Paragraph::new("No lossless files").style(Style::default().fg(Color::DarkGray)),
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

    let breakdown = data.lossless_breakdown();
    let items: Vec<ListItem> = breakdown
        .iter()
        .map(|(ftype, count)| {
            ListItem::new(format!("  {}: {}", ftype, count))
                .style(Style::default().fg(Color::White))
        })
        .collect();
    f.render_widget(List::new(items), chunks[1]);
}

fn render_lossy_section(data: &ShitFormatModalData, f: &mut Frame, area: Rect) {
    let has_files = data.has_lossy();
    let lossy_to_flac = data.lossy_to_flac;
    let color = if has_files { Color::Cyan } else { Color::DarkGray };

    let title = if lossy_to_flac {
        " Lossy \u{2192} FLAC (lossy capture) "
    } else {
        " Lossy \u{2192} Opus "
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(color))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));

    let inner = render_pane(f, area, block);

    if !has_files {
        f.render_widget(
            Paragraph::new("No lossy files").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    if lossy_to_flac {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(2)])
            .split(inner);

        f.render_widget(
            Paragraph::new("Capture decoded waveform to FLAC")
                .style(Style::default().fg(Color::DarkGray)),
            chunks[0],
        );

        let breakdown = data.lossy_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();
        f.render_widget(List::new(items), chunks[1]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(2)])
            .split(inner);

        let bitrate = data.opus_bitrate_kbps;
        let ratio = (bitrate as f64 - 32.0) / (512.0 - 32.0);
        let gauge = Gauge::default()
            .block(
                Block::default()
                    .title("Opus Bitrate")
                    .borders(Borders::NONE),
            )
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
            .ratio(ratio)
            .label(format!("{} kbps", bitrate));
        f.render_widget(gauge, chunks[0]);

        let breakdown = data.lossy_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();
        f.render_widget(List::new(items), chunks[1]);
    }
}

impl ModalFrame for ShitFormatPreviewState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        let mut hints = vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
        ];

        let on_lossy = matches!(
            self.frame.buttons.selected,
            ShitFormatButton::TranscodeLossy | ShitFormatButton::ConvertAll
        );
        if on_lossy && !self.cached_data.lossy_to_flac && self.cached_data.has_lossy() {
            hints.push(Span::styled("\u{2190}\u{2192}", s));
            hints.push(Span::styled(" bitrate  ", s));
        } else {
            hints.push(Span::styled("\u{2190}\u{2192}", s));
            hints.push(Span::styled(" select  ", s));
        }

        hints.push(Span::styled("Tab", s));
        hints.push(Span::styled(" switch  ", s));
        hints.push(Span::styled("Enter", s));
        hints.push(Span::styled(" confirm", s));
        hints
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        let lossless = self.cached_data.lossless_files.len();
        let lossy = self.cached_data.lossy_files.len();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Shit Format Resolution ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} lossless, {} lossy)", lossless, lossy),
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
        let lossless_len = self.cached_data.lossless_files.len();
        let file = if idx < lossless_len {
            &self.cached_data.lossless_files[idx]
        } else {
            &self.cached_data.lossy_files[idx - lossless_len]
        };

        let type_tag = format!("[{}] ", file.file_type);
        let tag_color = if file.is_lossless() {
            Color::Green
        } else {
            Color::Cyan
        };
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
                    Style::default().fg(tag_color)
                },
            ),
            Span::styled(path, style),
        ]))
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        render_lossless_section(&self.cached_data, f, chunks[0]);
        render_lossy_section(&self.cached_data, f, chunks[1]);
    }
}
