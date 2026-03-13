//! OOB (Out-of-Band) Tag Resolution
//!
//! Handles OOB sync, OOB conflict inspection, and moved file acknowledgement modals.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::domain_queries;
use mm_meta::decisions::DecisionKey;
use crate::{moved_file_modal, oob_conflict_modal, oob_sync_modal, ActiveView};

// ========================================================================
// OOB Tag Sync
// ========================================================================

impl HandleAction for oob_sync_modal::OobSyncAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            oob_sync_modal::OobSyncAction::None => {}
            oob_sync_modal::OobSyncAction::AcceptDisk => {
                let Some(w) = witness else { return };
                app.stage_oob_sync_mutations(mm_meta::views::OobSyncDirection::DiskToIndex, w);
                // Transition to review
                app.after_staging_decisions();
            }
            oob_sync_modal::OobSyncAction::AcceptDb => {
                let Some(w) = witness else { return };
                app.stage_oob_sync_mutations(mm_meta::views::OobSyncDirection::IndexToDisk, w);
                // Transition to review
                app.after_staging_decisions();
            }
            oob_sync_modal::OobSyncAction::Cancel => {
                app.cancel_and_return_to_source("OOB sync resolution cancelled");
            }
        }
    }
}

// ========================================================================
// OOB Conflict Inspection
// ========================================================================

impl HandleAction for oob_conflict_modal::OobConflictAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            oob_conflict_modal::OobConflictAction::None => {}
            oob_conflict_modal::OobConflictAction::Navigate => {
                // Mismatches are carried in BucketedOobFile — nothing to reload
            }
            oob_conflict_modal::OobConflictAction::Resolve => {
                let Some(w) = witness else { return };
                app.stage_oob_bucket_resolution(w);
            }
            oob_conflict_modal::OobConflictAction::Acknowledge => {
                let Some(w) = witness else { return };
                app.stage_oob_mtime_acknowledgement(w);
            }
            oob_conflict_modal::OobConflictAction::Cancel => {
                app.cancel_and_return_to_source("OOB conflict inspection closed");
            }
        }
    }
}

// ========================================================================
// Moved File Acknowledgement
// ========================================================================

impl HandleAction for moved_file_modal::MovedFileAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            moved_file_modal::MovedFileAction::None => {}
            moved_file_modal::MovedFileAction::Acknowledge => {
                let Some(w) = witness else { return };
                app.stage_moved_file_acknowledge(w);
                // Transition to review
                app.after_staging_decisions();
            }
            moved_file_modal::MovedFileAction::Cancel => {
                app.cancel_and_return_to_source("Moved file acknowledgement cancelled");
            }
        }
    }
}

// ========================================================================
// App helper methods (start_* and private staging/computation helpers)
// ========================================================================

impl App {
    /// Start OOB tag sync resolution from Insights view.
    pub(crate) fn start_oob_sync_resolution(&mut self) {
        let files = self.witch.query(domain_queries::GetOobSyncFiles);

        // Start transaction for the sync resolution
        let _ = self.witch.start_transaction("OOB tag sync");

        let state = oob_sync_modal::OobSyncState::new(files);
        self.view = ActiveView::OobSyncResolution(state);
    }

    /// Stage mutations for OOB tag sync (accept one direction).
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files matching the direction are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - IndexToDisk: ApplyDbTagsToDisk (writes DB tags to disk files)
    /// - DiskToIndex: AssimilateDiskTagsToDb (reads disk tags into DB index)
    fn stage_oob_sync_mutations(
        &mut self,
        direction: mm_meta::views::OobSyncDirection,
        gesture: &witness::ConfirmationGesture,
    ) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::indexing::{
            ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation,
        };
        use mm_meta::mutations::Mutation;
        use mm_meta::views::OobSyncDirection;

        let (selected_indices, files_ref) = match &self.view {
            ActiveView::OobSyncResolution(ref state) => {
                let indices = if state.selection.is_active() {
                    state.selection.selected_indices()
                } else {
                    (0..state.files.len()).collect()
                };
                // Collect the data we need before dropping the borrow
                let files: Vec<_> = indices
                    .iter()
                    .filter_map(|&idx| state.files.get(idx))
                    .filter(|file| file.direction == direction)
                    .map(|file| (file.inode, file.path.clone()))
                    .collect();
                (indices, files)
            }
            _ => return,
        };
        let _ = selected_indices; // used above for collecting

        let resolver = &self.resolver;

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
                tracks
                    .into_iter()
                    .map(|(inode, path)| {
                        Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
                            inode,
                            path,
                            zone: Zone::Corpus,
                        })
                    })
                    .collect(),
            ),
            OobSyncDirection::DiskToIndex => (
                "Sync disk tags \u{2192} index",
                tracks
                    .into_iter()
                    .map(|(inode, path)| {
                        Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation {
                            inode,
                            path,
                            zone: None,
                        })
                    })
                    .collect(),
            ),
        };

        let decision = gesture.decide(label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::OobSync,
            decision,
        );
    }

    /// Start OOB tag conflict inspection from Insights view.
    ///
    /// Loads all OOB signal files classified into four buckets (with mismatch
    /// data from signals) and starts a transaction for potential resolution.
    pub(crate) fn start_oob_conflict_inspection(&mut self) {
        let files = self.witch.query(domain_queries::GetOobFilesBucketed);

        // Start transaction for potential resolution
        let _ = self.witch.start_transaction("OOB tag resolution");

        let state = oob_conflict_modal::OobConflictState::new(files);
        self.view = ActiveView::OobConflictInspection(state);
    }

    /// Stage resolution mutations for files in the active bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - ApplyDbTagsToDisk: writes DB tags to disk files
    /// - AssimilateDiskTagsToDb: reads disk tags into DB index
    fn stage_oob_bucket_resolution(&mut self, gesture: &witness::ConfirmationGesture) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::indexing::{
            ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation,
        };
        use mm_meta::mutations::Mutation;
        use crate::oob_conflict_modal::types::OobConflictButton;

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
                (files, state.frame.buttons.selected)
            }
            _ => return,
        };

        if files_data.is_empty() {
            self.status_message = Some("No files selected to resolve".to_string());
            return;
        }

        let resolver = &self.resolver;

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
            OobConflictButton::ApplyDb => (
                "Apply DB tags \u{2192} files",
                tracks
                    .into_iter()
                    .map(|(inode, path)| {
                        Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
                            inode,
                            path,
                            zone: Zone::Corpus,
                        })
                    })
                    .collect(),
            ),
            OobConflictButton::AssimilateDisk => (
                "Assimilate file tags \u{2192} DB",
                tracks
                    .into_iter()
                    .map(|(inode, path)| {
                        Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation {
                            inode,
                            path,
                            zone: None,
                        })
                    })
                    .collect(),
            ),
            // Acknowledge and Cancel don't reach this function
            _ => return,
        };

        let decision = gesture.decide(label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::OobConflict,
            decision,
        );

        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
        self.after_staging_decisions();
    }

    /// Stage acknowledgement mutation for mtime-only files in MtimeOnly bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    fn stage_oob_mtime_acknowledgement(&mut self, gesture: &witness::ConfirmationGesture) {
        use mm_meta::mutations::indexing::AcknowledgeMtimeOnlyMutation;
        use mm_meta::mutations::Mutation;

        let resolver = &self.resolver;

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
        let mutations = vec![Mutation::AcknowledgeMtimeOnly(
            AcknowledgeMtimeOnlyMutation { tracks },
        )];

        let decision = gesture.decide("Acknowledge mtime changes", mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::MtimeAck,
            decision,
        );

        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
        self.after_staging_decisions();
    }

    /// Start moved file acknowledgement modal.
    pub(crate) fn start_moved_file_acknowledge(&mut self) {
        // Query files with moved_file signals
        let files = self.witch.query(domain_queries::GetMovedFiles);

        mm_meta::logging::log_general(format!(
            "Starting moved file acknowledgement: {} files",
            files.len()
        ));

        // Start transaction for the acknowledgement
        let _ = self.witch.start_transaction("Moved file acknowledgement");

        let state = moved_file_modal::MovedFileState::new(files);
        self.view = ActiveView::MovedFileAcknowledge(state);
    }

    /// Stage mutations for moved file acknowledgement.
    fn stage_moved_file_acknowledge(&mut self, gesture: &witness::ConfirmationGesture) {
        use mm_meta::mutations::indexing::UpdateFilePathMutation;
        use mm_meta::mutations::Mutation;
        use std::path::PathBuf;

        let (mutations, label) = match &self.view {
            ActiveView::MovedFileAcknowledge(ref state) => {
                if state.files.is_empty() {
                    return;
                }

                // Create UpdateFilePath mutations for each moved file
                let mut mutations = Vec::new();
                for (inode, new_path, old_zone, new_zone) in state.files_for_mutation() {
                    let cross_zone = if old_zone != new_zone {
                        Some(new_zone)
                    } else {
                        None
                    };
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
        let decision = gesture.decide(&label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::MovedFile,
            decision,
        );
    }
}
