//! Deployment Preview Modal
//!
//! Handles the deployment preview: staging deploy mutations via the shared
//! mm-ui deploy logic, then routing to transaction review.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::{deploy_modal, ActiveView};

impl HandleAction for deploy_modal::DeployAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            deploy_modal::DeployAction::Confirm => {
                let Some(w) = witness else { return };
                // Extract cached data from Preview state
                let cached_data = match &app.view {
                    ActiveView::Deploy(ref s) => match &s.data {
                        deploy_modal::DeployViewData::Preview { ref cached_data } => Some(cached_data.clone()),
                        _ => None,
                    },
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
        let prepared = mm_ui::deploy::prepare_deploy_decisions(data, &self.resolver);

        if prepared.skipped > 0 {
            mm_meta::logging::log_error(format!(
                "[DEPLOY] {} new files skipped: no library_name (source dir without libraries?)",
                prepared.skipped,
            ));
        }

        let count: usize = prepared.decisions.iter().map(|d| d.mutations.len()).sum();
        if count == 0 {
            return 0;
        }

        if !self.open_txn_mode() {
            let _ = self.start_transaction("Deploy");
        }

        for dd in prepared.decisions {
            let decision = gesture.decide(&dd.label, dd.mutations);
            let _ = super::super::operator_decisions::stage_decision(self, dd.key, decision);
        }

        count
    }
}
