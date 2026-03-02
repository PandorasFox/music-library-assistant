//! Deployment Preview UI
//!
//! Shows a tabbed view of deploy signals with an info pane.
//! - Tab navigation with Left/Right arrows
//! - Scrollable list of signals sorted by path (non-interactable)
//! - Info pane showing context for selected signal type
//! - Enter stages mutations and goes directly to TransactionReview
//! - Escape returns to Insights view

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use std::path::Path;

use super::types::DeployModalData;
use crate::ui::widgets::{DeployTab, SidecarSummary, SignalInfo, SignalInfoPane, TabbedSignalList};

/// State for the deployment preview modal.
#[derive(Debug)]
pub struct DeploymentPreviewState {
    /// Currently active tab.
    pub active_tab: DeployTab,
    /// Scroll position per tab.
    pub tab_scroll: [usize; 5],
    /// Cached signal data (loaded once on init).
    pub cached_data: DeployModalData,
}

impl DeploymentPreviewState {
    /// Path of the currently selected item (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        let scroll = self.tab_scroll[self.active_tab.index()];
        match self.active_tab {
            DeployTab::Healthy => self.cached_data.healthy.get(scroll).map(|f| f.corpus_path.as_str()),
            DeployTab::New => self.cached_data.new_by_dir.get(scroll).map(|d| d.directory.as_str()),
            DeployTab::Conflicts => self.cached_data.conflicts.get(scroll).map(|c| c.deploy_path.as_str()),
            DeployTab::Leftover => self.cached_data.leftover_by_dir.get(scroll).map(|d| d.directory.as_str()),
            DeployTab::Stale => self.cached_data.stale.get(scroll).map(|f| f.library_path.as_str()),
        }
    }

    /// Create a new deployment preview state with cached data.
    pub fn new(cached_data: DeployModalData) -> Self {
        // Start on the New tab if there are new files, otherwise Healthy
        let active_tab = if !cached_data.new.is_empty() {
            DeployTab::New
        } else {
            DeployTab::Healthy
        };

        Self {
            active_tab,
            tab_scroll: [0; 5],
            cached_data,
        }
    }

    /// Max scroll position for the currently active tab (used by DeployViewState).
    pub fn max_scroll_for_current_tab(&self) -> usize {
        self.max_scroll_for_tab()
    }

    fn max_scroll_for_tab(&self) -> usize {
        let count = match self.active_tab {
            DeployTab::Healthy => self.cached_data.healthy.len(),
            DeployTab::New => self.cached_data.new_by_dir.len(),
            DeployTab::Conflicts => self.cached_data.conflicts.len(),
            DeployTab::Leftover => self.cached_data.leftover_by_dir.len(),
            DeployTab::Stale => self.cached_data.stale.len(),
        };
        count.saturating_sub(1)
    }

    /// Render the deployment preview.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Title height: 3 normally, 4 with per-library subtitle
        let title_height = if self.cached_data.per_library.is_empty() { 3 } else { 4 };

        // Layout: title + main content + controls
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(title_height),
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Controls hint
            ])
            .split(area);

        // Render title
        self.render_title(f, main_chunks[0]);

        // Render main content (tabbed list + info pane)
        self.render_content(f, main_chunks[1]);

        // Render controls hint
        self.render_controls(f, main_chunks[2]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let data = &self.cached_data;
        let total_ops = data.total_operations();

        // Build summary line
        let mut summary_parts: Vec<String> = Vec::new();
        if !data.new.is_empty() {
            if data.sidecars.is_empty() {
                summary_parts.push(format!("{} new", data.new.len()));
            } else {
                summary_parts.push(format!("{} new + {} covers", data.new.len(), data.sidecars.len()));
            }
        }
        if !data.leftover.is_empty() {
            if data.replaced_count > 0 {
                summary_parts.push(format!(
                    "{} leftover ({} replaced)", data.leftover.len(), data.replaced_count
                ));
            } else {
                summary_parts.push(format!("{} leftover", data.leftover.len()));
            }
        }
        if !data.stale.is_empty() {
            summary_parts.push(format!("{} stale", data.stale.len()));
        }
        if !data.conflicts.is_empty() {
            summary_parts.push(format!("{} conflicts", data.conflicts.len()));
        }

        let summary_str = if summary_parts.is_empty() {
            format!("({} total operations)", total_ops)
        } else {
            format!("({} — {})", total_ops, summary_parts.join(", "))
        };

        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    " Deploy Preview ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", summary_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
        ];

        // Per-library subtitle when multiple libraries
        if !data.per_library.is_empty() {
            let lib_parts: Vec<String> = data.per_library.iter().map(|lib| {
                let mut parts = Vec::new();
                if lib.new_count > 0 { parts.push(format!("{}n", lib.new_count)); }
                if lib.leftover_count > 0 {
                    if lib.replaced_count > 0 {
                        parts.push(format!("{}l({}r)", lib.leftover_count, lib.replaced_count));
                    } else {
                        parts.push(format!("{}l", lib.leftover_count));
                    }
                }
                if lib.stale_count > 0 { parts.push(format!("{}s", lib.stale_count)); }
                format!("{}: {}", lib.library_name, parts.join(", "))
            }).collect();

            lines.push(Line::from(Span::styled(
                format!(" {}", lib_parts.join("  |  ")),
                Style::default().fg(Color::DarkGray),
            )));
        }

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL));

        f.render_widget(paragraph, area);
    }

    fn render_content(&self, f: &mut Frame, area: Rect) {
        // Split: tabbed list (60%) + info pane (40%)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // Render tabbed list on left
        self.render_tabbed_list(f, chunks[0]);

        // Render info pane on right
        self.render_info_pane(f, chunks[1]);
    }

    fn render_tabbed_list(&self, f: &mut Frame, area: Rect) {
        // Get display items for current tab
        // For New and Leftover, show "directory (count)" format
        let items: Vec<String> = match self.active_tab {
            DeployTab::Healthy => self
                .cached_data
                .healthy
                .iter()
                .map(|f| f.corpus_path.clone())
                .collect(),
            DeployTab::New => self
                .cached_data
                .new_by_dir
                .iter()
                .map(|d| format!("{} ({})", d.directory, d.count))
                .collect(),
            DeployTab::Conflicts => self
                .cached_data
                .conflicts
                .iter()
                .map(|g| g.deploy_path.clone())
                .collect(),
            DeployTab::Leftover => self
                .cached_data
                .leftover_by_dir
                .iter()
                .map(|d| format!("{} ({})", d.directory, d.count))
                .collect(),
            DeployTab::Stale => self
                .cached_data
                .stale
                .iter()
                .map(|f| f.library_path.clone())
                .collect(),
        };

        let paths: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        let scroll = self.tab_scroll[self.active_tab.index()];

        let widget = TabbedSignalList::new(self.active_tab)
            .items(paths)
            .scroll(scroll)
            .tab_counts(self.cached_data.tab_counts());

        widget.render(f, area);
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let scroll = self.tab_scroll[self.active_tab.index()];

        let info = match self.active_tab {
            DeployTab::Healthy => {
                self.cached_data.healthy.get(scroll).map(|file| SignalInfo::Healthy {
                    corpus_path: file.corpus_path.clone(),
                    library_path: file.deploy_path.clone(),
                })
            }
            DeployTab::New => {
                self.cached_data.new_by_dir.get(scroll).map(|dir| {
                    // Find sidecars matching this directory
                    let sidecars: Vec<SidecarSummary> = self.cached_data.sidecars.iter()
                        .filter(|s| {
                            Path::new(&s.corpus_image_path)
                                .parent()
                                .map(|p| p.to_string_lossy())
                                .is_some_and(|p| p == dir.directory)
                        })
                        .map(|s| SidecarSummary {
                            filename: s.filename.clone(),
                            format: s.format.clone(),
                            width: s.width,
                            height: s.height,
                            role: s.role.clone(),
                        })
                        .collect();

                    SignalInfo::NewDirectory {
                        directory: dir.directory.clone(),
                        file_count: dir.count,
                        sidecars,
                    }
                })
            }
            DeployTab::Conflicts => {
                self.cached_data.conflicts.get(scroll).map(|group| SignalInfo::Conflict {
                    deploy_path: group.deploy_path.clone(),
                    conflicting_files: group
                        .conflicting_files
                        .iter()
                        .map(|(path, _)| path.clone())
                        .collect(),
                })
            }
            DeployTab::Leftover => {
                self.cached_data.leftover_by_dir.get(scroll).map(|dir| SignalInfo::LeftoverDirectory {
                    directory: dir.directory.clone(),
                    file_count: dir.count,
                })
            }
            DeployTab::Stale => {
                self.cached_data.stale.get(scroll).map(|file| SignalInfo::Stale {
                    library_path: file.library_path.clone(),
                    expected_path: file.expected_path.clone(),
                })
            }
        };

        let info = info.unwrap_or(SignalInfo::None);
        let pane = SignalInfoPane::new(self.active_tab, &info);
        pane.render(f, area);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let controls = "[Tab/Arrows] Switch Tab  [Up/Down] Scroll  [Enter] Stage & Review  [Esc] Cancel";

        let paragraph = Paragraph::new(controls)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP));

        f.render_widget(paragraph, area);
    }
}
