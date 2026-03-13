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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use mm_meta::views::MatchClassification;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{
    render_button_row, ConfirmationButton, FocusPane, ListClickTargets, PathField, CURSOR_STYLE,
};

use super::types::{InboxCorpusMatchModalData, SelectedButton};

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
    pub selected_button: SelectedButton,
    pub focus_pane: FocusPane,
    /// Click targets for list items (set during render)
    pub click_targets: ListClickTargets,
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
            selected_button: SelectedButton::Cancel,
            focus_pane: FocusPane::List,
            click_targets: ListClickTargets::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<InboxCorpusMatchPreviewAction> {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.entries.len() {
                    self.focus_pane = FocusPane::List;
                    self.scroll = idx;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> InboxCorpusMatchPreviewAction {
        match action {
            // Shift+Up/Down: move focus between panes
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                InboxCorpusMatchPreviewAction::None
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                InboxCorpusMatchPreviewAction::None
            }

            InputAction::NavUp if self.focus_pane == FocusPane::List => {
                self.scroll = self.scroll.saturating_sub(1);
                InboxCorpusMatchPreviewAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::List => {
                let max = self.cached_data.entries.len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                InboxCorpusMatchPreviewAction::None
            }
            InputAction::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                InboxCorpusMatchPreviewAction::None
            }
            InputAction::PageDown => {
                let max = self.cached_data.entries.len().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                InboxCorpusMatchPreviewAction::None
            }

            InputAction::NavLeft if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.left();
                InboxCorpusMatchPreviewAction::None
            }
            InputAction::NavRight if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.right();
                InboxCorpusMatchPreviewAction::None
            }

            InputAction::Confirm if self.focus_pane == FocusPane::Buttons => {
                let has_entries = !self.cached_data.entries.is_empty();
                match self.selected_button {
                    SelectedButton::StashEquivalents if self.cached_data.stashable_count() > 0 => {
                        InboxCorpusMatchPreviewAction::ConfirmStash
                    }
                    SelectedButton::StashAll if has_entries => {
                        InboxCorpusMatchPreviewAction::ConfirmStashAll
                    }
                    SelectedButton::Cancel => InboxCorpusMatchPreviewAction::Cancel,
                    _ => InboxCorpusMatchPreviewAction::None,
                }
            }

            InputAction::Cancel => InboxCorpusMatchPreviewAction::Cancel,

            _ => InboxCorpusMatchPreviewAction::None,
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Min(10),   // Content
                Constraint::Length(3), // Controls
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_content(f, main_chunks[1]);
        self.render_controls(f, main_chunks[2]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let (better, equivalent, subpar) = self.cached_data.count_by_class();

        let title = Paragraph::new(Line::from(vec![
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
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.entries.len();
        let list_focused = self.focus_pane == FocusPane::List;

        // Split into detail pane (top) and list pane (bottom)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6), // Detail pane: corpus matches
                Constraint::Min(6),    // List pane
            ])
            .split(area);

        self.render_detail_pane(f, chunks[0]);

        let block = Block::default()
            .title(format!(" Inbox Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 {
                Color::Cyan
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if list_focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));

        let inner = render_pane(f, chunks[1], block);

        self.click_targets.populate(inner, self.scroll, self.cached_data.entries.len());

        if self.cached_data.entries.is_empty() {
            let empty = Paragraph::new("No inbox corpus matches found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let visible_lines = inner.height as usize;
        let scroll = self.scroll;

        let total_width = inner.width as usize;
        let icon_width = 3;
        let quality_width = 22;
        let path_width = total_width.saturating_sub(icon_width + quality_width);

        let items: Vec<ListItem> = self
            .cached_data
            .entries
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .enumerate()
            .map(|(visible_idx, entry)| {
                let is_selected = visible_idx == 0;
                let style = if is_selected && list_focused {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::White)
                };

                let (icon, icon_color) = match entry.classification {
                    MatchClassification::Better => ("B", Color::Cyan),
                    MatchClassification::Equivalent => ("=", Color::Green),
                    MatchClassification::Subpar => ("v", Color::Yellow),
                };

                let icon_style = if is_selected && list_focused {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(icon_color)
                };

                let quality_style = if is_selected && list_focused {
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
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, inner);
    }

    fn render_detail_pane(&self, f: &mut Frame, area: Rect) {
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

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let stashable = self.cached_data.stashable_count();
        let total = self.cached_data.total_count();
        let has_stashable = stashable > 0;
        let has_entries = total > 0;
        let buttons_focused = self.focus_pane == FocusPane::Buttons;

        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(if buttons_focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Buttons
                Constraint::Length(1), // Hint
            ])
            .split(inner);

        // Buttons via standard widget
        let stash_color = if has_stashable {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let stash_all_color = if has_entries {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let stash_selected = has_stashable
            && buttons_focused
            && self.selected_button == SelectedButton::StashEquivalents;
        let stash_all_selected =
            has_entries && buttons_focused && self.selected_button == SelectedButton::StashAll;
        let cancel_selected = buttons_focused && self.selected_button == SelectedButton::Cancel;

        let buttons = vec![
            ConfirmationButton::new(format!("Stash {} equiv+subpar", stashable), stash_color)
                .selected(stash_selected),
            ConfirmationButton::new(format!("Stash all {}", total), stash_all_color)
                .selected(stash_all_selected),
            ConfirmationButton::new("Cancel", Color::White).selected(cancel_selected),
        ];
        render_button_row(f, inner_chunks[0], &buttons);

        // Hint
        let hint = Paragraph::new(Line::from(Span::styled(
            "Shift+\u{2191}\u{2193} focus  \u{2190}\u{2192} select  Enter confirm",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(hint, inner_chunks[1]);
    }
}
