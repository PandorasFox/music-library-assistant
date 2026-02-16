//! Inbox view action handlers.
//!
//! Handles aggregate inbox overview actions:
//! - Enter on "Unindexed" bucket: launch inbox intake confirmation
//! - Enter on "Corpus matches" bucket: launch inbox corpus match resolution

use crate::meta::signals::data::InboxTagCanonicitySignal;
use crate::ui::active_view::ActiveView;
use crate::ui::inbox_corpus_match_modal;
use crate::ui::startup;
use crate::ui::{tag_canonicity_v2, CanonicitySignalKind, TagCanonicityClusters};
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
            InboxAction::LaunchInboxTagCanonicity => {
                self.start_inbox_tag_canonicity_resolution();
            }
        }
    }

    /// Start inbox tag canonicity resolution using the shared tag canonicity modal.
    ///
    /// Gathers all inbox tag canonicity signal keys, creates clusters with
    /// `InboxTagCanonicity` kind, and launches the standard canonicity modal.
    fn start_inbox_tag_canonicity_resolution(&mut self) {
        let signal_keys = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };
            read_db.aggregate_signal_keys::<InboxTagCanonicitySignal>()
                .unwrap_or_default()
        };

        if signal_keys.is_empty() {
            self.status_message = Some("No inbox tag canonicity signals to resolve".to_string());
            return;
        }

        let kind = CanonicitySignalKind::InboxTagCanonicity;
        let clusters = TagCanonicityClusters::new(signal_keys, kind);

        // Start transaction for the modal
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Inbox tag canonicalization");
        }

        // Load the first signal
        let first_key = clusters.signal_keys[0].clone();
        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };
            read_db.get_inbox_tag_canonicity_signal(&first_key)
                .ok()
                .flatten()
                .and_then(|signal| {
                    tag_canonicity_v2::TagCanonicalityModalDataV2::from_inbox_tag_canonicity(&signal, &read_db)
                })
        };

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to load inbox tag canonicity data".to_string());
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        let pre_fill = clusters.pre_fill();
        let (group_index, total_groups) = (clusters.current_index, clusters.signal_keys.len());
        let zone = crate::corpus::db::types::Zone::Inbox;
        let state = tag_canonicity_v2::TagCanonicalityStateV2::new(
            data, pre_fill, group_index, total_groups, false, zone,
        );
        self.view = ActiveView::TagCanonicityResolution { state, clusters };
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
