//! ModalFrame: trait-based frame rendering for modal dialogs.
//!
//! Logic (state accessors, input routing) lives in mm-ui's `ModalFrameCore`.
//! This module adds rendering via the `ModalFrame` supertrait.

pub use mm_ui::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::helpers::render_pane;
use super::modal_buttons::render_buttons;
use super::resolution_layout::FocusPane;

pub trait ModalFrame: ModalFrameCore {
    fn frame_title(&self) -> Line<'static> { Line::default() }
    fn accent_color(&self) -> Color { Color::Cyan }
    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
            Span::styled("\u{2190}\u{2192}", s),
            Span::styled(" select  ", s),
            Span::styled("Enter", s),
            Span::styled(" confirm", s),
        ]
    }

    // Data-level hooks
    fn render_list_item(&self, _idx: usize, _width: u16, _is_cursor: bool, _is_focused: bool) -> ListItem<'static> {
        unreachable!("modal overrides render_frame_list")
    }
    fn render_detail(&mut self, f: &mut Frame, area: Rect);
    fn render_header(&self, _f: &mut Frame, _area: Rect) {}
    fn render_info_bar(&self, _f: &mut Frame, _area: Rect) {}

    // === Provided defaults ===

    fn render_frame(&mut self, f: &mut Frame, area: Rect) {
        let controls_h = self.controls_height();
        match self.content_layout() {
            ContentLayout::FourSection { header_height, detail_height } => {
                f.render_widget(Clear, area);
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(header_height), Constraint::Min(5),
                        Constraint::Length(detail_height), Constraint::Length(controls_h),
                    ])
                    .split(area);
                self.render_header(f, chunks[0]);
                self.render_frame_list(f, chunks[1]);
                self.render_detail(f, chunks[2]);
                self.render_frame_controls(f, chunks[3]);
            }
            ContentLayout::ListAboveDetail { detail_height } => {
                f.render_widget(Clear, area);
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), Constraint::Min(10), Constraint::Length(controls_h),
                    ])
                    .split(area);
                self.render_frame_title(f, chunks[0]);
                let content = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(6), Constraint::Length(detail_height)])
                    .split(chunks[1]);
                self.render_frame_list(f, content[0]);
                self.render_detail(f, content[1]);
                self.render_frame_controls(f, chunks[2]);
            }
            ContentLayout::DetailAboveList { detail_height } => {
                f.render_widget(Clear, area);
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), Constraint::Min(10), Constraint::Length(controls_h),
                    ])
                    .split(area);
                self.render_frame_title(f, chunks[0]);
                let content = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(detail_height), Constraint::Min(6)])
                    .split(chunks[1]);
                self.render_detail(f, content[0]);
                self.render_frame_list(f, content[1]);
                self.render_frame_controls(f, chunks[2]);
            }
            ContentLayout::HorizontalSplit { list_percent, info_height } => {
                use super::resolution_layout::ResolutionLayout;
                let padded = ResolutionLayout::padded(area);
                f.render_widget(Clear, padded);
                let layout = ResolutionLayout::new(padded, info_height, controls_h, list_percent);
                self.render_info_bar(f, layout.info_bar);
                self.render_frame_list(f, layout.list_pane);
                self.render_detail(f, layout.details_pane);
                self.render_frame_controls(f, layout.buttons);
            }
        }
    }

    fn render_frame_title(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(self.frame_title()).block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    fn render_frame_list(&mut self, f: &mut Frame, area: Rect) {
        let count = self.list_len();
        let focused = self.frame_state().focus_pane == FocusPane::List;
        let accent = self.accent_color();
        let block = Block::default()
            .title(self.list_title())
            .title_style(Style::default().fg(if count > 0 { accent } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { accent } else { Color::DarkGray }));
        let inner = render_pane(f, area, block);
        let cursor = self.cursor();
        self.frame_state_mut().click_targets.populate(inner, cursor, count);
        if count == 0 {
            f.render_widget(
                Paragraph::new(self.empty_message()).style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }
        let visible = inner.height as usize;
        let items: Vec<ListItem> = (cursor..)
            .take(visible).take_while(|&i| i < count)
            .enumerate()
            .map(|(vi, i)| self.render_list_item(i, inner.width, vi == 0, focused))
            .collect();
        f.render_widget(List::new(items), inner);
    }

    fn render_frame_controls(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.frame_state().focus_pane == FocusPane::Buttons;
        let accent = self.accent_color();
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(if focused { accent } else { Color::DarkGray }));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        let ctx = self.button_ctx();
        render_buttons(&mut self.frame_state_mut().buttons, f, rows[0], &ctx, focused);
        let hints = self.controls_hints();
        if !hints.is_empty() {
            f.render_widget(Paragraph::new(Line::from(hints)).alignment(Alignment::Center), rows[1]);
        }
    }
}
