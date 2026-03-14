//! Inbox Corpus Match Resolution Preview UI
//!
//! Shows inbox files with corpus fingerprint matches, classified by quality.
//! Allows stashing equivalent/subpar inbox copies, or all duplicates.
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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use mm_meta::views::MatchClassification;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{
    FocusPane, FrameInputResult, ModalFrame, PathField, CURSOR_STYLE,
    modal_frame::{ContentLayout, FrameState, ModalFrameCore},
};

use super::types::{InboxCorpusMatchModalData, InboxMatchButton};

/// Actions returned from the inbox corpus match preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxCorpusMatchPreviewAction {
    None,
    /// Stash equivalent + subpar entries only
    ConfirmStash,
    /// Stash ALL inbox duplicates (including better-quality ones)
    ConfirmStashAll,
    Cancel,
}

/// State for the inbox corpus match resolution modal.
#[derive(Debug)]
pub struct InboxCorpusMatchPreviewState {
    pub cached_data: InboxCorpusMatchModalData,
    pub scroll: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<InboxMatchButton>,
}

impl InboxCorpusMatchPreviewState {
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .entries
            .get(self.scroll)
            .map(|e| e.inbox_path.as_str())
    }

    pub fn new(cached_data: InboxCorpusMatchModalData) -> Self {
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
    ) -> Option<InboxCorpusMatchPreviewAction> {
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.entries.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.scroll = idx;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> InboxCorpusMatchPreviewAction {
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                InboxCorpusMatchPreviewAction::None
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }
}

impl ModalFrameCore for InboxCorpusMatchPreviewState {
    type Button = InboxMatchButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::DetailAboveList { detail_height: 6 }
    }

    fn list_title(&self) -> String {
        format!(" Inbox Files ({}) ", self.cached_data.entries.len())
    }

    fn empty_message(&self) -> &'static str {
        "No inbox corpus matches found"
    }

    fn frame_state(&self) -> &FrameState<InboxMatchButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<InboxMatchButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.scroll }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.scroll }
    fn list_len(&self) -> usize { self.cached_data.entries.len() }
    fn button_ctx(&self) -> InboxCorpusMatchModalData { self.cached_data.clone() }
    fn escape_action(&self) -> InboxCorpusMatchPreviewAction { InboxCorpusMatchPreviewAction::Cancel }
}

impl ModalFrame for InboxCorpusMatchPreviewState {
    fn frame_title(&self) -> Line<'static> {
        let (better, equivalent, subpar) = self.cached_data.count_by_class();
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
        let entry = &self.cached_data.entries[idx];

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

        let current = self.cached_data.entries.get(self.scroll);

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
