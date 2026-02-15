//! OOB (Out-of-Band) Tag Resolution
//!
//! Handles OOB sync, OOB conflict inspection, and moved file acknowledgement modals.

use crate::corpus::paths;
use crate::ui::{filter_popup, moved_file_modal, oob_sync_modal, oob_conflict_modal, ActiveView, FilterOverlay, FilterPopupContext};
use super::witness;
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

        let state = oob_sync_modal::OobSyncState::new(files);
        self.view = ActiveView::OobSyncResolution(state);
    }

    /// Handle OOB sync resolution actions.
    pub(super) fn handle_oob_sync_action(&mut self, action: oob_sync_modal::OobSyncAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            oob_sync_modal::OobSyncAction::None => {}
            oob_sync_modal::OobSyncAction::AcceptDisk => {
                let Some(w) = witness else { return };
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::DiskToIndex, w);
                // Transition to review
                self.start_transaction_review();
            }
            oob_sync_modal::OobSyncAction::AcceptDb => {
                let Some(w) = witness else { return };
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::IndexToDisk, w);
                // Transition to review
                self.start_transaction_review();
            }
            oob_sync_modal::OobSyncAction::Cancel => {
                self.cancel_and_return_to_insights("OOB sync resolution cancelled");
            }
            oob_sync_modal::OobSyncAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_overlay = Some(FilterOverlay {
                    state: filter_popup::FilterPopupState::new(),
                    context: FilterPopupContext::OobSync,
                });
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
    fn stage_oob_sync_mutations(&mut self, direction: crate::corpus::db::types::OobSyncDirection, _witness: &witness::DecisionWitness) {
        use crate::corpus::db::types::{OobSyncDirection, Zone};
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::indexing::{ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation};

        let (selected_indices, files_ref) = match &self.view {
            ActiveView::OobSyncResolution(ref state) => {
                let indices = if state.selection.is_active() {
                    state.selection.selected_indices()
                } else {
                    (0..state.files.len()).collect()
                };
                // Collect the data we need before dropping the borrow
                let files: Vec<_> = indices.iter()
                    .filter_map(|&idx| state.files.get(idx))
                    .filter(|file| file.direction == direction)
                    .map(|file| (file.inode, file.path.clone()))
                    .collect();
                (indices, files)
            }
            _ => return,
        };
        let _ = selected_indices; // used above for collecting

        let resolver = paths::get_resolver();

        let tracks: Vec<(i64, std::path::PathBuf)> = files_ref
            .into_iter()
            .map(|(inode, path)| {
                let abs_path = resolver.resolve(std::path::Path::new(&path));
                (inode, abs_path)
            })
            .collect();

        if tracks.is_empty() {
            self.status_message = Some("No files to sync".to_string());
            return;
        }

        // Generate individual single-file mutations for each file
        let (label, mutations): (&str, Vec<Mutation>) = match direction {
            OobSyncDirection::IndexToDisk => (
                "Sync index tags \u{2192} disk",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation { inode, path, zone: Zone::Corpus }))
                    .collect(),
            ),
            OobSyncDirection::DiskToIndex => (
                "Sync disk tags \u{2192} index",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation { inode, path, zone: None }))
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

        let mut state = oob_conflict_modal::OobConflictState::new(files);

        // Second pass: compute initial diff for first file in the active bucket
        if let Some(file) = state.active_bucket_state().current_file() {
            let inode = file.inode;
            let path = file.path.clone();
            if let Some(w) = self.witch.as_mut() {
                let read_db = w.read_db();
                let resolver = paths::get_resolver();
                let abs_path = resolver.resolve(std::path::Path::new(&path));
                state.current_diff = oob_conflict_modal::types::compute_tag_diff(&read_db, inode, &abs_path);
            }
        }

        self.view = ActiveView::OobConflictInspection(state);
    }

    /// Handle OOB conflict inspection actions.
    pub(super) fn handle_oob_conflict_action(&mut self, action: oob_conflict_modal::OobConflictAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            oob_conflict_modal::OobConflictAction::None => {}
            oob_conflict_modal::OobConflictAction::Navigate => {
                // File or bucket selection changed -- recompute diff for the new file
                let diff = self.compute_current_conflict_diff();
                if let ActiveView::OobConflictInspection(ref mut state) = self.view {
                    state.current_diff = diff;
                }
            }
            oob_conflict_modal::OobConflictAction::Resolve => {
                let Some(w) = witness else { return };
                self.stage_oob_bucket_resolution(w);
            }
            oob_conflict_modal::OobConflictAction::Acknowledge => {
                let Some(w) = witness else { return };
                self.stage_oob_mtime_acknowledgement(w);
            }
            oob_conflict_modal::OobConflictAction::Cancel => {
                self.cancel_and_return_to_insights("OOB conflict inspection closed");
            }
            oob_conflict_modal::OobConflictAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_overlay = Some(FilterOverlay {
                    state: filter_popup::FilterPopupState::new(),
                    context: FilterPopupContext::OobConflict,
                });
            }
        }
    }

    /// Compute the tag diff for the currently selected conflict file.
    fn compute_current_conflict_diff(&mut self) -> Vec<crate::corpus::db::types::TagMismatchEntry> {
        let (inode, path) = match &self.view {
            ActiveView::OobConflictInspection(ref state) => {
                match state.active_bucket_state().current_file() {
                    Some(file) => (file.inode, file.path.clone()),
                    None => return Vec::new(),
                }
            }
            _ => return Vec::new(),
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => return Vec::new(),
        };

        let resolver = paths::get_resolver();
        let abs_path = resolver.resolve(std::path::Path::new(&path));
        oob_conflict_modal::types::compute_tag_diff(&read_db, inode, &abs_path)
    }

    /// Stage resolution mutations for files in the active bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - ApplyDbTagsToDisk: writes DB tags to disk files
    /// - AssimilateDiskTagsToDb: reads disk tags into DB index
    fn stage_oob_bucket_resolution(&mut self, _witness: &witness::DecisionWitness) {
        use crate::corpus::db::types::Zone;
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::indexing::{ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation};
        use crate::ui::oob_conflict_modal::types::ResolutionButton;

        let (files_data, button) = match &self.view {
            ActiveView::OobConflictInspection(ref state) => {
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
            _ => return,
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
                "Apply DB tags \u{2192} files",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation { inode, path, zone: Zone::Corpus }))
                    .collect(),
            ),
            ResolutionButton::AssimilateDisk => (
                "Assimilate file tags \u{2192} DB",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation { inode, path, zone: None }))
                    .collect(),
            ),
        };

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, 0, label, mutations);
        }

        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
        self.start_transaction_review();
    }

    /// Stage acknowledgement mutation for mtime-only files in MtimeOnly bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    fn stage_oob_mtime_acknowledgement(&mut self, _witness: &witness::DecisionWitness) {
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::indexing::AcknowledgeMtimeOnlyMutation;

        let resolver = paths::get_resolver();

        let tracks = match &self.view {
            ActiveView::OobConflictInspection(ref state) => {
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
            _ => return,
        };

        if tracks.is_empty() {
            self.status_message = Some("No files selected to acknowledge".to_string());
            return;
        }

        // Create single mutation with all files as (inode, path) pairs
        let mutations = vec![Mutation::AcknowledgeMtimeOnly(AcknowledgeMtimeOnlyMutation { tracks })];

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(
                witch,
                0,
                "Acknowledge mtime changes",
                mutations,
            );
        }

        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
        self.start_transaction_review();
    }

    // ========================================================================
    // Moved File Acknowledgement
    // ========================================================================

    /// Start moved file acknowledgement modal.
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

        let state = moved_file_modal::MovedFileState::new(files);
        self.view = ActiveView::MovedFileAcknowledge(state);
    }

    /// Handle moved file acknowledgement actions.
    pub(super) fn handle_moved_file_action(&mut self, action: moved_file_modal::MovedFileAction, witness: Option<&witness::DecisionWitness>) {
        match action {
            moved_file_modal::MovedFileAction::None => {}
            moved_file_modal::MovedFileAction::Acknowledge => {
                let Some(w) = witness else { return };
                self.stage_moved_file_acknowledge(w);
                // Transition to review
                self.start_transaction_review();
            }
            moved_file_modal::MovedFileAction::Cancel => {
                self.cancel_and_return_to_insights("Moved file acknowledgement cancelled");
            }
        }
    }

    /// Stage mutations for moved file acknowledgement.
    fn stage_moved_file_acknowledge(&mut self, _witness: &witness::DecisionWitness) {
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::indexing::UpdateFilePathMutation;
        use std::path::PathBuf;

        let (mutations, label) = match &self.view {
            ActiveView::MovedFileAcknowledge(ref state) => {
                if state.files.is_empty() {
                    return;
                }

                // Create UpdateFilePath mutations for each moved file
                let mut mutations = Vec::new();
                for (inode, new_path, old_zone, new_zone) in state.files_for_mutation() {
                    let cross_zone = if old_zone != new_zone { Some(new_zone) } else { None };
                    mutations.push(Mutation::UpdateFilePath(UpdateFilePathMutation {
                        zone: old_zone,
                        inode,
                        new_path: PathBuf::from(&new_path),
                        new_zone: cross_zone,
                    }));
                }

                let label = format!(
                    "Acknowledge {} moved file{}",
                    mutations.len(),
                    if mutations.len() == 1 { "" } else { "s" }
                );
                (mutations, label)
            }
            _ => return,
        };

        // Stage the UpdateFilePath mutations
        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, 0, &label, mutations);
        }
    }
}
