//! OOB (Out-of-Band) Tag Resolution + Moved File Acknowledgement

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::{moved_file_modal, oob_conflict_modal, ActiveView};

// ========================================================================
// OOB Resolution
// ========================================================================

impl HandleAction for oob_conflict_modal::OobAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::OobResolution(ref state) = app.view else {
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
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::OobResolution(ref state) = app.view else {
                        app.cancel_and_return_to_source("Resolution cancelled");
                        return;
                    };
                    state.cancel_message()
                };
                app.cancel_and_return_to_source(msg);
            }
            _ => {}
        }
    }
}

// ========================================================================
// Moved File Acknowledgement
// ========================================================================

impl HandleAction for moved_file_modal::MovedFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::MovedFileAcknowledge(ref state) = app.view else {
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
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::MovedFileAcknowledge(ref state) = app.view else {
                        app.cancel_and_return_to_source("Resolution cancelled");
                        return;
                    };
                    state.cancel_message()
                };
                app.cancel_and_return_to_source(msg);
            }
            _ => {}
        }
    }
}

// ========================================================================
// App helper methods
// ========================================================================

impl App {
    pub(crate) fn start_oob_conflict_inspection(&mut self) {
        let files = self.query(mm_meta::domain_queries::GetOobFiles { bucket: None });
        let _ = self.start_transaction("OOB tag resolution");
        let state = oob_conflict_modal::OobResolutionState::new(files);
        self.view = ActiveView::OobResolution(state);
    }

    pub(crate) fn start_moved_file_acknowledge(&mut self) {
        let files = self.query(mm_meta::domain_queries::GetMovedFiles);
        mm_meta::logging::log_general(format!(
            "Starting moved file acknowledgement: {} files", files.len()
        ));
        let _ = self.start_transaction("Moved file acknowledgement");
        let state = moved_file_modal::MovedFileState::new(moved_file_modal::MovedFileData { files });
        self.view = ActiveView::MovedFileAcknowledge(state);
    }
}
