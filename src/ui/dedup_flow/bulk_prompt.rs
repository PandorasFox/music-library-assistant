//! Bulk Review Prompt UI
//!
//! Shown when bulk cluster decisions are complete and only individual
//! 2-file duplicates remain. Offers the librarian a choice:
//! - Commit bulk changes now
//! - Continue to individual resolution
//! - Cancel session

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::deduplication::DeduplicationSession;

/// Actions returned from the bulk prompt
#[derive(Debug, Clone)]
pub enum BulkPromptAction {
    /// No action needed
    None,
    /// Go to session review and commit
    CommitBulk,
    /// Continue to individual 2-file conflicts
    ContinueIndividual,
    /// Cancel the entire session
    Cancel,
}

/// State for the bulk review prompt
#[derive(Debug, Clone)]
pub struct BulkPromptState {
    /// The deduplication session
    pub session: DeduplicationSession,
    /// Currently selected option (0 = commit, 1 = continue, 2 = cancel)
    selected_option: usize,
    /// List widget state
    list_state: ListState,
}

impl BulkPromptState {
    /// Create a new bulk prompt state
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
    pub fn handle_key(&mut self, key: KeyEvent) -> BulkPromptAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_option = self.selected_option.saturating_sub(1);
                self.list_state.select(Some(self.selected_option));
                BulkPromptAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected_option = (self.selected_option + 1).min(2);
                self.list_state.select(Some(self.selected_option));
                BulkPromptAction::None
            }
            KeyCode::Enter => match self.selected_option {
                0 => BulkPromptAction::CommitBulk,
                1 => BulkPromptAction::ContinueIndividual,
                2 => BulkPromptAction::Cancel,
                _ => BulkPromptAction::None,
            },
            KeyCode::Esc => BulkPromptAction::Cancel,
            _ => BulkPromptAction::None,
        }
    }

    /// Render the bulk prompt
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),  // Summary
                Constraint::Min(8),     // Options
                Constraint::Length(3),  // Controls
            ])
            .split(area);

        self.render_summary(f, chunks[0]);
        self.render_options(f, chunks[1]);
        self.render_controls(f, chunks[2]);
    }

    fn render_summary(&self, f: &mut Frame, area: Rect) {
        let total_changes = self.session.total_pending_changes();
        let remaining = self.session.clusters.len() - self.session.current_index;
        let resolved = self.session.decisions.iter()
            .filter(|d| d.keeper_dir.is_some())
            .count();

        let lines = vec![
            Line::from(Span::styled(
                "Bulk Decision Point",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "Bulk cluster decisions complete. {} clusters resolved.",
                resolved
            )),
            Line::from(Span::styled(
                format!("{} files queued for removal.", total_changes),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(format!(
                "Remaining: {} individual 2-file duplicates",
                remaining
            )),
        ];

        let para = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(para, area);
    }

    fn render_options(&mut self, f: &mut Frame, area: Rect) {
        let options = [("Review & commit bulk changes now", "Proceed to session review"),
            ("Continue to individual resolution", "Handle remaining 2-file conflicts one by one"),
            ("Cancel session", "Discard all pending changes")];

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
                        Span::styled(desc.to_string(), Style::default().fg(Color::DarkGray)),
                    ]),
                ])
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Choose action"));
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let controls = Line::from(vec![
            Span::styled("Up/Down", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Select | "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Choose | "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(": Cancel"),
        ]);

        let para = Paragraph::new(controls)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(para, area);
    }

    /// Get the session (for passing to next stage)
    pub fn into_session(self) -> DeduplicationSession {
        self.session
    }
}
