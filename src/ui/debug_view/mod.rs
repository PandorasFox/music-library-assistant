//! Debug View
//!
//! A lateral view for maintenance and debugging operations.
//! Part of the lateral view ring - cycle with Tab/Shift-Tab.

pub mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub use render::render_debug_view;

/// Action returned from input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugAction {
    None,
    RequestQuit,
    CycleNext,
    CyclePrev,
    /// Trigger fingerprint regeneration for all tracks.
    RebuildFingerprints,
}

/// Available maintenance operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedOperation {
    #[default]
    RebuildFingerprints,
}

/// State for the debug view.
pub struct DebugViewState {
    /// Currently selected operation.
    pub selected: SelectedOperation,
    /// Whether the Witch is busy (blocks actions).
    pub witch_busy: bool,
    /// Total track count (cached on view init).
    pub track_count: i64,
    /// Count of tracks with fingerprints (cached on view init).
    pub fingerprinted_count: i64,
}

impl DebugViewState {
    /// Create a new debug view state.
    pub fn new(track_count: i64, fingerprinted_count: i64) -> Self {
        Self {
            selected: SelectedOperation::RebuildFingerprints,
            witch_busy: false,
            track_count,
            fingerprinted_count,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> DebugAction {
        match key.code {
            KeyCode::Esc => DebugAction::RequestQuit,

            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    DebugAction::CyclePrev
                } else {
                    DebugAction::CycleNext
                }
            }
            KeyCode::BackTab => DebugAction::CyclePrev,

            // Navigation (for future additional operations)
            KeyCode::Up | KeyCode::Char('k') => {
                // Currently only one operation, so no-op
                DebugAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Currently only one operation, so no-op
                DebugAction::None
            }

            KeyCode::Enter => {
                if self.witch_busy {
                    return DebugAction::None;
                }
                match self.selected {
                    SelectedOperation::RebuildFingerprints => DebugAction::RebuildFingerprints,
                }
            }

            _ => DebugAction::None,
        }
    }
}
