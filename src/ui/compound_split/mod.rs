//! Compound Tag Split Resolution Modal
//!
//! Handles splitting compound tag values (e.g., "Rock; Metal") into
//! multiple individual tag values.

pub mod types;
pub mod render;

pub use types::{CompoundSplitClusters, CompoundSplitData, CompoundSplitState};
pub use render::render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ============================================================================
// Actions
// ============================================================================

/// Action returned from keyboard handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundSplitAction {
    /// No action needed
    None,
    /// User confirmed this split - stage decision and advance
    Confirmed,
    /// User chose to canonicalize - mark as single entity, don't split
    Canonicalize,
    /// User cancelled the entire flow
    Cancelled,
    /// Navigate to another signal (Tab/Shift-Tab)
    Navigate { forward: bool },
    /// Jump to review screen (Ctrl+R)
    ShowReview,
    /// Stage ALL splits and go to review (Ctrl+A)
    StageAllAndReview,
}

// ============================================================================
// Keyboard Handling
// ============================================================================

impl CompoundSplitState {
    /// Handle keyboard input.
    pub fn handle_key(&mut self, key: KeyEvent) -> CompoundSplitAction {
        match key.code {
            // Navigation within split parts (informational only)
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor_up();
                CompoundSplitAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor_down();
                CompoundSplitAction::None
            }

            // Confirm split and advance
            KeyCode::Enter => CompoundSplitAction::Confirmed,

            // Canonicalize - mark as single entity, don't split
            KeyCode::Char('c') | KeyCode::Char('C') => CompoundSplitAction::Canonicalize,

            // Navigate between signals non-committally
            KeyCode::Tab => {
                let forward = !key.modifiers.contains(KeyModifiers::SHIFT);
                CompoundSplitAction::Navigate { forward }
            }

            // Jump to review
            KeyCode::Char('r') | KeyCode::Char('R')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                CompoundSplitAction::ShowReview
            }

            // Stage ALL and go to review
            KeyCode::Char('a') | KeyCode::Char('A')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                CompoundSplitAction::StageAllAndReview
            }

            // Cancel flow
            KeyCode::Esc => CompoundSplitAction::Cancelled,

            _ => CompoundSplitAction::None,
        }
    }
}
