//! State and input handling for the external match review modal.

use ratatui::layout::Rect;

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::bulk_selection::BulkSelectionState;
use crate::ui::input::InputAction;

use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::widgets::{ButtonRects, FocusPane, ListClickTargets, rect_contains};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalMatchButton {
    Accept,
    DropSelected,
    Dismiss,
    Cancel,
}

impl ExternalMatchButton {
    pub fn left(self) -> Self {
        match self {
            Self::Accept => Self::Accept,
            Self::DropSelected => Self::Accept,
            Self::Dismiss => Self::DropSelected,
            Self::Cancel => Self::Dismiss,
        }
    }

    pub fn right(self) -> Self {
        match self {
            Self::Accept => Self::DropSelected,
            Self::DropSelected => Self::Dismiss,
            Self::Dismiss => Self::Cancel,
            Self::Cancel => Self::Cancel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalMatchReviewAction {
    None,
    /// Apply external tag values for the selected file.
    Accept,
    /// Drop external match data for all selected files.
    DropSelected,
    /// Skip this file without mutation.
    Dismiss,
    /// Discard transaction and return to Insights.
    Cancel,
    /// Open MusicBrainz recording URL in browser.
    OpenRecordingUrl(String),
}

pub struct ExternalMatchReviewState {
    pub entries: Vec<ExternalMatchReviewEntry>,
    pub cursor: usize,
    pub scroll: usize,
    pub selected_button: ExternalMatchButton,
    pub focus_pane: FocusPane,
    pub button_rects: ButtonRects,
    pub click_targets: ListClickTargets,
    /// Click rect for the MusicBrainz recording link (set during render).
    pub recording_link_rect: Option<Rect>,
    /// Multi-selection state for bulk drop operations.
    pub selection: BulkSelectionState,
    /// Scroll offset for the tag diff table in the details pane.
    pub diff_scroll: usize,
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
            click_targets: ListClickTargets::new(),
            recording_link_rect: None,
            selection: BulkSelectionState::new(),
            diff_scroll: 0,
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
            self.diff_scroll = 0;
            true
        } else {
            false
        }
    }

    /// MusicBrainz recording URL for the current entry.
    pub fn current_recording_url(&self) -> Option<String> {
        self.entries.get(self.cursor).map(|e| {
            format!("https://musicbrainz.org/recording/{}", e.recording_id)
        })
    }

    /// Handle a mouse click at (x, y). Returns an action if a button was clicked.
    pub fn handle_click(&mut self, x: u16, y: u16, _gesture: &ConfirmationGesture) -> Option<ExternalMatchReviewAction> {
        // Check buttons first (highest priority, triggers action)
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            self.focus_pane = FocusPane::Buttons;
            return match button_name {
                "accept" => {
                    self.selected_button = ExternalMatchButton::Accept;
                    Some(ExternalMatchReviewAction::Accept)
                }
                "drop_selected" => {
                    self.selected_button = ExternalMatchButton::DropSelected;
                    Some(ExternalMatchReviewAction::DropSelected)
                }
                "dismiss" => {
                    self.selected_button = ExternalMatchButton::Dismiss;
                    Some(ExternalMatchReviewAction::Dismiss)
                }
                "cancel" => {
                    self.selected_button = ExternalMatchButton::Cancel;
                    Some(ExternalMatchReviewAction::Cancel)
                }
                _ => None,
            };
        }
        // Check MusicBrainz recording link
        if let Some(rect) = self.recording_link_rect {
            if rect_contains(rect, x, y) {
                if let Some(url) = self.current_recording_url() {
                    return Some(ExternalMatchReviewAction::OpenRecordingUrl(url));
                }
            }
        }
        // Check list items (focus + cursor change, no action)
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.entries.len() {
                    self.focus_pane = FocusPane::List;
                    self.cursor = idx;
                    self.diff_scroll = 0;
                }
            }
        }
        None
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
                    self.diff_scroll = 0;
                }
                ExternalMatchReviewAction::None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                    self.diff_scroll = 0;
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

            // Space: toggle selection on current entry
            InputAction::Toggle => {
                self.selection.toggle(self.cursor);
                ExternalMatchReviewAction::None
            }

            // Enter: confirm selected button
            InputAction::Confirm => {
                if self.focus_pane == FocusPane::Buttons {
                    match self.selected_button {
                        ExternalMatchButton::Accept => ExternalMatchReviewAction::Accept,
                        ExternalMatchButton::DropSelected => ExternalMatchReviewAction::DropSelected,
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
