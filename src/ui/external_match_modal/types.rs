//! State and input handling for the external match review modal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::widgets::{ButtonRects, FocusPane};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalMatchButton {
    Accept,
    Dismiss,
    Cancel,
}

impl ExternalMatchButton {
    pub fn left(self) -> Self {
        match self {
            Self::Accept => Self::Accept,
            Self::Dismiss => Self::Accept,
            Self::Cancel => Self::Dismiss,
        }
    }

    pub fn right(self) -> Self {
        match self {
            Self::Accept => Self::Dismiss,
            Self::Dismiss => Self::Cancel,
            Self::Cancel => Self::Cancel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalMatchReviewAction {
    None,
    /// Apply external tag values for the selected file.
    Accept,
    /// Skip this file without mutation.
    Dismiss,
    /// Discard transaction and return to Insights.
    Cancel,
}

pub struct ExternalMatchReviewState {
    pub entries: Vec<ExternalMatchReviewEntry>,
    pub cursor: usize,
    pub scroll: usize,
    pub selected_button: ExternalMatchButton,
    pub focus_pane: FocusPane,
    pub button_rects: ButtonRects,
}

impl ExternalMatchReviewState {
    pub fn new(entries: Vec<ExternalMatchReviewEntry>) -> Self {
        Self {
            entries,
            cursor: 0,
            scroll: 0,
            selected_button: ExternalMatchButton::Accept,
            focus_pane: FocusPane::List,
            button_rects: ButtonRects::new(),
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.entries.get(self.cursor).map(|e| e.path.as_str())
    }

    pub fn current_entry(&self) -> Option<&ExternalMatchReviewEntry> {
        self.entries.get(self.cursor)
    }

    /// Advance cursor to next entry. Returns true if advanced, false if at end.
    pub fn advance(&mut self) -> bool {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ExternalMatchReviewAction {
        // Shift+Up / Shift+Down: cycle focus pane
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return ExternalMatchReviewAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return ExternalMatchReviewAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            // Up/Down: navigate list (regardless of focus)
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ExternalMatchReviewAction::None
            }
            KeyCode::Down => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                }
                ExternalMatchReviewAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            KeyCode::Left => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                ExternalMatchReviewAction::None
            }
            KeyCode::Right => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                ExternalMatchReviewAction::None
            }

            // Enter: confirm selected button
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        ExternalMatchButton::Accept => ExternalMatchReviewAction::Accept,
                        ExternalMatchButton::Dismiss => ExternalMatchReviewAction::Dismiss,
                        ExternalMatchButton::Cancel => ExternalMatchReviewAction::Cancel,
                    }
                } else {
                    ExternalMatchReviewAction::None
                }
            }

            KeyCode::Esc => ExternalMatchReviewAction::Cancel,

            _ => ExternalMatchReviewAction::None,
        }
    }
}
