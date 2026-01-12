//! Session Review UI
//!
//! Shows a summary of all deduplication decisions and pending changes.
//! Allows the librarian to commit, preview, export, or cancel.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::deduplication::DeduplicationSession;

/// Actions returned from the session review
#[derive(Debug, Clone)]
pub enum SessionReviewAction {
    /// No action needed
    None,
    /// Commit all pending changes
    Commit,
    /// Preview changes (dry run)
    Preview,
    /// Export change list to file
    Export,
    /// Cancel and discard all changes
    Cancel,
}

/// State for the session review
#[derive(Debug, Clone)]
pub struct SessionReviewState {
    /// The deduplication session
    pub session: DeduplicationSession,
    /// Currently selected option
    selected_option: usize,
    /// List widget state
    list_state: ListState,
}

impl SessionReviewState {
    /// Create a new session review state
    pub fn new(session: DeduplicationSession) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            session,
            selected_option: 0,
            list_state,
        }
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> SessionReviewAction {
        match key.code {
            KeyCode::Up => {
                self.selected_option = self.selected_option.saturating_sub(1);
                self.list_state.select(Some(self.selected_option));
                SessionReviewAction::None
            }
            KeyCode::Down => {
                self.selected_option = (self.selected_option + 1).min(3);
                self.list_state.select(Some(self.selected_option));
                SessionReviewAction::None
            }
            KeyCode::Enter => match self.selected_option {
                0 => SessionReviewAction::Commit,
                1 => SessionReviewAction::Preview,
                2 => SessionReviewAction::Export,
                3 => SessionReviewAction::Cancel,
                _ => SessionReviewAction::None,
            },
            KeyCode::Esc => SessionReviewAction::Cancel,
            _ => SessionReviewAction::None,
        }
    }

    /// Render the session review
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // Summary stats
                Constraint::Min(6),     // Keeper breakdown
                Constraint::Length(10), // Options
                Constraint::Length(3),  // Controls
            ])
            .split(area);

        self.render_summary(f, chunks[0]);
        self.render_keeper_stats(f, chunks[1]);
        self.render_options(f, chunks[2]);
        self.render_controls(f, chunks[3]);
    }

    fn render_summary(&self, f: &mut Frame, area: Rect) {
        let total_clusters = self.session.clusters.len();
        let resolved = self.session.decisions.iter()
            .filter(|d| d.keeper_dir.is_some())
            .count();
        let skipped = self.session.decisions.iter()
            .filter(|d| d.keeper_dir.is_none())
            .count();
        let pending_changes = self.session.total_pending_changes();
        let files_kept = self.session.files_to_keep();

        let lines = vec![
            Line::from(Span::styled(
                "Session Review",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!("Clusters resolved: {}/{}", resolved, total_clusters)),
            if skipped > 0 {
                Line::from(Span::styled(
                    format!("Clusters skipped: {}", skipped),
                    Style::default().fg(Color::Yellow),
                ))
            } else {
                Line::from("")
            },
            Line::from(""),
            Line::from(Span::styled(
                format!("Files to remove: {}", pending_changes),
                Style::default().fg(Color::Red),
            )),
            Line::from(Span::styled(
                format!("Files to keep: {}", files_kept),
                Style::default().fg(Color::Green),
            )),
            Line::from(format!("Divergence root: {}", self.session.divergence_root)),
        ];

        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Summary"));
        f.render_widget(para, area);
    }

    fn render_keeper_stats(&self, f: &mut Frame, area: Rect) {
        let keeper_stats = self.session.keeper_stats();

        let mut lines = vec![
            Line::from(Span::styled(
                "Files kept by directory:",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        // Sort by count descending
        let mut stats_vec: Vec<_> = keeper_stats.iter().collect();
        stats_vec.sort_by(|a, b| b.1.cmp(a.1));

        for (dir, count) in stats_vec {
            lines.push(Line::from(format!("  {}: {} files", dir, count)));
        }

        if keeper_stats.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No decisions made yet",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let para = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Keeper Breakdown"));
        f.render_widget(para, area);
    }

    fn render_options(&mut self, f: &mut Frame, area: Rect) {
        let pending = self.session.total_pending_changes();
        let options = vec![
            (
                "Commit all changes",
                format!("Execute {} file moves to lost-files/", pending),
            ),
            (
                "Preview (dry run)",
                "Show what would be moved without executing".to_string(),
            ),
            (
                "Export change list",
                "Save pending changes to file for review".to_string(),
            ),
            (
                "Cancel",
                "Discard all pending changes".to_string(),
            ),
        ];

        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, desc))| {
                let is_selected = i == self.selected_option;
                let prefix = if is_selected { ">> " } else { "   " };
                let style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
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

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Actions"));
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let controls = Line::from(vec![
            Span::styled("Up/Down", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Select | "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Execute | "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Cancel"),
        ]);

        let para = Paragraph::new(controls)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(para, area);
    }

    /// Get the session (for use after review)
    pub fn into_session(self) -> DeduplicationSession {
        self.session
    }
}
