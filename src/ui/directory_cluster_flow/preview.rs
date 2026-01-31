//! Directory Overlap Cluster Resolution Preview UI
//!
//! Shows directory overlap clusters with resolution options, allowing
//! the operator to choose which directory to keep for each cluster.
//!
//! Navigation:
//! - Up/Down: Navigate resolution options
//! - Tab/Shift+Tab: Navigate between clusters (stage decision and advance)
//! - Enter: Confirm selected option for current cluster
//! - Ctrl+R: Jump to transaction review
//! - Escape: Cancel

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::types::{ClusterResolutionOption, DirectoryClusterModalData, SelectedButton};
use crate::ui::helpers::truncate_left;

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Options,
    Buttons,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::Options => Self::Buttons,
            Self::Buttons => Self::Buttons,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Options => Self::Options,
            Self::Buttons => Self::Options,
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
    /// Selected resolution option index for current cluster.
    pub selected_option_index: usize,
    /// Resolution options for current cluster (rebuilt when cluster changes).
    pub current_options: Vec<ClusterResolutionOption>,
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Which pane has focus.
    pub focus_pane: FocusPane,
}

impl DirectoryClusterPreviewState {
    /// Create a new preview state with cached data.
    pub fn new(cached_data: DirectoryClusterModalData) -> Self {
        let current_options = if cached_data.clusters.is_empty() {
            Vec::new()
        } else {
            Self::build_options_for_cluster(&cached_data.clusters[0])
        };

        Self {
            cached_data,
            current_cluster_index: 0,
            selected_option_index: 0,
            current_options,
            selected_button: SelectedButton::Cancel,
            focus_pane: FocusPane::Options,
        }
    }

    /// Build resolution options for a cluster.
    fn build_options_for_cluster(
        cluster: &super::types::DirectoryClusterEntry,
    ) -> Vec<ClusterResolutionOption> {
        let mut options = Vec::new();

        // Check if we can offer auto-quality option
        // (different dominant formats across directories)
        let formats: Vec<&str> = cluster
            .directories
            .iter()
            .map(|d| {
                // Extract dominant format from summary (e.g., "FLAC" from "FLAC (3)")
                d.format_summary.split_whitespace().next().unwrap_or("")
            })
            .collect();

        let unique_formats: std::collections::HashSet<&str> = formats.iter().copied().collect();
        if unique_formats.len() > 1 {
            // Rank formats: FLAC > OPUS > OGG > MP3 > others
            let format_rank = |f: &str| -> u8 {
                match f {
                    "FLAC" => 4,
                    "OPUS" => 3,
                    "OGG" => 2,
                    "MP3" => 1,
                    _ => 0,
                }
            };

            let best_format = unique_formats.iter().max_by_key(|f| format_rank(f)).unwrap_or(&"");
            let worst_format = unique_formats.iter().min_by_key(|f| format_rank(f)).unwrap_or(&"");

            if !best_format.is_empty() && best_format != worst_format {
                options.push(ClusterResolutionOption::AutoQuality {
                    keep_format: (*best_format).to_string(),
                    stash_format: (*worst_format).to_string(),
                });
            }
        }

        // Add per-directory options
        for dir in &cluster.directories {
            options.push(ClusterResolutionOption::KeepDirectory {
                keep_suffix: dir.path_suffix.clone(),
            });
        }

        // Always add skip option
        options.push(ClusterResolutionOption::Skip);

        options
    }

    /// Update options when cluster changes.
    fn update_options_for_current_cluster(&mut self) {
        self.current_options = if let Some(cluster) = self.cached_data.clusters.get(self.current_cluster_index) {
            Self::build_options_for_cluster(cluster)
        } else {
            Vec::new()
        };
        self.selected_option_index = 0;
    }

    /// Get the currently selected resolution option.
    pub fn selected_option(&self) -> Option<&ClusterResolutionOption> {
        self.current_options.get(self.selected_option_index)
    }

    /// Get the current cluster.
    pub fn current_cluster(&self) -> Option<&super::types::DirectoryClusterEntry> {
        self.cached_data.clusters.get(self.current_cluster_index)
    }

    /// Navigate to the next cluster.
    pub fn navigate_next(&mut self) -> bool {
        if self.current_cluster_index + 1 < self.cached_data.clusters.len() {
            self.current_cluster_index += 1;
            self.update_options_for_current_cluster();
            true
        } else {
            false
        }
    }

    /// Navigate to the previous cluster.
    pub fn navigate_prev(&mut self) -> bool {
        if self.current_cluster_index > 0 {
            self.current_cluster_index -= 1;
            self.update_options_for_current_cluster();
            true
        } else {
            false
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> DirectoryClusterPreviewAction {
        let has_options = !self.current_options.is_empty();

        // Shift+Up/Down: move focus between panes
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return DirectoryClusterPreviewAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return DirectoryClusterPreviewAction::None;
                }
                _ => {}
            }
        }

        // Ctrl+R: show review
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('r') {
                return DirectoryClusterPreviewAction::ShowReview;
            }
        }

        match key.code {
            // Navigate options (only when options focused)
            KeyCode::Up | KeyCode::Char('k') if self.focus_pane == FocusPane::Options => {
                if self.selected_option_index > 0 {
                    self.selected_option_index -= 1;
                }
                DirectoryClusterPreviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.focus_pane == FocusPane::Options => {
                if self.selected_option_index + 1 < self.current_options.len() {
                    self.selected_option_index += 1;
                }
                DirectoryClusterPreviewAction::None
            }

            // Button navigation (when buttons focused)
            KeyCode::Left | KeyCode::Char('h') if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.left(has_options);
                DirectoryClusterPreviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') if self.focus_pane == FocusPane::Buttons => {
                self.selected_button.right(has_options);
                DirectoryClusterPreviewAction::None
            }

            // Tab: navigate to next cluster
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    DirectoryClusterPreviewAction::NavigatePrev
                } else {
                    DirectoryClusterPreviewAction::NavigateNext
                }
            }
            KeyCode::BackTab => DirectoryClusterPreviewAction::NavigatePrev,

            // Enter: confirm (context-dependent)
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        SelectedButton::Confirm => DirectoryClusterPreviewAction::ConfirmCurrent,
                        SelectedButton::Cancel => DirectoryClusterPreviewAction::Cancel,
                    }
                } else {
                    // Enter on options pane = confirm current option
                    DirectoryClusterPreviewAction::ConfirmCurrent
                }
            }

            // Cancel
            KeyCode::Esc => DirectoryClusterPreviewAction::Cancel,

            _ => DirectoryClusterPreviewAction::None,
        }
    }

    /// Render the directory cluster resolution modal.
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

    fn render_content(&self, f: &mut Frame, area: Rect) {
        let options_focused = self.focus_pane == FocusPane::Options;

        // Split into directories pane (left) and options pane (right)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60), // Directories
                Constraint::Percentage(40), // Options
            ])
            .split(area);

        self.render_directories_pane(f, chunks[0]);
        self.render_options_pane(f, chunks[1], options_focused);
    }

    fn render_directories_pane(&self, f: &mut Frame, area: Rect) {
        let cluster = self.current_cluster();

        let block = Block::default()
            .title(" Overlapping Directories ")
            .title_style(Style::default().fg(Color::Cyan))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if cluster.is_none() {
            let empty = Paragraph::new("No clusters to display")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let cluster = cluster.unwrap();

        // Render directory entries
        let items: Vec<ListItem> = cluster
            .directories
            .iter()
            .map(|dir| {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{:<30}", truncate_left(&dir.path_suffix, 28)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("  {:>3} tracks  ", dir.track_ids.len()),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("{:<10}", dir.format_summary),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:>8.1} MB", dir.total_size_mb),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, inner);
    }

    fn render_options_pane(&self, f: &mut Frame, area: Rect, focused: bool) {
        let block = Block::default()
            .title(" Resolution ")
            .title_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.current_options.is_empty() {
            let empty = Paragraph::new("No options available")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Render options as radio buttons
        let items: Vec<ListItem> = self
            .current_options
            .iter()
            .enumerate()
            .map(|(idx, option)| {
                let is_selected = idx == self.selected_option_index;
                let marker = if is_selected { "(•)" } else { "( )" };
                let style = if is_selected && focused {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };

                let line = Line::from(vec![
                    Span::styled(format!("{} ", marker), style),
                    Span::styled(option.label(), style),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, inner);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_options = !self.current_options.is_empty();
        let buttons_focused = self.focus_pane == FocusPane::Buttons;

        // Build button line
        let mut buttons = Vec::new();

        // Confirm button
        let confirm_style = if !has_options {
            Style::default().fg(Color::DarkGray)
        } else if buttons_focused && self.selected_button == SelectedButton::Confirm {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        buttons.push(Span::styled(" Confirm ", confirm_style));
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
            "[↑↓ select] [Tab next] [Ctrl+R review] [Enter confirm]",
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
