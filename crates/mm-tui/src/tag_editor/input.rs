//! Tag Editor Input Handling
//!
//! Delegates field editing to FieldFormState, handles modals and button
//! actions locally.

use mm_ui::field_form::FieldEditMode;
use mm_ui::tag_editor_state::FocusPane;

use crate::input::InputAction;

use super::state::UnifiedTagEditorState;
use super::types::{
    NavigationDirection, StageChangesButton, TagEditorLaunchMode,
    UnifiedTagEditorAction, UnifiedTagEditorModal, UnsavedChangesButton,
};

impl UnifiedTagEditorState {
    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_input(action);
        }

        match self.core.focus {
            FocusPane::Content => self.handle_content_input(action),
            FocusPane::Buttons => self.handle_buttons_input(action),
        }
    }

    // ========================================================================
    // Modal input
    // ========================================================================

    fn handle_modal_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        match &mut self.modal {
            Some(UnifiedTagEditorModal::UnsavedChanges { selected_button }) => {
                match action {
                    InputAction::Confirm => {
                        match selected_button {
                            UnsavedChangesButton::KeepEditing => {
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                            UnsavedChangesButton::DiscardAndProceed => {
                                self.drop_changes_for_current_item();
                                self.modal = None;
                                if self.is_embedded() {
                                    UnifiedTagEditorAction::CloseEmbedded
                                } else {
                                    UnifiedTagEditorAction::DiscardTransaction
                                }
                            }
                        }
                    }
                    InputAction::Cancel => {
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    InputAction::NavLeft | InputAction::NavRight | InputAction::CycleNext => {
                        *selected_button = match selected_button {
                            UnsavedChangesButton::KeepEditing => {
                                UnsavedChangesButton::DiscardAndProceed
                            }
                            UnsavedChangesButton::DiscardAndProceed => {
                                UnsavedChangesButton::KeepEditing
                            }
                        };
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::MultiValueEditor {
                field_idx,
                values,
                current_value_idx,
                editing,
                edit_input,
            }) => {
                let num_values = values.len();
                let add_entry_idx = num_values;

                match action {
                    InputAction::Cancel => {
                        if *editing {
                            *editing = false;
                            edit_input.clear();
                        } else {
                            // Close modal — apply changes to the TagSet
                            let field_idx_copy = *field_idx;
                            let values_copy = values.clone();
                            self.apply_multi_value_changes(field_idx_copy, values_copy);
                            self.modal = None;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Confirm => {
                        if *editing {
                            if *current_value_idx < num_values {
                                values[*current_value_idx] = edit_input.value().to_string();
                            } else if *current_value_idx == add_entry_idx && !edit_input.is_empty()
                            {
                                values.push(edit_input.value().to_string());
                            }
                            *editing = false;
                            edit_input.clear();
                        } else if *current_value_idx < num_values {
                            *editing = true;
                            edit_input.set_value(&values[*current_value_idx]);
                        } else if *current_value_idx == add_entry_idx {
                            *editing = true;
                            edit_input.clear();
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavUp => {
                        if !*editing && *current_value_idx > 0 {
                            *current_value_idx -= 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavDown => {
                        if !*editing && *current_value_idx < add_entry_idx {
                            *current_value_idx += 1;
                        }
                        UnifiedTagEditorAction::None
                    }
                    InputAction::Delete | InputAction::Backspace if !*editing => {
                        if *current_value_idx < num_values && num_values > 0 {
                            values.remove(*current_value_idx);
                            if *current_value_idx >= values.len() && !values.is_empty() {
                                *current_value_idx = values.len() - 1;
                            }
                        }
                        UnifiedTagEditorAction::None
                    }
                    _ if *editing => {
                        edit_input.handle_input(action);
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            Some(UnifiedTagEditorModal::StageChangesConfirm {
                direction,
                selected_button,
            }) => {
                match action {
                    InputAction::Confirm => {
                        let direction = *direction;
                        match selected_button {
                            StageChangesButton::Yes => {
                                let mutations = self.generate_mutations_for_current_item();
                                let key_item = self.decision_key_item(&mutations);
                                self.modal = None;
                                UnifiedTagEditorAction::StageDecisionAndNavigate {
                                    key: mm_ui::decision_keys::tag_edit(key_item),
                                    mutations,
                                    direction,
                                }
                            }
                            StageChangesButton::No => {
                                self.modal = None;
                                match direction {
                                    NavigationDirection::Next => UnifiedTagEditorAction::NextItem,
                                    NavigationDirection::Prev => UnifiedTagEditorAction::PrevItem,
                                }
                            }
                            StageChangesButton::Cancel => {
                                self.modal = None;
                                UnifiedTagEditorAction::CloseModal
                            }
                        }
                    }
                    InputAction::Cancel => {
                        self.modal = None;
                        UnifiedTagEditorAction::CloseModal
                    }
                    InputAction::NavLeft => {
                        *selected_button = match selected_button {
                            StageChangesButton::Yes => StageChangesButton::Yes,
                            StageChangesButton::No => StageChangesButton::Yes,
                            StageChangesButton::Cancel => StageChangesButton::No,
                        };
                        UnifiedTagEditorAction::None
                    }
                    InputAction::NavRight | InputAction::CycleNext => {
                        *selected_button = match selected_button {
                            StageChangesButton::Yes => StageChangesButton::No,
                            StageChangesButton::No => StageChangesButton::Cancel,
                            StageChangesButton::Cancel => StageChangesButton::Cancel,
                        };
                        UnifiedTagEditorAction::None
                    }
                    _ => UnifiedTagEditorAction::None,
                }
            }
            None => UnifiedTagEditorAction::None,
        }
    }

    // ========================================================================
    // Content pane input (field form)
    // ========================================================================

    fn handle_content_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        // Cancel handling — needs to check for unsaved changes before delegating
        if matches!(action, InputAction::Cancel) {
            if self.core.form.edit_mode != FieldEditMode::Navigating {
                self.core.form.edit_mode = FieldEditMode::Navigating;
                return UnifiedTagEditorAction::None;
            } else if self.has_changes_for_current_item() {
                self.modal = Some(UnifiedTagEditorModal::UnsavedChanges {
                    selected_button: UnsavedChangesButton::default(),
                });
                return UnifiedTagEditorAction::None;
            } else if self.is_embedded() {
                return UnifiedTagEditorAction::CloseEmbedded;
            } else {
                return UnifiedTagEditorAction::DiscardTransaction;
            }
        }

        // Tab/Shift-Tab for item navigation (non-aggregated mode only)
        if matches!(action, InputAction::CycleNext | InputAction::CyclePrev) {
            if self.is_aggregated_mode() {
                return UnifiedTagEditorAction::None;
            }
            let direction = if matches!(action, InputAction::CycleNext) {
                NavigationDirection::Next
            } else {
                NavigationDirection::Prev
            };
            if self.has_changes_for_current_item() && !self.changes_match_staged() {
                self.modal = Some(UnifiedTagEditorModal::StageChangesConfirm {
                    direction,
                    selected_button: StageChangesButton::default(),
                });
                return UnifiedTagEditorAction::None;
            }
            return match direction {
                NavigationDirection::Next => UnifiedTagEditorAction::NextItem,
                NavigationDirection::Prev => UnifiedTagEditorAction::PrevItem,
            };
        }

        // Ctrl+R for review
        if matches!(action, InputAction::Shortcut('r')) {
            if self.is_embedded() {
                return UnifiedTagEditorAction::None;
            }
            return UnifiedTagEditorAction::RequestTransactionReview;
        }

        // Check for multi-value Enter (open modal for multi-value tags)
        if matches!(action, InputAction::Confirm)
            && self.core.form.edit_mode == FieldEditMode::Navigating
        {
            if let Some(entry) = self.current_tag_set().and_then(|ts| ts.get(self.core.form.cursor))
            {
                if entry.values.len() > 1 {
                    self.open_multi_value_editor();
                    return UnifiedTagEditorAction::None;
                }
            }
        }

        // Delegate to FieldFormState.
        // Extract the tag set index, then borrow form and tag_sets separately
        // to avoid double mutable borrow through self.
        let idx = if self.is_aggregated_mode() {
            0
        } else {
            self.core.current_file
        };
        if let Some(tag_set) = self.core.tag_sets.get_mut(idx) {
            let result = self.core.form.handle_input(action, tag_set);
            // NavDown at the bottom of the field list falls through as Unhandled —
            // switch focus to the buttons row.
            if result == mm_ui::field_form::FieldFormResult::Unhandled {
                if matches!(action, InputAction::NavDown) {
                    self.core.focus = FocusPane::Buttons;
                    return UnifiedTagEditorAction::None;
                }
            }
        }
        UnifiedTagEditorAction::None
    }

    // ========================================================================
    // Buttons pane input
    // ========================================================================

    fn handle_buttons_input(&mut self, action: &InputAction) -> UnifiedTagEditorAction {
        use mm_ui::tag_editor_state::TagEditorButtonAction;

        let ctx = self.core.button_ctx();

        match action {
            InputAction::Cancel => {
                self.core.focus = FocusPane::Content;
                UnifiedTagEditorAction::None
            }
            InputAction::NavUp => {
                // Return focus to content (tag fields above)
                self.core.focus = FocusPane::Content;
                UnifiedTagEditorAction::None
            }
            InputAction::NavLeft => {
                self.core.buttons.nav_left(&ctx);
                UnifiedTagEditorAction::None
            }
            InputAction::NavRight => {
                self.core.buttons.nav_right(&ctx);
                UnifiedTagEditorAction::None
            }
            InputAction::NavDown => {
                // Already at the bottom — no-op
                UnifiedTagEditorAction::None
            }
            InputAction::Confirm => {
                match self.core.buttons.confirm(&ctx) {
                    Some(TagEditorButtonAction::ReviewAll) => {
                        if self.is_embedded() {
                            let mutations = self.collect_all_mutations();
                            if mutations.is_empty() {
                                UnifiedTagEditorAction::CloseEmbedded
                            } else if let TagEditorLaunchMode::Embedded {
                                ref decision_key,
                                ref decision_label,
                            } = self.launch_mode
                            {
                                UnifiedTagEditorAction::StageAndCloseEmbedded {
                                    decision_key: decision_key.clone(),
                                    decision_label: decision_label.clone(),
                                    mutations,
                                }
                            } else {
                                unreachable!()
                            }
                        } else {
                            let current_unstaged =
                                self.has_changes_for_current_item() && !self.changes_match_staged();
                            let has_anything =
                                current_unstaged || self.core.staged_decision_count > 0;

                            if current_unstaged {
                                let mutations = self.generate_mutations_for_current_item();
                                let key_item = self.decision_key_item(&mutations);
                                UnifiedTagEditorAction::StageDecisionAndReview {
                                    key: mm_ui::decision_keys::tag_edit(key_item),
                                    mutations,
                                }
                            } else if has_anything {
                                UnifiedTagEditorAction::RequestTransactionReview
                            } else {
                                UnifiedTagEditorAction::StatusMessage(
                                    "No changes to review".to_string(),
                                )
                            }
                        }
                    }
                    Some(TagEditorButtonAction::Revert) => {
                        self.drop_changes_for_current_item();
                        UnifiedTagEditorAction::StatusMessage("Changes reverted".to_string())
                    }
                    Some(TagEditorButtonAction::Cancel) => {
                        if self.has_changes_for_current_item() {
                            self.modal = Some(UnifiedTagEditorModal::UnsavedChanges {
                                selected_button: UnsavedChangesButton::default(),
                            });
                            UnifiedTagEditorAction::None
                        } else if self.is_embedded() {
                            UnifiedTagEditorAction::CloseEmbedded
                        } else {
                            UnifiedTagEditorAction::DiscardTransaction
                        }
                    }
                    None => UnifiedTagEditorAction::None,
                }
            }
            _ => UnifiedTagEditorAction::None,
        }
    }

    // ========================================================================
    // Multi-value helpers
    // ========================================================================

    /// Get current TagSet (for individual mode).
    pub(crate) fn current_tag_set(&self) -> Option<&mm_ui::tag_set::TagSet> {
        if self.is_aggregated_mode() {
            // In aggregated mode, we don't have per-file tag sets for editing.
            // The FieldForm operates on the first file's tag set.
            // TODO: aggregated editing needs a different path.
            self.core.tag_sets.first()
        } else {
            self.core.tag_sets.get(self.core.current_file)
        }
    }

    /// Get current TagSet mutably.
    pub(crate) fn current_tag_set_mut(&mut self) -> Option<&mut mm_ui::tag_set::TagSet> {
        let idx = self.core.current_file;
        if self.is_aggregated_mode() {
            self.core.tag_sets.first_mut()
        } else {
            self.core.tag_sets.get_mut(idx)
        }
    }

    /// Open multi-value editor modal for current field.
    fn open_multi_value_editor(&mut self) {
        let tag_set = match self.current_tag_set() {
            Some(ts) => ts,
            None => return,
        };
        let entry = match tag_set.get(self.core.form.cursor) {
            Some(e) => e,
            None => return,
        };

        let values = entry.values.clone();
        self.modal = Some(UnifiedTagEditorModal::MultiValueEditor {
            field_idx: self.core.form.cursor,
            values,
            current_value_idx: 0,
            editing: false,
            edit_input: crate::widgets::TextInputState::new(),
        });
    }

    /// Apply multi-value changes back to the TagSet.
    fn apply_multi_value_changes(&mut self, entry_idx: usize, new_values: Vec<String>) {
        let tag_set = match self.current_tag_set_mut() {
            Some(ts) => ts,
            None => return,
        };

        let entry_name = match tag_set.get(entry_idx) {
            Some(e) => e.name.clone(),
            None => return,
        };

        // Remove the old entry and insert a new one with updated values
        tag_set.drop_entry(entry_idx);

        // Re-insert with new values at the same position
        for (i, value) in new_values.into_iter().enumerate() {
            if i == 0 {
                // Insert entry at the original position
                // add_entry appends, but we want positional insertion.
                // Use the lower-level approach: we already dropped, so entries shifted.
                // Add at end, then we'll fix position.
                tag_set.add_entry(entry_name.clone(), value);
            } else {
                // Add additional values
                tag_set.add_value(tag_set.entry_count() - 1, value);
            }
        }
    }
}
