//! OOB (Out-of-Band) Tag Resolution
//!
//! Handles OOB conflict inspection and moved file acknowledgement modals.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::decisions::DecisionKey;
use mm_meta::views::ConflictBucket;
use crate::{moved_file_modal, oob_conflict_modal, ActiveView};

// ========================================================================
// OOB Conflict Inspection
// ========================================================================

impl HandleAction for oob_conflict_modal::OobConflictAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            oob_conflict_modal::OobConflictAction::None => {}
            oob_conflict_modal::OobConflictAction::Navigate => {
                // Mismatches are carried in OobFile — nothing to reload
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
                app.cancel_and_return_to_source("OOB resolution closed");
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
    /// Start OOB tag conflict inspection from Insights view.
    ///
    /// Loads all OOB signal files classified into four buckets (with mismatch
    /// data from signals) and starts a transaction for potential resolution.
    pub(crate) fn start_oob_conflict_inspection(&mut self) {
        let files = self.query(mm_meta::domain_queries::GetOobFiles { bucket: None });

        // Start transaction for potential resolution
        let _ = self.start_transaction("OOB tag resolution");

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

        let (files_data, button, active_bucket) = match &self.view {
            ActiveView::OobConflictInspection(ref state) => {
                let bucket_state = state.active_bucket_state();

                // Use selected indices if any, otherwise all files
                let indices: Vec<usize> = if !bucket_state.list.selected.is_empty() {
                    bucket_state.list.selected.iter().copied().collect()
                } else {
                    (0..bucket_state.files.len()).collect()
                };

                let files: Vec<(i64, String)> = indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.inode, f.path.clone()))
                    .collect();
                (files, state.frame.buttons.selected, state.active_bucket)
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
            self,
            DecisionKey::OobResolution { bucket: active_bucket },
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

                // Use selected indices if any, otherwise all files
                let indices: Vec<usize> = if !bucket_state.list.selected.is_empty() {
                    bucket_state.list.selected.iter().copied().collect()
                } else {
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
            self,
            DecisionKey::OobResolution { bucket: ConflictBucket::MtimeOnly },
            decision,
        );

        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
        self.after_staging_decisions();
    }

    /// Start moved file acknowledgement modal.
    pub(crate) fn start_moved_file_acknowledge(&mut self) {
        // Query files with moved_file signals
        let files = self.query(mm_meta::domain_queries::GetMovedFiles);

        mm_meta::logging::log_general(format!(
            "Starting moved file acknowledgement: {} files",
            files.len()
        ));

        // Start transaction for the acknowledgement
        let _ = self.start_transaction("Moved file acknowledgement");

        let state = moved_file_modal::MovedFileState::new(moved_file_modal::MovedFileData { files });
        self.view = ActiveView::MovedFileAcknowledge(state);
    }

    /// Stage mutations for moved file acknowledgement.
    fn stage_moved_file_acknowledge(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, label) = match &self.view {
            ActiveView::MovedFileAcknowledge(ref state) => {
                if state.data.files.is_empty() {
                    return;
                }

                let mutations: Vec<_> = state
                    .data
                    .files
                    .iter()
                    .map(|f| f.to_update_mutation())
                    .collect();

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
            self,
            DecisionKey::MovedFile,
            decision,
        );
    }
}
