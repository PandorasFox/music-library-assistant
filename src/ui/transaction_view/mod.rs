//! Transaction tab view — persistent transaction review and management.
//!
//! Shows accumulated decisions when `leave_transactions_open` is enabled.
//! Provides commit, discard, and granular removal of decisions/mutations.

pub mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::meta::decisions::DecisionKey;

/// Button focus within the transaction view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionButtonFocus {
    /// Discard all decisions (destructive)
    Discard,
    /// Confirm and execute all decisions
    #[default]
    Confirm,
}

/// State for the Transaction tab view.
pub struct TransactionViewState {
    pub cursor: usize,
    pub scroll: usize,
    pub button_focus: TransactionButtonFocus,
}

impl TransactionViewState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            button_focus: TransactionButtonFocus::Confirm,
        }
    }

    /// Move button focus left (Confirm -> Discard).
    fn focus_left(&mut self) {
        self.button_focus = TransactionButtonFocus::Discard;
    }

    /// Move button focus right (Discard -> Confirm).
    fn focus_right(&mut self) {
        self.button_focus = TransactionButtonFocus::Confirm;
    }
}

/// Actions produced by the transaction view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionViewAction {
    None,
    CycleNext,
    CyclePrev,
    Commit,
    DiscardAll,
    RemoveDecision(DecisionKey),
    RemoveMutation(DecisionKey, usize),
}

impl TransactionViewState {
    pub fn handle_key(&mut self, key: KeyEvent, decision_count: usize) -> TransactionViewAction {
        match key.code {
            KeyCode::Tab => TransactionViewAction::CycleNext,
            KeyCode::BackTab => TransactionViewAction::CyclePrev,
            KeyCode::Char('k') | KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TransactionViewAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if decision_count > 0 && self.cursor < decision_count.saturating_sub(1) {
                    self.cursor += 1;
                }
                TransactionViewAction::None
            }
            // Shift+Arrow to move button focus
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_left();
                TransactionViewAction::None
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus_right();
                TransactionViewAction::None
            }
            // Enter activates focused button
            KeyCode::Enter => {
                if decision_count == 0 {
                    return TransactionViewAction::None;
                }
                match self.button_focus {
                    TransactionButtonFocus::Confirm => TransactionViewAction::Commit,
                    TransactionButtonFocus::Discard => TransactionViewAction::DiscardAll,
                }
            }
            _ => TransactionViewAction::None,
        }
    }
}
