//! Subpar Duplicate Resolution Preview UI
//!
//! Shows subpar duplicate files (lower quality versions identified by
//! fingerprint analysis) with action buttons for stash/drop operations.
//!
//! Navigation:
//! - Up/Down: Scroll file list
//! - Shift+Up/Down: Move focus between list and buttons
//! - Left/Right: Move between action buttons (when buttons focused)
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::types::{SubparButton, SubparDuplicateModalData};
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{
    FocusPane, FrameInputResult, ModalFrame, PathField, CURSOR_STYLE,
    modal_frame::{ContentLayout, FrameState},
};

/// Actions returned from the subpar duplicate preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubparDuplicatePreviewAction {
    /// No action needed.
    None,
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the subpar duplicate resolution modal.
#[derive(Debug)]
pub struct SubparDuplicatePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: SubparDuplicateModalData,
    /// Scroll position for the file list.
    pub scroll: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<SubparButton>,
}

impl SubparDuplicatePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .files
            .get(self.scroll)
            .map(|f| f.corpus_path.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: SubparDuplicateModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            frame: FrameState::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<SubparDuplicatePreviewAction> {
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.files.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> SubparDuplicatePreviewAction {
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                SubparDuplicatePreviewAction::None
            }
        }
    }

    /// Render the subpar duplicate resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }

    /// Build the detail lines for the currently selected pair.
    fn build_detail_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self.cached_data.files.get(self.scroll) {
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
}

impl ModalFrame for SubparDuplicatePreviewState {
    type Button = SubparButton;

    fn frame_title(&self) -> Line<'static> {
        let total = self.cached_data.total_count();
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

    fn content_layout(&self) -> ContentLayout {
        // Compute dynamic detail height from path lengths
        let detail_inner_width = 80u16; // approximate; actual width comes from render area
        let detail_lines = self.build_detail_lines(detail_inner_width);
        let detail_height = (detail_lines.len() as u16) + 2; // +2 for borders
        ContentLayout::ListAboveDetail { detail_height }
    }

    fn list_title(&self) -> String {
        format!(" Subpar Files ({}) ", self.cached_data.files.len())
    }

    fn empty_message(&self) -> &'static str {
        "No subpar duplicates found"
    }

    fn frame_state(&self) -> &FrameState<SubparButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<SubparButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.scroll }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.scroll }
    fn list_len(&self) -> usize { self.cached_data.files.len() }
    fn button_ctx(&self) -> SubparDuplicateModalData { self.cached_data.clone() }
    fn escape_action(&self) -> SubparDuplicatePreviewAction { SubparDuplicatePreviewAction::Cancel }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.cached_data.files[idx];

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

    fn render_detail(&self, f: &mut Frame, area: Rect) {
        let detail_inner_width = area.width.saturating_sub(2);
        let detail_lines = self.build_detail_lines(detail_inner_width);

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
