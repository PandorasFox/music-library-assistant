//! Manual Review Action Handlers
//!
//! Handles all actions from the V3 ManualReview modal: stash confirmation,
//! group navigation, and transaction management.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::resolutions::manual_review::{ReviewButton, ReviewButtonCtx};
use crate::manual_review_modal::types;
use crate::ActiveView;

// =========================================================================
// V3: Single-load manual review with packed data
// =========================================================================

impl HandleAction for mm_ui::resolutions::manual_review::ReviewAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::manual_review::ReviewAction;

        match self {
            ReviewAction::Stash => {
                let Some(g) = witness else { return };
                app.stage_stash_for_selected_file_v3(g);
            }
            ReviewAction::MarkExpected => {
                let Some(g) = witness else { return };
                app.stage_mark_expected_duplicate_v3(g);
            }
            ReviewAction::Cancel => {
                app.cancel_and_return_to_source("Manual review cancelled");
            }
        }
    }
}

impl App {
    /// Start V3 manual review from Insights view.
    pub(crate) fn start_manual_review_v3(&mut self, kind: types::ReviewKind) {
        let data = self
            .query(mm_meta::domain_queries::GetManualReviewData { kind });

        if data.groups.is_empty() {
            self.status_message = Some("No review groups found".to_string());
            return;
        }

        let _ = self.start_transaction(kind.transaction_label());

        self.view = ActiveView::ManualReviewResolution {
            data,
            review_kind: kind,
            current_group: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            focus: mm_ui::geometry::FocusPane::List,
        };
    }

    /// Stage StashFromZone + DropFromIndex for the file at cursor (V3).
    ///
    /// Stash is per-file (cursor position), not per-group.
    fn stage_stash_for_selected_file_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (group_idx, file_idx, corpus_path, inode, stash_name, review_kind) = {
            let ActiveView::ManualReviewResolution {
                ref data, review_kind, current_group, ref list, ..
            } = self.view
            else {
                return;
            };
            let Some(group) = data.groups.get(current_group) else {
                return;
            };
            let Some(file) = group.files.get(list.cursor) else {
                return;
            };
            if file.stashed {
                return;
            }
            (
                current_group,
                list.cursor,
                file.corpus_path.clone(),
                file.inode,
                review_kind.stash_name(),
                review_kind,
            )
        };

        let mutations =
            types::stash_file_mutations(&corpus_path, inode, stash_name, &self.resolver);

        let ctx = ReviewButtonCtx {
            has_files: true,
            review_kind,
            current_group_index: group_idx,
        };
        let key = ReviewButton::Stash.protocol_binding(&ctx)
            .decision_key().unwrap().clone();
        let label = format!("Stash {}", corpus_path);
        let decision = gesture.decide(&label, mutations);
        let _ = super::super::operator_decisions::stage_decision(self, key, decision);

        // Mark file as stashed in the UI state — stay on the same group
        if let ActiveView::ManualReviewResolution {
            ref mut data, ..
        } = self.view
        {
            if let Some(group) = data.groups.get_mut(group_idx) {
                if let Some(file) = group.files.get_mut(file_idx) {
                    file.stashed = true;
                }
            }
        }
    }

    /// Stage EmitExpectedDuplicate for the current group (V3).
    ///
    /// Only valid for RedundantDuplicate review kind.
    fn stage_mark_expected_duplicate_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (group_idx, fingerprint_key, group_label, review_kind) = {
            let ActiveView::ManualReviewResolution {
                ref data, review_kind, current_group, ..
            } = self.view
            else {
                return;
            };
            if review_kind != types::ReviewKind::RedundantDuplicate {
                return;
            }
            let Some(group) = data.groups.get(current_group) else {
                return;
            };
            let Some(ref key) = group.signal_key else {
                return;
            };
            (current_group, key.clone(), group.label.clone(), review_kind)
        };

        let ctx = ReviewButtonCtx {
            has_files: true,
            review_kind,
            current_group_index: group_idx,
        };
        let key = ReviewButton::MarkExpected.protocol_binding(&ctx)
            .decision_key().unwrap().clone();
        let mutation = mm_meta::mutations::Mutation::EmitExpectedDuplicate(
            mm_meta::mutations::indexing::EmitExpectedDuplicateMutation { fingerprint_key },
        );

        let label = format!("Mark expected duplicate: {}", group_label);
        let decision = gesture.decide(&label, vec![mutation]);
        let _ = super::super::operator_decisions::stage_decision(self, key, decision);

        // Advance to next group or show review
        if let ActiveView::ManualReviewResolution {
            ref data,
            ref mut current_group,
            ref mut list,
            ..
        } = self.view
        {
            if *current_group + 1 < data.groups.len() {
                *current_group += 1;
                list.reset();
            } else {
                self.after_staging_decisions();
            }
        } else {
            self.after_staging_decisions();
        }
    }
}
