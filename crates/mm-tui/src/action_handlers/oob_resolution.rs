//! OOB (Out-of-Band) Tag Resolution + Moved File Acknowledgement

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::views::ConflictBucket;
use mm_ui::decision_keys;
use crate::{moved_file_modal, oob_conflict_modal, ActiveView};

// ========================================================================
// OOB Resolution
// ========================================================================

impl HandleAction for oob_conflict_modal::OobAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use oob_conflict_modal::OobAction;
        match self {
            OobAction::None => {}
            OobAction::Navigate => {}
            OobAction::Resolve => {
                let Some(w) = witness else { return };
                app.stage_oob_bucket_resolution(w);
            }
            OobAction::Acknowledge => {
                let Some(w) = witness else { return };
                app.stage_oob_mtime_acknowledgement(w);
            }
            OobAction::Cancel => {
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
                app.after_staging_decisions();
            }
            moved_file_modal::MovedFileAction::Cancel => {
                app.cancel_and_return_to_source("Moved file acknowledgement cancelled");
            }
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

    fn stage_oob_bucket_resolution(&mut self, gesture: &witness::ConfirmationGesture) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::indexing::{
            ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation,
        };
        use mm_meta::mutations::Mutation;
        use oob_conflict_modal::OobButton;

        let (files_data, button, active_bucket) = match &self.view {
            ActiveView::OobResolution(ref state) => {
                let bucket_state = state.active_bucket_state();
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
        let tracks: Vec<(i64, std::path::PathBuf)> = files_data
            .iter()
            .map(|(inode, path)| (*inode, resolver.resolve(std::path::Path::new(path))))
            .collect();

        let (label, mutations): (&str, Vec<Mutation>) = match button {
            OobButton::ApplyDb => (
                "Apply DB tags \u{2192} files",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::ApplyDbTagsToDisk(
                        ApplyDbTagsToDiskMutation { inode, path, zone: Zone::Corpus },
                    ))
                    .collect(),
            ),
            OobButton::AssimilateDisk => (
                "Assimilate file tags \u{2192} DB",
                tracks.into_iter()
                    .map(|(inode, path)| Mutation::AssimilateDiskTagsToDb(
                        AssimilateDiskTagsToDbMutation { inode, path, zone: None },
                    ))
                    .collect(),
            ),
            _ => return,
        };

        let decision = gesture.decide(label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self,
            decision_keys::oob_resolution(active_bucket),
            decision,
        );
        self.after_staging_decisions();
    }

    fn stage_oob_mtime_acknowledgement(&mut self, gesture: &witness::ConfirmationGesture) {
        use mm_meta::mutations::indexing::AcknowledgeMtimeOnlyMutation;
        use mm_meta::mutations::Mutation;

        let resolver = &self.resolver;
        let tracks = match &self.view {
            ActiveView::OobResolution(ref state) => {
                let bucket_state = state.active_bucket_state();
                let indices: Vec<usize> = if !bucket_state.list.selected.is_empty() {
                    bucket_state.list.selected.iter().copied().collect()
                } else {
                    (0..bucket_state.files.len()).collect()
                };
                indices.iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.inode, resolver.resolve(std::path::Path::new(&f.path))))
                    .collect::<Vec<_>>()
            }
            _ => return,
        };

        if tracks.is_empty() {
            self.status_message = Some("No files selected to acknowledge".to_string());
            return;
        }

        let mutations = vec![Mutation::AcknowledgeMtimeOnly(
            AcknowledgeMtimeOnlyMutation { tracks },
        )];
        let decision = gesture.decide("Acknowledge mtime changes", mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self,
            decision_keys::oob_resolution(ConflictBucket::MtimeOnly),
            decision,
        );
        self.after_staging_decisions();
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

    fn stage_moved_file_acknowledge(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, label) = match &self.view {
            ActiveView::MovedFileAcknowledge(ref state) => {
                if state.data.files.is_empty() { return; }
                let mutations: Vec<_> = state.data.files.iter().map(|f| f.to_update_mutation()).collect();
                let label = format!(
                    "Acknowledge {} moved file{}",
                    mutations.len(), if mutations.len() == 1 { "" } else { "s" }
                );
                (mutations, label)
            }
            _ => return,
        };
        let decision = gesture.decide(&label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self, decision_keys::moved_file(), decision,
        );
    }
}
