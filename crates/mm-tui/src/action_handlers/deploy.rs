//! Deployment Preview Modal
//!
//! Handles the deployment preview: loading deploy data, staging deploy mutations
//! (leftovers, stale, new, sidecars), and the preview action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::decisions::DecisionKey;
use crate::{deploy_modal, ActiveView};

impl HandleAction for deploy_modal::DeployAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            deploy_modal::DeployAction::None => {}
            deploy_modal::DeployAction::CycleNext => {
                app.handle_lateral_cycle(crate::widgets::LateralView::Deploy, true);
            }
            deploy_modal::DeployAction::CyclePrev => {
                app.handle_lateral_cycle(crate::widgets::LateralView::Deploy, false);
            }
            deploy_modal::DeployAction::Confirm => {
                let Some(w) = witness else { return };
                // Extract cached data from Preview state
                let cached_data = match &app.view {
                    ActiveView::Deploy(deploy_modal::DeployViewState::Preview(ref preview)) => {
                        Some(preview.cached_data.clone())
                    }
                    _ => None,
                };
                if let Some(data) = cached_data {
                    let mutation_count = app.stage_deploy_mutations(&data, w);
                    if mutation_count > 0 {
                        app.after_staging_decisions();
                    } else {
                        app.status_message = Some("No deploy operations needed".to_string());
                    }
                }
            }
            deploy_modal::DeployAction::RequestQuit => app.handle_request_quit(),
        }
    }
}

impl App {
    /// Stage deploy mutations for transaction review.
    ///
    /// Returns the number of mutations staged.
    fn stage_deploy_mutations(
        &mut self,
        data: &deploy_modal::DeployModalData,
        gesture: &witness::ConfirmationGesture,
    ) -> usize {
        let open_txn = self.open_txn_mode();
        let mutation_set = data.to_mutations(&self.resolver);

        if mutation_set.skipped > 0 {
            mm_meta::logging::log_error(format!(
                "[DEPLOY] {} new files skipped: no library_name (source dir without libraries?)",
                mutation_set.skipped,
            ));
        }

        let count = mutation_set.deploy.len() + mutation_set.sidecars.len();
        if count == 0 {
            return 0;
        }

        // Start transaction and stage decisions
        if !open_txn {
            let _ = self.witch.start_transaction("Deploy");
        }
        if !mutation_set.deploy.is_empty() {
            let decision = gesture.decide("Deploy operations", mutation_set.deploy);
            let _ = super::super::operator_decisions::stage_decision(
                &mut self.witch,
                DecisionKey::Deploy,
                decision,
            );
        }
        if !mutation_set.sidecars.is_empty() {
            let sidecar_label = format!("Deploy cover art ({} images)", mutation_set.sidecars.len());
            let decision = gesture.decide(&sidecar_label, mutation_set.sidecars);
            let _ = super::super::operator_decisions::stage_decision(
                &mut self.witch,
                DecisionKey::DeploySidecars,
                decision,
            );
        }

        count
    }
}
