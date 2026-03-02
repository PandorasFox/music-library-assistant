//! Missing File Resolution Preview UI
//!
//! Shows categorized missing files (restorable vs non-restorable) with
//! action buttons for restore/drop operations.
//!
//! - Tab: Switch between lists
//! - Up/Down: Scroll within focused list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{MissingFileModalData, SelectedButton};
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{render_file_path_list, ButtonRects, ListClickTargets, PathEntry};

/// Actions returned from the missing file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingFilePreviewAction {
    /// No action needed.
    None,
    /// User confirmed restore action - generate HardLink mutations.
    ConfirmRestore,
    /// User confirmed drop action - generate DropFromIndex mutations.
    ConfirmDrop,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the missing file resolution modal.
#[derive(Debug)]
pub struct MissingFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingFileModalData,
    /// Which list has focus (0 = restorable, 1 = non-restorable).
    pub focused_list: usize,
    /// Scroll position for each list.
    pub scroll: [usize; 2],
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Click targets for restorable list (set during render).
    pub click_targets_restorable: ListClickTargets,
    /// Click targets for non-restorable list (set during render).
    pub click_targets_non_restorable: ListClickTargets,
    /// Click targets for buttons (set during render).
    pub button_rects: ButtonRects,
}

impl MissingFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        let idx = self.scroll[self.focused_list];
        if self.focused_list == 0 {
            self.cached_data.restorable.get(idx).map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data.non_restorable.get(idx).map(|f| f.corpus_path.as_str())
        }
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: MissingFileModalData) -> Self {
        // Focus the list that has items
        let focused_list = if cached_data.has_restorable() {
            0
        } else if cached_data.has_non_restorable() {
            1
        } else {
            0
        };

        Self {
            cached_data,
            focused_list,
            scroll: [0, 0],
            selected_button: SelectedButton::Cancel,
            click_targets_restorable: ListClickTargets::new(),
            click_targets_non_restorable: ListClickTargets::new(),
            button_rects: ButtonRects::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(&mut self, x: u16, y: u16, _gesture: &ConfirmationGesture) -> Option<MissingFilePreviewAction> {
        let has_restorable = self.cached_data.has_restorable();

        // Check buttons first
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            match button_name {
                "restore_all" => {
                    self.selected_button = SelectedButton::RestoreAll;
                    if has_restorable {
                        return Some(MissingFilePreviewAction::ConfirmRestore);
                    }
                }
                "drop_lost" => {
                    self.selected_button = SelectedButton::DropLost;
                    return Some(MissingFilePreviewAction::ConfirmDrop);
                }
                "cancel" => {
                    self.selected_button = SelectedButton::Cancel;
                    return Some(MissingFilePreviewAction::Cancel);
                }
                _ => {}
            }
        }
        // Check restorable list
        if let Some(id) = self.click_targets_restorable.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.restorable.len() {
                    self.focused_list = 0;
                    self.scroll[0] = idx;
                }
            }
            return None;
        }
        // Check non-restorable list
        if let Some(id) = self.click_targets_non_restorable.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.non_restorable.len() {
                    self.focused_list = 1;
                    self.scroll[1] = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingFilePreviewAction {
        let has_restorable = self.cached_data.has_restorable();
        let has_non_restorable = self.cached_data.has_non_restorable();

        match action {
            // Switch between lists
            InputAction::CycleNext | InputAction::CyclePrev => {
                if has_restorable && has_non_restorable {
                    self.focused_list = 1 - self.focused_list;
                }
                MissingFilePreviewAction::None
            }

            // Scroll within focused list
            InputAction::NavUp => {
                self.scroll[self.focused_list] = self.scroll[self.focused_list].saturating_sub(1);
                MissingFilePreviewAction::None
            }
            InputAction::NavDown => {
                let max = self.max_scroll_for_list(self.focused_list);
                if self.scroll[self.focused_list] < max {
                    self.scroll[self.focused_list] += 1;
                }
                MissingFilePreviewAction::None
            }
            InputAction::PageUp => {
                self.scroll[self.focused_list] = self.scroll[self.focused_list].saturating_sub(10);
                MissingFilePreviewAction::None
            }
            InputAction::PageDown => {
                let max = self.max_scroll_for_list(self.focused_list);
                self.scroll[self.focused_list] = (self.scroll[self.focused_list] + 10).min(max);
                MissingFilePreviewAction::None
            }

            // Button navigation
            InputAction::NavLeft => {
                self.selected_button.left(has_restorable);
                MissingFilePreviewAction::None
            }
            InputAction::NavRight => {
                self.selected_button.right(has_restorable);
                MissingFilePreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => match self.selected_button {
                SelectedButton::RestoreAll if has_restorable => {
                    MissingFilePreviewAction::ConfirmRestore
                }
                SelectedButton::DropLost => MissingFilePreviewAction::ConfirmDrop,
                SelectedButton::Cancel => MissingFilePreviewAction::Cancel,
                _ => MissingFilePreviewAction::None,
            },

            // Cancel
            InputAction::Cancel => MissingFilePreviewAction::Cancel,

            _ => MissingFilePreviewAction::None,
        }
    }

    fn max_scroll_for_list(&self, list_idx: usize) -> usize {
        let count = if list_idx == 0 {
            self.cached_data.restorable.len()
        } else {
            self.cached_data.non_restorable.len()
        };
        count.saturating_sub(1)
    }

    /// Render the missing file resolution modal.
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
        let restorable = self.cached_data.restorable.len();
        let non_restorable = self.cached_data.non_restorable.len();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing File Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} total: {} restorable, {} lost)", total, restorable, non_restorable),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        // Split into two panes for the two lists
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        self.render_restorable_list(f, chunks[0]);
        self.render_non_restorable_list(f, chunks[1]);
    }

    fn render_restorable_list(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focused_list == 0;
        let count = self.cached_data.restorable.len();

        let border_style = if focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = format!(" Restorable from Library ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 { Color::Green } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        // Populate click targets for restorable list
        self.click_targets_restorable.clear();
        self.click_targets_restorable.set_list_area(inner);
        let visible_height = inner.height as usize;
        for (vis_idx, entry_idx) in (self.scroll[0]..).take(visible_height).enumerate() {
            if entry_idx >= self.cached_data.restorable.len() { break; }
            self.click_targets_restorable.add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
        }

        if self.cached_data.restorable.is_empty() {
            let empty = Paragraph::new("No restorable files")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.scroll[0], self.scroll[0]);
    }

    fn render_non_restorable_list(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focused_list == 1;
        let count = self.cached_data.non_restorable.len();

        let border_style = if focused {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = format!(" Non-Restorable / Data Lost ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 { Color::Red } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        // Populate click targets for non-restorable list
        self.click_targets_non_restorable.clear();
        self.click_targets_non_restorable.set_list_area(inner);
        let visible_height = inner.height as usize;
        for (vis_idx, entry_idx) in (self.scroll[1]..).take(visible_height).enumerate() {
            if entry_idx >= self.cached_data.non_restorable.len() { break; }
            self.click_targets_non_restorable.add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
        }

        if self.cached_data.non_restorable.is_empty() {
            let empty = Paragraph::new("No non-restorable files")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .non_restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.scroll[1], self.scroll[1]);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let has_restorable = self.cached_data.has_restorable();

        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        let button_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .split(inner);

        // Track button rects for click detection
        self.button_rects.clear();
        self.button_rects.set("restore_all", button_chunks[0]);
        self.button_rects.set("drop_lost", button_chunks[1]);
        self.button_rects.set("cancel", button_chunks[2]);

        // Restore All button
        let restore_style = if !has_restorable {
            Style::default().fg(Color::DarkGray)
        } else if self.selected_button == SelectedButton::RestoreAll {
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        let restore_text = Paragraph::new(" Restore All ")
            .style(restore_style)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(restore_text, button_chunks[0]);

        // Drop Missing button
        let drop_style = if self.selected_button == SelectedButton::DropLost {
            Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };
        let drop_text = Paragraph::new(" Drop Missing ")
            .style(drop_style)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(drop_text, button_chunks[1]);

        // Cancel button
        let cancel_style = if self.selected_button == SelectedButton::Cancel {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let cancel_text = Paragraph::new(" Cancel ")
            .style(cancel_style)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(cancel_text, button_chunks[2]);
    }
}
