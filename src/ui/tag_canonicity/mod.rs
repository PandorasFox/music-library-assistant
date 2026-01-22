//! Tag Canonicity Resolution Modal
//!
//! Provides the interactive workflow for resolving tag canonicity issues:
//! - Artist, genre, album tag collisions (pre-filled canonical value)
//! - Album artist resolution for multi-artist albums (user inputs value)
//!
//! ## Layout
//!
//! Text field ("Squash to:") at top, variant list below.
//! Cursor -1 = text field, cursor 0+ = variant list.
//! Default focus = first list item (cursor=0).
//!
//! ## Key Behaviors
//!
//! - Up/Down: Navigate between text field and list items
//! - Space: Toggle selection (only in list)
//! - Enter: Confirm current squash, stage decision, and advance to next group
//! - Tab/Shift-Tab: Navigate to next/prev group (non-committal, does NOT stage)
//! - Ctrl+R: Stage current decision and jump to review screen
//! - Esc: Cancel entire flow

pub mod render;
pub mod types;

pub use render::{render, render_review};
pub use types::{
    DecisionSummary, ReviewButtonFocus, TagCanonicalityAction, TagCanonicalityModal,
    TagCanonicalityModalData, TagCanonicalityState, TagCanonicityReviewAction,
    TagCanonicityReviewState, TagVariantEntry,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl TagCanonicalityState {
    /// Handle keyboard input for the modal.
    ///
    /// Returns the action to take based on the input.
    pub fn handle_key(&mut self, key: KeyEvent) -> TagCanonicalityAction {
        // If a modal overlay is open, handle it first
        if self.has_modal() {
            return self.handle_modal_key(key);
        }

        // Main modal handling
        match key.code {
            // Ctrl+R: Jump to review screen
            KeyCode::Char('r') | KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                TagCanonicalityAction::ShowReview
            }

            // Navigation: Up/Down move between text field and list
            KeyCode::Up => {
                self.cursor_up();
                TagCanonicalityAction::None
            }
            KeyCode::Down => {
                self.cursor_down();
                TagCanonicalityAction::None
            }

            // Space: Toggle selection (only when on list item)
            KeyCode::Char(' ') if self.cursor >= 0 => {
                self.toggle_selection();
                TagCanonicalityAction::None
            }

            // Enter: Confirm current squash and advance
            KeyCode::Enter => {
                if self.can_submit() {
                    TagCanonicalityAction::Confirmed
                } else {
                    TagCanonicalityAction::None
                }
            }

            // Tab/Shift-Tab: Navigate without saving (non-committal browsing)
            KeyCode::Tab => {
                let forward = !key.modifiers.contains(KeyModifiers::SHIFT);
                TagCanonicalityAction::Navigate { forward }
            }
            KeyCode::BackTab => {
                TagCanonicalityAction::Navigate { forward: false }
            }

            // Esc: Cancel entire flow
            KeyCode::Esc => TagCanonicalityAction::Cancelled,

            // When cursor is on text field (-1), delegate to TextInputState
            _ if self.cursor == -1 => {
                self.canonical_input.handle_key(key);
                TagCanonicalityAction::None
            }

            _ => TagCanonicalityAction::None,
        }
    }

    /// Handle input when a modal overlay is open.
    ///
    /// Currently no modals are used, so this just returns None.
    fn handle_modal_key(&mut self, key: KeyEvent) -> TagCanonicalityAction {
        // Esc closes any modal
        if key.code == KeyCode::Esc {
            self.close_modal();
        }
        TagCanonicalityAction::None
    }
}

impl TagCanonicityReviewState {
    /// Handle keyboard input for the review screen.
    pub fn handle_key(&mut self, key: KeyEvent) -> TagCanonicityReviewAction {
        use types::ReviewButtonFocus;

        match key.code {
            // Up/Down: Navigate decision list
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TagCanonicityReviewAction::None
            }
            KeyCode::Down => {
                if self.cursor + 1 < self.decisions.len() {
                    self.cursor += 1;
                }
                TagCanonicityReviewAction::None
            }

            // Left: Move focus left (Cancel <- Discard <- Confirm)
            KeyCode::Left => {
                self.focus_left();
                TagCanonicityReviewAction::None
            }

            // Right/Tab: Move focus right (Cancel -> Discard -> Confirm)
            KeyCode::Right | KeyCode::Tab => {
                self.focus_right();
                TagCanonicityReviewAction::None
            }

            // Enter: Activate focused button
            KeyCode::Enter => match self.button_focus {
                ReviewButtonFocus::Confirm => TagCanonicityReviewAction::Confirm,
                ReviewButtonFocus::Discard => TagCanonicityReviewAction::Discard,
                ReviewButtonFocus::Cancel => TagCanonicityReviewAction::Cancel,
            },

            // Esc: Cancel (return to resolution modal)
            KeyCode::Esc => TagCanonicityReviewAction::Cancel,

            // Y: Quick confirm
            KeyCode::Char('y') | KeyCode::Char('Y') => TagCanonicityReviewAction::Confirm,

            // D: Quick discard
            KeyCode::Char('d') | KeyCode::Char('D') => TagCanonicityReviewAction::Discard,

            _ => TagCanonicityReviewAction::None,
        }
    }
}
