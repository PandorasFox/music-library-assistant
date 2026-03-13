//! Corrupt File Resolution Preview UI
//!
//! Shows corrupt files (tag parse errors or waveform decode failures) with
//! action buttons for stash/drop operations.
//!
//! - Up/Down: Scroll file list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{CorruptButton, CorruptFileModalData};
use crate::helpers::render_pane;
use crate::widgets::{
    render_file_path_list, ButtonRowState, ListClickTargets, PathEntry,
};

/// Actions returned from the corrupt file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptFilePreviewAction {
    /// No action needed.
    None,
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the corrupt file resolution modal.
#[derive(Debug)]
pub struct CorruptFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: CorruptFileModalData,
    /// Scroll position for the file list.
    pub scroll: usize,
    /// Button row state.
    pub buttons: ButtonRowState<CorruptButton>,
    /// Click targets for file list items (set during render).
    pub click_targets: ListClickTargets,
}

impl CorruptFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .files
            .get(self.scroll)
            .map(|f| f.corpus_path.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: CorruptFileModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            buttons: ButtonRowState::new(),
            click_targets: ListClickTargets::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<CorruptFilePreviewAction> {
        // Check buttons first
        if let Some(action) = self.buttons.handle_click(x, y, &self.cached_data) {
            return Some(action);
        }
        // Check list items
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.files.len() {
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> CorruptFilePreviewAction {
        if crate::helpers::handle_scroll_input(&mut self.scroll, action, self.cached_data.files.len()) {
            return CorruptFilePreviewAction::None;
        }

        match action {
            // Button navigation
            InputAction::NavLeft => {
                self.buttons.nav_left(&self.cached_data);
                CorruptFilePreviewAction::None
            }
            InputAction::NavRight => {
                self.buttons.nav_right(&self.cached_data);
                CorruptFilePreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => {
                self.buttons.confirm(&self.cached_data)
                    .unwrap_or(CorruptFilePreviewAction::None)
            }

            // Cancel
            InputAction::Cancel => CorruptFilePreviewAction::Cancel,

            _ => CorruptFilePreviewAction::None,
        }
    }

    /// Render the corrupt file resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Layout: title + content + controls
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Controls
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_content(f, main_chunks[1]);
        self.render_controls(f, main_chunks[2]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let total = self.cached_data.total_count();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Corrupt File Resolution ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", total),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();

        let block = Block::default()
            .title(format!(" Corrupt Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));

        let inner = render_pane(f, area, block);

        if self.cached_data.files.is_empty() {
            let empty = Paragraph::new("No corrupt files found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Render description
        let desc_height = 3;
        let desc_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: desc_height,
        };
        let list_area = Rect {
            x: inner.x,
            y: inner.y + desc_height,
            width: inner.width,
            height: inner.height.saturating_sub(desc_height),
        };

        self.click_targets.populate(list_area, self.scroll, self.cached_data.files.len());

        let description = Paragraph::new(vec![
            Line::from("These files have corrupt tags or unreadable audio data."),
            Line::from("Stashing will move them to stash/corrupt/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, desc_area);

        let entries: Vec<PathEntry> = self
            .cached_data
            .files
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, list_area, &entries, self.scroll, self.scroll);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        // Buttons render themselves and populate click rects.
        self.buttons.render(f, inner, &self.cached_data, true);
    }
}
