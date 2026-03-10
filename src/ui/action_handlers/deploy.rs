//! Deployment Preview Modal
//!
//! Handles the deployment preview: loading deploy data, staging deploy mutations
//! (leftovers, stale, new, sidecars), and the preview action handler.

use super::super::App;
use super::witness;
use crate::corpus::paths;
use crate::meta::decisions::DecisionKey;
use crate::ui::{deploy_modal, ActiveView};

impl App {
    /// Handle Deploy lateral view actions.
    pub(super) fn handle_deploy_action(
        &mut self,
        action: deploy_modal::DeployAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            deploy_modal::DeployAction::None => {}
            deploy_modal::DeployAction::CycleNext => {
                self.handle_lateral_cycle(crate::ui::widgets::LateralView::Deploy, true);
            }
            deploy_modal::DeployAction::CyclePrev => {
                self.handle_lateral_cycle(crate::ui::widgets::LateralView::Deploy, false);
            }
            deploy_modal::DeployAction::Confirm => {
                let Some(w) = witness else { return };
                // Extract cached data from Preview state
                let cached_data = match &self.view {
                    ActiveView::Deploy(deploy_modal::DeployViewState::Preview(ref preview)) => {
                        Some(preview.cached_data.clone())
                    }
                    _ => None,
                };
                if let Some(data) = cached_data {
                    let mutation_count = self.stage_deploy_mutations(&data, w);
                    if mutation_count > 0 {
                        self.after_staging_decisions();
                    } else {
                        self.status_message = Some("No deploy operations needed".to_string());
                    }
                }
            }
            deploy_modal::DeployAction::RequestQuit => self.handle_request_quit(),
        }
    }

    /// Stage deploy mutations for transaction review.
    ///
    /// Returns the number of mutations staged.
    fn stage_deploy_mutations(
        &mut self,
        data: &deploy_modal::DeployModalData,
        gesture: &witness::ConfirmationGesture,
    ) -> usize {
        let open_txn = self.open_txn_mode();
        let resolver = paths::get_resolver();
        let mutation_set = data.to_mutations(resolver);

        if mutation_set.skipped > 0 {
            crate::logging::log_error(format!(
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
            let _ = super::super::operator_decisions::stage_decision(
                &mut self.witch,
                DecisionKey::Deploy,
                "Deploy operations",
                mutation_set.deploy,
                gesture,
            );
        }
        if !mutation_set.sidecars.is_empty() {
            let _ = super::super::operator_decisions::stage_decision(
                &mut self.witch,
                DecisionKey::DeploySidecars,
                &format!("Deploy cover art ({} images)", mutation_set.sidecars.len()),
                mutation_set.sidecars,
                gesture,
            );
        }

        count
    }
}
