//! Inbox view action handlers.
//!
//! Handles aggregate inbox overview actions:
//! - Enter on "Unindexed" bucket: launch inbox intake confirmation
//! - Enter on "Corpus matches" bucket: launch inbox corpus match resolution
//! - Enter on "Organize" bucket: launch inbox organize workflow

use super::witness;
use super::HandleAction;
use super::App;
use mm_ui::decision_keys;
use crate::active_view::ActiveView;
use crate::inbox_corpus_match_modal;
use crate::inbox_organize;
use crate::startup;
use mm_ui::domain_types::InboxInsightAction;

impl HandleAction for InboxInsightAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            InboxInsightAction::LaunchIntake => {
                // Gather inbox unindexed files and show intake confirmation
                let intake_state = app
                    .query(mm_meta::domain_queries::GetIntakeConfirmation {
                        source: startup::IntakeSource::Inbox,
                        zone: Some(mm_meta::db_types::Zone::Inbox),
                    });

                if let Some(state) = intake_state {
                    app.view = ActiveView::IntakeConfirmation(state);
                }
            }
            InboxInsightAction::LaunchCorpusMatchResolution => {
                app.start_inbox_corpus_match_resolution();
            }
            InboxInsightAction::LaunchInboxTagCanonicity => {
                app.start_inbox_tag_canonicity_resolution();
            }
            InboxInsightAction::LaunchOrganize => {
                app.start_inbox_organize();
            }
            InboxInsightAction::LaunchInboxCompoundSplit => {
                // TODO: V3 compound split doesn't yet support inbox zone
                app.status_message = Some("Inbox compound split not yet implemented in V3".to_string());
            }
            InboxInsightAction::Informational => {
                // No action — informational entries can't be confirmed
            }
        }
    }
}

impl HandleAction for inbox_corpus_match_modal::InboxCorpusMatchAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            inbox_corpus_match_modal::InboxCorpusMatchAction::ConfirmStash => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.data.0.stash_and_drop_mutations(&app.resolver)
                    }
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash inbox corpus matches", decision_keys::inbox_corpus_match(), "No files to stash", w);
            }
            inbox_corpus_match_modal::InboxCorpusMatchAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                let mutations = match &app.view {
                    ActiveView::InboxCorpusMatchResolution(ref preview) => {
                        preview.data.0.stash_all_mutations(&app.resolver)
                    }
                    _ => Vec::new(),
                };
                app.stage_resolution(mutations, "Stash all inbox duplicates", decision_keys::inbox_corpus_match(), "No files to stash", w);
            }
            inbox_corpus_match_modal::InboxCorpusMatchAction::Cancel => {
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
                        decision_keys::inbox_organize(),
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
    /// Start inbox tag canonicity resolution using the V3 packed query.
    fn start_inbox_tag_canonicity_resolution(&mut self) {
        use mm_ui::resolutions::tag_canonicity::{CanonicityMode, TagCanonicityData, TagCanonicityViewState};

        let zone = mm_meta::db_types::Zone::Inbox;

        // Single packed query — all clusters at once
        let data = self.query(mm_meta::domain_queries::GetTagCanonicityResolution {
            tag_name: String::new(), // empty = all tag names
            zone,
            filter_existing_canonicals: true,
        });

        if data.clusters.is_empty() {
            self.status_message = Some("No inbox tag canonicity signals to resolve".to_string());
            return;
        }

        let _ = self.start_transaction("Inbox tag canonicalization");

        let prefill: String = data.clusters.first()
            .and_then(|c| c.suggested_canonical.as_deref())
            .unwrap_or("")
            .to_string();

        let tc_data = TagCanonicityData::new(data, zone, CanonicityMode::TagCanonicity);
        let state = TagCanonicityViewState::new(tc_data, "Squash to:", &prefill);
        self.view = ActiveView::TagCanonicityResolution(state);
    }

    /// Start inbox corpus match resolution modal.
    pub(crate) fn start_inbox_corpus_match_resolution(&mut self) {
        let fuzz = self
            .config()
            .opinions
            .quality_resolution
            .inbox_bitrate_fuzz_percent;
        let data = self
            .query(mm_meta::domain_queries::GetInboxCorpusMatchData {
                bitrate_fuzz_percent: fuzz,
            });

        let preview = inbox_corpus_match_modal::InboxCorpusMatchState::new(
            inbox_corpus_match_modal::InboxCorpusMatchData(data),
        );
        self.view = ActiveView::InboxCorpusMatchResolution(preview);
    }

    /// Start the inbox organize workflow.
    fn start_inbox_organize(&mut self) {
        let config = self.config();
        let directories = self
            .query(mm_meta::domain_queries::GetInboxOrganizeData {
                config: (*config).clone(),
            });

        if let Some(state) =
            inbox_organize::InboxOrganizeState::from_directories(directories, &config)
        {
            self.view = ActiveView::InboxOrganize(state);
        }
    }
}
