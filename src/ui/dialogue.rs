//! Conversational Decision Flow UI
//!
//! Provides a decision-by-decision workflow for corpus operations like
//! fingerprint deduplication. Users navigate through decisions using
//! arrow keys and enter (no alphanumeric hotkeys).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::corpus::db::{Decision, DecisionOutcome, DecisionStack};

// ============================================================================
// Types
// ============================================================================

/// Actions available for each decision (rendered as selectable list)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionAction {
    Accept,
    Reject,
    Defer,
    ShowDetails,
}

impl DecisionAction {
    fn label(&self) -> &'static str {
        match self {
            DecisionAction::Accept => "Accept recommendation",
            DecisionAction::Reject => "Reject (keep all)",
            DecisionAction::Defer => "Defer to later",
            DecisionAction::ShowDetails => "Show/hide details",
        }
    }

    fn all() -> &'static [DecisionAction] {
        &[
            DecisionAction::Accept,
            DecisionAction::Reject,
            DecisionAction::Defer,
            DecisionAction::ShowDetails,
        ]
    }
}

/// Active decision flow state
#[derive(Debug)]
pub struct DialogueState {
    pub stack: DecisionStack,
    pub show_details: bool,
    /// Currently selected action index
    pub action_index: usize,
    pub action_list_state: ListState,
}

/// Summary shown after completing a decision flow session
#[derive(Debug, Clone, Default)]
pub struct DialogueSummaryState {
    pub accepted_count: usize,
    pub rejected_count: usize,
    pub deferred_count: usize,
    pub patterns_learned: Vec<String>,
    pub changes_pending: usize,
    /// The actual pending changes to commit (from accepted decisions)
    pub pending_changes: Vec<crate::corpus::db::PendingChange>,
    /// Selected action in summary (0 = Commit, 1 = Revert, 2 = Return)
    pub action_index: usize,
    pub action_list_state: ListState,
}

/// Result of handling a key press in dialogue
#[derive(Debug)]
pub enum DialogueResult {
    /// Continue in dialogue
    None,
    /// Decision was made, continue to next
    Continue,
    /// Exit dialogue and show summary
    Finish(DialogueSummaryState),
    /// Commit changes and return to menu
    Commit,
    /// Revert all and return to menu
    Revert,
    /// Return to menu without commit
    Cancel,
}

// ============================================================================
// DialogueState Implementation
// ============================================================================

impl DialogueState {
    /// Create a new dialogue state with the given decision stack
    #[allow(dead_code)]
    pub fn new(stack: DecisionStack) -> Self {
        let mut action_list_state = ListState::default();
        action_list_state.select(Some(0));

        Self {
            stack,
            show_details: false,
            action_index: 0,
            action_list_state,
        }
    }

    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogueResult {
        match key.code {
            KeyCode::Up => {
                if self.action_index > 0 {
                    self.action_index -= 1;
                    self.action_list_state.select(Some(self.action_index));
                }
                DialogueResult::None
            }
            KeyCode::Down => {
                let max = DecisionAction::all().len().saturating_sub(1);
                if self.action_index < max {
                    self.action_index += 1;
                    self.action_list_state.select(Some(self.action_index));
                }
                DialogueResult::None
            }
            KeyCode::Enter => self.execute_action(),
            KeyCode::Esc => {
                // Exit and show summary
                DialogueResult::Finish(self.create_summary())
            }
            _ => DialogueResult::None,
        }
    }

    fn execute_action(&mut self) -> DialogueResult {
        let action = DecisionAction::all()
            .get(self.action_index)
            .copied()
            .unwrap_or(DecisionAction::Accept);

        match action {
            DecisionAction::Accept => {
                if self.stack.current_index < self.stack.decisions.len() {
                    let decision = self.stack.decisions[self.stack.current_index].clone();
                    self.stack
                        .resolved
                        .push((decision, DecisionOutcome::Accept));
                    self.stack.current_index += 1;
                    self.reset_action_selection();
                    self.check_completion()
                } else {
                    DialogueResult::None
                }
            }
            DecisionAction::Reject => {
                if self.stack.current_index < self.stack.decisions.len() {
                    let decision = self.stack.decisions[self.stack.current_index].clone();
                    self.stack
                        .resolved
                        .push((decision, DecisionOutcome::Reject));
                    self.stack.current_index += 1;
                    self.reset_action_selection();
                    self.check_completion()
                } else {
                    DialogueResult::None
                }
            }
            DecisionAction::Defer => {
                if self.stack.current_index < self.stack.decisions.len() {
                    // Move current decision to end of queue
                    let decision = self.stack.decisions.remove(self.stack.current_index);
                    self.stack.decisions.push(decision);
                    // Don't increment index - we'll now see the next decision
                    self.reset_action_selection();
                    DialogueResult::Continue
                } else {
                    DialogueResult::None
                }
            }
            DecisionAction::ShowDetails => {
                self.show_details = !self.show_details;
                DialogueResult::None
            }
        }
    }

    fn reset_action_selection(&mut self) {
        self.action_index = 0;
        self.action_list_state.select(Some(0));
    }

    fn check_completion(&self) -> DialogueResult {
        if self.stack.current_index >= self.stack.decisions.len() {
            DialogueResult::Finish(self.create_summary())
        } else {
            DialogueResult::Continue
        }
    }

    fn create_summary(&self) -> DialogueSummaryState {
        let accepted = self
            .stack
            .resolved
            .iter()
            .filter(|(_, o)| {
                *o == DecisionOutcome::Accept || *o == DecisionOutcome::AcceptPattern
            })
            .count();
        let rejected = self
            .stack
            .resolved
            .iter()
            .filter(|(_, o)| {
                *o == DecisionOutcome::Reject || *o == DecisionOutcome::RejectPattern
            })
            .count();
        let deferred = self
            .stack
            .resolved
            .iter()
            .filter(|(_, o)| *o == DecisionOutcome::Defer)
            .count();

        // Collect pending changes from accepted decisions
        let pending_changes: Vec<crate::corpus::db::PendingChange> = self
            .stack
            .resolved
            .iter()
            .filter(|(_, o)| {
                *o == DecisionOutcome::Accept || *o == DecisionOutcome::AcceptPattern
            })
            .flat_map(|(decision, _)| decision.pending_changes.clone())
            .collect();

        let changes_pending = pending_changes.len();

        let mut summary = DialogueSummaryState {
            accepted_count: accepted,
            rejected_count: rejected,
            deferred_count: deferred,
            patterns_learned: self.stack.ignore_patterns.clone(),
            changes_pending,
            pending_changes,
            action_index: 0,
            action_list_state: ListState::default(),
        };
        summary.action_list_state.select(Some(0));
        summary
    }

    /// Render the decision flow UI
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        if self.stack.current_index >= self.stack.decisions.len() {
            // All decisions processed - shouldn't reach here normally
            let message = Paragraph::new("All decisions have been reviewed.\nPress ESC to continue.")
                .block(Block::default().borders(Borders::ALL).title("Complete"));
            f.render_widget(message, area);
            return;
        }

        let decision = &self.stack.decisions[self.stack.current_index];

        // Split into decision info (top) and actions (bottom)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),    // Decision info
                Constraint::Length(8), // Actions
            ])
            .split(area);

        self.render_decision_info(f, chunks[0], decision);
        self.render_actions(f, chunks[1]);
    }

    fn render_decision_info(&self, f: &mut Frame, area: Rect, decision: &Decision) {
        let progress = format!(
            "Decision {}/{} [{}]",
            self.stack.current_index + 1,
            self.stack.decisions.len(),
            decision.priority.as_str()
        );

        let mut lines = vec![
            Line::from(progress).style(Style::default().fg(Color::Cyan)),
            Line::from(""),
            Line::from(format!("Category: {}", decision.category.as_str())),
            Line::from(""),
            Line::from(decision.summary.as_str()).style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(format!("Impact: {}", decision.impact_summary)),
            Line::from(format!("Files affected: {}", decision.affected_paths.len())),
        ];

        if let Some(rec) = &decision.recommendation {
            lines.push(Line::from(""));
            lines.push(
                Line::from(format!("Recommendation: {}", rec))
                    .style(Style::default().fg(Color::Green)),
            );
        }

        if self.show_details {
            lines.push(Line::from(""));
            lines.push(
                Line::from("─── Details ───").style(Style::default().fg(Color::DarkGray)),
            );
            lines.push(Line::from(decision.details.as_str()));
            for path in &decision.affected_paths {
                lines.push(
                    Line::from(format!("  • {}", path))
                        .style(Style::default().fg(Color::DarkGray)),
                );
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Decision"));
        f.render_widget(paragraph, area);
    }

    fn render_actions(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = DecisionAction::all()
            .iter()
            .map(|action| {
                let style = if *action == DecisionAction::ShowDetails && self.show_details {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(action.label())).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Actions"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        f.render_stateful_widget(list, area, &mut self.action_list_state);
    }
}

// ============================================================================
// DialogueSummaryState Implementation
// ============================================================================

impl DialogueSummaryState {
    /// Summary actions
    const ACTIONS: &'static [&'static str] = &[
        "Commit changes",
        "Revert all",
        "Return to menu",
    ];

    /// Handle a key event in the summary view
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogueResult {
        match key.code {
            KeyCode::Up => {
                if self.action_index > 0 {
                    self.action_index -= 1;
                    self.action_list_state.select(Some(self.action_index));
                }
                DialogueResult::None
            }
            KeyCode::Down => {
                if self.action_index < Self::ACTIONS.len().saturating_sub(1) {
                    self.action_index += 1;
                    self.action_list_state.select(Some(self.action_index));
                }
                DialogueResult::None
            }
            KeyCode::Enter => match self.action_index {
                0 => DialogueResult::Commit,
                1 => DialogueResult::Revert,
                _ => DialogueResult::Cancel,
            },
            KeyCode::Esc => DialogueResult::Cancel,
            _ => DialogueResult::None,
        }
    }

    /// Render the summary view
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),   // Summary info
                Constraint::Length(7), // Actions
            ])
            .split(area);

        self.render_summary_info(f, chunks[0]);
        self.render_summary_actions(f, chunks[1]);
    }

    fn render_summary_info(&self, f: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::from("Session Summary").style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(format!("  Accepted: {}", self.accepted_count))
                .style(Style::default().fg(Color::Green)),
            Line::from(format!("  Rejected: {}", self.rejected_count))
                .style(Style::default().fg(Color::Red)),
            Line::from(format!("  Deferred: {}", self.deferred_count))
                .style(Style::default().fg(Color::Yellow)),
            Line::from(""),
            Line::from(format!("Pending changes to commit: {}", self.changes_pending)),
        ];

        if !self.patterns_learned.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("Patterns learned this session:"));
            for pattern in &self.patterns_learned {
                lines.push(
                    Line::from(format!("  • {}", pattern))
                        .style(Style::default().fg(Color::Cyan)),
                );
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Complete"));
        f.render_widget(paragraph, area);
    }

    fn render_summary_actions(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = Self::ACTIONS
            .iter()
            .map(|action| ListItem::new(Line::from(*action)))
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Choose Action"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        f.render_stateful_widget(list, area, &mut self.action_list_state);
    }
}
