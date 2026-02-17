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
use crate::ui::helpers::{render_pane, truncate_left};
use crate::ui::widgets::{PathField, CURSOR_STYLE};

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
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data.files.get(self.scroll).map(|f| f.corpus_path.as_str())
    }

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

        // Split into detail pane (top) and list pane (bottom)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Detail pane: 2 lines + borders
                Constraint::Min(6),    // List pane
            ])
            .split(area);

        // Render detail pane with full paths of current selection
        self.render_detail_pane(f, chunks[0]);

        // Render list pane with three columns
        let block = Block::default()
            .title(format!(" Subpar Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 { Color::Cyan } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if list_focused { Color::Cyan } else { Color::DarkGray }));

        let inner = render_pane(f, chunks[1], block);

        if self.cached_data.files.is_empty() {
            let empty = Paragraph::new("No subpar duplicates found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Calculate visible lines
        let visible_lines = inner.height as usize;
        let scroll = self.scroll;

        // Four columns: 40% subpar path | 10% reason | 8% score | 42% superior path
        let total_width = inner.width as usize;
        let left_width = (total_width * 40) / 100;
        let mid_width = (total_width * 10) / 100;
        let score_width = (total_width * 8) / 100;
        let right_width = total_width.saturating_sub(left_width + mid_width + score_width);

        let items: Vec<ListItem> = self
            .cached_data
            .files
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .enumerate()
            .map(|(visible_idx, file)| {
                // First visible item (visible_idx 0) is the selected one
                let is_selected = visible_idx == 0;
                let style = if is_selected && list_focused {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::White)
                };
                let reason_style = if is_selected && list_focused {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::Yellow)
                };

                let subpar_path = truncate_left(&file.corpus_path, left_width.saturating_sub(1));
                let superior_path = truncate_left(&file.superior_path, right_width.saturating_sub(1));
                let score_str = format!("{:.1}%", file.similarity_score);

                let score_style = if is_selected && list_focused {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::DarkGray)
                };

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
                        format!(" {:<width$}", superior_path, width = right_width.saturating_sub(1)),
                        style,
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
            .title(" Selected Pair ")
            .title_style(Style::default().fg(Color::DarkGray))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = render_pane(f, area, block);

        // Get current file if any
        let current_file = self.cached_data.files.get(self.scroll);

        let lines = if let Some(file) = current_file {
            let mut lines = PathField::new(
                Span::styled("Subpar: ", Style::default().fg(Color::Red)),
                &file.corpus_path,
            )
            .style(Style::default().fg(Color::White))
            .render_lines(inner.width);

            let mut better_lines = PathField::new(
                Span::styled("Better: ", Style::default().fg(Color::Green)),
                &file.superior_path,
            )
            .style(Style::default().fg(Color::White))
            .render_lines(inner.width);
            if let Some(last) = better_lines.last_mut() {
                last.spans.push(Span::styled(
                    format!("  [{:.1}% match]", file.similarity_score),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.extend(better_lines);
            lines
        } else {
            vec![
                Line::from(Span::styled("No file selected", Style::default().fg(Color::DarkGray))),
            ]
        };

        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
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
