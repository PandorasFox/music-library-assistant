//! Tag Canonicity Resolution
//!
//! Uses Dispatchable for mutation building. ViewState handles input/navigation;
//! this handler dispatches actions and manages view transitions.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};
use mm_ui::resolutions::tag_canonicity::{CanonicityAction, CanonicityMode, TagCanonicityData, TagCanonicityViewState};
use crate::{insights_view, ActiveView};

impl HandleAction for CanonicityAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        let result = {
            let ActiveView::TagCanonicityResolution(ref state) = app.view else {
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
                if let ActiveView::TagCanonicityResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Cancel => {
                app.cancel_and_return_to_source("Tag canonicity resolution cancelled");
            }
            DispatchResult::Skip
            | DispatchResult::StageKeep { .. }
            | DispatchResult::Handled => {}
        }
    }
}

impl App {
    /// Start tag canonicity resolution using the packed query (V3).
    pub(crate) fn start_tag_canonicity_resolution_v3(&mut self) {
        let insight_type = match &self.view {
            ActiveView::Insights(ref s) => s.data.insight_type_at(s.interaction.list.cursor),
            _ => None,
        };
        let insight_type = match insight_type {
            Some(t) => t,
            None => {
                self.status_message = Some("No insight selected".to_string());
                return;
            }
        };

        // Determine tag_name, zone, and canonicity mode
        let (tag_name, zone, mode) = match &insight_type {
            insights_view::InsightType::InconsistentAlbumArtist => {
                ("ALBUMARTIST".to_string(), Zone::Corpus, CanonicityMode::InconsistentAlbumArtist)
            }
            insights_view::InsightType::TagCanonicity { tag_name } => {
                (tag_name.clone(), Zone::Corpus, CanonicityMode::TagCanonicity)
            }
            _ => {
                self.status_message = Some("Invalid insight type for tag resolution".to_string());
                return;
            }
        };

        // Single packed query — all clusters at once
        let data = self.query(mm_meta::domain_queries::GetTagCanonicityResolution {
            tag_name,
            zone,
            filter_existing_canonicals: true,
        });

        if data.clusters.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        // Start transaction
        let _ = self.start_transaction("Tag canonicalization");

        // Pre-fill DecisionField with first cluster's canonical candidate
        let prefill: String = data.clusters.first()
            .and_then(|c| c.suggested_canonical.as_deref())
            .unwrap_or("")
            .to_string();
        let field_label = match mode {
            CanonicityMode::InconsistentAlbumArtist => "Album artist:",
            CanonicityMode::TagCanonicity => "Squash to:",
        };

        let tc_data = TagCanonicityData::new(data, zone, mode);
        let state = TagCanonicityViewState::new(tc_data, field_label, &prefill);
        self.view = ActiveView::TagCanonicityResolution(state);
    }
}
