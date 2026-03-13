//! Missing Directory Resolution Preview UI
//!
//! Shows missing directories with action buttons for acknowledgment/drop operations.
//!
//! - Up/Down: Scroll through directory list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use std::borrow::Cow;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::MissingDirectoryModalData;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{ButtonRowState, ListClickTargets, ModalButtons};

/// Actions returned from the missing directory preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingDirectoryPreviewAction {
    /// No action needed.
    None,
    /// User confirmed drop action - generate DropDirectoryFromIndex mutations.
    ConfirmDrop,
    /// Cancel and return to Insights view.
    Cancel,
}

/// Button choices for the missing directory resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingDirectoryButton {
    #[default]
    Drop,
    Cancel,
}

impl ModalButtons for MissingDirectoryButton {
    type Context = MissingDirectoryModalData;
    type Action = MissingDirectoryPreviewAction;

    fn all() -> &'static [Self] {
        &[Self::Drop, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Drop => "Drop All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Drop if ctx.count() > 0 => Color::Yellow,
            Self::Drop => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Drop => ctx.count() > 0,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingDirectoryPreviewAction {
        match self {
            Self::Drop => MissingDirectoryPreviewAction::ConfirmDrop,
            Self::Cancel => MissingDirectoryPreviewAction::Cancel,
        }
    }
}

/// State for the missing directory resolution modal.
#[derive(Debug)]
pub struct MissingDirectoryPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingDirectoryModalData,
    /// Scroll position for the directory list.
    pub scroll: usize,
    /// Button row state.
    pub buttons: ButtonRowState<MissingDirectoryButton>,
    /// Click targets for directory list items (set during render).
    pub click_targets: ListClickTargets,
}

impl MissingDirectoryPreviewState {
    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .directories
            .get(self.scroll)
            .map(|s| s.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: MissingDirectoryModalData) -> Self {
        let mut buttons = ButtonRowState::new();
        // Default to Drop if there are directories, otherwise Cancel.
        if cached_data.count() > 0 {
            buttons.selected = MissingDirectoryButton::Drop;
        }

        Self {
            cached_data,
            scroll: 0,
            buttons,
            click_targets: ListClickTargets::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<MissingDirectoryPreviewAction> {
        if let Some(action) = self.buttons.handle_click(x, y, &self.cached_data) {
            return Some(action);
        }
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.count() {
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingDirectoryPreviewAction {
        if crate::helpers::handle_scroll_input(&mut self.scroll, action, self.cached_data.count()) {
            return MissingDirectoryPreviewAction::None;
        }

        match action {
            // Button navigation
            InputAction::NavLeft => {
                self.buttons.nav_left(&self.cached_data);
                MissingDirectoryPreviewAction::None
            }
            InputAction::NavRight => {
                self.buttons.nav_right(&self.cached_data);
                MissingDirectoryPreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => {
                self.buttons.confirm(&self.cached_data)
                    .unwrap_or(MissingDirectoryPreviewAction::None)
            }

            // Cancel
            InputAction::Cancel => MissingDirectoryPreviewAction::Cancel,

            _ => MissingDirectoryPreviewAction::None,
        }
    }

    /// Render the missing directory resolution modal.
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
        let count = self.cached_data.count();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing Directory Acknowledgment ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} directories)", count),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.count();

        let border_style = Style::default().fg(Color::Yellow);

        let title = format!(" Deleted Directories ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        if self.cached_data.directories.is_empty() {
            let empty = Paragraph::new("No missing directories")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Description text
        let desc_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 2,
        };
        let desc = Paragraph::new(
            "These directories were deleted externally. Dropping will remove them and their files from the index."
        )
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, desc_area);

        // List area below description
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 3,
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };

        self.click_targets.populate(list_area, self.scroll, self.cached_data.count());

        let visible_lines = list_area.height as usize;
        let items: Vec<ListItem> = self
            .cached_data
            .directories
            .iter()
            .skip(self.scroll)
            .take(visible_lines)
            .map(|dir| {
                let path = truncate_left(dir, list_area.width.saturating_sub(2) as usize);
                ListItem::new(path).style(Style::default().fg(Color::White))
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, list_area);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        self.buttons.render(f, inner, &self.cached_data, true);
    }
}
