//! Compound Tag Split Resolution
//!
//! Uses Dispatchable for mutation building. ViewState handles input/navigation;
//! this handler dispatches actions and manages view transitions.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_ui::resolutions::compound_split::{CompoundSplitAction, CompoundSplitData, CompoundSplitViewState};
use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};
use crate::ActiveView;

impl HandleAction for CompoundSplitAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        let result = {
            let ActiveView::CompoundTagSplitResolution(ref state) = app.view else {
                return;
            };
            state.dispatch(self, &app.resolver)
        };

        match result {
            DispatchResult::Stage { key, label, mutations } => {
                let Some(w) = witness else { return };
                if mutations.is_empty() { return; }
                let decision = w.decide(&label, mutations);
                let _ = super::super::operator_decisions::stage_decision(app, key, decision);
                if let ActiveView::CompoundTagSplitResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Cancel => {
                app.cancel_and_return_to_source("Compound tag split cancelled");
            }
            DispatchResult::Skip
            | DispatchResult::StageKeep { .. }
            | DispatchResult::Handled => {}
        }
    }
}

impl App {
    /// Start compound split resolution using the packed query (V3).
    pub(crate) fn start_compound_split_resolution_v3(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
    ) {
        self.start_compound_split_resolution_v3_for_zone(safe_only, tag_filter, Zone::Corpus);
    }

    /// Start compound split resolution V3 for a given zone.
    fn start_compound_split_resolution_v3_for_zone(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
        zone: Zone,
    ) {
        // Determine tag_name: use filter if provided, otherwise empty string to load all
        let tag_name = tag_filter.unwrap_or("").to_string();

        // Single packed query -- all groups at once
        let data = self.query(mm_meta::domain_queries::GetCompoundSplitResolution {
            tag_name,
            zone,
            safe_only,
        });

        if data.groups.is_empty() {
            self.status_message = Some("No compound tag signals to resolve".to_string());
            return;
        }

        // Start transaction
        let mode_str = if zone == Zone::Inbox {
            "inbox"
        } else if safe_only {
            "safe"
        } else {
            "review"
        };
        let _ = self.start_transaction(&format!("Compound tag split ({})", mode_str));

        // Pre-fill DecisionField with first group's split parts
        let prefill = data.groups.first()
            .map(|g| g.split_parts.join("; "))
            .unwrap_or_default();

        let cs_data = CompoundSplitData::new(data, zone, safe_only);
        let state = CompoundSplitViewState::new(cs_data, &prefill);
        self.view = ActiveView::CompoundTagSplitResolution(state);
    }
}
