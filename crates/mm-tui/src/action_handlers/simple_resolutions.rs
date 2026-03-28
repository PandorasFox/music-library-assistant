//! Simple Resolution Modals
//!
//! Handles missing file, missing directory, corrupt file, lossless remux,
//! subpar duplicate, and directory overlap resolution modals.
//! These flows share a common pattern: load data, show preview, stage mutations.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::{
    corrupt_file_modal, lossless_remux_modal, missing_directory_modal, missing_file_modal,
    subpar_duplicate_modal, ActiveView,
};

/// Start a simple resolution modal: load data via domain query, create preview, set view.
macro_rules! start_resolution {
    ($self:ident, $query:expr, $preview:path, $view:ident) => {{
        let data = $self
            .query($query);
        let preview = <$preview>::new(data);
        $self.view = ActiveView::$view(preview);
    }};
}

// =========================================================================
// Missing File Resolution
// =========================================================================

impl HandleAction for missing_file_modal::MissingFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::MissingFileResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                app.stage_mutations_with_transaction(mutations, &label, key, w);
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::MissingFileResolution(ref state) = app.view else {
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

// =========================================================================
// Missing Directory Resolution
// =========================================================================

impl HandleAction for missing_directory_modal::MissingDirectoryAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::MissingDirectoryResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                app.stage_mutations_with_transaction(mutations, &label, key, w);
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::MissingDirectoryResolution(ref state) = app.view else {
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
// Corrupt File Resolution
// ========================================================================

impl HandleAction for corrupt_file_modal::CorruptFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::CorruptFileResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                app.stage_mutations_with_transaction(mutations, &label, key, w);
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::CorruptFileResolution(ref state) = app.view else {
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
// Lossless Remux Resolution
// ========================================================================

impl HandleAction for lossless_remux_modal::LosslessRemuxAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::LosslessRemuxResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                app.stage_mutations_with_transaction(mutations, &label, key, w);
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::LosslessRemuxResolution(ref state) = app.view else {
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
// Subpar Duplicate Resolution
// ========================================================================

impl HandleAction for subpar_duplicate_modal::SubparDuplicateAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::SubparDuplicateResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                app.stage_mutations_with_transaction(mutations, &label, key, w);
                app.after_staging_decisions();
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::SubparDuplicateResolution(ref state) = app.view else {
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

impl App {
    // =========================================================================
    // Start Resolution Methods
    // =========================================================================

    pub(crate) fn start_missing_file_resolution(&mut self) {
        start_resolution!(self,
            mm_meta::domain_queries::GetMissingFileData,
            missing_file_modal::MissingFilePreviewState,
            MissingFileResolution
        );
    }

    pub(crate) fn start_missing_directory_resolution(&mut self) {
        let data = self.query(mm_meta::domain_queries::GetMissingDirectoryData);
        let preview = missing_directory_modal::MissingDirectoryState::new(
            missing_directory_modal::MissingDirectoryData(data),
        );
        self.view = ActiveView::MissingDirectoryResolution(preview);
    }

    pub(crate) fn start_corrupt_file_resolution(&mut self) {
        let data = self.query(mm_meta::domain_queries::GetCorruptFileData);
        let preview = corrupt_file_modal::CorruptFileState::new(
            corrupt_file_modal::CorruptFileData(data),
        );
        self.view = ActiveView::CorruptFileResolution(preview);
    }

    pub(crate) fn start_lossless_remux_resolution(&mut self) {
        let data = self
            .query(mm_meta::domain_queries::GetLosslessRemuxData);
        let preview = lossless_remux_modal::LosslessRemuxPreviewState::new(data);
        self.view = ActiveView::LosslessRemuxResolution(preview);
    }

    pub(crate) fn start_subpar_duplicate_resolution(&mut self) {
        let data = self.query(mm_meta::domain_queries::GetSubparDuplicateData);
        let preview = subpar_duplicate_modal::SubparDuplicateState::new(
            subpar_duplicate_modal::SubparDuplicateData(data),
        );
        self.view = ActiveView::SubparDuplicateResolution(preview);
    }

    // =========================================================================
    // Release Overlap Resolution
    // =========================================================================

    /// Start release overlap resolution (reuses DirectoryClusterResolution).
    pub(crate) fn start_release_overlap_resolution(&mut self) {
        let data = self.query(mm_meta::domain_queries::GetReleaseOverlapData);
        if data.clusters.is_empty() {
            self.status_message = Some("No release overlap clusters found".to_string());
            return;
        }
        let _ = self.start_transaction("Release overlap resolution");
        let state = mm_ui::resolutions::directory_cluster::DirectoryClusterState::with_list_config(
            mm_ui::resolutions::directory_cluster::DirectoryClusterData::new(data),
            mm_ui::standard_list::StandardListConfig {
                radio_select: true,
                ..Default::default()
            },
        );
        self.view = ActiveView::DirectoryClusterResolution(state);
    }

    /// Start V3 directory cluster resolution.
    pub(crate) fn start_directory_cluster_resolution_v3(&mut self) {
        let data = self.query(mm_meta::domain_queries::GetDirectoryClusterData);
        if data.clusters.is_empty() {
            self.status_message = Some("No directory overlap clusters found".to_string());
            return;
        }
        let _ = self.start_transaction("Directory overlap resolution");
        let state = mm_ui::resolutions::directory_cluster::DirectoryClusterState::with_list_config(
            mm_ui::resolutions::directory_cluster::DirectoryClusterData::new(data),
            mm_ui::standard_list::StandardListConfig {
                radio_select: true,
                ..Default::default()
            },
        );
        self.view = ActiveView::DirectoryClusterResolution(state);
    }
}

// =========================================================================
// V3: Directory Cluster HandleAction (Dispatchable)
// =========================================================================

impl HandleAction for mm_ui::resolutions::directory_cluster::DirectoryClusterAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

        let result = {
            let ActiveView::DirectoryClusterResolution(ref state) = app.view else {
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
                // Advance to next cluster or go to review
                if let ActiveView::DirectoryClusterResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Cancel => {
                let msg = {
                    let ActiveView::DirectoryClusterResolution(ref state) = app.view else {
                        app.cancel_and_return_to_source("Resolution cancelled");
                        return;
                    };
                    state.cancel_message()
                };
                app.cancel_and_return_to_source(msg);
            }
            DispatchResult::Skip => {
                if let ActiveView::DirectoryClusterResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::StageKeep { key, label, mutations } => {
                let Some(w) = witness else { return };
                if mutations.is_empty() { return; }
                let decision = w.decide(&label, mutations);
                let _ = super::super::operator_decisions::stage_decision(app, key, decision);
            }
            DispatchResult::Handled => {}
        }
    }
}
