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
    /// Whether the button row has focus (vs the decision list).
    pub buttons_focused: bool,
    /// When set, a confirmation popup is shown for removing this decision.
    pub pending_removal: Option<DecisionKey>,
}

impl TransactionViewState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            button_focus: TransactionButtonFocus::Confirm,
            buttons_focused: false,
            pending_removal: None,
        }
    }

    /// Move button focus left (Confirm -> Discard).
    fn focus_left(&mut self) {
        self.buttons_focused = true;
        self.button_focus = TransactionButtonFocus::Discard;
    }

    /// Move button focus right (Discard -> Confirm).
    fn focus_right(&mut self) {
        self.buttons_focused = true;
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
    /// User pressed x on a decision — handler should resolve cursor to DecisionKey and set pending_removal
    RequestRemoval,
    /// User confirmed removal in the popup — handler should execute removal
    RemoveDecision(DecisionKey),
}

impl TransactionViewState {
    pub fn handle_key(&mut self, key: KeyEvent, decision_count: usize) -> TransactionViewAction {
        // Confirmation popup mode — intercept all keys
        if self.pending_removal.is_some() {
            return match key.code {
                KeyCode::Enter => {
                    let k = self.pending_removal.take().unwrap();
                    TransactionViewAction::RemoveDecision(k)
                }
                KeyCode::Esc => {
                    self.pending_removal = None;
                    TransactionViewAction::None
                }
                _ => TransactionViewAction::None,
            };
        }

        match key.code {
            KeyCode::Tab => TransactionViewAction::CycleNext,
            KeyCode::BackTab => TransactionViewAction::CyclePrev,
            KeyCode::Up => {
                self.buttons_focused = false;
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TransactionViewAction::None
            }
            KeyCode::Down => {
                self.buttons_focused = false;
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
            // Enter activates focused button (only when buttons are focused)
            KeyCode::Enter if self.buttons_focused => {
                if decision_count == 0 {
                    return TransactionViewAction::None;
                }
                match self.button_focus {
                    TransactionButtonFocus::Confirm => TransactionViewAction::Commit,
                    TransactionButtonFocus::Discard => TransactionViewAction::DiscardAll,
                }
            }
            // x to remove selected decision
            KeyCode::Char('x') if decision_count > 0 => {
                TransactionViewAction::RequestRemoval
            }
            _ => TransactionViewAction::None,
        }
    }
}
