//! Inbox view action handlers.
//!
//! Handles aggregate inbox overview actions:
//! - Enter on "Unindexed" bucket: launch inbox intake confirmation
//! - Enter on "Corpus matches" bucket: launch inbox corpus match resolution
//! - Enter on "Organize" bucket: launch inbox organize workflow

use crate::meta::decisions::DecisionKey;
use crate::meta::signals::data::InboxTagCanonicitySignal;
use crate::ui::active_view::ActiveView;
use crate::ui::inbox_corpus_match_modal;
use crate::ui::inbox_organize;
use crate::ui::startup;
use crate::ui::{CanonicitySignalKind, TagCanonicityClusters};
use super::witness;
use super::App;

impl App {
    pub(super) fn handle_inbox_action(&mut self, action: super::super::inbox_view::InboxAction, _witness: Option<&witness::ConfirmationGesture>) {
        use super::super::inbox_view::InboxAction;

        match action {
            InboxAction::None => {}
            InboxAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }
            InboxAction::CycleNext => {
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.next(self.transactions_open()));
            }
            InboxAction::CyclePrev => {
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.prev(self.transactions_open()));
            }
            InboxAction::LaunchIntake => {
                // Gather inbox unindexed files and show intake confirmation
                let intake_state = self.cache.query(|db| {
                    startup::IntakeConfirmationState::gather_inbox(db, startup::IntakeSource::Inbox)
                }).recv();

                if let Some(state) = intake_state {
                    self.view = ActiveView::IntakeConfirmation(state);
                }
            }
            InboxAction::LaunchCorpusMatchResolution => {
                self.start_inbox_corpus_match_resolution();
            }
            InboxAction::LaunchInboxTagCanonicity => {
                self.start_inbox_tag_canonicity_resolution();
            }
            InboxAction::LaunchOrganize => {
                self.start_inbox_organize();
            }
            InboxAction::LaunchInboxCompoundSplit => {
                self.start_compound_split_resolution_for_zone(false, None, crate::db::types::Zone::Inbox);
            }
        }
    }

    /// Start inbox tag canonicity resolution using the shared tag canonicity modal.
    ///
    /// Gathers all inbox tag canonicity signal keys, creates clusters with
    /// `InboxTagCanonicity` kind, and launches the standard canonicity modal.
    fn start_inbox_tag_canonicity_resolution(&mut self) {
        let signal_keys = self.cache.query(|db| {
            db.aggregate_signal_keys::<InboxTagCanonicitySignal>()
                .unwrap_or_default()
        }).recv();

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
    pub(in crate::ui) fn start_inbox_corpus_match_resolution(&mut self) {
        let fuzz = self.config().opinions.quality_resolution.inbox_bitrate_fuzz_percent;
        let data = self.cache.query(move |db| {
            inbox_corpus_match_modal::InboxCorpusMatchModalData::load(db, fuzz).ok().unwrap_or_default()
        }).recv();

        let preview = inbox_corpus_match_modal::InboxCorpusMatchPreviewState::new(data);
        self.view = ActiveView::InboxCorpusMatchResolution(preview);
    }

    /// Handle inbox corpus match preview actions.
    pub(super) fn handle_inbox_corpus_match_preview_action(
        &mut self,
        action: inbox_corpus_match_modal::InboxCorpusMatchPreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::None => {}
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::ConfirmStash => {
                let Some(w) = witness else { return };
                let mutations = match &self.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.cached_data.stash_and_drop_mutations()
                    }
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash inbox corpus matches", DecisionKey::InboxCorpusMatch, w);
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = match &self.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.cached_data.stash_all_mutations()
                    }
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash all inbox duplicates", DecisionKey::InboxCorpusMatch, w);
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Inbox corpus match resolution cancelled");
            }
        }
    }

    /// Start the inbox organize workflow.
    fn start_inbox_organize(&mut self) {
        let config = self.config().clone();
        let state = self.cache.query(move |db| {
            inbox_organize::InboxOrganizeState::load_from_read_db(db, &config)
        }).recv();

        if let Some(state) = state {
            self.view = ActiveView::InboxOrganize(state);
        }
    }

    /// Handle inbox organize workflow actions.
    pub(super) fn handle_inbox_organize_action(
        &mut self,
        action: inbox_organize::InboxOrganizeAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            inbox_organize::InboxOrganizeAction::None => {}
            inbox_organize::InboxOrganizeAction::Complete(mutations) => {
                let Some(w) = witness else { return };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Organize inbox into corpus", DecisionKey::InboxOrganize, w);
                    self.after_staging_decisions();
                } else {
                    self.cancel_and_return_to_source("No mutations generated");
                }
            }
            inbox_organize::InboxOrganizeAction::Cancel => {
                self.cancel_and_return_to_source("Inbox organize cancelled");
            }
        }
    }
}
