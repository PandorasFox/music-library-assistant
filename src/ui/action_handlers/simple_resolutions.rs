//! Simple Resolution Flows
//!
//! Handles missing file, missing directory, corrupt file, shit format,
//! subpar duplicate, and directory overlap resolution flows.
//! These flows share a common pattern: load data, show preview, stage mutations.

use crate::ui::{corrupt_file_flow, missing_directory_flow, missing_file_flow, shit_format_flow, subpar_duplicate_flow, transaction_review};
use crate::ui::types::UiMode;
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
                missing_file_flow::MissingFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No missing files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = missing_file_flow::MissingFilePreviewState::new(data);
        self.missing_file_preview = Some(preview);
        self.mode = UiMode::MissingFileResolution;
    }

    /// Handle missing file preview actions.
    pub(in crate::ui) fn handle_missing_file_preview_action(&mut self, action: missing_file_flow::MissingFilePreviewAction) {
        match action {
            missing_file_flow::MissingFilePreviewAction::None => {}
            missing_file_flow::MissingFilePreviewAction::ConfirmRestore => {
                // Generate restore mutations (HardLink) and stage for review
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.restore_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Restore missing files");
                        // Note: missing_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::MissingFileResolution);
                    } else {
                        self.status_message = Some("No files to restore".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::ConfirmDrop => {
                // Generate drop mutations (DropFromIndex) and stage for review
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.drop_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Drop non-restorable files");
                        // Note: missing_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::MissingFileResolution);
                    } else {
                        self.status_message = Some("No files to drop".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Missing file resolution cancelled");
                self.missing_file_preview = None;
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
                missing_directory_flow::MissingDirectoryModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.count() == 0 {
            self.status_message = Some("No missing directories to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = missing_directory_flow::MissingDirectoryPreviewState::new(data);
        self.missing_directory_preview = Some(preview);
        self.mode = UiMode::MissingDirectoryResolution;
    }

    /// Handle missing directory preview actions.
    pub(in crate::ui) fn handle_missing_directory_preview_action(&mut self, action: missing_directory_flow::MissingDirectoryPreviewAction) {
        match action {
            missing_directory_flow::MissingDirectoryPreviewAction::None => {}
            missing_directory_flow::MissingDirectoryPreviewAction::ConfirmDrop => {
                // Generate drop mutations (DropDirectoryFromIndex) and stage for review
                if let Some(ref preview) = self.missing_directory_preview {
                    let mutations = preview.cached_data.drop_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Drop missing directories");
                        // Note: missing_directory_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::MissingDirectoryResolution);
                    } else {
                        self.status_message = Some("No directories to drop".to_string());
                    }
                }
            }
            missing_directory_flow::MissingDirectoryPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Missing directory resolution cancelled");
                self.missing_directory_preview = None;
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
                corrupt_file_flow::CorruptFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No corrupt files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = corrupt_file_flow::CorruptFilePreviewState::new(data);
        self.corrupt_file_preview = Some(preview);
        self.mode = UiMode::CorruptFileResolution;
    }

    /// Handle corrupt file preview actions.
    pub(in crate::ui) fn handle_corrupt_file_preview_action(&mut self, action: corrupt_file_flow::CorruptFilePreviewAction) {
        match action {
            corrupt_file_flow::CorruptFilePreviewAction::None => {}
            corrupt_file_flow::CorruptFilePreviewAction::ConfirmStashAll => {
                // Generate stash + drop mutations and stage for review
                if let Some(ref preview) = self.corrupt_file_preview {
                    let mutations = preview.cached_data.stash_and_drop_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Stash corrupt files");
                        // Note: corrupt_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::CorruptFileResolution);
                    } else {
                        self.status_message = Some("No files to stash".to_string());
                    }
                }
            }
            corrupt_file_flow::CorruptFilePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Corrupt file resolution cancelled");
                self.corrupt_file_preview = None;
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
                shit_format_flow::ShitFormatModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No shit format files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = shit_format_flow::ShitFormatPreviewState::new(data);
        self.shit_format_preview = Some(preview);
        self.mode = UiMode::ShitFormatResolution;
    }

    /// Handle shit format preview actions.
    pub(in crate::ui) fn handle_shit_format_preview_action(&mut self, action: shit_format_flow::ShitFormatPreviewAction) {
        match action {
            shit_format_flow::ShitFormatPreviewAction::None => {}
            shit_format_flow::ShitFormatPreviewAction::ConfirmRemuxLossless => {
                // Generate FLAC remux mutations for lossless files only
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.lossless_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Remux to FLAC");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No lossless files to remux".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::ConfirmTranscodeLossy => {
                // Generate Opus transcode mutations for lossy files only
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.lossy_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Transcode to Opus");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No lossy files to transcode".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::ConfirmConvertAll => {
                // Generate mutations for all files (lossless → FLAC, lossy → Opus)
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.all_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Convert all formats");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No files to convert".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Shit format resolution cancelled");
                self.shit_format_preview = None;
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
                subpar_duplicate_flow::SubparDuplicateModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No subpar duplicates to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = subpar_duplicate_flow::SubparDuplicatePreviewState::new(data);
        self.subpar_duplicate_preview = Some(preview);
        self.mode = UiMode::SubparDuplicateResolution;
    }

    /// Handle subpar duplicate preview actions.
    pub(in crate::ui) fn handle_subpar_duplicate_preview_action(&mut self, action: subpar_duplicate_flow::SubparDuplicatePreviewAction) {
        match action {
            subpar_duplicate_flow::SubparDuplicatePreviewAction::None => {}
            subpar_duplicate_flow::SubparDuplicatePreviewAction::ConfirmStashAll => {
                // Generate stash + drop mutations and stage for review
                if let Some(ref preview) = self.subpar_duplicate_preview {
                    let mutations = preview.cached_data.stash_and_drop_mutations();
                    if !mutations.is_empty() {
                        self.stage_mutations_with_transaction(mutations, "Stash subpar duplicates");
                        // Note: subpar_duplicate_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::SubparDuplicateResolution);
                    } else {
                        self.status_message = Some("No files to stash".to_string());
                    }
                }
            }
            subpar_duplicate_flow::SubparDuplicatePreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Subpar duplicate resolution cancelled");
                self.subpar_duplicate_preview = None;
            }
        }
    }

    // ========================================================================
    // Directory Overlap Cluster Resolution
    // ========================================================================

    /// Start directory overlap cluster resolution modal from Insights view.
    pub(in crate::ui) fn start_directory_overlap_resolution(&mut self) {
        use super::super::directory_cluster_flow;

        // Load directory overlap cluster data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                directory_cluster_flow::DirectoryClusterModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if !data.has_clusters() {
            self.status_message = Some("No directory overlap clusters to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = directory_cluster_flow::DirectoryClusterPreviewState::new(data);
        self.directory_cluster_preview = Some(preview);
        self.mode = UiMode::DirectoryClusterResolution;
    }

    /// Handle directory cluster preview actions.
    pub(in crate::ui) fn handle_directory_cluster_preview_action(
        &mut self,
        action: super::super::directory_cluster_flow::DirectoryClusterPreviewAction,
    ) {
        use super::super::directory_cluster_flow::DirectoryClusterPreviewAction;

        match action {
            DirectoryClusterPreviewAction::None => {}
            DirectoryClusterPreviewAction::ConfirmCurrent => {
                // Stage mutations for current cluster's selected option and advance
                if let Some(ref preview) = self.directory_cluster_preview {
                    if let Some(option) = preview.selected_option() {
                        let mutations = preview.cached_data.mutations_for_resolution(
                            preview.current_cluster_index,
                            option,
                        );
                        if !mutations.is_empty() {
                            self.stage_directory_cluster_mutations(preview.current_cluster_index, mutations, "Resolve directory overlap");
                        }
                    }
                }
                // Navigate to next cluster
                if let Some(ref mut preview) = self.directory_cluster_preview {
                    if !preview.navigate_next() {
                        // Last cluster - go to review
                        self.start_transaction_review(transaction_review::TransactionReviewSource::DirectoryClusterResolution);
                    }
                }
            }
            DirectoryClusterPreviewAction::NavigateNext => {
                if let Some(ref mut preview) = self.directory_cluster_preview {
                    preview.navigate_next();
                }
            }
            DirectoryClusterPreviewAction::NavigatePrev => {
                if let Some(ref mut preview) = self.directory_cluster_preview {
                    preview.navigate_prev();
                }
            }
            DirectoryClusterPreviewAction::ShowReview => {
                // Jump directly to transaction review
                self.start_transaction_review(transaction_review::TransactionReviewSource::DirectoryClusterResolution);
            }
            DirectoryClusterPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Directory overlap cluster resolution cancelled");
                self.directory_cluster_preview = None;
            }
        }
    }

    /// Stage directory cluster mutations for transaction review.
    fn stage_directory_cluster_mutations(&mut self, cluster_index: usize, mutations: Vec<crate::meta::mutations::Mutation>, label: &str) {
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
