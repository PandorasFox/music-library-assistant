//! Missing album single resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::active_view::ActiveView;
use mm_ui::resolutions::dispatch::{Dispatchable, DispatchResult};

impl HandleAction for mm_ui::resolutions::missing_album::MissingAlbumAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        let result = {
            let ActiveView::MissingAlbumSingleResolution(ref state) = app.view else {
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
                if let ActiveView::MissingAlbumSingleResolution(ref mut state) = app.view {
                    if !state.advance() {
                        app.after_staging_decisions();
                    }
                }
            }
            DispatchResult::Cancel => {
                app.cancel_and_return_to_source("Missing album single resolution cancelled");
            }
            DispatchResult::Skip
            | DispatchResult::StageKeep { .. }
            | DispatchResult::Handled => {}
        }
    }
}

impl App {
    /// Start V3 missing album single resolution.
    pub(super) fn start_missing_album_single_resolution_v3(&mut self) {
        use mm_ui::resolutions::missing_album::{MissingAlbumData, MissingAlbumState};

        let signals = self.query(mm_meta::domain_queries::GetMissingAlbumSingleSignals);

        if signals.is_empty() {
            self.status_message = Some("No missing album signals found".to_string());
            return;
        }

        let suffix = self
            .config()
            .opinions
            .health_detection
            .single_album_suffix
            .clone();

        let _ = self.start_transaction("Missing album singles");

        let state = MissingAlbumState::new(MissingAlbumData::new(signals, suffix));
        self.view = ActiveView::MissingAlbumSingleResolution(state);
    }
}
