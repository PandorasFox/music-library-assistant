//! Deployment Preview UI
//!
//! Shows a tabbed view of deploy signals with an info pane.
//! - Tab navigation with Left/Right arrows
//! - Scrollable list of signals sorted by path (non-interactable)
//! - Info pane showing context for selected signal type
//! - Enter opens confirmation dialog
//! - Escape returns to Insights view

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{DeployConfirmModal, DeployModalData};
use crate::ui::widgets::{DeployTab, ModalStyle, SignalInfo, SignalInfoPane, TabbedSignalList};

/// Actions returned from the deployment preview.
#[derive(Debug, Clone)]
pub enum DeploymentPreviewAction {
    /// No action needed.
    None,
    /// User confirmed deployment - generate mutations.
    Confirm,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the deployment preview modal.
#[derive(Debug)]
pub struct DeploymentPreviewState {
    /// Currently active tab.
    pub active_tab: DeployTab,
    /// Scroll position per tab.
    pub tab_scroll: [usize; 5],
    /// Cached signal data (loaded once on init).
    pub cached_data: DeployModalData,
    /// Confirmation dialog (if open).
    pub confirm_modal: Option<DeployConfirmModal>,
}

impl DeploymentPreviewState {
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
            confirm_modal: None,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> DeploymentPreviewAction {
        // If confirmation dialog is open, handle its keys
        if self.confirm_modal.is_some() {
            return self.handle_confirm_key(key);
        }

        match key.code {
            // Tab navigation (arrows and Tab/Shift-Tab)
            KeyCode::Left | KeyCode::BackTab => {
                self.active_tab = self.active_tab.prev();
                DeploymentPreviewAction::None
            }
            KeyCode::Right | KeyCode::Tab => {
                self.active_tab = self.active_tab.next();
                DeploymentPreviewAction::None
            }

            // Scroll within tab
            KeyCode::Up => {
                let idx = self.active_tab.index();
                self.tab_scroll[idx] = self.tab_scroll[idx].saturating_sub(1);
                DeploymentPreviewAction::None
            }
            KeyCode::Down => {
                let idx = self.active_tab.index();
                let max_scroll = self.max_scroll_for_tab();
                if self.tab_scroll[idx] < max_scroll {
                    self.tab_scroll[idx] += 1;
                }
                DeploymentPreviewAction::None
            }
            KeyCode::PageUp => {
                let idx = self.active_tab.index();
                self.tab_scroll[idx] = self.tab_scroll[idx].saturating_sub(10);
                DeploymentPreviewAction::None
            }
            KeyCode::PageDown => {
                let idx = self.active_tab.index();
                let max_scroll = self.max_scroll_for_tab();
                self.tab_scroll[idx] = (self.tab_scroll[idx] + 10).min(max_scroll);
                DeploymentPreviewAction::None
            }

            // Open confirmation dialog (wanting to confirm - default to Cancel for safety)
            KeyCode::Enter => {
                let summary = self.cached_data.summary();
                self.confirm_modal = Some(DeployConfirmModal::for_confirm(summary));
                DeploymentPreviewAction::None
            }

            // Open confirmation dialog (wanting to leave - default to Discard, changes are trivial to re-stage)
            KeyCode::Esc => {
                let summary = self.cached_data.summary();
                self.confirm_modal = Some(DeployConfirmModal::for_escape(summary));
                DeploymentPreviewAction::None
            }

            _ => DeploymentPreviewAction::None,
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> DeploymentPreviewAction {
        match key.code {
            // Navigate button selection
            KeyCode::Left | KeyCode::BackTab => {
                if let Some(ref mut modal) = self.confirm_modal {
                    modal.select_prev();
                }
                DeploymentPreviewAction::None
            }
            KeyCode::Right | KeyCode::Tab => {
                if let Some(ref mut modal) = self.confirm_modal {
                    modal.select_next();
                }
                DeploymentPreviewAction::None
            }

            // Execute selected button action
            KeyCode::Enter => {
                let selected = self.confirm_modal.as_ref()
                    .map(|m| m.selected_button);
                self.confirm_modal = None;

                match selected {
                    Some(super::types::DeployConfirmButton::Confirm) => {
                        DeploymentPreviewAction::Confirm
                    }
                    Some(super::types::DeployConfirmButton::Discard) => {
                        DeploymentPreviewAction::Cancel
                    }
                    Some(super::types::DeployConfirmButton::Cancel) | None => {
                        // Cancel = go back to preview (don't close)
                        DeploymentPreviewAction::None
                    }
                }
            }

            // Close dialog without action (same as Cancel button)
            KeyCode::Esc => {
                self.confirm_modal = None;
                DeploymentPreviewAction::None
            }

            _ => DeploymentPreviewAction::None,
        }
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

        // Layout: title (3) + main content
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title bar
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

        // Render confirmation dialog if open
        if let Some(ref modal) = self.confirm_modal {
            self.render_confirm_dialog(f, area, modal);
        }
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Deploy Preview ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} total operations)", self.cached_data.summary().total_operations()),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
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
                self.cached_data.new_by_dir.get(scroll).map(|dir| SignalInfo::NewDirectory {
                    directory: dir.directory.clone(),
                    file_count: dir.count,
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
        let controls = if self.confirm_modal.is_some() {
            "[←/→] Select  [Enter] Execute  [Esc] Back to preview"
        } else {
            "[Tab/Arrows] Switch Tab  [Up/Down] Scroll  [Enter] Deploy  [Esc] Leave"
        };

        let paragraph = Paragraph::new(controls)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP));

        f.render_widget(paragraph, area);
    }

    fn render_confirm_dialog(&self, f: &mut Frame, area: Rect, modal: &DeployConfirmModal) {
        use super::types::DeployConfirmButton;

        let summary = &modal.summary;
        let selected = modal.selected_button;

        // Build message lines
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Deploy Summary",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        if summary.new_count > 0 {
            lines.push(Line::from(format!(
                "  New files to deploy: {}",
                summary.new_count
            )));
        }
        if summary.stale_count > 0 {
            lines.push(Line::from(format!(
                "  Stale files to fix: {}",
                summary.stale_count
            )));
        }
        if summary.leftover_count > 0 {
            lines.push(Line::from(format!(
                "  Leftover files to remove: {}",
                summary.leftover_count
            )));
        }
        if summary.healthy_count > 0 {
            lines.push(Line::from(Span::styled(
                format!("  Already healthy: {}", summary.healthy_count),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines.push(Line::from(""));

        // Show conflict info (auto-resolved by picking first alphabetical)
        if summary.conflict_count > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} conflicts (first alphabetical path wins)",
                    summary.conflict_count
                ),
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
        }

        lines.push(Line::from(format!(
            "Total operations: {}",
            summary.total_operations()
        )));
        lines.push(Line::from(""));

        // Button styles: selected gets highlighted, others are dim
        let style_for = |btn: DeployConfirmButton, color: Color| {
            if selected == btn {
                Style::default().fg(Color::Black).bg(color)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        };

        lines.push(Line::from(vec![
            Span::styled(" Cancel ", style_for(DeployConfirmButton::Cancel, Color::Gray)),
            Span::raw("  "),
            Span::styled(" Confirm ", style_for(DeployConfirmButton::Confirm, Color::Green)),
            Span::raw("  "),
            Span::styled(" Discard ", style_for(DeployConfirmButton::Discard, Color::Red)),
        ]));

        // Render as centered modal
        let modal_style = ModalStyle::info();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(modal_style.border_color))
            .title("Confirm Deployment")
            .style(Style::default().bg(Color::Black));

        // Calculate centered area
        let popup_area = centered_rect(50, 50, area);
        f.render_widget(Clear, popup_area);

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, popup_area);
    }
}

/// Compute a centered rectangle.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
