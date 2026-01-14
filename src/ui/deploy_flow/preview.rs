//! Deployment Preview UI
//!
//! Shows a comprehensive preview of deployment status for all libraries.
//! Displays healthy, to_deploy, stale, orphan, and conflict counts.
//! Allows the librarian to confirm (generate mutations) or cancel.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::ops::deploy::FullDeploymentStatus;
use crate::ui::widgets::{LateralView, UnifiedTitleBar};

/// Actions returned from the deployment preview
#[derive(Debug, Clone)]
pub enum DeploymentPreviewAction {
    /// No action needed
    None,
    /// Confirm and generate mutations, transition to session review
    Confirm,
    /// Cancel and return to main menu
    Cancel,
    /// Cycle to next view in lateral ring (Tab)
    CycleNext,
    /// Cycle to previous view in lateral ring (Shift-Tab)
    CyclePrev,
}

/// State for the deployment preview
#[derive(Debug)]
pub struct DeploymentPreviewState {
    /// Computed deployment statuses for all libraries
    pub statuses: Vec<FullDeploymentStatus>,
    /// Session ID for mutations
    pub session_id: String,
    /// Currently selected library index
    selected_library: usize,
    /// Currently selected action (0=Confirm, 1=Cancel)
    selected_action: usize,
    /// List state for library selection
    library_list_state: ListState,
    /// List state for action selection
    action_list_state: ListState,
    /// Focus: 0=libraries, 1=actions
    focus: usize,
}

impl DeploymentPreviewState {
    /// Create a new deployment preview state
    pub fn new(statuses: Vec<FullDeploymentStatus>, session_id: String) -> Self {
        let mut library_list_state = ListState::default();
        if !statuses.is_empty() {
            library_list_state.select(Some(0));
        }
        let mut action_list_state = ListState::default();
        action_list_state.select(Some(0));

        Self {
            statuses,
            session_id,
            selected_library: 0,
            selected_action: 0,
            library_list_state,
            action_list_state,
            focus: 1, // Start on actions
        }
    }

    /// Get total mutations that will be generated
    pub fn total_mutations(&self) -> usize {
        self.statuses
            .iter()
            .map(|s| s.to_deploy.len() + s.stale.len() + s.orphans.len())
            .sum()
    }

    /// Get total conflicts across all libraries
    pub fn total_conflicts(&self) -> usize {
        self.statuses
            .iter()
            .map(|s| s.conflicts.len())
            .sum()
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> DeploymentPreviewAction {
        match key.code {
            KeyCode::Up => {
                if self.focus == 0 && !self.statuses.is_empty() {
                    self.selected_library = self.selected_library.saturating_sub(1);
                    self.library_list_state.select(Some(self.selected_library));
                } else if self.focus == 1 {
                    self.selected_action = self.selected_action.saturating_sub(1);
                    self.action_list_state.select(Some(self.selected_action));
                }
                DeploymentPreviewAction::None
            }
            KeyCode::Down => {
                if self.focus == 0 && !self.statuses.is_empty() {
                    self.selected_library =
                        (self.selected_library + 1).min(self.statuses.len().saturating_sub(1));
                    self.library_list_state.select(Some(self.selected_library));
                } else if self.focus == 1 {
                    self.selected_action = (self.selected_action + 1).min(1);
                    self.action_list_state.select(Some(self.selected_action));
                }
                DeploymentPreviewAction::None
            }
            KeyCode::Left | KeyCode::Right => {
                // Toggle focus between libraries (left pane) and actions (right pane)
                if !self.statuses.is_empty() {
                    self.focus = if self.focus == 0 { 1 } else { 0 };
                }
                DeploymentPreviewAction::None
            }
            // Tab/Shift-Tab for lateral view cycling
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    DeploymentPreviewAction::CyclePrev
                } else {
                    DeploymentPreviewAction::CycleNext
                }
            }
            KeyCode::BackTab => DeploymentPreviewAction::CyclePrev,
            KeyCode::Enter => {
                if self.focus == 1 {
                    match self.selected_action {
                        0 => DeploymentPreviewAction::Confirm,
                        1 => DeploymentPreviewAction::Cancel,
                        _ => DeploymentPreviewAction::None,
                    }
                } else {
                    DeploymentPreviewAction::None
                }
            }
            KeyCode::Esc => DeploymentPreviewAction::Cancel,
            _ => DeploymentPreviewAction::None,
        }
    }

    /// Render the deployment preview
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Layout: Title bar at top (3 rows for borders), content below
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(UnifiedTitleBar::height()), // Title bar with borders
                Constraint::Length(6),                          // Summary
                Constraint::Min(10),                            // 3-pane content
            ])
            .split(area);

        // Render unified title bar
        let titlebar = UnifiedTitleBar::new(LateralView::Deploy);
        titlebar.render(f, main_chunks[0]);

        self.render_summary(f, main_chunks[1]);

        // 3-pane layout: Libraries | Details | Actions
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25), // Library list
                Constraint::Percentage(50), // Details
                Constraint::Percentage(25), // Actions
            ])
            .split(main_chunks[2]);

        self.render_library_list(f, content_chunks[0]);
        self.render_selected_details(f, content_chunks[1]);
        self.render_actions(f, content_chunks[2]);
    }

    fn render_summary(&self, f: &mut Frame, area: Rect) {
        let total_healthy: usize = self.statuses.iter().map(|s| s.healthy.len()).sum();
        let total_to_deploy: usize = self.statuses.iter().map(|s| s.to_deploy.len()).sum();
        let total_stale: usize = self.statuses.iter().map(|s| s.stale.len()).sum();
        let total_orphans: usize = self.statuses.iter().map(|s| s.orphans.len()).sum();
        let total_conflicts: usize = self.statuses.iter().map(|s| s.conflicts.len()).sum();
        let total_mutations = total_to_deploy + total_stale + total_orphans;

        let mut lines = vec![
            Line::from(Span::styled(
                "Deployment Preview",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(format!("{}", total_healthy), Style::default().fg(Color::Green)),
                Span::raw(" healthy | "),
                Span::styled(format!("{}", total_to_deploy), Style::default().fg(Color::Yellow)),
                Span::raw(" to deploy | "),
                Span::styled(format!("{}", total_stale), Style::default().fg(Color::Magenta)),
                Span::raw(" stale | "),
                Span::styled(format!("{}", total_orphans), Style::default().fg(Color::Red)),
                Span::raw(" orphans"),
            ]),
        ];

        if total_conflicts > 0 {
            lines.push(Line::from(Span::styled(
                format!("{} conflicts (blocked from deploy)", total_conflicts),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("Total mutations: "),
            Span::styled(
                format!("{}", total_mutations),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]));

        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Summary"));
        f.render_widget(para, area);
    }

    fn render_library_list(&mut self, f: &mut Frame, area: Rect) {
        if self.statuses.is_empty() {
            let para = Paragraph::new("No libraries configured")
                .block(Block::default().borders(Borders::ALL).title("Libraries"));
            f.render_widget(para, area);
            return;
        }

        let items: Vec<ListItem> = self
            .statuses
            .iter()
            .enumerate()
            .map(|(i, status)| {
                let mutations = status.to_deploy.len() + status.stale.len() + status.orphans.len();
                let is_selected = i == self.selected_library;
                let style = if is_selected && self.focus == 0 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let indicator = if status.conflicts.is_empty() {
                    if mutations == 0 { "✓" } else { "•" }
                } else {
                    "!"
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", indicator), style),
                    Span::styled(&status.library_name, style),
                    Span::styled(format!(" ({})", mutations), style),
                ]))
            })
            .collect();

        let list_title = if self.focus == 0 { "[Libraries]" } else { "Libraries" };
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(list_title)
                .border_style(if self.focus == 0 {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        );
        f.render_stateful_widget(list, area, &mut self.library_list_state);
    }

    fn render_selected_details(&self, f: &mut Frame, area: Rect) {
        if let Some(status) = self.statuses.get(self.selected_library) {
            let mut lines = vec![
                Line::from(Span::styled(
                    &status.library_name,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Healthy: ", Style::default()),
                    Span::styled(
                        format!("{}", status.healthy.len()),
                        Style::default().fg(Color::Green),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("To deploy: ", Style::default()),
                    Span::styled(
                        format!("{}", status.to_deploy.len()),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Stale (redeploy): ", Style::default()),
                    Span::styled(
                        format!("{}", status.stale.len()),
                        Style::default().fg(Color::Magenta),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Orphans (to stash): ", Style::default()),
                    Span::styled(
                        format!("{}", status.orphans.len()),
                        Style::default().fg(Color::Red),
                    ),
                ]),
            ];

            if !status.conflicts.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("Conflicts: {} (blocked)", status.conflicts.len()),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                // Show first few conflict paths
                for (i, conflict) in status.conflicts.iter().take(3).enumerate() {
                    let path_str = conflict.target_path.to_string_lossy();
                    let truncated = crate::ui::helpers::truncate_left(&path_str, 40);
                    lines.push(Line::from(Span::styled(
                        format!("  {} ({} files)", truncated, conflict.conflicting_tracks.len()),
                        Style::default().fg(Color::DarkGray),
                    )));
                    if i == 2 && status.conflicts.len() > 3 {
                        lines.push(Line::from(Span::styled(
                            format!("  ... and {} more", status.conflicts.len() - 3),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }

            let para = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title("Details"));
            f.render_widget(para, area);
        } else {
            let para = Paragraph::new("No library selected")
                .block(Block::default().borders(Borders::ALL).title("Details"));
            f.render_widget(para, area);
        }
    }

    fn render_actions(&mut self, f: &mut Frame, area: Rect) {
        let total_mutations = self.total_mutations();
        let options = [
            (
                "Confirm",
                format!("Generate {} mutations and review", total_mutations),
            ),
            ("Cancel", "Return to main menu".to_string()),
        ];

        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, desc))| {
                let is_selected = i == self.selected_action;
                let style = if is_selected && self.focus == 1 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().fg(Color::White)
                } else {
                    Style::default()
                };
                let prefix = if is_selected && self.focus == 1 {
                    ">> "
                } else {
                    "   "
                };

                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(label.to_string(), style),
                    ]),
                    Line::from(vec![
                        Span::raw("      "),
                        Span::styled(desc.clone(), Style::default().fg(Color::DarkGray)),
                    ]),
                ])
            })
            .collect();

        let actions_title = if self.focus == 1 { "[Actions]" } else { "Actions" };
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(actions_title)
                .border_style(if self.focus == 1 {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        );
        f.render_stateful_widget(list, area, &mut self.action_list_state);
    }
}
