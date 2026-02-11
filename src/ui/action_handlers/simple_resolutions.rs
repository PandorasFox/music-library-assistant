//! Simple Resolution Modals
//!
//! Handles missing file, missing directory, corrupt file, shit format,
//! subpar duplicate, and directory overlap resolution modals.
//! These flows share a common pattern: load data, show preview, stage mutations.

use crate::ui::{corrupt_file_modal, missing_directory_modal, missing_file_modal, shit_format_modal, subpar_duplicate_modal, ActiveView};
use super::witness;
use super::super::App;

impl App {
    // =========================================================================
    // Missing File Resolution
    // =========================================================================

    /// Start missing file resolution modal from Insights view.
    pub(in crate::ui) fn start_missing_file_resolution(&mut self) {
        // Load categorized missing file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                missing_file_modal::MissingFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No missing files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = missing_file_modal::MissingFilePreviewState::new(data);
        self.view = ActiveView::MissingFileResolution(preview);
    }

    /// Handle missing file preview actions.
    pub(super) fn handle_missing_file_preview_action(&mut self, action: missing_file_modal::MissingFilePreviewAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            missing_file_modal::MissingFilePreviewAction::None => {}
            missing_file_modal::MissingFilePreviewAction::ConfirmRestore => {
                let Some(w) = witness else { return };
                // Generate restore mutations (HardLink) and stage for review
                let mutations = match &self.view {
                    ActiveView::MissingFileResolution(ref preview) => preview.cached_data.restore_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Restore missing files", w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to restore".to_string());
                }
            }
            missing_file_modal::MissingFilePreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                // Generate drop mutations (DropFromIndex) and stage for review
                let mutations = match &self.view {
                    ActiveView::MissingFileResolution(ref preview) => preview.cached_data.drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Drop non-restorable files", w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to drop".to_string());
                }
            }
            missing_file_modal::MissingFilePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Missing file resolution cancelled");
            }
        }
    }

    // =========================================================================
    // Missing Directory Resolution
    // =========================================================================

    /// Start missing directory resolution modal from Insights view.
    pub(in crate::ui) fn start_missing_directory_resolution(&mut self) {
        // Load missing directory data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                missing_directory_modal::MissingDirectoryModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.count() == 0 {
            self.status_message = Some("No missing directories to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = missing_directory_modal::MissingDirectoryPreviewState::new(data);
        self.view = ActiveView::MissingDirectoryResolution(preview);
    }

    /// Handle missing directory preview actions.
    pub(super) fn handle_missing_directory_preview_action(&mut self, action: missing_directory_modal::MissingDirectoryPreviewAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            missing_directory_modal::MissingDirectoryPreviewAction::None => {}
            missing_directory_modal::MissingDirectoryPreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                // Generate drop mutations (DropDirectoryFromIndex) and stage for review
                let mutations = match &self.view {
                    ActiveView::MissingDirectoryResolution(ref preview) => preview.cached_data.drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Drop missing directories", w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No directories to drop".to_string());
                }
            }
            missing_directory_modal::MissingDirectoryPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Missing directory resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Corrupt File Resolution
    // ========================================================================

    /// Start corrupt file resolution modal from Insights view.
    pub(in crate::ui) fn start_corrupt_file_resolution(&mut self) {
        // Load corrupt file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                corrupt_file_modal::CorruptFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No corrupt files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = corrupt_file_modal::CorruptFilePreviewState::new(data);
        self.view = ActiveView::CorruptFileResolution(preview);
    }

    /// Handle corrupt file preview actions.
    pub(super) fn handle_corrupt_file_preview_action(&mut self, action: corrupt_file_modal::CorruptFilePreviewAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            corrupt_file_modal::CorruptFilePreviewAction::None => {}
            corrupt_file_modal::CorruptFilePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                // Generate stash + drop mutations and stage for review
                let mutations = match &self.view {
                    ActiveView::CorruptFileResolution(ref preview) => preview.cached_data.stash_and_drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash corrupt files", w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            corrupt_file_modal::CorruptFilePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Corrupt file resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Shit Format Resolution
    // ========================================================================

    /// Start shit format resolution modal from Insights view.
    pub(in crate::ui) fn start_shit_format_resolution(&mut self) {
        // Load shit format file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                shit_format_modal::ShitFormatModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No shit format files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = shit_format_modal::ShitFormatPreviewState::new(data);
        self.view = ActiveView::ShitFormatResolution(preview);
    }

    /// Handle shit format preview actions.
    pub(super) fn handle_shit_format_preview_action(&mut self, action: shit_format_modal::ShitFormatPreviewAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            shit_format_modal::ShitFormatPreviewAction::None => {}
            shit_format_modal::ShitFormatPreviewAction::ConfirmRemuxLossless => {
                let Some(w) = witness else { return };
                // Generate FLAC remux mutations for lossless files only
                let mutations = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => preview.cached_data.lossless_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Remux to FLAC", w);
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No lossless files to remux".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmTranscodeLossy => {
                let Some(w) = witness else { return };
                // Generate Opus transcode mutations for lossy files only
                let mutations = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => preview.cached_data.lossy_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Transcode to Opus", w);
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No lossy files to transcode".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmConvertAll => {
                let Some(w) = witness else { return };
                // Generate mutations for all files (lossless -> FLAC, lossy -> Opus)
                let mutations = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => preview.cached_data.all_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Convert all formats", w);
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to convert".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Shit format resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Subpar Duplicate Resolution
    // ========================================================================

    /// Start subpar duplicate resolution modal from Insights view.
    pub(in crate::ui) fn start_subpar_duplicate_resolution(&mut self) {
        // Load subpar duplicate file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                subpar_duplicate_modal::SubparDuplicateModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No subpar duplicates to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = subpar_duplicate_modal::SubparDuplicatePreviewState::new(data);
        self.view = ActiveView::SubparDuplicateResolution(preview);
    }

    /// Handle subpar duplicate preview actions.
    pub(super) fn handle_subpar_duplicate_preview_action(&mut self, action: subpar_duplicate_modal::SubparDuplicatePreviewAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            subpar_duplicate_modal::SubparDuplicatePreviewAction::None => {}
            subpar_duplicate_modal::SubparDuplicatePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                // Generate stash + drop mutations and stage for review
                let mutations = match &self.view {
                    ActiveView::SubparDuplicateResolution(ref preview) => preview.cached_data.stash_and_drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash subpar duplicates", w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            subpar_duplicate_modal::SubparDuplicatePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Subpar duplicate resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Directory Overlap Cluster Resolution
    // ========================================================================

    /// Start directory overlap cluster resolution modal from Insights view.
    pub(in crate::ui) fn start_directory_overlap_resolution(&mut self) {
        use super::super::directory_cluster_modal;

        // Load directory overlap cluster data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                directory_cluster_modal::DirectoryClusterModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if !data.has_clusters() {
            self.status_message = Some("No directory overlap clusters to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = directory_cluster_modal::DirectoryClusterPreviewState::new(data);
        self.view = ActiveView::DirectoryClusterResolution(preview);
    }

    /// Handle directory cluster preview actions.
    pub(super) fn handle_directory_cluster_preview_action(
        &mut self,
        action: super::super::directory_cluster_modal::DirectoryClusterPreviewAction,
        witness: Option<&witness::DecisionWitness>,
    ) {
        use super::super::directory_cluster_modal::DirectoryClusterPreviewAction;

        match action {
            DirectoryClusterPreviewAction::None => {}
            DirectoryClusterPreviewAction::ConfirmCurrent => {
                let Some(w) = witness else { return };
                // Stage mutations for current cluster's selected option and advance
                let (cluster_index, mutations) = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        if let Some(option) = preview.selected_option() {
                            let mutations = preview.cached_data.mutations_for_resolution(
                                preview.current_cluster_index,
                                option,
                            );
                            (preview.current_cluster_index, mutations)
                        } else {
                            (0, Vec::new())
                        }
                    }
                    _ => (0, Vec::new()),
                };
                if !mutations.is_empty() {
                    self.stage_directory_cluster_mutations(cluster_index, mutations, "Resolve directory overlap", w);
                }
                // Navigate to next cluster
                let at_last = if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    !preview.navigate_next()
                } else {
                    true
                };
                if at_last {
                    // Last cluster - go to review
                    self.start_transaction_review();
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
                self.start_transaction_review();
            }
            DirectoryClusterPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Directory overlap cluster resolution cancelled");
            }
        }
    }

    /// Stage directory cluster mutations for transaction review.
    fn stage_directory_cluster_mutations(&mut self, cluster_index: usize, mutations: Vec<crate::meta::mutations::Mutation>, label: &str, _witness: &witness::DecisionWitness) {
        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Start transaction if not already started
        if !witch.has_transaction() {
            let _ = witch.start_transaction("Directory overlap resolution");
        }
        let _ = super::super::operator_decisions::stage_decision(
            witch,
            cluster_index,
            label,
            mutations,
        );
    }
}
