//! Simple Resolution Modals
//!
//! Handles missing file, missing directory, corrupt file, shit format,
//! subpar duplicate, and directory overlap resolution modals.
//! These flows share a common pattern: load data, show preview, stage mutations.

use super::super::App;
use super::witness;
use crate::meta::decisions::DecisionKey;
use crate::ui::{
    corrupt_file_modal, missing_directory_modal, missing_file_modal, shit_format_modal,
    subpar_duplicate_modal, ActiveView,
};

/// Start a simple resolution modal: load data via domain query, create preview, set view.
macro_rules! start_resolution {
    ($self:ident, $query:expr, $preview:path, $view:ident) => {{
        let data = $self
            .cache
            .domain_query($query)
            .recv();
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
}

impl App {
    // =========================================================================
    // Missing File Resolution
    // =========================================================================

    pub(in crate::ui) fn start_missing_file_resolution(&mut self) {
        start_resolution!(self,
            crate::db::domain::GetMissingFileData,
            missing_file_modal::MissingFilePreviewState,
            MissingFileResolution
        );
    }

    pub(super) fn handle_missing_file_preview_action(
        &mut self,
        action: missing_file_modal::MissingFilePreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            missing_file_modal::MissingFilePreviewAction::None => {}
            missing_file_modal::MissingFilePreviewAction::ConfirmRestore => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, MissingFileResolution, restore_mutations);
                self.stage_resolution(mutations, "Restore missing files", DecisionKey::MissingFile, "No files to restore", w);
            }
            missing_file_modal::MissingFilePreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, MissingFileResolution, drop_all_missing);
                self.stage_resolution(mutations, "Drop missing files", DecisionKey::MissingFile, "No files to drop", w);
            }
            missing_file_modal::MissingFilePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Missing file resolution cancelled");
            }
        }
    }

    // =========================================================================
    // Missing Directory Resolution
    // =========================================================================

    pub(in crate::ui) fn start_missing_directory_resolution(&mut self) {
        start_resolution!(self,
            crate::db::domain::GetMissingDirectoryData,
            missing_directory_modal::MissingDirectoryPreviewState,
            MissingDirectoryResolution
        );
    }

    pub(super) fn handle_missing_directory_preview_action(
        &mut self,
        action: missing_directory_modal::MissingDirectoryPreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            missing_directory_modal::MissingDirectoryPreviewAction::None => {}
            missing_directory_modal::MissingDirectoryPreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, MissingDirectoryResolution, drop_mutations);
                self.stage_resolution(mutations, "Drop missing directories", DecisionKey::MissingDirectory, "No directories to drop", w);
            }
            missing_directory_modal::MissingDirectoryPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Missing directory resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Corrupt File Resolution
    // ========================================================================

    pub(in crate::ui) fn start_corrupt_file_resolution(&mut self) {
        start_resolution!(self,
            crate::db::domain::GetCorruptFileData,
            corrupt_file_modal::CorruptFilePreviewState,
            CorruptFileResolution
        );
    }

    pub(super) fn handle_corrupt_file_preview_action(
        &mut self,
        action: corrupt_file_modal::CorruptFilePreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            corrupt_file_modal::CorruptFilePreviewAction::None => {}
            corrupt_file_modal::CorruptFilePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, CorruptFileResolution, stash_and_drop_mutations);
                self.stage_resolution(mutations, "Stash corrupt files", DecisionKey::CorruptFile, "No files to stash", w);
            }
            corrupt_file_modal::CorruptFilePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Corrupt file resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Shit Format Resolution
    // ========================================================================

    pub(in crate::ui) fn start_shit_format_resolution(&mut self) {
        let mut data = self
            .cache
            .domain_query(crate::db::domain::GetShitFormatData)
            .recv();
        data.lossy_to_flac = self.config().opinions.lossy_shit_formats_to_flac;
        let preview = shit_format_modal::ShitFormatPreviewState::new(data);
        self.view = ActiveView::ShitFormatResolution(preview);
    }

    pub(super) fn handle_shit_format_preview_action(
        &mut self,
        action: shit_format_modal::ShitFormatPreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            shit_format_modal::ShitFormatPreviewAction::None => {}
            shit_format_modal::ShitFormatPreviewAction::ConfirmRemuxLossless => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, ShitFormatResolution, lossless_mutations);
                self.stage_resolution(mutations, "Remux to FLAC", DecisionKey::ShitFormat, "No lossless files to remux", w);
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmTranscodeLossy => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => (
                        preview.cached_data.lossy_mutations(),
                        preview.cached_data.lossy_to_flac,
                    ),
                    _ => (Vec::new(), false),
                };
                let label = if lossy_to_flac { "Capture lossy to FLAC" } else { "Transcode to Opus" };
                self.stage_resolution(mutations, label, DecisionKey::ShitFormat, "No lossy files to transcode", w);
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmConvertAll => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => (
                        preview.cached_data.all_mutations(),
                        preview.cached_data.lossy_to_flac,
                    ),
                    _ => (Vec::new(), false),
                };
                let label = if lossy_to_flac { "Remux and capture all to FLAC" } else { "Convert all formats" };
                self.stage_resolution(mutations, label, DecisionKey::ShitFormat, "No files to convert", w);
            }
            shit_format_modal::ShitFormatPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Shit format resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Subpar Duplicate Resolution
    // ========================================================================

    pub(in crate::ui) fn start_subpar_duplicate_resolution(&mut self) {
        start_resolution!(self,
            crate::db::domain::GetSubparDuplicateData,
            subpar_duplicate_modal::SubparDuplicatePreviewState,
            SubparDuplicateResolution
        );
    }

    pub(super) fn handle_subpar_duplicate_preview_action(
        &mut self,
        action: subpar_duplicate_modal::SubparDuplicatePreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            subpar_duplicate_modal::SubparDuplicatePreviewAction::None => {}
            subpar_duplicate_modal::SubparDuplicatePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = extract_mutations!(self, SubparDuplicateResolution, stash_and_drop_mutations);
                self.stage_resolution(mutations, "Stash subpar duplicates", DecisionKey::SubparDuplicate, "No files to stash", w);
            }
            subpar_duplicate_modal::SubparDuplicatePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Subpar duplicate resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Directory Overlap Cluster Resolution
    // ========================================================================

    pub(in crate::ui) fn start_directory_overlap_resolution(&mut self) {
        start_resolution!(self,
            crate::db::domain::GetDirectoryClusterData,
            super::super::directory_cluster_modal::DirectoryClusterPreviewState,
            DirectoryClusterResolution
        );
    }

    /// Handle directory cluster preview actions.
    pub(super) fn handle_directory_cluster_preview_action(
        &mut self,
        action: super::super::directory_cluster_modal::DirectoryClusterPreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::super::directory_cluster_modal::DirectoryClusterPreviewAction;

        match action {
            DirectoryClusterPreviewAction::None => {}
            DirectoryClusterPreviewAction::ConfirmCurrent => {
                let Some(w) = witness else { return };

                // Check if selected option is MarkExpected — handle via dedicated path
                let is_mark_expected = matches!(
                    &self.view,
                    ActiveView::DirectoryClusterResolution(ref preview)
                        if matches!(preview.selected_option(), Some(crate::ui::directory_cluster_modal::types::ClusterResolutionOption::MarkExpected))
                );

                if is_mark_expected {
                    // Delegate to MarkExpected handler
                    self.handle_directory_cluster_preview_action(
                        DirectoryClusterPreviewAction::MarkExpected,
                        Some(w),
                    );
                    return;
                }

                // Check if selected option is EditTags — launch tag editor
                let edit_tags_inodes = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        match preview.selected_option() {
                            Some(crate::ui::directory_cluster_modal::types::ClusterResolutionOption::EditTags { inodes, .. }) => {
                                Some(inodes)
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };

                if let Some(inodes) = edit_tags_inodes {
                    if !inodes.is_empty() {
                        let audio_files = self
                            .cache
                            .domain_query(crate::db::domain::GetAudioFilesByInodes {
                                inodes,
                                zone: crate::db::types::Zone::Corpus,
                            })
                            .recv();
                        if !audio_files.is_empty() {
                            self.open_unified_tag_editor_bulk(
                                audio_files,
                                super::super::tag_editor::TagEditorSource::HealthModal,
                                None,
                            );
                        }
                    }
                    return;
                }

                // Stage mutations for current cluster's selected option and advance
                let (cluster_index, mutations) = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        if let Some(option) = preview.selected_option() {
                            let mutations = preview
                                .cached_data
                                .mutations_for_resolution(preview.current_cluster_index, &option);
                            (preview.current_cluster_index, mutations)
                        } else {
                            (0, Vec::new())
                        }
                    }
                    _ => (0, Vec::new()),
                };
                if !mutations.is_empty() {
                    self.stage_directory_cluster_mutations(
                        cluster_index,
                        mutations,
                        "Resolve directory overlap",
                        w,
                    );
                }
                // Navigate to next cluster
                let at_last =
                    if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                        !preview.navigate_next()
                    } else {
                        true
                    };
                if at_last {
                    // Last cluster - go to review
                    self.after_staging_decisions();
                }
            }
            DirectoryClusterPreviewAction::MarkExpected => {
                let Some(w) = witness else { return };
                // Extract source_a/source_b from current cluster's key and stage EmitExpectedOverlap
                let mutation = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        preview.current_cluster().map(|cluster| {
                            let parts: Vec<&str> = cluster.cluster_key.splitn(2, '|').collect();
                            let source_a = parts.first().unwrap_or(&"").to_string();
                            let source_b = parts.get(1).unwrap_or(&"").to_string();
                            crate::meta::mutations::Mutation::EmitExpectedOverlap(
                                crate::meta::mutations::indexing::EmitExpectedOverlapMutation {
                                    source_a,
                                    source_b,
                                },
                            )
                        })
                    }
                    _ => None,
                };
                if let Some(mutation) = mutation {
                    self.stage_directory_cluster_mutations(
                        match &self.view {
                            ActiveView::DirectoryClusterResolution(ref preview) => {
                                preview.current_cluster_index
                            }
                            _ => 0,
                        },
                        vec![mutation],
                        "Mark expected overlap",
                        w,
                    );
                }
                // Navigate to next cluster
                let at_last =
                    if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                        !preview.navigate_next()
                    } else {
                        true
                    };
                if at_last {
                    self.after_staging_decisions();
                }
            }
            DirectoryClusterPreviewAction::NavigateNext => {
                if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    preview.navigate_next();
                }
            }
            DirectoryClusterPreviewAction::NavigatePrev => {
                if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    preview.navigate_prev();
                }
            }
            DirectoryClusterPreviewAction::ShowReview => {
                // Jump directly to transaction review
                self.after_staging_decisions();
            }
            DirectoryClusterPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Directory overlap cluster resolution cancelled");
            }
        }
    }

    // =========================================================================
    // Release Overlap Resolution
    // =========================================================================

    /// Start release overlap resolution (reuses DirectoryClusterResolution view).
    pub(in crate::ui) fn start_release_overlap_resolution(&mut self) {
        use super::super::directory_cluster_modal;
        let data = self
            .cache
            .domain_query(crate::db::domain::GetReleaseOverlapData)
            .recv();
        let preview = directory_cluster_modal::DirectoryClusterPreviewState::new(data);
        self.view = ActiveView::DirectoryClusterResolution(preview);
    }

    /// Stage directory cluster mutations for transaction review.
    fn stage_directory_cluster_mutations(
        &mut self,
        cluster_index: usize,
        mutations: Vec<crate::meta::mutations::Mutation>,
        label: &str,
        gesture: &witness::ConfirmationGesture,
    ) {
        // Start transaction if not already started
        if !self.witch.has_transaction() {
            let _ = self.witch.start_transaction("Directory overlap resolution");
        }
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::DirectoryCluster { cluster_index },
            label,
            mutations,
            gesture,
        );
    }
}
