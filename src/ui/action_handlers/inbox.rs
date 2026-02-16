//! Inbox view action handlers.
//!
//! Handles aggregate inbox overview actions:
//! - Enter on "Unindexed" bucket: launch inbox intake confirmation
//! - Enter on "Corpus matches" bucket: launch inbox corpus match resolution

use crate::ui::active_view::ActiveView;
use crate::ui::inbox_corpus_match_modal;
use crate::ui::startup;
use super::witness;
use super::App;

impl App {
    pub(super) fn handle_inbox_action(&mut self, action: super::super::inbox_view::InboxAction, _witness: Option<&witness::DecisionWitness>) {
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
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.next());
            }
            InboxAction::CyclePrev => {
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.prev());
            }
            InboxAction::LaunchIntake => {
                // Gather inbox unindexed files and show intake confirmation
                let intake_state = self.witch.as_mut().and_then(|w| {
                    let read_db = w.read_db();
                    startup::IntakeConfirmationState::gather_inbox(&read_db)
                });

                match intake_state {
                    Some(state) => {
                        self.view = ActiveView::IntakeConfirmation(state);
                    }
                    None => {
                        self.status_message = Some("No unindexed inbox files to process".to_string());
                    }
                }
            }
            InboxAction::LaunchCorpusMatchResolution => {
                self.start_inbox_corpus_match_resolution();
            }
        }
    }

    /// Start inbox corpus match resolution modal.
    pub(in crate::ui) fn start_inbox_corpus_match_resolution(&mut self) {
        let fuzz = self.config.opinions.quality_resolution.inbox_bitrate_fuzz_percent;
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                inbox_corpus_match_modal::InboxCorpusMatchModalData::load(&read_db, fuzz).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No inbox corpus matches to resolve".to_string());
            return;
        }

        let preview = inbox_corpus_match_modal::InboxCorpusMatchPreviewState::new(data);
        self.view = ActiveView::InboxCorpusMatchResolution(preview);
    }

    /// Handle inbox corpus match preview actions.
    pub(super) fn handle_inbox_corpus_match_preview_action(
        &mut self,
        action: inbox_corpus_match_modal::InboxCorpusMatchPreviewAction,
        witness: Option<&witness::DecisionWitness>,
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
                    self.stage_mutations_with_transaction(mutations, "Stash inbox corpus matches", w);
                    self.start_transaction_review();
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
                    self.stage_mutations_with_transaction(mutations, "Stash all inbox duplicates", w);
                    self.start_transaction_review();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            inbox_corpus_match_modal::InboxCorpusMatchPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Inbox corpus match resolution cancelled");
            }
        }
    }
}
