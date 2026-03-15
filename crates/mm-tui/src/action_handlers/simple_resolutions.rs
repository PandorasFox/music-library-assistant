//! Simple Resolution Modals
//!
//! Handles missing file, missing directory, corrupt file, shit format,
//! subpar duplicate, and directory overlap resolution modals.
//! These flows share a common pattern: load data, show preview, stage mutations.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_ui::modal_buttons::ModalButtons;
use crate::{
    corrupt_file_modal, missing_directory_modal, missing_file_modal, shit_format_modal,
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

/// Extract mutations from the current view state for staging.
macro_rules! extract_mutations {
    ($self:ident, $view_variant:ident, $method:ident) => {
        match &$self.view {
            ActiveView::$view_variant(ref p) => p.cached_data.$method(),
            _ => Vec::new(),
        }
    };
    ($self:ident, $view_variant:ident, $method:ident, $resolver:expr) => {
        match &$self.view {
            ActiveView::$view_variant(ref p) => p.cached_data.$method($resolver),
            _ => Vec::new(),
        }
    };
}

// =========================================================================
// Missing File Resolution
// =========================================================================

impl HandleAction for missing_file_modal::MissingFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            missing_file_modal::MissingFileAction::None => {}
            missing_file_modal::MissingFileAction::ConfirmRestore => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::MissingFileResolution(ref p) => missing_file_modal::restore_mutations(&p.cached_data, &app.resolver),
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Restore missing files", mm_ui::decision_keys::missing_file(), "No files to restore", w);
            }
            missing_file_modal::MissingFileAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(app, MissingFileResolution, drop_all_missing);
                app.stage_resolution(mutations, "Drop missing files", mm_ui::decision_keys::missing_file(), "No files to drop", w);
            }
            missing_file_modal::MissingFileAction::Cancel => {
                app.cancel_and_return_to_source("Missing file resolution cancelled");
            }
        }
    }
}

// =========================================================================
// Missing Directory Resolution
// =========================================================================

impl HandleAction for missing_directory_modal::MissingDirectoryAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            missing_directory_modal::MissingDirectoryAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::MissingDirectoryResolution(ref p) => p.data.0.drop_mutations(),
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Drop missing directories", mm_ui::decision_keys::missing_directory(), "No directories to drop", w);
            }
            missing_directory_modal::MissingDirectoryAction::Cancel => {
                app.cancel_and_return_to_source("Missing directory resolution cancelled");
            }
        }
    }
}

// ========================================================================
// Corrupt File Resolution
// ========================================================================

impl HandleAction for corrupt_file_modal::CorruptFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            corrupt_file_modal::CorruptFileAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::CorruptFileResolution(ref p) => corrupt_file_modal::stash_and_drop_mutations(&p.data.0, &app.resolver),
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash corrupt files", mm_ui::decision_keys::corrupt_file(), "No files to stash", w);
            }
            corrupt_file_modal::CorruptFileAction::Cancel => {
                app.cancel_and_return_to_source("Corrupt file resolution cancelled");
            }
        }
    }
}

// ========================================================================
// Shit Format Resolution
// ========================================================================

impl HandleAction for shit_format_modal::ShitFormatAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            shit_format_modal::ShitFormatAction::None => {}
            shit_format_modal::ShitFormatAction::ConfirmRemuxLossless => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(app, ShitFormatResolution, lossless_mutations, &app.resolver);
                app.stage_resolution(mutations, "Remux to FLAC", mm_ui::decision_keys::shit_format(), "No lossless files to remux", w);
            }
            shit_format_modal::ShitFormatAction::ConfirmTranscodeLossy => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &app.view {
                    ActiveView::ShitFormatResolution(ref preview) => (
                        preview.cached_data.lossy_mutations(&app.resolver),
                        preview.cached_data.lossy_to_flac,
                    ),
                    _ => (Vec::new(), false),
                };
                let label = if lossy_to_flac { "Capture lossy to FLAC" } else { "Transcode to Opus" };
                app.stage_resolution(mutations, label, mm_ui::decision_keys::shit_format(), "No lossy files to transcode", w);
            }
            shit_format_modal::ShitFormatAction::ConfirmConvertAll => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &app.view {
                    ActiveView::ShitFormatResolution(ref preview) => (
                        preview.cached_data.all_mutations(&app.resolver),
                        preview.cached_data.lossy_to_flac,
                    ),
                    _ => (Vec::new(), false),
                };
                let label = if lossy_to_flac { "Remux and capture all to FLAC" } else { "Convert all formats" };
                app.stage_resolution(mutations, label, mm_ui::decision_keys::shit_format(), "No files to convert", w);
            }
            shit_format_modal::ShitFormatAction::Cancel => {
                app.cancel_and_return_to_source("Shit format resolution cancelled");
            }
        }
    }
}

// ========================================================================
// Subpar Duplicate Resolution
// ========================================================================

impl HandleAction for subpar_duplicate_modal::SubparDuplicateAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            subpar_duplicate_modal::SubparDuplicateAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::SubparDuplicateResolution(ref p) => subpar_duplicate_modal::stash_and_drop_mutations(&p.data.0, &app.resolver),
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash subpar duplicates", mm_ui::decision_keys::subpar_duplicate(), "No files to stash", w);
            }
            subpar_duplicate_modal::SubparDuplicateAction::Cancel => {
                app.cancel_and_return_to_source("Subpar duplicate resolution cancelled");
            }
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

    pub(crate) fn start_shit_format_resolution(&mut self) {
        let mut data = self
            .query(mm_meta::domain_queries::GetShitFormatData);
        data.lossy_to_flac = self.config().opinions.lossy_shit_formats_to_flac;
        let preview = shit_format_modal::ShitFormatPreviewState::new(data);
        self.view = ActiveView::ShitFormatResolution(preview);
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
        let state = mm_ui::resolutions::directory_cluster::DirectoryClusterState::new(
            mm_ui::resolutions::directory_cluster::DirectoryClusterData::new(data),
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
        let state = mm_ui::resolutions::directory_cluster::DirectoryClusterState::new(
            mm_ui::resolutions::directory_cluster::DirectoryClusterData::new(data),
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
