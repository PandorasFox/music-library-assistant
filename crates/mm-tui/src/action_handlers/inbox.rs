//! Inbox view action handlers.
//!
//! Handles aggregate inbox overview actions:
//! - Enter on "Unindexed" bucket: launch inbox intake confirmation
//! - Enter on "Corpus matches" bucket: launch inbox corpus match resolution
//! - Enter on "Organize" bucket: launch inbox organize workflow

use super::witness;
use super::HandleAction;
use super::App;
use mm_meta::decisions::DecisionKey;
use crate::active_view::ActiveView;
use crate::inbox_corpus_match_modal;
use crate::inbox_corpus_match_modal::types::InboxCorpusMatchModalDataExt;
use crate::inbox_organize;
use crate::startup;
use crate::{CanonicitySignalKind, TagCanonicityClusters};

impl HandleAction for super::super::inbox_view::InboxAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        use super::super::inbox_view::InboxAction;

        match self {
            InboxAction::LaunchIntake => {
                // Gather inbox unindexed files and show intake confirmation
                let intake_state = app
                    .witch
                    .query(mm_meta::domain_queries::GetIntakeConfirmation {
                        source: startup::IntakeSource::Inbox,
                        zone: Some(mm_meta::db_types::Zone::Inbox),
                    });

                if let Some(state) = intake_state {
                    app.view = ActiveView::IntakeConfirmation(state);
                }
            }
            InboxAction::LaunchCorpusMatchResolution => {
                app.start_inbox_corpus_match_resolution();
            }
            InboxAction::LaunchInboxTagCanonicity => {
                app.start_inbox_tag_canonicity_resolution();
            }
            InboxAction::LaunchOrganize => {
                app.start_inbox_organize();
            }
            InboxAction::LaunchInboxCompoundSplit => {
                app.start_compound_split_resolution_for_zone(
                    false,
                    None,
                    mm_meta::db_types::Zone::Inbox,
                );
            }
        }
    }
}

impl HandleAction for inbox_corpus_match_modal::InboxCorpusMatchPreviewAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::None => {}
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::ConfirmStash => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.cached_data.stash_and_drop_mutations(&app.resolver)
                    }
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash inbox corpus matches", DecisionKey::InboxCorpusMatch, "No files to stash", w);
            }
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.cached_data.stash_all_mutations(&app.resolver)
                    }
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash all inbox duplicates", DecisionKey::InboxCorpusMatch, "No files to stash", w);
            }
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::Cancel => {
                app.cancel_and_return_to_source("Inbox corpus match resolution cancelled");
            }
        }
    }
}

impl HandleAction for inbox_organize::InboxOrganizeAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            inbox_organize::InboxOrganizeAction::None => {}
            inbox_organize::InboxOrganizeAction::Complete(mutations) => {
                let Some(w) = witness else { return };
                if !mutations.is_empty() {
                    app.stage_mutations_with_transaction(
                        mutations,
                        "Organize inbox into corpus",
                        DecisionKey::InboxOrganize,
                        w,
                    );
                    app.after_staging_decisions();
                } else {
                    app.cancel_and_return_to_source("No mutations generated");
                }
            }
            inbox_organize::InboxOrganizeAction::Cancel => {
                app.cancel_and_return_to_source("Inbox organize cancelled");
            }
        }
    }
}

impl App {
    /// Start inbox tag canonicity resolution using the shared tag canonicity modal.
    ///
    /// Gathers all inbox tag canonicity signal keys, creates clusters with
    /// `InboxTagCanonicity` kind, and launches the standard canonicity modal.
    fn start_inbox_tag_canonicity_resolution(&mut self) {
        let signal_keys = self
            .witch
            .query(mm_meta::domain_queries::GetInboxTagCanonicityKeys);

        if signal_keys.is_empty() {
            self.status_message = Some("No inbox tag canonicity signals to resolve".to_string());
            return;
        }

        let kind = CanonicitySignalKind::InboxTagCanonicity;
        let clusters = TagCanonicityClusters::new(signal_keys, kind);

        // Start transaction for the modal
        let _ = self.witch.start_transaction("Inbox tag canonicalization");

        // Fire async load for the first signal — tick handler will complete it
        if !self.start_async_cluster_load(clusters) {
            self.status_message = Some("Failed to load inbox tag canonicity data".to_string());
            let _ = super::super::operator_decisions::discard_transaction(&mut self.witch);
        }
    }

    /// Start inbox corpus match resolution modal.
    pub(crate) fn start_inbox_corpus_match_resolution(&mut self) {
        let fuzz = self
            .config()
            .opinions
            .quality_resolution
            .inbox_bitrate_fuzz_percent;
        let data = self
            .witch
            .query(mm_meta::domain_queries::GetInboxCorpusMatchData {
                bitrate_fuzz_percent: fuzz,
            });

        let preview = inbox_corpus_match_modal::InboxCorpusMatchPreviewState::new(data);
        self.view = ActiveView::InboxCorpusMatchResolution(preview);
    }

    /// Start the inbox organize workflow.
    fn start_inbox_organize(&mut self) {
        let config = self.config().clone();
        let directories = self
            .witch
            .query(mm_meta::domain_queries::GetInboxOrganizeData {
                config: config.clone(),
            });

        if let Some(state) =
            inbox_organize::InboxOrganizeState::from_directories(directories, &config)
        {
            self.view = ActiveView::InboxOrganize(state);
        }
    }
}
