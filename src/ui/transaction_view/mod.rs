//! Transaction tab view — persistent transaction review and management.
//!
//! Shows accumulated decisions when `leave_transactions_open` is enabled.
//! Provides commit, discard, and granular removal of decisions/mutations.

pub mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::meta::decisions::DecisionKey;

/// Focus area within the transaction view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionViewFocus {
    #[default]
    List,
    Detail,
    Buttons,
}

/// State for the Transaction tab view.
pub struct TransactionViewState {
    pub cursor: usize,
    pub scroll: usize,
    pub detail_scroll: usize,
    pub focus: TransactionViewFocus,
}

impl TransactionViewState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            detail_scroll: 0,
            focus: TransactionViewFocus::List,
        }
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
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if decision_count > 0 {
                    TransactionViewAction::Commit
                } else {
                    TransactionViewAction::None
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                TransactionViewAction::DiscardAll
            }
            _ => TransactionViewAction::None,
        }
    }
}
