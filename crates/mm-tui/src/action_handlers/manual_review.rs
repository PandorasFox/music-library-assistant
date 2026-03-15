//! Manual Review Action Handler
//!
//! Uses Dispatchable for mutation building. ManualReview::Stash uses StageKeep
//! (per-file stash without advancing) and needs post-dispatch file marking.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::ActiveView;
use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};
use mm_ui::resolutions::manual_review::ReviewAction;

impl HandleAction for ReviewAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        let result = {
            let ActiveView::ManualReviewResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                if mutations.is_empty() { return; }
                let decision = w.decide(&label, mutations);
                let _ = super::super::operator_decisions::stage_decision(app, key, decision);
                if let ActiveView::ManualReviewResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::StageKeep { key, label, mutations } => {
                let Some(w) = witness else { return };
                if mutations.is_empty() { return; }
                // Capture cursor before staging
                let file_idx = if let ActiveView::ManualReviewResolution(ref state) = app.view {
                    state.list.cursor
                } else {
                    return;
                };
                let decision = w.decide(&label, mutations);
                let _ = super::super::operator_decisions::stage_decision(app, key, decision);
                // Post-dispatch: mark file as stashed in UI state
                if let ActiveView::ManualReviewResolution(ref mut state) = app.view {
                    if let Some(group) = state.data.inner.groups.get_mut(state.data.current_group) {
                        if let Some(file) = group.files.get_mut(file_idx) {
                            file.stashed = true;
                        }
                    }
                }
            }
            DispatchResult::Cancel => {
                app.cancel_and_return_to_source("Manual review cancelled");
            }
            DispatchResult::Skip | DispatchResult::Handled => {}
        }
    }
}

impl App {
    /// Start V3 manual review from Insights view.
    pub(crate) fn start_manual_review_v3(
        &mut self,
        kind: mm_meta::views::review_match::ReviewKind,
    ) {
        use mm_ui::resolutions::manual_review::{ManualReviewResolutionData, ManualReviewState};

        let data = self.query(mm_meta::domain_queries::GetManualReviewData { kind });

        if data.groups.is_empty() {
            self.status_message = Some("No review groups found".to_string());
            return;
        }

        let _ = self.start_transaction(kind.transaction_label());

        let state = ManualReviewState::new(ManualReviewResolutionData::new(data, kind));
        self.view = ActiveView::ManualReviewResolution(state);
    }
}
