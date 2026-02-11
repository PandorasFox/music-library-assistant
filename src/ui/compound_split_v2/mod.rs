//! Compound Tag Split Resolution V2 - Three-Pane Layout
//!
//! Provides a fullscreen three-pane interface for splitting compound tag values:
//! - Left pane (25%): Split parts with exists/new indicators
//! - Middle pane (35%): Files with selection checkboxes (for exclusion)
//! - Right pane (40%): Tag values for selected file (informational)
//!
//! ## Layout
//!
//! ```text
//! +------------------------------------------------------------------+
//! | Split "artist" compound (3/47) - Safe                            |
//! +------------------------------------------------------------------+
//! | SPLIT PARTS     | TRACKS (1/1)     | TAG VALUES                  |
//! |-----------------|------------------|----------------------------|
//! | [ok] Priority   | [x] 01-Track.flac| artist: Priority & TwoThirds|
//! | [ok] TwoThirds  |                  | album: Collabs              |
//! |                 |                  | genre: Electronic           |
//! +-----------------+------------------+----------------------------+
//! | ^/v nav | </> pane | Space toggle | Enter split | ^Q keep        |
//! +------------------------------------------------------------------+
//! ```
//!
//! ## Key Behaviors
//!
//! - Left/Right: Switch focus between parts pane and files pane
//! - Up/Down: Navigate within focused pane
//! - Space: Toggle file selection (files pane only)
//! - E: Edit part value (review mode only, parts pane)
//! - Enter: Confirm split, stage decision, advance to next
//! - Ctrl+Q: Canonicalize (mark as single entity, don't split)
//! - Tab/Shift-Tab: Navigate to next/prev signal (non-committal)
//! - Ctrl+R: Stage current decision and jump to review screen
//! - Ctrl+A: Stage ALL signals and jump to review (safe mode only)
//! - Esc: Cancel entire modal (or cancel edit if editing)

pub mod render;
pub mod types;

pub use render::render;
pub use types::{
    CompoundSplitActionV2, CompoundSplitClustersV2, CompoundSplitDataV2,
    CompoundSplitStateV2, FocusPaneV2,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl CompoundSplitStateV2 {
    /// Handle keyboard input for the modal.
    pub fn handle_key(&mut self, key: KeyEvent) -> CompoundSplitActionV2 {
        // If confirming canonicalize, intercept all input
        if self.confirming_canonicalize {
            return match key.code {
                KeyCode::Enter => {
                    self.confirming_canonicalize = false;
                    CompoundSplitActionV2::Canonicalize
                }
                KeyCode::Esc => {
                    self.confirming_canonicalize = false;
                    CompoundSplitActionV2::None
                }
                _ => CompoundSplitActionV2::None,
            };
        }

        // If editing, handle edit-specific keys first
        if self.is_editing() {
            return self.handle_editing_key(key);
        }

        // Ctrl shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('r' | 'R') => return CompoundSplitActionV2::ShowReview,
                KeyCode::Char('q' | 'Q') => {
                    self.confirming_canonicalize = true;
                    return CompoundSplitActionV2::None;
                }
                // Ctrl+A only available in safe mode (bulk confirm all)
                KeyCode::Char('a' | 'A') if self.is_safe_mode => {
                    return CompoundSplitActionV2::StageAllAndReview;
                }
                _ => {}
            }
        }

        match key.code {
            // Pane switching with Left/Right
            KeyCode::Left => {
                self.focus_pane = FocusPaneV2::Parts;
                CompoundSplitActionV2::None
            }
            KeyCode::Right => {
                self.focus_pane = FocusPaneV2::Files;
                CompoundSplitActionV2::None
            }

            // Navigation within pane
            KeyCode::Up => {
                match self.focus_pane {
                    FocusPaneV2::Parts => self.part_cursor_up(),
                    FocusPaneV2::Files => self.file_cursor_up(),
                }
                CompoundSplitActionV2::None
            }
            KeyCode::Down => {
                match self.focus_pane {
                    FocusPaneV2::Parts => self.part_cursor_down(),
                    FocusPaneV2::Files => self.file_cursor_down(),
                }
                CompoundSplitActionV2::None
            }

            // Toggle file selection (files pane only)
            KeyCode::Char(' ') if self.focus_pane == FocusPaneV2::Files => {
                self.toggle_file_selection();
                CompoundSplitActionV2::None
            }

            // Edit part (review mode only, parts pane)
            KeyCode::Char('e' | 'E')
                if self.focus_pane == FocusPaneV2::Parts && !self.is_safe_mode =>
            {
                self.start_editing();
                CompoundSplitActionV2::None
            }

            // Enter: confirm split
            KeyCode::Enter => {
                if self.can_submit() {
                    CompoundSplitActionV2::Confirmed
                } else {
                    CompoundSplitActionV2::None
                }
            }

            // Tab/Shift-Tab: navigate signals
            KeyCode::Tab => CompoundSplitActionV2::Navigate {
                forward: !key.modifiers.contains(KeyModifiers::SHIFT),
            },
            KeyCode::BackTab => CompoundSplitActionV2::Navigate { forward: false },

            // Esc: cancel
            KeyCode::Esc => CompoundSplitActionV2::Cancelled,

            _ => CompoundSplitActionV2::None,
        }
    }

    /// Handle keyboard input while editing a part.
    fn handle_editing_key(&mut self, key: KeyEvent) -> CompoundSplitActionV2 {
        match key.code {
            KeyCode::Enter => {
                self.confirm_edit();
                CompoundSplitActionV2::None
            }
            KeyCode::Esc => {
                self.cancel_edit();
                CompoundSplitActionV2::None
            }
            _ => {
                self.part_input.handle_key(key);
                CompoundSplitActionV2::None
            }
        }
    }
}
