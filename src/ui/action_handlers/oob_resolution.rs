//! OOB (Out-of-Band) Tag Resolution
//!
//! Handles OOB sync, OOB conflict inspection, and moved file acknowledgement flows.

use crate::corpus::paths;
use crate::ui::{filter_popup, moved_file_flow, oob_sync_flow, oob_conflict_flow, transaction_review, FilterPopupContext};
use crate::ui::types::UiMode;
use super::super::App;

impl App {
    // ========================================================================
    // OOB Tag Sync
    // ========================================================================

    /// Start OOB tag sync resolution from Insights view.
    pub(in crate::ui) fn start_oob_sync_resolution(&mut self) {
        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        let files = read_db.get_oob_sync_files().unwrap_or_default();
        if files.is_empty() {
            self.status_message = Some("No syncable tag changes".to_string());
            return;
        }

        // Start transaction for the sync resolution
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("OOB tag sync");
        }

        let state = oob_sync_flow::OobSyncState::new(files);
        self.oob_sync_state = Some(state);
        self.mode = UiMode::OobSyncResolution;
    }

    /// Handle OOB sync resolution actions.
    pub(in crate::ui) fn handle_oob_sync_action(&mut self, action: oob_sync_flow::OobSyncAction) {
        match action {
            oob_sync_flow::OobSyncAction::None => {}
            oob_sync_flow::OobSyncAction::AcceptDisk => {
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::DiskToIndex);
                // Transition to review
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobSyncResolution);
            }
            oob_sync_flow::OobSyncAction::AcceptDb => {
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::IndexToDisk);
                // Transition to review
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobSyncResolution);
            }
            oob_sync_flow::OobSyncAction::Cancel => {
                self.cancel_and_return_to_insights("OOB sync resolution cancelled");
                self.oob_sync_state = None;
            }
            oob_sync_flow::OobSyncAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_popup_state = Some(filter_popup::FilterPopupState::new());
                self.filter_popup_context = Some(FilterPopupContext::OobSync);
            }
        }
    }

    /// Stage mutations for OOB tag sync (accept one direction).
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files matching the direction are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - IndexToDisk: ApplyDbTagsToDisk (writes DB tags to disk files)
    /// - DiskToIndex: AssimilateDiskTagsToDb (reads disk tags into DB index)
    fn stage_oob_sync_mutations(&mut self, direction: crate::corpus::db::types::OobSyncDirection) {
        use crate::corpus::db::types::OobSyncDirection;
        use crate::meta::mutations::Mutation;

        let Some(ref state) = self.oob_sync_state else {
            return;
        };

        let resolver = paths::get_resolver();

        // Determine which indices to process
        let selected_indices = if state.selection.is_active() {
            state.selection.selected_indices()
        } else {
            // No selection - process all files matching direction
            (0..state.files.len()).collect()
        };

        // Collect tracks matching the direction
        let tracks: Vec<(i64, std::path::PathBuf)> = selected_indices
            .iter()
            .filter_map(|&idx| state.files.get(idx))
            .filter(|file| file.direction == direction)
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.path));
                (file.inode, abs_path)
            })
            .collect();

        if tracks.is_empty() {
            self.status_message = Some("No files to sync".to_string());
            return;
        }

        // Generate individual single-file mutations for each file
        let (label, mutations): (&str, Vec<Mutation>) = match direction {
            OobSyncDirection::IndexToDisk => (
                "Sync index tags → disk",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::ApplyDbTagsToDisk { inode, path })
                    .collect(),
            ),
            OobSyncDirection::DiskToIndex => (
                "Sync disk tags → index",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::AssimilateDiskTagsToDb { inode, path })
                    .collect(),
            ),
        };

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, 0, label, mutations);
        }
    }

    // ========================================================================
    // OOB Conflict Inspection
    // ========================================================================

    /// Start OOB tag conflict inspection from Insights view.
    ///
    /// Loads all OOB signal files classified into four buckets, starts a
    /// transaction for potential resolution, and computes the initial diff.
    pub(in crate::ui) fn start_oob_conflict_inspection(&mut self) {
        // First pass: query bucketed files (scoped borrow)
        let files = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };
            match read_db.get_oob_files_bucketed() {
                Ok(f) => f,
                Err(e) => {
                    crate::logging::log_error(format!("get_oob_files_bucketed failed: {}", e));
                    self.status_message = Some(format!("Query failed: {}", e));
                    return;
                }
            }
        };

        if files.is_empty() {
            self.status_message = Some("No OOB tag signals to inspect".to_string());
            return;
        }

        // Start transaction for potential resolution
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("OOB tag resolution");
        }

        let mut state = oob_conflict_flow::OobConflictState::new(files);

        // Second pass: compute initial diff for first file in the active bucket
        if let Some(file) = state.active_bucket_state().current_file() {
            let inode = file.inode;
            let path = file.path.clone();
            if let Some(w) = self.witch.as_mut() {
                let read_db = w.read_db();
                let resolver = paths::get_resolver();
                let abs_path = resolver.resolve(std::path::Path::new(&path));
                state.current_diff = oob_conflict_flow::types::compute_tag_diff(&read_db, inode, &abs_path);
            }
        }

        self.oob_conflict_state = Some(state);
        self.mode = UiMode::OobConflictInspection;
    }

    /// Handle OOB conflict inspection actions.
    pub(in crate::ui) fn handle_oob_conflict_action(&mut self, action: oob_conflict_flow::OobConflictAction) {
        match action {
            oob_conflict_flow::OobConflictAction::None => {}
            oob_conflict_flow::OobConflictAction::Navigate => {
                // File or bucket selection changed — recompute diff for the new file
                let diff = self.compute_current_conflict_diff();
                if let Some(ref mut state) = self.oob_conflict_state {
                    state.current_diff = diff;
                }
            }
            oob_conflict_flow::OobConflictAction::Resolve => {
                self.stage_oob_bucket_resolution();
            }
            oob_conflict_flow::OobConflictAction::Acknowledge => {
                self.stage_oob_mtime_acknowledgement();
            }
            oob_conflict_flow::OobConflictAction::Cancel => {
                self.cancel_and_return_to_insights("OOB conflict inspection closed");
                self.oob_conflict_state = None;
            }
            oob_conflict_flow::OobConflictAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_popup_state = Some(filter_popup::FilterPopupState::new());
                self.filter_popup_context = Some(FilterPopupContext::OobConflict);
            }
        }
    }

    /// Compute the tag diff for the currently selected conflict file.
    fn compute_current_conflict_diff(&mut self) -> Vec<crate::corpus::db::types::TagMismatchEntry> {
        let (inode, path) = match self.oob_conflict_state.as_ref()
            .and_then(|s| s.active_bucket_state().current_file())
        {
            Some(file) => (file.inode, file.path.clone()),
            None => return Vec::new(),
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => return Vec::new(),
        };

        let resolver = paths::get_resolver();
        let abs_path = resolver.resolve(std::path::Path::new(&path));
        oob_conflict_flow::types::compute_tag_diff(&read_db, inode, &abs_path)
    }

    /// Stage resolution mutations for files in the active bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - ApplyDbTagsToDisk: writes DB tags to disk files
    /// - AssimilateDiskTagsToDb: reads disk tags into DB index
    fn stage_oob_bucket_resolution(&mut self) {
        use crate::meta::mutations::Mutation;
        use crate::ui::oob_conflict_flow::types::ResolutionButton;

        let (files_data, button) = match self.oob_conflict_state.as_ref() {
            Some(state) => {
                let bucket_state = state.active_bucket_state();

                // Determine which indices to process
                let indices: Vec<usize> = if bucket_state.selection.is_active() {
                    bucket_state.selection.selected_indices()
                } else {
                    // No selection - process all files in bucket
                    (0..bucket_state.files.len()).collect()
                };

                let files: Vec<(i64, String)> = indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.inode, f.path.clone()))
                    .collect();
                (files, state.selected_button)
            }
            None => return,
        };

        if files_data.is_empty() {
            self.status_message = Some("No files selected to resolve".to_string());
            return;
        }

        let resolver = paths::get_resolver();

        // Convert to (inode, abs_path) pairs for individual mutations
        let tracks: Vec<(i64, std::path::PathBuf)> = files_data
            .iter()
            .map(|(inode, path)| {
                let abs_path = resolver.resolve(std::path::Path::new(path));
                (*inode, abs_path)
            })
            .collect();

        // Generate individual single-file mutations (batch scheduling at UI layer)
        let (label, mutations): (&str, Vec<Mutation>) = match button {
            ResolutionButton::ApplyDb => (
                "Apply DB tags → files",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::ApplyDbTagsToDisk { inode, path })
                    .collect(),
            ),
            ResolutionButton::AssimilateDisk => (
                "Assimilate file tags → DB",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::AssimilateDiskTagsToDb { inode, path })
                    .collect(),
            ),
        };

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, 0, label, mutations);
        }

        // Note: oob_conflict_state is NOT cleared - preserved for Cancel return
        self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
    }

    /// Stage acknowledgement mutation for mtime-only files in MtimeOnly bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    fn stage_oob_mtime_acknowledgement(&mut self) {
        use crate::meta::mutations::Mutation;

        let resolver = paths::get_resolver();

        let tracks = match self.oob_conflict_state.as_ref() {
            Some(state) => {
                let bucket_state = state.active_bucket_state();

                // Determine which indices to process
                let indices: Vec<usize> = if bucket_state.selection.is_active() {
                    bucket_state.selection.selected_indices()
                } else {
                    // No selection - process all files in bucket
                    (0..bucket_state.files.len()).collect()
                };

                indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| {
                        let abs_path = resolver.resolve(std::path::Path::new(&f.path));
                        (f.inode, abs_path)
                    })
                    .collect::<Vec<_>>()
            }
            None => return,
        };

        if tracks.is_empty() {
            self.status_message = Some("No files selected to acknowledge".to_string());
            return;
        }

        // Create single mutation with all files as (inode, path) pairs
        let mutations = vec![Mutation::AcknowledgeMtimeOnly { tracks }];

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(
                witch,
                0,
                "Acknowledge mtime changes",
                mutations,
            );
        }

        // Note: oob_conflict_state is NOT cleared - preserved for Cancel return
        self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
    }

    // ========================================================================
    // Moved File Acknowledgement
    // ========================================================================

    /// Start moved file acknowledgement flow.
    pub(in crate::ui) fn start_moved_file_acknowledge(&mut self) {
        let Some(ref mut witch) = self.witch else {
            self.status_message = Some("No database connection".to_string());
            return;
        };

        // Query files with moved_file signals
        let files = {
            let read_db = witch.read_db();
            match read_db.get_moved_files() {
                Ok(f) => f,
                Err(e) => {
                    self.status_message = Some(format!("Failed to query moved files: {}", e));
                    return;
                }
            }
        };

        if files.is_empty() {
            self.status_message = Some("No moved files to acknowledge".to_string());
            return;
        }

        crate::logging::log_general(format!(
            "Starting moved file acknowledgement: {} files",
            files.len()
        ));

        // Start transaction for the acknowledgement
        let _ = witch.start_transaction("Moved file acknowledgement");

        self.moved_file_state = Some(moved_file_flow::MovedFileState::new(files));
        self.mode = UiMode::MovedFileAcknowledge;
    }

    /// Handle moved file acknowledgement actions.
    pub(in crate::ui) fn handle_moved_file_action(&mut self, action: moved_file_flow::MovedFileAction) {
        match action {
            moved_file_flow::MovedFileAction::None => {}
            moved_file_flow::MovedFileAction::Acknowledge => {
                self.stage_moved_file_acknowledge();
                // Transition to review
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
            }
            moved_file_flow::MovedFileAction::Cancel => {
                self.cancel_and_return_to_insights("Moved file acknowledgement cancelled");
                self.moved_file_state = None;
            }
        }
    }

    /// Stage mutations for moved file acknowledgement.
    fn stage_moved_file_acknowledge(&mut self) {
        use crate::meta::mutations::Mutation;
        use std::path::PathBuf;

        let Some(ref state) = self.moved_file_state else {
            return;
        };

        if state.files.is_empty() {
            return;
        }

        // Create UpdateFilePath mutations for each moved file
        let mut mutations = Vec::new();
        for (inode, new_path) in state.files_for_mutation() {
            mutations.push(Mutation::UpdateFilePath {
                source: "corpus".to_string(),
                inode,
                new_path: PathBuf::from(&new_path),
            });
        }

        let label = format!(
            "Acknowledge {} moved file{}",
            mutations.len(),
            if mutations.len() == 1 { "" } else { "s" }
        );

        // Stage the UpdateFilePath mutations
        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, 0, &label, mutations);
        }
    }
}
