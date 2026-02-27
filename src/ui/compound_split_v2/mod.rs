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
//! | Space toggle | Enter split | ^F keep | T edit tags                |
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
//! - Ctrl+F: Canonicalize (mark as single entity, don't split)
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

use crate::ui::input::InputAction;

impl CompoundSplitStateV2 {
    /// Handle semantic input for the modal.
    pub fn handle_input(&mut self, action: &InputAction) -> CompoundSplitActionV2 {
        // If confirming canonicalize, intercept all input
        if self.confirming_canonicalize {
            return match action {
                InputAction::Confirm => {
                    self.confirming_canonicalize = false;
                    CompoundSplitActionV2::Canonicalize
                }
                InputAction::Cancel => {
                    self.confirming_canonicalize = false;
                    CompoundSplitActionV2::None
                }
                _ => CompoundSplitActionV2::None,
            };
        }

        // If confirming bulk stage all, intercept all input
        if self.confirming_bulk_stage {
            return match action {
                InputAction::Confirm => {
                    self.confirming_bulk_stage = false;
                    CompoundSplitActionV2::StageAllAndReview
                }
                InputAction::Cancel => {
                    self.confirming_bulk_stage = false;
                    CompoundSplitActionV2::None
                }
                _ => CompoundSplitActionV2::None,
            };
        }

        // If editing, handle edit-specific input first
        if self.is_editing() {
            return self.handle_editing_input(action);
        }

        // Ctrl shortcuts (mapped to semantic actions by map_key)
        match action {
            InputAction::Shortcut('r') => return CompoundSplitActionV2::ShowReview,
            InputAction::OpenFilter => {
                self.confirming_canonicalize = true;
                return CompoundSplitActionV2::None;
            }
            // Ctrl+A maps to TextHome; in this context it means bulk confirm all (safe mode only)
            InputAction::TextHome if self.is_safe_mode => {
                self.confirming_bulk_stage = true;
                return CompoundSplitActionV2::None;
            }
            _ => {}
        }

        match action {
            // Pane switching with Left/Right
            InputAction::NavLeft => {
                self.focus_pane = FocusPaneV2::Parts;
                CompoundSplitActionV2::None
            }
            InputAction::NavRight => {
                self.focus_pane = FocusPaneV2::Files;
                CompoundSplitActionV2::None
            }

            // Navigation within pane
            InputAction::NavUp => {
                match self.focus_pane {
                    FocusPaneV2::Parts => self.part_cursor_up(),
                    FocusPaneV2::Files => self.file_cursor_up(),
                }
                CompoundSplitActionV2::None
            }
            InputAction::NavDown => {
                match self.focus_pane {
                    FocusPaneV2::Parts => self.part_cursor_down(),
                    FocusPaneV2::Files => self.file_cursor_down(),
                }
                CompoundSplitActionV2::None
            }

            // Toggle file selection (files pane only)
            InputAction::Toggle if self.focus_pane == FocusPaneV2::Files => {
                self.toggle_file_selection();
                CompoundSplitActionV2::None
            }

            // Edit part (review mode only, parts pane)
            InputAction::Char('e' | 'E')
                if self.focus_pane == FocusPaneV2::Parts && !self.is_safe_mode =>
            {
                self.start_editing();
                CompoundSplitActionV2::None
            }

            // T: open tag editor for current group (individual mode)
            InputAction::Char('t') => CompoundSplitActionV2::OpenTagEditorIndividual,
            // Shift+T: open tag editor for current group (aggregated mode)
            InputAction::Char('T') => CompoundSplitActionV2::OpenTagEditorAggregated,

            // Enter: confirm split
            InputAction::Confirm => {
                if self.can_submit() {
                    CompoundSplitActionV2::Confirmed
                } else {
                    CompoundSplitActionV2::None
                }
            }

            // Tab/Shift-Tab: navigate signals
            InputAction::CycleNext => CompoundSplitActionV2::Navigate { forward: true },
            InputAction::CyclePrev => CompoundSplitActionV2::Navigate { forward: false },

            // Esc: cancel
            InputAction::Cancel => CompoundSplitActionV2::Cancelled,

            _ => CompoundSplitActionV2::None,
        }
    }

    /// Handle input while editing a part.
    fn handle_editing_input(&mut self, action: &InputAction) -> CompoundSplitActionV2 {
        match action {
            InputAction::Confirm => {
                self.confirm_edit();
                CompoundSplitActionV2::None
            }
            InputAction::Cancel => {
                self.cancel_edit();
                CompoundSplitActionV2::None
            }
            _ => {
                self.part_input.handle_input(action);
                CompoundSplitActionV2::None
            }
        }
    }
}
