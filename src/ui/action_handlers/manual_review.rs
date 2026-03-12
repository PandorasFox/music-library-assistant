//! Manual Review Action Handlers
//!
//! Handles all actions from the ManualReview modal: stash confirmation,
//! tag editor launch, group navigation, and transaction management.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::ui::manual_review_modal::types;
use crate::ui::{manual_review_modal, tag_editor, ActiveView};
use crate::witch::WitchClient;

impl HandleAction for manual_review_modal::ManualReviewAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use manual_review_modal::ManualReviewAction;

        match self {
            ManualReviewAction::None => {}

            ManualReviewAction::Cancel => {
                app.cancel_and_return_to_source("Manual review cancelled");
            }

            ManualReviewAction::RequestStash => {
                // Popup is already shown by the state - no action handler needed
            }

            ManualReviewAction::CancelStash => {
                // Popup dismissed by state - no action handler needed
            }

            ManualReviewAction::ConfirmStash => {
                let Some(g) = witness else { return };
                app.stage_stash_for_selected_file(g);
            }

            ManualReviewAction::ConfirmStashAndAdvance => {
                let Some(g) = witness else { return };
                app.stage_stash_for_selected_file(g);
                // Advance to next group
                app.advance_manual_review_group(true);
            }

            ManualReviewAction::NavigateGroup(forward) => {
                app.advance_manual_review_group(forward);
            }

            ManualReviewAction::ShowReview => {
                app.after_staging_decisions();
            }

            ManualReviewAction::OpenTagEditorIndividual => {
                app.open_manual_review_tag_editor(false);
            }

            ManualReviewAction::OpenTagEditorAggregated => {
                app.open_manual_review_tag_editor(true);
            }

            ManualReviewAction::MarkExpectedDuplicate => {
                let Some(g) = witness else { return };
                app.stage_mark_expected_duplicate(g);
            }
        }
    }
}

impl App {
    /// Start manual review modal from Insights view.
    ///
    /// Loads data from the database based on review kind, starts a transaction,
    /// and switches to the ManualReview view.
    pub(in crate::ui) fn start_manual_review(&mut self, kind: types::ReviewKind) {
        let data = self
            .cache
            .domain_query(crate::db::domain::GetManualReviewData { kind })
            .recv();

        // Start transaction for the review session
        let _ = self.witch.start_transaction(kind.transaction_label());

        let state = manual_review_modal::ManualReviewState::new(kind, data);
        self.view = ActiveView::ManualReview(state);
    }

    /// Stage StashFromZone + DropFromIndex mutations for the currently selected file.
    fn stage_stash_for_selected_file(&mut self, gesture: &witness::ConfirmationGesture) {
        let (group_idx, file_idx, corpus_path, inode, stash_name) = {
            let ActiveView::ManualReview(ref state) = self.view else {
                return;
            };
            let Some(file) = state.selected_file() else {
                return;
            };
            if file.stashed {
                return;
            }
            (
                state.current_group,
                state.file_cursor,
                file.corpus_path.clone(),
                file.inode,
                state.kind.stash_name(),
            )
        };

        let mutations = types::stash_file_mutations(&corpus_path, inode, stash_name);

        // Stage the decision
        let label = format!("Stash {}", corpus_path);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::ManualReview {
                group_index: group_idx,
            },
            &label,
            mutations,
            gesture,
        );

        // Mark file as stashed in the UI state
        if let ActiveView::ManualReview(ref mut state) = self.view {
            state.mark_stashed(group_idx, file_idx);
        }
    }

    /// Stage an EmitExpectedDuplicate mutation for the current group's fingerprint key.
    fn stage_mark_expected_duplicate(&mut self, gesture: &witness::ConfirmationGesture) {
        let (group_idx, fingerprint_key, group_label, is_last) = {
            let ActiveView::ManualReview(ref state) = self.view else {
                return;
            };
            if state.kind != types::ReviewKind::RedundantDuplicate {
                return;
            }
            let Some(group) = state.current_group_ref() else {
                return;
            };
            let Some(ref key) = group.signal_key else {
                return;
            };
            (
                state.current_group,
                key.clone(),
                group.label.clone(),
                state.current_group + 1 >= state.data.groups.len(),
            )
        };

        let mutation = crate::meta::mutations::Mutation::EmitExpectedDuplicate(
            crate::meta::mutations::indexing::EmitExpectedDuplicateMutation { fingerprint_key },
        );

        let label = format!("Mark expected duplicate: {}", group_label);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::ManualReview {
                group_index: group_idx,
            },
            &label,
            vec![mutation],
            gesture,
        );

        // Advance to next group or show review if at the end
        if is_last {
            self.after_staging_decisions();
        } else {
            self.advance_manual_review_group(true);
        }
    }

    /// Navigate to next/prev group in the manual review.
    fn advance_manual_review_group(&mut self, forward: bool) {
        if let ActiveView::ManualReview(ref mut state) = self.view {
            if forward {
                if state.current_group + 1 < state.data.groups.len() {
                    state.current_group += 1;
                    state.file_cursor = 0;
                }
            } else if state.current_group > 0 {
                state.current_group -= 1;
                state.file_cursor = 0;
            }
        }
    }

    /// Open the embedded tag editor from within the manual review modal.
    fn open_manual_review_tag_editor(&mut self, aggregated: bool) {
        let (inodes, decision_key, decision_label) = {
            let ActiveView::ManualReview(ref state) = self.view else {
                return;
            };
            if !state.kind.supports_tag_edit() {
                return;
            }

            let group_inodes = state.current_group_inodes();
            if group_inodes.is_empty() {
                return;
            }

            let inodes = if aggregated {
                group_inodes
            } else {
                state
                    .selected_file()
                    .filter(|f| !f.stashed)
                    .map(|f| vec![f.inode])
                    .unwrap_or_default()
            };

            if inodes.is_empty() {
                return;
            }

            let label = state
                .current_group_ref()
                .map(|g| g.label.clone())
                .unwrap_or_else(|| "Manual review".to_string());

            (
                inodes,
                DecisionKey::ManualReview {
                    group_index: state.current_group,
                },
                label,
            )
        };

        let mode = if aggregated {
            tag_editor::TagEditorMode::Aggregated
        } else {
            tag_editor::TagEditorMode::Individual
        };
        self.open_tag_editor_for_inodes(inodes, Zone::Corpus, decision_key, decision_label, mode);
    }
}
