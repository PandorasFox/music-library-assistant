//! Album Artist Phase Selector
//!
//! A single-selection dialogue for choosing which phase of the
//! album artist resolution flow to run.

use crossterm::event::{KeyCode, KeyEvent};

use super::types::{AlbumArtistPhase, PhaseSelectorAction};

/// State for the phase selector popup.
#[derive(Debug)]
pub struct PhaseSelectorState {
    /// Current cursor position (0 = Canonicalization, 1 = Collation, 2 = Population)
    cursor: usize,
    /// Number of phases available
    phase_count: usize,
}

impl Default for PhaseSelectorState {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseSelectorState {
    /// Create a new phase selector.
    pub fn new() -> Self {
        Self {
            cursor: 0,
            phase_count: AlbumArtistPhase::all().len(),
        }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> PhaseSelectorAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                PhaseSelectorAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < self.phase_count {
                    self.cursor += 1;
                }
                PhaseSelectorAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                // Select the current phase and proceed
                let phase = AlbumArtistPhase::all()[self.cursor];
                PhaseSelectorAction::Proceed(phase)
            }
            KeyCode::Esc => PhaseSelectorAction::Cancel,
            _ => PhaseSelectorAction::None,
        }
    }

    /// Get cursor position (0-indexed phase).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Get the currently highlighted phase.
    pub fn current_phase(&self) -> AlbumArtistPhase {
        AlbumArtistPhase::all()[self.cursor]
    }
}
