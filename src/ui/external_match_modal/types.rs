//! State and input handling for the external match review modal.

use crate::ui::input::InputAction;

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

    pub fn handle_input(&mut self, action: &InputAction) -> ExternalMatchReviewAction {
        // FocusUp / FocusDown: cycle focus pane
        match action {
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                return ExternalMatchReviewAction::None;
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                return ExternalMatchReviewAction::None;
            }
            _ => {}
        }

        match action {
            // Up/Down: navigate list (regardless of focus)
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ExternalMatchReviewAction::None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                }
                ExternalMatchReviewAction::None
            }

            // Left/Right: navigate buttons when focused on buttons pane
            InputAction::NavLeft => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.left();
                }
                ExternalMatchReviewAction::None
            }
            InputAction::NavRight => {
                if self.focus_pane == FocusPane::Buttons {
                    self.selected_button = self.selected_button.right();
                }
                ExternalMatchReviewAction::None
            }

            // Enter: confirm selected button
            InputAction::Confirm => {
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

            InputAction::Cancel => ExternalMatchReviewAction::Cancel,

            _ => ExternalMatchReviewAction::None,
        }
    }
}
