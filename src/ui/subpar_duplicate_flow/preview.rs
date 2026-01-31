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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::types::{SubparDuplicateModalData, SelectedButton};
use crate::ui::helpers::truncate_left;

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    List,
    Buttons,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::List => Self::Buttons,
            Self::Buttons => Self::Buttons,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::List => Self::List,
            Self::Buttons => Self::List,
        }
    }
}

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
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Which pane has focus
    pub focus_pane: FocusPane,
}

impl SubparDuplicatePreviewState {
    /// Create a new preview state with cached data.
    pub fn new(cached_data: SubparDuplicateModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            selected_button: SelectedButton::Cancel,
            focus_pane: FocusPane::List,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> SubparDuplicatePreviewAction {
        let has_files = self.cached_data.has_files();

        // Shift+Up/Down: move focus between panes
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return SubparDuplicatePreviewAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return SubparDuplicatePreviewAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            // Scroll file list (only when list focused)
            KeyCode::Up | KeyCode::Char('k') if self.focus_pane == FocusPane::List => {
                self.scroll = self.scroll.saturating_sub(1);
                SubparDuplicatePreviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.focus_pane == FocusPane::List => {
                let max = self.cached_data.files.len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                SubparDuplicatePreviewAction::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                SubparDuplicatePreviewAction::None
            }
            KeyCode::PageDown => {
                let max = self.cached_data.files.len().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                SubparDuplicatePreviewAction::None
            }

            // Button navigation (when buttons focused)
            KeyCode::Left | KeyCode::Char('h') if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.left(has_files);
                SubparDuplicatePreviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.right(has_files);
                SubparDuplicatePreviewAction::None
            }

            // Execute selected button (only when buttons focused)
            KeyCode::Enter if self.focus_pane == FocusPane::Buttons => {
                match self.selected_button {
                    SelectedButton::StashAll if has_files => {
                        SubparDuplicatePreviewAction::ConfirmStashAll
                    }
                    SelectedButton::Cancel => SubparDuplicatePreviewAction::Cancel,
                    _ => SubparDuplicatePreviewAction::None,
                }
            }

            // Cancel
            KeyCode::Esc => SubparDuplicatePreviewAction::Cancel,

            _ => SubparDuplicatePreviewAction::None,
        }
    }

    /// Render the subpar duplicate resolution modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Layout: title + content + controls
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
        let total = self.cached_data.total_count();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Subpar Duplicate Resolution ",
                Style::default()
                    .fg(Color::Cyan)
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

    fn render_content(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();
        let list_focused = self.focus_pane == FocusPane::List;

        let block = Block::default()
            .title(format!(" Subpar Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 { Color::Cyan } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if list_focused { Color::Cyan } else { Color::DarkGray }));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.cached_data.files.is_empty() {
            let empty = Paragraph::new("No subpar duplicates found")
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

        let description = Paragraph::new(vec![
            Line::from("These files are lower-quality versions of tracks you already have."),
            Line::from("Stashing will move them to stash/subpar/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, desc_area);

        // Calculate visible lines based on list area height
        let visible_lines = list_area.height as usize;
        let scroll = self.scroll;

        // Calculate width for path (leave room for reason column)
        let reason_width = 20usize;
        let path_width = list_area.width.saturating_sub(reason_width as u16 + 4) as usize;

        let items: Vec<ListItem> = self
            .cached_data
            .files
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .map(|file| {
                let path = truncate_left(&file.corpus_path, path_width);
                let line = Line::from(vec![
                    Span::styled(
                        format!("{:<width$}", path, width = path_width),
                        Style::default().fg(Color::White),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<width$}", file.reason, width = reason_width),
                        Style::default().fg(Color::Yellow),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, list_area);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_files();
        let buttons_focused = self.focus_pane == FocusPane::Buttons;

        // Build button line
        let mut buttons = Vec::new();

        // Stash All button
        let stash_style = if !has_files {
            Style::default().fg(Color::DarkGray)
        } else if buttons_focused && self.selected_button == SelectedButton::StashAll {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        buttons.push(Span::styled(" Stash & Drop All ", stash_style));
        buttons.push(Span::raw("  "));

        // Cancel button
        let cancel_style = if buttons_focused && self.selected_button == SelectedButton::Cancel {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        buttons.push(Span::styled(" Cancel ", cancel_style));

        // Hint text
        buttons.push(Span::raw("    "));
        buttons.push(Span::styled(
            "[Shift+↑↓ focus] [←→ select] [Enter confirm]",
            Style::default().fg(Color::DarkGray),
        ));

        let controls = Paragraph::new(Line::from(buttons))
            .block(Block::default().borders(Borders::TOP).border_style(
                Style::default().fg(if buttons_focused { Color::Cyan } else { Color::DarkGray })
            ));

        f.render_widget(controls, area);
    }
}
