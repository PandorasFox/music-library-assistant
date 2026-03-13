//! Directory Overlap Cluster Resolution Preview UI
//!
//! Shows directory overlap clusters with inline action buttons per directory row
//! and a stash file preview list, allowing the operator to see exactly which
//! files will be stashed before confirming.
//!
//! Navigation:
//! - Up/Down: Navigate rows (directory rows + "mark expected" row)
//! - Left/Right: Switch between buttons within a directory row
//! - Shift+Up/Down: Toggle focus between DirectoryList and FileList panes
//! - Tab/Shift+Tab: Navigate between clusters (stage decision and advance)
//! - Enter: Confirm selected option for current cluster
//! - Ctrl+R: Jump to transaction review
//! - Escape: Cancel

use super::types::DirectoryClusterModalDataExt;
use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{ClusterResolutionOption, DirectoryClusterModalData, StashFileEntry};
use crate::ui::helpers::{render_pane, truncate_left, truncate_right};
use crate::ui::widgets::file_path_list::{render_file_path_list, PathEntry};
use crate::ui::widgets::{rect_contains, ListClickTargets, PathField, CURSOR_STYLE};

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    DirectoryList,
    FileList,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::DirectoryList => Self::FileList,
            Self::FileList => Self::FileList,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::DirectoryList => Self::DirectoryList,
            Self::FileList => Self::DirectoryList,
        }
    }
}

/// Actions returned from the directory cluster preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryClusterPreviewAction {
    /// No action needed.
    None,
    /// User confirmed resolution for current cluster.
    ConfirmCurrent,
    /// Navigate to next cluster (Tab).
    NavigateNext,
    /// Navigate to previous cluster (Shift+Tab).
    NavigatePrev,
    /// Jump to transaction review (Ctrl+R).
    ShowReview,
    /// Mark current cluster's source pair as expected overlap (Ctrl+F flag).
    MarkExpected,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the directory cluster resolution modal.
#[derive(Debug)]
pub struct DirectoryClusterPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: DirectoryClusterModalData,
    /// Current cluster index.
    pub current_cluster_index: usize,
    /// Selected row index: 0..N-1 = directory rows, N = actions row.
    pub selected_row: usize,
    /// Selected button within a row.
    /// Directory rows: 0 = edit tags, 1 = stash dir.
    /// Actions row: 0 = auto quality (if available) or mark expected, 1 = mark expected (if auto quality present).
    pub selected_button: usize,
    /// Which pane has focus.
    pub focus_pane: FocusPane,
    /// Files that would be stashed by the currently selected option.
    pub stash_files: Vec<StashFileEntry>,
    /// Cursor position within the stash file list.
    pub file_cursor: usize,
    /// Scroll offset for the stash file list.
    pub file_scroll: usize,
    /// Click targets for inline buttons: (rect, row_index, button_index).
    pub button_rects: Vec<(Rect, usize, usize)>,
    /// Stored directory pane Rect for pane-level focus detection.
    pub dir_pane_rect: Option<Rect>,
    /// Click targets for stash file list items (set during render).
    pub file_click_targets: ListClickTargets,
    /// Stored file list pane Rect for pane-level focus detection.
    pub file_list_pane_rect: Option<Rect>,
}

impl DirectoryClusterPreviewState {
    /// Path of the current cluster's first directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .clusters
            .get(self.current_cluster_index)
            .and_then(|c| c.directories.first())
            .map(|d| d.path_suffix.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: DirectoryClusterModalData) -> Self {
        let stash_files = Vec::new();

        let mut state = Self {
            cached_data,
            current_cluster_index: 0,
            selected_row: 0,
            selected_button: 0,
            focus_pane: FocusPane::DirectoryList,
            stash_files,
            file_cursor: 0,
            file_scroll: 0,
            button_rects: Vec::new(),
            dir_pane_rect: None,
            file_click_targets: ListClickTargets::new(),
            file_list_pane_rect: None,
        };
        state.update_for_current_cluster();
        state
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<DirectoryClusterPreviewAction> {
        // Check inline button rects (covers directory rows AND actions row)
        for &(rect, row, btn) in &self.button_rects {
            if rect_contains(rect, x, y) {
                self.focus_pane = FocusPane::DirectoryList;
                self.selected_row = row;
                self.selected_button = btn;
                self.clamp_button();
                self.recompute_stash_files();
                return None;
            }
        }
        // Check file list items
        if let Some(id) = self.file_click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.stash_files.len() {
                    self.focus_pane = FocusPane::FileList;
                    self.file_cursor = idx;
                }
            }
            return None;
        }
        // Pane-level focus detection
        if let Some(rect) = self.dir_pane_rect {
            if rect_contains(rect, x, y) {
                self.focus_pane = FocusPane::DirectoryList;
                return None;
            }
        }
        if let Some(rect) = self.file_list_pane_rect {
            if rect_contains(rect, x, y) {
                self.focus_pane = FocusPane::FileList;
                return None;
            }
        }
        None
    }

    /// Update state when cluster changes: reset row/button, recompute stash files.
    fn update_for_current_cluster(&mut self) {
        self.selected_row = 0;
        self.selected_button = 0;
        self.recompute_stash_files();
    }

    /// Recompute the stash file list from the currently selected option.
    fn recompute_stash_files(&mut self) {
        self.stash_files = if let Some(option) = self.selected_option() {
            self.cached_data
                .stash_files_for_option(self.current_cluster_index, &option)
        } else {
            Vec::new()
        };
        self.file_cursor = 0;
        self.file_scroll = 0;
    }

    /// Get the currently selected resolution option (owned, computed from row+button).
    pub fn selected_option(&self) -> Option<ClusterResolutionOption> {
        let cluster = self.cached_data.clusters.get(self.current_cluster_index)?;
        let dir_count = cluster.directories.len();

        if self.selected_row >= dir_count {
            // Actions row
            let has_auto_q = self.auto_quality_format().is_some();
            if has_auto_q {
                match self.selected_button {
                    0 => {
                        let stash_format = self.auto_quality_format().unwrap();
                        return Some(ClusterResolutionOption::AutoQuality { stash_format });
                    }
                    _ => return Some(ClusterResolutionOption::MarkExpected),
                }
            } else {
                return Some(ClusterResolutionOption::MarkExpected);
            }
        }

        let dir = &cluster.directories[self.selected_row];
        match self.selected_button {
            0 => Some(ClusterResolutionOption::EditTags {
                dir_suffix: dir.path_suffix.clone(),
                inodes: dir.inodes.clone(),
            }),
            1 => {
                if dir.can_stash_dupes {
                    Some(ClusterResolutionOption::StashDirectory {
                        stash_suffix: dir.path_suffix.clone(),
                    })
                } else {
                    // Disabled — shouldn't be reachable, but return EditTags as fallback
                    Some(ClusterResolutionOption::EditTags {
                        dir_suffix: dir.path_suffix.clone(),
                        inodes: dir.inodes.clone(),
                    })
                }
            }
            _ => None,
        }
    }

    /// Compute the auto-quality stash format for the current cluster, if applicable.
    ///
    /// Returns the worst format string when the cluster has directories with
    /// different dominant formats and the worst-format directories can all be stashed.
    fn auto_quality_format(&self) -> Option<String> {
        let cluster = self.cached_data.clusters.get(self.current_cluster_index)?;

        let formats: Vec<&str> = cluster
            .directories
            .iter()
            .map(|d| d.format_summary.split_whitespace().next().unwrap_or(""))
            .collect();

        let unique_formats: std::collections::HashSet<&str> = formats.iter().copied().collect();
        if unique_formats.len() <= 1 {
            return None;
        }

        let format_rank = |f: &str| -> u8 {
            match f {
                "FLAC" => 4,
                "OPUS" => 3,
                "OGG" => 2,
                "MP3" => 1,
                _ => 0,
            }
        };

        let best_format = unique_formats.iter().max_by_key(|f| format_rank(f))?;
        let worst_format = unique_formats.iter().min_by_key(|f| format_rank(f))?;

        if best_format == worst_format || worst_format.is_empty() {
            return None;
        }

        // Only offer if worst-format directories can all be stashed
        let stashable = cluster
            .directories
            .iter()
            .filter(|d| d.format_summary.starts_with(*worst_format))
            .all(|d| d.can_stash_dupes);

        if stashable {
            Some((*worst_format).to_string())
        } else {
            None
        }
    }

    /// Get the current cluster.
    pub fn current_cluster(&self) -> Option<&super::types::DirectoryClusterEntry> {
        self.cached_data.clusters.get(self.current_cluster_index)
    }

    /// Navigate to the next cluster.
    pub fn navigate_next(&mut self) -> bool {
        if self.current_cluster_index + 1 < self.cached_data.clusters.len() {
            self.current_cluster_index += 1;
            self.update_for_current_cluster();
            true
        } else {
            false
        }
    }

    /// Navigate to the previous cluster.
    pub fn navigate_prev(&mut self) -> bool {
        if self.current_cluster_index > 0 {
            self.current_cluster_index -= 1;
            self.update_for_current_cluster();
            true
        } else {
            false
        }
    }

    /// The row index of the actions row (after all directory rows).
    fn actions_row_index(&self) -> usize {
        self.cached_data
            .clusters
            .get(self.current_cluster_index)
            .map(|c| c.directories.len())
            .unwrap_or(0)
    }

    /// Total number of navigable rows (directory rows + mark expected).
    fn total_rows(&self) -> usize {
        self.actions_row_index() + 1
    }

    /// Whether the given directory row can stash.
    fn can_stash_row(&self, row: usize) -> bool {
        self.cached_data
            .clusters
            .get(self.current_cluster_index)
            .and_then(|c| c.directories.get(row))
            .map(|d| d.can_stash_dupes)
            .unwrap_or(false)
    }

    /// Number of navigable buttons on the actions row.
    fn actions_row_button_count(&self) -> usize {
        if self.auto_quality_format().is_some() {
            2
        } else {
            1
        }
    }

    /// Clamp selected_button so it doesn't land on a disabled or nonexistent button.
    fn clamp_button(&mut self) {
        let actions_row = self.actions_row_index();
        if self.selected_row < actions_row {
            // On a directory row: clamp to edit tags if stash is disabled
            if self.selected_button == 1 && !self.can_stash_row(self.selected_row) {
                self.selected_button = 0;
            }
        } else {
            // On actions row: clamp to available button count
            let max_btn = self.actions_row_button_count().saturating_sub(1);
            if self.selected_button > max_btn {
                self.selected_button = max_btn;
            }
        }
    }

    /// Handle semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> DirectoryClusterPreviewAction {
        match action {
            // Shift+Up/Down: move focus between panes
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                DirectoryClusterPreviewAction::None
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                DirectoryClusterPreviewAction::None
            }

            // Ctrl+R: show review
            InputAction::Shortcut('r') => DirectoryClusterPreviewAction::ShowReview,

            // Ctrl+F: mark expected overlap
            InputAction::FlagValue => DirectoryClusterPreviewAction::MarkExpected,

            // Navigate rows (only when directory list focused)
            InputAction::NavUp if self.focus_pane == FocusPane::DirectoryList => {
                if self.selected_row > 0 {
                    self.selected_row -= 1;
                    self.clamp_button();
                    self.recompute_stash_files();
                }
                DirectoryClusterPreviewAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::DirectoryList => {
                if self.selected_row + 1 < self.total_rows() {
                    self.selected_row += 1;
                    self.clamp_button();
                    self.recompute_stash_files();
                }
                DirectoryClusterPreviewAction::None
            }

            // Switch buttons within a row (directory rows and actions row)
            InputAction::NavLeft if self.focus_pane == FocusPane::DirectoryList => {
                if self.selected_button > 0 {
                    self.selected_button -= 1;
                    self.recompute_stash_files();
                }
                DirectoryClusterPreviewAction::None
            }
            InputAction::NavRight if self.focus_pane == FocusPane::DirectoryList => {
                let actions_row = self.actions_row_index();
                if self.selected_row < actions_row {
                    // Directory row: can go right to stash if enabled
                    if self.selected_button == 0 && self.can_stash_row(self.selected_row) {
                        self.selected_button = 1;
                        self.recompute_stash_files();
                    }
                } else {
                    // Actions row: can go right if more buttons available
                    let max_btn = self.actions_row_button_count().saturating_sub(1);
                    if self.selected_button < max_btn {
                        self.selected_button += 1;
                        self.recompute_stash_files();
                    }
                }
                DirectoryClusterPreviewAction::None
            }

            // Navigate file list (when file list focused)
            InputAction::NavUp if self.focus_pane == FocusPane::FileList => {
                self.file_cursor = self.file_cursor.saturating_sub(1);
                DirectoryClusterPreviewAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::FileList => {
                if self.file_cursor + 1 < self.stash_files.len() {
                    self.file_cursor += 1;
                }
                DirectoryClusterPreviewAction::None
            }

            // Tab / Shift+Tab: navigate between clusters
            InputAction::CycleNext => DirectoryClusterPreviewAction::NavigateNext,
            InputAction::CyclePrev => DirectoryClusterPreviewAction::NavigatePrev,

            // Enter: confirm current option
            InputAction::Confirm => DirectoryClusterPreviewAction::ConfirmCurrent,

            // Cancel
            InputAction::Cancel => DirectoryClusterPreviewAction::Cancel,

            _ => DirectoryClusterPreviewAction::None,
        }
    }

    /// Render the directory cluster resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Layout: title + directory table (capped) + stash preview (fills) + hints
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),      // Title
                Constraint::Percentage(25), // Directory table with inline buttons
                Constraint::Min(5),         // Stash file preview
                Constraint::Length(2),      // Hints bar
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_directory_table(f, main_chunks[1]);
        self.render_stash_preview(f, main_chunks[2]);
        self.render_hints(f, main_chunks[3]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let total = self.cached_data.total_count();
        let current = self.current_cluster_index + 1;

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Directory Overlap Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({}/{}) ", current, total),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_directory_table(&mut self, f: &mut Frame, area: Rect) {
        let dir_focused = self.focus_pane == FocusPane::DirectoryList;
        let cluster_idx = self.current_cluster_index;

        let title = match self.cached_data.clusters.get(cluster_idx) {
            Some(c) => {
                let plural = if c.overlap_count == 1 { "" } else { "s" };
                format!(
                    " {} \u{2014} {} overlap{} ",
                    c.cluster_key, c.overlap_count, plural
                )
            }
            None => " Overlapping Directories ".to_string(),
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if dir_focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if dir_focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));

        let inner = render_pane(f, area, block);
        self.dir_pane_rect = Some(area);
        self.button_rects.clear();

        let Some(cluster) = self.cached_data.clusters.get(cluster_idx) else {
            let empty = Paragraph::new("No clusters to display")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        };

        let dir_count = cluster.directories.len();

        // Compute column widths from available space
        // Layout: path | tracks | format | [edit tags] [stash dir]
        let avail_w = inner.width as usize;
        let tracks_w = 10; // " 23 tracks"
        let format_w = 12; // "FLAC (23)   "
        let btn_w = 24; // "[edit tags] [stash dir]"
        let path_w = avail_w.saturating_sub(tracks_w + format_w + btn_w + 2);

        let mut y = inner.y;
        for (row_idx, dir) in cluster.directories.iter().enumerate() {
            if y >= inner.y + inner.height {
                break;
            }

            let is_selected_row = dir_focused && self.selected_row == row_idx;

            // Build the row content: path | tracks | format
            let row_line = Line::from(vec![
                Span::styled(
                    format!(
                        " {:<width$}",
                        truncate_left(&dir.path_suffix, path_w.saturating_sub(1)),
                        width = path_w
                    ),
                    if is_selected_row {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::styled(
                    format!("{:>3} tracks", dir.inodes.len()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!(
                        "  {:<width$}",
                        truncate_right(&dir.format_summary, format_w.saturating_sub(2)),
                        width = format_w
                    ),
                    Style::default().fg(Color::Cyan),
                ),
            ]);

            // Render row text
            let row_area = Rect::new(inner.x, y, inner.width.saturating_sub(btn_w as u16), 1);
            f.render_widget(Paragraph::new(row_line), row_area);

            // Render inline buttons
            let btn_x = inner.x + inner.width.saturating_sub(btn_w as u16);

            // [edit tags] button
            let edit_btn_text = "[edit tags]";
            let edit_btn_w = edit_btn_text.len() as u16;
            let edit_btn_rect = Rect::new(btn_x, y, edit_btn_w, 1);
            let edit_btn_style = if is_selected_row && self.selected_button == 0 {
                CURSOR_STYLE
            } else {
                Style::default().fg(Color::Green)
            };
            f.render_widget(
                Paragraph::new(Span::styled(edit_btn_text, edit_btn_style)),
                edit_btn_rect,
            );
            self.button_rects.push((edit_btn_rect, row_idx, 0));

            // [stash dir] button
            let stash_btn_text = " [stash dir]";
            let stash_btn_w = stash_btn_text.len() as u16;
            let stash_btn_rect = Rect::new(btn_x + edit_btn_w, y, stash_btn_w, 1);
            let stash_btn_style = if !dir.can_stash_dupes {
                Style::default().fg(Color::DarkGray) // Disabled
            } else if is_selected_row && self.selected_button == 1 {
                CURSOR_STYLE
            } else {
                Style::default().fg(Color::Yellow)
            };
            f.render_widget(
                Paragraph::new(Span::styled(stash_btn_text, stash_btn_style)),
                stash_btn_rect,
            );
            if dir.can_stash_dupes {
                self.button_rects.push((stash_btn_rect, row_idx, 1));
            }

            y += 1;
        }

        // Actions row: [auto quality] (if available) + [mark expected]
        if y < inner.y + inner.height {
            let is_actions_row = dir_focused && self.selected_row == dir_count;
            let auto_q_format = self.auto_quality_format();
            let mut btn_x = inner.x + 1; // indent

            if let Some(ref fmt) = auto_q_format {
                let auto_q_text = format!("[stash {} quality]", fmt);
                let auto_q_w = auto_q_text.len() as u16;
                let auto_q_rect = Rect::new(btn_x, y, auto_q_w, 1);
                let auto_q_style = if is_actions_row && self.selected_button == 0 {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(Color::Yellow)
                };
                f.render_widget(
                    Paragraph::new(Span::styled(&auto_q_text, auto_q_style)),
                    auto_q_rect,
                );
                self.button_rects.push((auto_q_rect, dir_count, 0));
                btn_x += auto_q_w + 1; // gap
            }

            let mark_btn_idx = if auto_q_format.is_some() { 1 } else { 0 };
            let mark_text = "[mark expected]";
            let mark_w = mark_text.len() as u16;
            let mark_rect = Rect::new(btn_x, y, mark_w, 1);
            let mark_style = if is_actions_row && self.selected_button == mark_btn_idx {
                CURSOR_STYLE
            } else {
                Style::default().fg(Color::Magenta)
            };
            f.render_widget(
                Paragraph::new(Span::styled(mark_text, mark_style)),
                mark_rect,
            );
            self.button_rects.push((mark_rect, dir_count, mark_btn_idx));
        }
    }

    fn render_stash_preview(&mut self, f: &mut Frame, area: Rect) {
        let file_list_focused = self.focus_pane == FocusPane::FileList;
        let count = self.stash_files.len();

        // Show stash files when cursor is on a stash-producing button
        let show_files = if self.selected_row < self.actions_row_index() {
            // Directory row: stash dir button (button 1)
            self.selected_button == 1 && self.can_stash_row(self.selected_row)
        } else {
            // Actions row: auto quality button (button 0, only when auto quality available)
            self.auto_quality_format().is_some() && self.selected_button == 0
        };

        let title = if show_files {
            format!(" Files to Stash ({}) ", count)
        } else {
            " Files to Stash ".to_string()
        };

        if file_list_focused {
            // Split 66/34: file list on left, detail pane on right
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
                .split(area);

            // File list pane (focused)
            let list_block = Block::default()
                .title(title)
                .title_style(Style::default().fg(Color::Cyan))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));

            let inner = render_pane(f, chunks[0], list_block);
            if show_files {
                self.render_file_list(f, inner);
            } else {
                self.render_empty_stash(f, inner);
            }

            // Detail pane
            if show_files {
                self.render_file_detail(f, chunks[1]);
            } else {
                let detail_block = Block::default()
                    .title(" Details ")
                    .title_style(Style::default().fg(Color::DarkGray))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray));
                let detail_inner = render_pane(f, chunks[1], detail_block);
                f.render_widget(
                    Paragraph::new("Select a stash button to preview files")
                        .style(Style::default().fg(Color::DarkGray)),
                    detail_inner,
                );
            }
        } else {
            // Full-width file list (unfocused)
            let block = Block::default()
                .title(title)
                .title_style(Style::default().fg(Color::DarkGray))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));

            let inner = render_pane(f, area, block);
            if show_files {
                self.render_file_list(f, inner);
            } else {
                self.render_empty_stash(f, inner);
            }
        }
    }

    fn render_empty_stash(&self, f: &mut Frame, area: Rect) {
        let msg = if self.selected_row >= self.actions_row_index()
            && self.selected_option() == Some(ClusterResolutionOption::MarkExpected)
        {
            "Mark expected does not stash files"
        } else {
            "Select a stash button to preview files"
        };
        let empty = Paragraph::new(msg).style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, area);
    }

    fn render_file_list(&mut self, f: &mut Frame, area: Rect) {
        // Store pane rect for focus detection + populate click targets
        self.file_list_pane_rect = Some(area);
        self.file_click_targets.populate(area, self.file_scroll, self.stash_files.len());

        if self.stash_files.is_empty() {
            let empty =
                Paragraph::new("No files to stash").style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, area);
            return;
        }

        let entries: Vec<PathEntry> = self
            .stash_files
            .iter()
            .map(|sf| PathEntry::plain(&sf.corpus_path))
            .collect();

        render_file_path_list(f, area, &entries, self.file_cursor, self.file_scroll);
    }

    fn render_file_detail(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Details ")
            .title_style(Style::default().fg(Color::DarkGray))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = render_pane(f, area, block);

        let file = self.stash_files.get(self.file_cursor);
        let max_lines = inner.height as usize;

        let label_style = Style::default().fg(Color::DarkGray);
        let value_style = Style::default().fg(Color::White);

        let mut lines = Vec::new();

        let Some(file) = file else {
            lines.push(Line::from(Span::styled(
                "No file selected",
                Style::default().fg(Color::DarkGray),
            )));
            let para = Paragraph::new(lines);
            f.render_widget(para, inner);
            return;
        };

        // Path
        lines.extend(
            PathField::new(Span::styled("Path: ", label_style), &file.corpus_path)
                .style(value_style)
                .render_lines(inner.width),
        );

        // Audio metadata from cache
        if let Some(meta) = self.cached_data.file_meta_cache.get(&file.inode) {
            lines.extend(crate::ui::helpers::render_audio_metadata_lines(meta, label_style, value_style));

            // Tags section
            if !meta.tags.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Tags:",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));

                let tag_budget = max_lines.saturating_sub(lines.len());
                for (name, value) in meta.tags.iter().take(tag_budget) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {}: ", name), label_style),
                        Span::styled(truncate_right(value, 30), value_style),
                    ]));
                }
                let remaining = meta.tags.len().saturating_sub(tag_budget);
                if remaining > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  ... +{} more", remaining),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }

    fn render_hints(&self, f: &mut Frame, area: Rect) {
        let hints = Line::from(vec![
            Span::styled(" \u{2191}\u{2193}", Style::default().fg(Color::Cyan)),
            Span::styled(" select ", Style::default().fg(Color::DarkGray)),
            Span::styled(" \u{2190}\u{2192}", Style::default().fg(Color::Cyan)),
            Span::styled(" button ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                " \u{21e7}\u{2191}\u{2193}",
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(" pane ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Tab", Style::default().fg(Color::Cyan)),
            Span::styled(" next ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" confirm ", Style::default().fg(Color::DarkGray)),
            Span::styled(" ^R", Style::default().fg(Color::Cyan)),
            Span::styled(" review ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]);

        let controls = Paragraph::new(hints);
        f.render_widget(controls, area);
    }
}
