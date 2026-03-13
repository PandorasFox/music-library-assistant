//! ModalFrame: trait-based frame rendering and input routing for modal dialogs.
//!
//! Each modal state describes its layout via `ContentLayout`, implements data-level
//! hooks (render_list_item, render_detail), and gets rendering + input routing defaults.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::helpers::render_pane;
use crate::input::InputAction;
use super::list_click_targets::ListClickTargets;
use super::modal_buttons::{ButtonRowState, ModalButtons};
use super::resolution_layout::FocusPane;

pub enum FrameInputResult<A> {
    Action(A),
    Consumed,
    Unhandled,
}

pub enum ContentLayout {
    ListAboveDetail { detail_height: u16 },
    DetailAboveList { detail_height: u16 },
    FourSection { header_height: u16, detail_height: u16 },
    HorizontalSplit { list_percent: u16, info_height: u16 },
}

/// Common frame state fields shared by all ModalFrame implementors.
#[derive(Debug)]
pub struct FrameState<B: ModalButtons> {
    pub focus_pane: FocusPane,
    pub buttons: ButtonRowState<B>,
    pub click_targets: ListClickTargets,
}

impl<B: ModalButtons> Default for FrameState<B> {
    fn default() -> Self {
        Self {
            focus_pane: FocusPane::List,
            buttons: ButtonRowState::new(),
            click_targets: ListClickTargets::new(),
        }
    }
}

impl<B: ModalButtons> FrameState<B> {
    pub fn new() -> Self {
        Self::default()
    }
}

pub trait ModalFrame {
    type Button: ModalButtons;

    fn frame_title(&self) -> Line<'static> { Line::default() }
    fn content_layout(&self) -> ContentLayout;
    fn list_title(&self) -> String;
    fn accent_color(&self) -> Color { Color::Cyan }
    fn empty_message(&self) -> &'static str { "No items" }
    fn controls_height(&self) -> u16 { 3 }
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

    // State accessors
    fn frame_state(&self) -> &FrameState<Self::Button>;
    fn frame_state_mut(&mut self) -> &mut FrameState<Self::Button>;
    fn cursor(&self) -> usize;
    fn cursor_mut(&mut self) -> &mut usize;
    fn list_len(&self) -> usize;
    fn button_ctx(&self) -> <Self::Button as ModalButtons>::Context;
    fn escape_action(&self) -> <Self::Button as ModalButtons>::Action;

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
        self.frame_state_mut().buttons.render(f, rows[0], &ctx, focused);
        let hints = self.controls_hints();
        if !hints.is_empty() {
            f.render_widget(Paragraph::new(Line::from(hints)).alignment(Alignment::Center), rows[1]);
        }
    }

    fn handle_frame_input(
        &mut self, action: &InputAction,
    ) -> FrameInputResult<<Self::Button as ModalButtons>::Action> {
        match action {
            InputAction::FocusUp => {
                let fp = &mut self.frame_state_mut().focus_pane;
                *fp = fp.prev();
                return FrameInputResult::Consumed;
            }
            InputAction::FocusDown => {
                let fp = &mut self.frame_state_mut().focus_pane;
                *fp = fp.next();
                return FrameInputResult::Consumed;
            }
            _ => {}
        }
        match action {
            InputAction::NavUp if self.frame_state().focus_pane == FocusPane::List => {
                *self.cursor_mut() = self.cursor().saturating_sub(1);
                FrameInputResult::Consumed
            }
            InputAction::NavDown if self.frame_state().focus_pane == FocusPane::List => {
                let max = self.list_len().saturating_sub(1);
                let c = self.cursor_mut();
                if *c < max { *c += 1; }
                FrameInputResult::Consumed
            }
            InputAction::PageUp => {
                *self.cursor_mut() = self.cursor().saturating_sub(10);
                FrameInputResult::Consumed
            }
            InputAction::PageDown => {
                let max = self.list_len().saturating_sub(1);
                *self.cursor_mut() = (self.cursor() + 10).min(max);
                FrameInputResult::Consumed
            }
            InputAction::NavLeft if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                self.frame_state_mut().buttons.nav_left(&ctx);
                FrameInputResult::Consumed
            }
            InputAction::NavRight if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                self.frame_state_mut().buttons.nav_right(&ctx);
                FrameInputResult::Consumed
            }
            InputAction::Confirm if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                match self.frame_state_mut().buttons.confirm(&ctx) {
                    Some(a) => FrameInputResult::Action(a),
                    None => FrameInputResult::Consumed,
                }
            }
            InputAction::Cancel => FrameInputResult::Action(self.escape_action()),
            _ => FrameInputResult::Unhandled,
        }
    }
}
