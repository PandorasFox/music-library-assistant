//! Directory Overlap Cluster Resolution Preview UI
//!
//! Shows directory overlap clusters with resolution options and a stash file
//! preview list, allowing the operator to see exactly which files will be
//! stashed before confirming.
//!
//! Navigation:
//! - Up/Down: Navigate resolution options (Options pane) or files (FileList pane)
//! - Shift+Up/Down: Toggle focus between Options and FileList panes
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

use super::types::{ClusterResolutionOption, DirectoryClusterModalData, StashFileEntry};
use crate::ui::helpers::{render_pane, truncate_left, truncate_right};
use crate::ui::widgets::file_path_list::{render_file_path_list, PathEntry};
use crate::ui::widgets::{PathField, CURSOR_STYLE};

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Options,
    FileList,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::Options => Self::FileList,
            Self::FileList => Self::FileList,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Options => Self::Options,
            Self::FileList => Self::Options,
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
    /// Mark current cluster's source pair as expected overlap (Ctrl+F).
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
    /// Selected resolution option index for current cluster.
    pub selected_option_index: usize,
    /// Resolution options for current cluster (rebuilt when cluster changes).
    pub current_options: Vec<ClusterResolutionOption>,
    /// Which pane has focus.
    pub focus_pane: FocusPane,
    /// Files that would be stashed by the currently selected option.
    pub stash_files: Vec<StashFileEntry>,
    /// Cursor position within the stash file list.
    pub file_cursor: usize,
    /// Scroll offset for the stash file list.
    pub file_scroll: usize,
}

impl DirectoryClusterPreviewState {
    /// Path of the current cluster's first directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data.clusters
            .get(self.current_cluster_index)
            .and_then(|c| c.directories.first())
            .map(|d| d.path_suffix.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: DirectoryClusterModalData) -> Self {
        let current_options = if cached_data.clusters.is_empty() {
            Vec::new()
        } else {
            Self::build_options_for_cluster(&cached_data.clusters[0])
        };

        let stash_files = if !current_options.is_empty() {
            cached_data.stash_files_for_option(0, &current_options[0])
        } else {
            Vec::new()
        };

        Self {
            cached_data,
            current_cluster_index: 0,
            selected_option_index: 0,
            current_options,
            focus_pane: FocusPane::Options,
            stash_files,
            file_cursor: 0,
            file_scroll: 0,
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
                // AutoQuality stashes directories with the worst format — only
                // offer if those directories allow stashing.
                let stashable = cluster.directories.iter()
                    .filter(|d| d.format_summary.starts_with(worst_format))
                    .all(|d| d.can_stash_dupes);
                if stashable {
                    options.push(ClusterResolutionOption::AutoQuality {
                        stash_format: (*worst_format).to_string(),
                    });
                }
            }
        }

        // Add per-directory options: "Stash X" is available when X allows stashing.
        for dir in &cluster.directories {
            if dir.can_stash_dupes {
                options.push(ClusterResolutionOption::StashDirectory {
                    stash_suffix: dir.path_suffix.clone(),
                });
            }
        }

        // Always offer "Mark expected" as the last option
        options.push(ClusterResolutionOption::MarkExpected);

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
        self.recompute_stash_files();
    }

    /// Recompute the stash file list from the currently selected option.
    fn recompute_stash_files(&mut self) {
        self.stash_files = if let Some(option) = self.current_options.get(self.selected_option_index) {
            self.cached_data.stash_files_for_option(self.current_cluster_index, option)
        } else {
            Vec::new()
        };
        self.file_cursor = 0;
        self.file_scroll = 0;
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            return DirectoryClusterPreviewAction::ShowReview;
        }

        // Ctrl+F: mark expected overlap
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f') {
            return DirectoryClusterPreviewAction::MarkExpected;
        }

        match key.code {
            // Navigate options (only when options focused)
            KeyCode::Up if self.focus_pane == FocusPane::Options => {
                if self.selected_option_index > 0 {
                    self.selected_option_index -= 1;
                    self.recompute_stash_files();
                }
                DirectoryClusterPreviewAction::None
            }
            KeyCode::Down if self.focus_pane == FocusPane::Options => {
                if self.selected_option_index + 1 < self.current_options.len() {
                    self.selected_option_index += 1;
                    self.recompute_stash_files();
                }
                DirectoryClusterPreviewAction::None
            }

            // Navigate file list (when file list focused)
            KeyCode::Up if self.focus_pane == FocusPane::FileList => {
                self.file_cursor = self.file_cursor.saturating_sub(1);
                DirectoryClusterPreviewAction::None
            }
            KeyCode::Down if self.focus_pane == FocusPane::FileList => {
                if self.file_cursor + 1 < self.stash_files.len() {
                    self.file_cursor += 1;
                }
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

            // Enter: confirm current option
            KeyCode::Enter => DirectoryClusterPreviewAction::ConfirmCurrent,

            // Cancel
            KeyCode::Esc => DirectoryClusterPreviewAction::Cancel,

            _ => DirectoryClusterPreviewAction::None,
        }
    }

    /// Render the directory cluster resolution modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Layout: title + top panes (capped) + stash preview (fills) + hints
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),    // Title
                Constraint::Percentage(25), // Top panes (directories + options)
                Constraint::Min(5),       // Stash file preview
                Constraint::Length(2),    // Hints bar
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_top_panes(f, main_chunks[1]);
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

    fn render_top_panes(&self, f: &mut Frame, area: Rect) {
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

        let title = match cluster {
            Some(c) => {
                let plural = if c.overlap_count == 1 { "" } else { "s" };
                format!(" {} — {} overlap{} ", c.cluster_key, c.overlap_count, plural)
            }
            None => " Overlapping Directories ".to_string(),
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(Color::Cyan))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = render_pane(f, area, block);

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
                        format!("  {:>3} tracks  ", dir.inodes.len()),
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

        let inner = render_pane(f, area, block);

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
                    CURSOR_STYLE
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

    fn render_stash_preview(&self, f: &mut Frame, area: Rect) {
        let file_list_focused = self.focus_pane == FocusPane::FileList;
        let count = self.stash_files.len();
        let title = format!(" Files to Stash ({}) ", count);

        if file_list_focused {
            // Split 66/34: file list on left, detail pane on right
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(66),
                    Constraint::Percentage(34),
                ])
                .split(area);

            // File list pane (focused)
            let list_block = Block::default()
                .title(title)
                .title_style(Style::default().fg(Color::Cyan))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));

            let inner = render_pane(f, chunks[0], list_block);
            self.render_file_list(f, inner);

            // Detail pane
            self.render_file_detail(f, chunks[1]);
        } else {
            // Full-width file list (unfocused)
            let block = Block::default()
                .title(title)
                .title_style(Style::default().fg(Color::DarkGray))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));

            let inner = render_pane(f, area, block);
            self.render_file_list(f, inner);
        }
    }

    fn render_file_list(&self, f: &mut Frame, area: Rect) {
        if self.stash_files.is_empty() {
            let empty = Paragraph::new("No files to stash")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, area);
            return;
        }

        let entries: Vec<PathEntry> = self.stash_files
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
            lines.push(Line::from(""));

            lines.push(Line::from(vec![
                Span::styled("Format: ", label_style),
                Span::styled(meta.file_type.to_uppercase(), value_style),
            ]));

            if let Some(dur) = meta.duration_ms {
                let secs = dur / 1000;
                let mins = secs / 60;
                let rem = secs % 60;
                lines.push(Line::from(vec![
                    Span::styled("Duration: ", label_style),
                    Span::styled(format!("{}:{:02}", mins, rem), value_style),
                ]));
            }

            if let Some(br) = meta.bitrate_kbps {
                lines.push(Line::from(vec![
                    Span::styled("Bitrate: ", label_style),
                    Span::styled(format!("{} kbps", br), value_style),
                ]));
            }

            if let Some(sr) = meta.sample_rate {
                let display = if sr >= 1000 && sr % 1000 == 0 {
                    format!("{} kHz", sr / 1000)
                } else if sr >= 1000 {
                    format!("{:.1} kHz", sr as f64 / 1000.0)
                } else {
                    format!("{} Hz", sr)
                };
                lines.push(Line::from(vec![
                    Span::styled("Sample rate: ", label_style),
                    Span::styled(display, value_style),
                ]));
            }

            if meta.file_size > 0 {
                let size_str = if meta.file_size >= 1_048_576 {
                    format!("{:.1} MB", meta.file_size as f64 / 1_048_576.0)
                } else {
                    format!("{:.0} KB", meta.file_size as f64 / 1024.0)
                };
                lines.push(Line::from(vec![
                    Span::styled("Size: ", label_style),
                    Span::styled(size_str, value_style),
                ]));
            }

            let art_label = if meta.has_pictures { "Yes" } else { "No" };
            let art_style = if meta.has_pictures {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(vec![
                Span::styled("Album art: ", label_style),
                Span::styled(art_label, art_style),
            ]));

            // Tags section
            if !meta.tags.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Tags:",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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
            Span::styled(" \u{21e7}\u{2191}\u{2193}", Style::default().fg(Color::Cyan)),
            Span::styled(" pane ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Tab", Style::default().fg(Color::Cyan)),
            Span::styled(" next ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" confirm ", Style::default().fg(Color::DarkGray)),
            Span::styled(" ^R", Style::default().fg(Color::Cyan)),
            Span::styled(" review ", Style::default().fg(Color::DarkGray)),
            Span::styled(" ^F", Style::default().fg(Color::Cyan)),
            Span::styled(" expected ", Style::default().fg(Color::DarkGray)),
            Span::styled(" Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]);

        let controls = Paragraph::new(hints);
        f.render_widget(controls, area);
    }
}
