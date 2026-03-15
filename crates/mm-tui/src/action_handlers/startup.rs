//! Startup and lifecycle action handlers.
//!
//! Handles intake confirmation and exit confirmation actions.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::active_view::ActiveView;

impl HandleAction for super::super::ExitConfirmAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        use super::super::ExitConfirmAction;
        match self {
            ExitConfirmAction::None => {}
            ExitConfirmAction::Quit => {
                app.should_quit = true;
            }
            ExitConfirmAction::QuitAndShutdown => {
                let _ = app.shutdown();
                app.should_quit = true;
            }
            ExitConfirmAction::Cancel => {
                app.start_health_view();
            }
        }
    }
}

impl HandleAction for crate::startup::IntakeConfirmationAction {
    fn handle(self, app: &mut App, gesture: Option<&witness::ConfirmationGesture>) {
        use crate::startup::IntakeConfirmationAction;
        use super::super::operator_decisions;

        // Determine source before matching (used for post-action routing)
        let is_inbox_source = matches!(
            app.view,
            ActiveView::IntakeConfirmation(ref s) if s.source == crate::startup::IntakeSource::Inbox
        );

        match self {
            IntakeConfirmationAction::None => {}
            IntakeConfirmationAction::Confirmed => {
                let Some(g) = gesture else { return };

                // User confirmed - create IndexTrack mutations and stage for review
                let mutations = if let ActiveView::IntakeConfirmation(ref s) = app.view {
                    s.create_index_mutations()
                } else {
                    Vec::new()
                };

                if mutations.is_empty() {
                    // No files to index (all deleted since detection?)
                    mm_meta::logging::log_general("IntakeConfirmation: no mutations to queue");
                    if is_inbox_source {
                        app.view = ActiveView::Inbox {
                            data: super::super::inbox_view::InboxViewData::new(),
                            interaction: super::super::inbox_view::InboxInteraction::new(),
                        };
                    } else {
                        app.start_health_view();
                    }
                } else {
                    let count = mutations.len();
                    mm_meta::logging::log_general(format!(
                        "IntakeConfirmation: user confirmed, staging {} IndexTrack mutations for review",
                        count
                    ));

                    // Start transaction and stage the decision
                    let open_txn = app.open_txn_mode();
                    if !open_txn {
                        let _ = app.start_transaction("Intake indexing");
                    }
                    let decision = g.decide("Index unindexed files", mutations);
                    let _ = operator_decisions::stage_decision(
                        app,
                        mm_ui::decision_keys::intake_index(),
                        decision,
                    );

                    if open_txn {
                        app.start_tabbed_transaction_review();
                        app.status_message = Some(format!("{} files staged for indexing", count));
                    } else {
                        // Note: IntakeConfirmation state is preserved inside the suspended view for Cancel return
                        // Transition to review modal with ContentAnalysis phase for post-commit
                        app.start_transaction_review_with_phase(
                            crate::transaction_review::PostCommitPhase::ContentAnalysis,
                        );
                    }
                }
            }
            IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran
                // Unindexed signals remain for later handling
                mm_meta::logging::log_general("IntakeConfirmation: user skipped indexing");

                // Discard any active transaction from review modal (not in open-txn mode)
                if !app.open_txn_mode() && app.witch_status().transaction.is_some() {
                    let _ = operator_decisions::discard_transaction(app);
                }

                if is_inbox_source {
                    app.view = ActiveView::Inbox {
                            data: super::super::inbox_view::InboxViewData::new(),
                            interaction: super::super::inbox_view::InboxInteraction::new(),
                        };
                } else {
                    app.start_health_view();
                }
            }
        }
    }
}

impl App {
    /// Start intake confirmation from Insights view.
    ///
    /// Gathers unindexed files and opens the intake confirmation modal.
    pub(super) fn start_intake_confirmation_from_health(&mut self) {
        let intake_state = self
            .query(mm_meta::domain_queries::GetIntakeConfirmation {
                source: crate::startup::IntakeSource::Health,
                zone: Some(mm_meta::db_types::Zone::Corpus),
            });

        if let Some(state) = intake_state {
            self.view = ActiveView::IntakeConfirmation(state);
        }
    }
}
