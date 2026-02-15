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
//! | Squash to: [The Correct Artist____________]                      |
//! +------------------------------------------------------------------+
//! | VALUES          | TRACKS (7)       | TAG VALUES                  |
//! |-----------------|------------------|----------------------------|
//! | [x] Variant A   | > 01 - Song.flac | artist: Variant A          |
//! | [x] Variant B   |   02 - Song.flac | album_artist: Various      |
//! | [ ] Variant C   |   03 - Song.flac | genre: Electronic          |
//! +-----------------+------------------+----------------------------+
//! | ^/v navigate | Space toggle | F fill | Tab next | ^R review      |
//! +------------------------------------------------------------------+
//! ```
//!
//! ## Key Behaviors
//!
//! - Left/Right: Switch focus between variants pane and files pane
//! - Up/Down: Navigate within focused pane; Up from first variant goes to text field (at top)
//! - Space: Toggle selection (variants pane only, when cursor >= 0)
//! - F: Fill text input from hovered variant value
//! - Enter: Confirm current squash, stage decision, and advance to next group
//! - Tab/Shift-Tab: Navigate to next/prev group (non-committal, does NOT stage)
//! - Ctrl+R: Stage current decision and jump to review screen
//! - Esc: Cancel entire modal

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
        // If flag confirmation popup is showing, intercept all input
        if self.flag_confirmation_pending {
            return match key.code {
                KeyCode::Enter => {
                    self.flag_confirmation_pending = false;
                    TagCanonicalityActionV2::FlagNonCompilation
                }
                KeyCode::Esc => {
                    self.flag_confirmation_pending = false;
                    TagCanonicalityActionV2::None
                }
                _ => TagCanonicalityActionV2::None,
            };
        }

        // Ctrl shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('r' | 'R') => return TagCanonicalityActionV2::ShowReview,
                KeyCode::Char('f' | 'F') => {
                    if self.is_album_artist_mode {
                        self.flag_confirmation_pending = true;
                        return TagCanonicalityActionV2::None;
                    } else {
                        return TagCanonicalityActionV2::FlagCanonical;
                    }
                }
                _ => {}
            }
        }

        // When the text input is focused, delegate to it first
        let text_input_focused =
            self.focus_pane == FocusPaneV2::Variants && self.variant_cursor == -1;

        match key.code {
            // Left/Right: text input cursor when editing, pane switch otherwise
            KeyCode::Left if text_input_focused => {
                self.canonical_input.handle_key(key);
                TagCanonicalityActionV2::None
            }
            KeyCode::Right if text_input_focused => {
                self.canonical_input.handle_key(key);
                TagCanonicalityActionV2::None
            }
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

            // E: jump to text field for editing
            KeyCode::Char('e' | 'E') if !text_input_focused => {
                self.variant_cursor = -1;
                self.canonical_input.focused = true;
                self.focus_pane = FocusPaneV2::Variants;
                TagCanonicalityActionV2::None
            }

            // T: open tag editor for current group (individual mode)
            KeyCode::Char('t') if !text_input_focused => {
                TagCanonicalityActionV2::OpenTagEditorIndividual
            }
            // Shift+T: open tag editor for current group (aggregated mode)
            KeyCode::Char('T') if !text_input_focused => {
                TagCanonicalityActionV2::OpenTagEditorAggregated
            }

            // Enter: confirm (check BEFORE text input handling to avoid swallowing Enter)
            KeyCode::Enter => {
                if self.can_submit() {
                    TagCanonicalityActionV2::Confirmed
                } else {
                    TagCanonicalityActionV2::None
                }
            }

            // Text input handling (when cursor is on text field, variant_cursor == -1)
            _ if self.focus_pane == FocusPaneV2::Variants && self.variant_cursor == -1 => {
                self.canonical_input.handle_key(key);
                TagCanonicalityActionV2::None
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
