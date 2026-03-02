//! External Match action handlers.
//!
//! Handles both:
//! - The External Matches lateral view (browse/fetch/launch)
//! - The External Match Review modal (accept/dismiss/cancel)

use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
use crate::meta::mutations::TagOp;
use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::{external_match_modal, external_match_view, widgets, ActiveView};
use super::witness;
use super::super::App;

impl App {
    // =========================================================================
    // External Matches Lateral View Actions
    // =========================================================================

    /// Handle actions from the External Matches lateral view.
    pub(super) fn handle_external_matches_action(
        &mut self,
        action: external_match_view::ExternalMatchesAction,
    ) {
        match action {
            external_match_view::ExternalMatchesAction::None => {}
            external_match_view::ExternalMatchesAction::CycleNext => {
                let txn = self.transactions_open();
                self.start_lateral_view(widgets::LateralView::ExternalMatches.next(txn));
            }
            external_match_view::ExternalMatchesAction::CyclePrev => {
                let txn = self.transactions_open();
                self.start_lateral_view(widgets::LateralView::ExternalMatches.prev(txn));
            }
            external_match_view::ExternalMatchesAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }
            external_match_view::ExternalMatchesAction::RequestFetch => {
                self.witch.request_external_fetch();
                self.status_message = Some("AcoustID lookup requested".to_string());
                if let ActiveView::ExternalMatches(ref mut state) = self.view {
                    state.fetch_active = self.witch.is_external_fetch_active();
                }
            }
            external_match_view::ExternalMatchesAction::LaunchUntaggedReview => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
                    state.cached_data.as_ref()
                        .map(|d| d.untagged_entries.clone())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                self.start_external_match_review_with(entries);
            }
            external_match_view::ExternalMatchesAction::LaunchTierReview(tier) => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
                    state.cached_data.as_ref()
                        .and_then(|d| d.confidence_buckets.iter()
                            .find(|b| b.tier == tier)
                            .map(|b| b.entries.clone()))
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                self.start_external_match_review_with(entries);
            }
        }
    }

    /// Start external match review with pre-filtered entries (from the lateral view).
    fn start_external_match_review_with(&mut self, entries: Vec<ExternalMatchReviewEntry>) {
        if entries.is_empty() {
            self.status_message = Some("No external matches to review".to_string());
            return;
        }

        let show_url = self.config().opinions.external_matching.show_musicbrainz_url;
        let _ = self.witch.start_transaction("External match review");
        let state = external_match_modal::ExternalMatchReviewState::new(entries, show_url);
        self.view = ActiveView::ExternalMatchReview(state);
    }

    // =========================================================================
    // External Match Review Modal Actions
    // =========================================================================

    /// Handle external match review actions.
    pub(super) fn handle_external_match_review_action(
        &mut self,
        action: external_match_modal::ExternalMatchReviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            external_match_modal::ExternalMatchReviewAction::None => {}
            external_match_modal::ExternalMatchReviewAction::Accept => {
                let Some(w) = witness else { return };
                self.stage_external_match_accept(w);
            }
            external_match_modal::ExternalMatchReviewAction::Dismiss => {
                // Skip to next entry without staging a mutation.
                if let ActiveView::ExternalMatchReview(ref mut state) = self.view {
                    if !state.advance() {
                        // Was the last entry — show review if any decisions were staged.
                        self.after_staging_decisions();
                    }
                }
            }
            external_match_modal::ExternalMatchReviewAction::Cancel => {
                self.cancel_and_return_to_source("External match review cancelled");
            }
            external_match_modal::ExternalMatchReviewAction::OpenRecordingUrl(url) => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }

    /// Stage tag edit mutations from the current entry's external match diffs.
    fn stage_external_match_accept(&mut self, gesture: &witness::ConfirmationGesture) {
        let (inode, ops) = {
            let state = match self.view {
                ActiveView::ExternalMatchReview(ref state) => state,
                _ => return,
            };

            let Some(entry) = state.current_entry() else { return };
            let inode = entry.inode;

            let mut ops = Vec::new();
            for diff in &entry.diffs {
                match &diff.corpus_value {
                    Some(old) => {
                        // Replace existing tag value with external value
                        ops.push(TagOp::replace_tag(
                            inode,
                            &diff.tag_name,
                            old.clone(),
                            &diff.external_value,
                        ));
                    }
                    None => {
                        // Add tag value (corpus doesn't have it)
                        ops.push(TagOp::add_tag(
                            inode,
                            &diff.tag_name,
                            &diff.external_value,
                        ));
                    }
                }
            }

            (inode, ops)
        };

        if ops.is_empty() {
            return;
        }

        let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops,
            zone: Zone::Corpus,
        });
        let key = DecisionKey::ExternalMatch { inode };
        let label = "Accept external match tags";

        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            label,
            vec![mutation],
            gesture,
        );

        // Advance to next entry, or show review if at end.
        if let ActiveView::ExternalMatchReview(ref mut state) = self.view {
            if !state.advance() {
                self.after_staging_decisions();
            }
        }
    }
}
