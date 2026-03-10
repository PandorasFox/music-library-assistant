//! Transaction review action handlers.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::ui::active_view::ActiveView;
use crate::ui::suspended_views::SuspendTarget;
use crate::ui::{progress_screen, transaction_review, widgets};

impl HandleAction for transaction_review::TransactionReviewAction {
    fn handle(self, app: &mut App, gesture: Option<&witness::ConfirmationGesture>) {
        use transaction_review::TransactionReviewAction;

        match self {
            TransactionReviewAction::None => {}

            TransactionReviewAction::Cancel => {
                if app.witch.decision_keys().is_empty() {
                    // Empty transaction — treat Esc as exit request
                    if app.has_pending_operations() {
                        app.status_message =
                            Some("Cannot quit while operations are pending".to_string());
                    } else {
                        app.view =
                            ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                    }
                } else {
                    // Pop the view stack to restore the parent view
                    if !app.pop_and_restore() {
                        app.start_health_view();
                    }
                }
            }

            TransactionReviewAction::Discard => {
                // Discard transaction, clear entire view stack, return to health
                app.clear_view_stack();
                let _ = super::super::operator_decisions::discard_transaction(&mut app.witch);
                app.start_health_view();
                app.status_message = Some("Transaction discarded".to_string());
            }

            TransactionReviewAction::Confirm => {
                let Some(g) = gesture else { return };

                // Determine progress phase before clearing state
                let post_commit_phase = if let ActiveView::TransactionReview(ref review) = app.view
                {
                    review.post_commit_phase
                } else {
                    transaction_review::PostCommitPhase::default()
                };

                // Clear entire view stack — commit is a hard navigation
                app.clear_view_stack();

                // Commit transaction
                let commit_result =
                    super::super::operator_decisions::commit_transaction(&mut app.witch, g);

                match commit_result {
                    Ok(()) => {
                        // Don't set status_message here — it would suppress the
                        // selected-path display in status_line_1, and the commit
                        // outcome is already evident from the progress screen.

                        // Use appropriate progress phase based on source
                        let phase = match post_commit_phase {
                            transaction_review::PostCommitPhase::SignalRefresh => {
                                progress_screen::ProgressPhase::SignalRefresh
                            }
                            transaction_review::PostCommitPhase::ContentAnalysis => {
                                progress_screen::ProgressPhase::ContentAnalysis
                            }
                        };
                        app.transition_to_progress_after_mutations(phase);
                    }
                    Err(e) => {
                        app.status_message = Some(format!("Commit failed: {}", e));
                        app.start_health_view();
                    }
                }
            }

            TransactionReviewAction::RequestRemoval => {
                // Map cursor position to DecisionKey and set pending_removal
                if let ActiveView::TransactionReview(ref mut review) = app.view {
                    if let Some(d) = review.decisions.get(review.cursor()) {
                        review.pending_removal = Some(d.key.clone());
                    }
                }
            }

            TransactionReviewAction::ConfirmRemoval(key) => {
                let Some(g) = gesture else { return };
                let _ = super::super::operator_decisions::remove_decision(&mut app.witch, &key, g);

                // If transaction is now empty, auto-close review
                if app.witch.decision_keys().is_empty() {
                    if !app.pop_and_restore() {
                        app.start_health_view();
                    }
                    app.status_message = Some("Decision removed, transaction empty".to_string());
                    return;
                }
                // Refresh decisions and clamp cursor after removal
                if let ActiveView::TransactionReview(ref mut review) = app.view {
                    review.refresh_decisions(&app.witch);
                }
                app.status_message = Some("Decision removed".to_string());
            }
        }
    }
}

impl HandleAction for super::super::tabbed_transaction_review::TabbedTransactionReviewAction {
    fn handle(self, app: &mut App, gesture: Option<&witness::ConfirmationGesture>) {
        use super::super::tabbed_transaction_review::TabbedTransactionReviewAction;
        use transaction_review::TransactionReviewAction;

        match self {
            TabbedTransactionReviewAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::Transaction, true);
            }
            TabbedTransactionReviewAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::Transaction, false);
            }
            TabbedTransactionReviewAction::RequestQuit => app.handle_request_quit(),
            TabbedTransactionReviewAction::Review(review_action) => match review_action {
                TransactionReviewAction::None => {}
                TransactionReviewAction::Cancel => {} // No cancel in tabbed mode
                TransactionReviewAction::Confirm => {
                    let Some(g) = gesture else { return };
                    let _ = super::super::operator_decisions::commit_transaction(&mut app.witch, g);
                    // Re-open transaction immediately
                    let _ = app.witch.start_transaction("Open");
                    app.transition_to_progress_after_mutations(
                        progress_screen::ProgressPhase::SignalRefresh,
                    );
                }
                TransactionReviewAction::Discard => {
                    let _ = super::super::operator_decisions::discard_transaction(&mut app.witch);
                    // Re-open transaction immediately
                    let _ = app.witch.start_transaction("Open");
                    app.sync_browser_pending_edits();
                    app.status_message = Some("Transaction discarded".into());
                }
                TransactionReviewAction::RequestRemoval => {
                    if let ActiveView::TabbedTransactionReview(ref mut state) = app.view {
                        if let Some(d) = state.review.decisions.get(state.review.cursor()) {
                            state.review.pending_removal = Some(d.key.clone());
                        }
                    }
                }
                TransactionReviewAction::ConfirmRemoval(key) => {
                    let Some(g) = gesture else { return };
                    let _ = super::super::operator_decisions::remove_decision(&mut app.witch, &key, g);
                    app.status_message = Some("Decision removed".into());
                    app.sync_browser_pending_edits();
                    // Refresh decisions and clamp cursor
                    if let ActiveView::TabbedTransactionReview(ref mut state) = app.view {
                        state.review.refresh_decisions(&app.witch);
                    }
                }
            },
        }
    }
}

impl App {
    /// Transition to the standardized transaction review modal.
    ///
    /// Called after staging decisions to show the review before commit.
    /// Pushes the current view onto the view stack and switches to review.
    pub(in crate::ui) fn start_transaction_review(&mut self) {
        let mut review = transaction_review::TransactionReviewState::new();
        review.refresh_decisions(&self.witch);
        self.push_and_switch(SuspendTarget::TransactionReview(review));
    }

    /// Transition to transaction review modal with custom post-commit phase.
    ///
    /// Used for intake indexing which needs ContentAnalysis instead of SignalRefresh.
    pub(super) fn start_transaction_review_with_phase(
        &mut self,
        phase: transaction_review::PostCommitPhase,
    ) {
        let mut review =
            transaction_review::TransactionReviewState::new().with_post_commit_phase(phase);
        review.refresh_decisions(&self.witch);
        self.push_and_switch(SuspendTarget::TransactionReview(review));
    }
}
