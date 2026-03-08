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

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::types::{SelectedButton, SubparDuplicateModalData};
use crate::ui::helpers::{render_pane, truncate_left};
use crate::ui::widgets::{ListClickTargets, PathField, CURSOR_STYLE};

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
    /// Click targets for file list items (set during render)
    pub click_targets: ListClickTargets,
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
    ) -> Option<SubparDuplicatePreviewAction> {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.files.len() {
                    self.focus_pane = FocusPane::List;
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> SubparDuplicatePreviewAction {
        let has_files = self.cached_data.has_files();

        // FocusUp/FocusDown: move focus between panes
        match action {
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                return SubparDuplicatePreviewAction::None;
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                return SubparDuplicatePreviewAction::None;
            }
            _ => {}
        }

        match action {
            // Scroll file list (only when list focused)
            InputAction::NavUp if self.focus_pane == FocusPane::List => {
                self.scroll = self.scroll.saturating_sub(1);
                SubparDuplicatePreviewAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::List => {
                let max = self.cached_data.files.len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                SubparDuplicatePreviewAction::None
            }
            InputAction::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                SubparDuplicatePreviewAction::None
            }
            InputAction::PageDown => {
                let max = self.cached_data.files.len().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                SubparDuplicatePreviewAction::None
            }

            // Button navigation (when buttons focused)
            InputAction::NavLeft if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.left(has_files);
                SubparDuplicatePreviewAction::None
            }
            InputAction::NavRight if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.right(has_files);
                SubparDuplicatePreviewAction::None
            }

            // Execute selected button (only when buttons focused)
            InputAction::Confirm if self.focus_pane == FocusPane::Buttons => {
                match self.selected_button {
                    SelectedButton::StashAll if has_files => {
                        SubparDuplicatePreviewAction::ConfirmStashAll
                    }
                    SelectedButton::Cancel => SubparDuplicatePreviewAction::Cancel,
                    _ => SubparDuplicatePreviewAction::None,
                }
            }

            // Cancel
            InputAction::Cancel => SubparDuplicatePreviewAction::Cancel,

            _ => SubparDuplicatePreviewAction::None,
        }
    }

    /// Render the subpar duplicate resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
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

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();
        let list_focused = self.focus_pane == FocusPane::List;

        // Pre-compute detail lines to determine dynamic height.
        // Use area width minus 2 (borders) as the inner width for wrapping.
        let detail_inner_width = area.width.saturating_sub(2);
        let detail_lines = self.build_detail_lines(detail_inner_width);
        let detail_height = (detail_lines.len() as u16) + 2; // +2 for borders

        // Split into list pane (top) and detail pane (bottom, dynamic)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),                // List pane (top)
                Constraint::Length(detail_height), // Detail pane (bottom, dynamic)
            ])
            .split(area);

        // Render list pane (top)
        let block = Block::default()
            .title(format!(" Subpar Files ({}) ", count))
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

        let inner = render_pane(f, chunks[0], block);

        // Populate click targets for list items
        self.click_targets.clear();
        self.click_targets.set_list_area(inner);

        if !self.cached_data.files.is_empty() {
            // Calculate visible lines
            let visible_lines = inner.height as usize;
            let scroll = self.scroll;

            // Track click target rows
            for (vis_idx, entry_idx) in (scroll..).take(visible_lines).enumerate() {
                if entry_idx >= self.cached_data.files.len() {
                    break;
                }
                self.click_targets
                    .add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
            }

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

                    let subpar_path =
                        truncate_left(&file.corpus_path, left_width.saturating_sub(1));
                    let superior_path =
                        truncate_left(&file.superior_path, right_width.saturating_sub(1));
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
                            format!(
                                " {:<width$}",
                                superior_path,
                                width = right_width.saturating_sub(1)
                            ),
                            style,
                        ),
                    ]);
                    ListItem::new(line)
                })
                .collect();

            let list = List::new(items);
            f.render_widget(list, inner);
        } else {
            let empty = Paragraph::new("No subpar duplicates found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
        }

        // Render detail pane (bottom, dynamically sized)
        let detail_block = Block::default()
            .title(" Selected Pair ")
            .title_style(Style::default().fg(Color::DarkGray))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let detail_inner = render_pane(f, chunks[1], detail_block);
        let para = Paragraph::new(detail_lines);
        f.render_widget(para, detail_inner);
    }

    /// Build the detail lines for the currently selected pair.
    /// Returns owned lines so they can be used both for height measurement and rendering.
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

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_files();
        let buttons_focused = self.focus_pane == FocusPane::Buttons;

        // Build button line
        let mut buttons = Vec::new();

        // Stash All button
        let stash_style = if !has_files {
            Style::default().fg(Color::DarkGray)
        } else if buttons_focused && self.selected_button == SelectedButton::StashAll {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        buttons.push(Span::styled(" Stash & Drop All ", stash_style));
        buttons.push(Span::raw("  "));

        // Cancel button
        let cancel_style = if buttons_focused && self.selected_button == SelectedButton::Cancel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
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

        let controls = Paragraph::new(Line::from(buttons)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(if buttons_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        );

        f.render_widget(controls, area);
    }
}
