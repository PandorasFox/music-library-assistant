//! Tag Canonicity Resolution V2 - Three-Pane Layout
//!
//! Provides a fullscreen three-pane interface for resolving tag canonicity issues:
//! - Left pane (25%): Variant values with toggle checkboxes
//! - Middle pane (35%): File list (navigable)
//! - Right pane (40%): Tag values for selected file (informational)
//!
//! ## Layout
//!
//! ```text
//! +------------------------------------------------------------------+
//! | Set "artist" variants (2/5) - Album: Clockwork Hearts            |
//! +------------------------------------------------------------------+
//! | VALUES          | TRACKS (7)       | TAG VALUES                  |
//! |-----------------|------------------|----------------------------|
//! | [x] Variant A   | > 01 - Song.flac | artist: Variant A          |
//! | [x] Variant B   |   02 - Song.flac | album_artist: Various      |
//! | [ ] Variant C   |   03 - Song.flac | genre: Electronic          |
//! +-----------------+------------------+----------------------------+
//! | Squash to: [The Correct Artist____________]                      |
//! +------------------------------------------------------------------+
//! | ^/v navigate | Space toggle | F fill | Tab next | ^R review      |
//! +------------------------------------------------------------------+
//! ```
//!
//! ## Key Behaviors
//!
//! - Left/Right: Switch focus between variants pane and files pane
//! - Up/Down: Navigate within focused pane; from variants pane, navigate into text field
//! - Space: Toggle selection (variants pane only, when cursor >= 0)
//! - F: Fill text input from hovered variant value
//! - Enter: Confirm current squash, stage decision, and advance to next group
//! - Tab/Shift-Tab: Navigate to next/prev group (non-committal, does NOT stage)
//! - Ctrl+R: Stage current decision and jump to review screen
//! - Esc: Cancel entire flow

pub mod render;
pub mod types;

pub use render::render;
pub use types::{
    FocusPaneV2, TagCanonicalityActionV2, TagCanonicalityModalDataV2,
    TagCanonicalityStateV2,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl TagCanonicalityStateV2 {
    /// Handle keyboard input for the modal.
    ///
    /// Returns the action to take based on the input.
    pub fn handle_key(&mut self, key: KeyEvent) -> TagCanonicalityActionV2 {
        // Ctrl+R: Show review
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('r' | 'R'))
        {
            return TagCanonicalityActionV2::ShowReview;
        }

        match key.code {
            // Pane switching with Left/Right
            KeyCode::Left => {
                self.focus_pane = FocusPaneV2::Variants;
                TagCanonicalityActionV2::None
            }
            KeyCode::Right => {
                self.focus_pane = FocusPaneV2::Files;
                TagCanonicalityActionV2::None
            }

            // Navigation within pane
            KeyCode::Up => {
                match self.focus_pane {
                    FocusPaneV2::Variants => self.variant_cursor_up(),
                    FocusPaneV2::Files => self.file_cursor_up(),
                }
                TagCanonicalityActionV2::None
            }
            KeyCode::Down => {
                match self.focus_pane {
                    FocusPaneV2::Variants => self.variant_cursor_down(),
                    FocusPaneV2::Files => self.file_cursor_down(),
                }
                TagCanonicalityActionV2::None
            }

            // Toggle selection (variants pane only, when on list item)
            KeyCode::Char(' ')
                if self.focus_pane == FocusPaneV2::Variants && self.variant_cursor >= 0 =>
            {
                self.toggle_selection();
                TagCanonicalityActionV2::None
            }

            // Fill from hover (F key) - variants pane only, when on list item
            KeyCode::Char('f' | 'F')
                if self.focus_pane == FocusPaneV2::Variants && self.variant_cursor >= 0 =>
            {
                if let Some(variant) = self.data.variants.get(self.variant_cursor as usize) {
                    self.canonical_input.set_value(variant.value.clone());
                }
                TagCanonicalityActionV2::None
            }

            // Text input handling (when cursor is on text field, variant_cursor == -1)
            _ if self.focus_pane == FocusPaneV2::Variants && self.variant_cursor == -1 => {
                self.canonical_input.handle_key(key);
                TagCanonicalityActionV2::None
            }

            // Enter: confirm
            KeyCode::Enter => {
                if self.can_submit() {
                    TagCanonicalityActionV2::Confirmed
                } else {
                    TagCanonicalityActionV2::None
                }
            }

            // Tab/Shift-Tab: navigate clusters
            KeyCode::Tab => TagCanonicalityActionV2::Navigate {
                forward: !key.modifiers.contains(KeyModifiers::SHIFT),
            },
            KeyCode::BackTab => TagCanonicalityActionV2::Navigate { forward: false },

            // Esc: cancel
            KeyCode::Esc => TagCanonicalityActionV2::Cancelled,

            _ => TagCanonicalityActionV2::None,
        }
    }
}
