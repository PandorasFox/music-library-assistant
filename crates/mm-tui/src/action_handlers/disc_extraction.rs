//! Disc extraction resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::active_view::ActiveView;
use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

impl HandleAction for mm_ui::resolutions::disc_extraction::DiscExtractionAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        let result = {
            let ActiveView::DiscExtractionResolution(ref state) = app.view else {
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
                if let ActiveView::DiscExtractionResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Skip => {
                if let ActiveView::DiscExtractionResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Cancel => {
                app.cancel_and_return_to_source("Disc extraction resolution cancelled");
            }
            DispatchResult::StageKeep { .. } | DispatchResult::Handled => {}
        }
    }
}

impl App {
    /// Start V3 disc extraction resolution.
    pub(super) fn start_disc_extraction_resolution_v3(&mut self) {
        use mm_meta::domain_queries::GetDiscExtractionData;
        use mm_ui::resolutions::disc_extraction::{DiscExtractionData, DiscExtractionState};

        let config = self.config().opinions.disc_extraction.clone();

        let data = self.query(GetDiscExtractionData {
            map_letters_to_numbers: config.map_letters_to_numbers,
        });

        if data.groups.is_empty() {
            self.status_message = Some("No disc extraction signals found".to_string());
            return;
        }

        let _ = self.start_transaction("Disc extraction");

        let state = DiscExtractionState::new(
            DiscExtractionData::new(data, config.disc_tag_name),
        );
        self.view = ActiveView::DiscExtractionResolution(state);
    }
}
