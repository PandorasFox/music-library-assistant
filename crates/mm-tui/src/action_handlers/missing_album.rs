//! Missing album single resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::resolutions::missing_album::{MissingAlbumButton, MissingAlbumButtonCtx};
use crate::active_view::ActiveView;

// =========================================================================
// V3: Single-load missing album with packed data
// =========================================================================

impl HandleAction for mm_ui::resolutions::missing_album::MissingAlbumAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_meta::mutations::{
            indexing::EmitExpectedMissingTagMutation, Mutation,
            TagOp,
        };
        use mm_ui::resolutions::missing_album::MissingAlbumAction;

        match self {
            MissingAlbumAction::PerTrackTitle => {
                let Some(g) = witness else { return };
                app.stage_missing_album_v3(g, |signal, suffix| {
                    signal.data.tracks.iter().map(|t| {
                        TagOp::add_tag(t.inode, "ALBUM", format!("{}{}", t.title, suffix))
                    }).collect()
                }, "Tag as singles");
                app.advance_missing_album_v3();
            }
            MissingAlbumAction::AllSingles => {
                let Some(g) = witness else { return };
                app.stage_missing_album_v3(g, |signal, _suffix| {
                    signal.data.tracks.iter().map(|t| {
                        TagOp::add_tag(t.inode, "ALBUM", "Singles")
                    }).collect()
                }, "Tag all as Singles");
                app.advance_missing_album_v3();
            }
            MissingAlbumAction::Suppress => {
                let Some(g) = witness else { return };
                let (group_idx, inodes) = match &app.view {
                    ActiveView::MissingAlbumSingleResolution {
                        ref data, current_group, ..
                    } => {
                        let inodes: Vec<i64> = data.get(*current_group)
                            .map(|s| s.data.tracks.iter().map(|t| t.inode).collect())
                            .unwrap_or_default();
                        (*current_group, inodes)
                    }
                    _ => return,
                };
                if !inodes.is_empty() {
                    let ctx = MissingAlbumButtonCtx { has_tracks: true, group_index: group_idx };
                    let key = MissingAlbumButton::Suppress.protocol_binding(&ctx)
                        .decision_key().unwrap().clone();
                    let mutation = Mutation::EmitExpectedMissingTag(EmitExpectedMissingTagMutation {
                        inodes,
                    });
                    let decision = g.decide("Suppress missing album", vec![mutation]);
                    let _ = super::super::operator_decisions::stage_decision(app, key, decision);
                }
                app.advance_missing_album_v3();
            }
            MissingAlbumAction::Cancel => {
                app.cancel_and_return_to_source("Missing album single resolution cancelled");
            }
        }
    }
}

impl App {
    /// Start V3 missing album single resolution.
    pub(super) fn start_missing_album_single_resolution_v3(&mut self) {
        let signals = self
            .query(mm_meta::domain_queries::GetMissingAlbumSingleSignals);

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

        self.view = ActiveView::MissingAlbumSingleResolution {
            data: signals,
            current_group: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            focus: mm_ui::geometry::FocusPane::List,
            suffix,
        };
    }

    /// Stage a missing album decision from the V3 view.
    fn stage_missing_album_v3(
        &mut self,
        gesture: &witness::ConfirmationGesture,
        build_ops: impl FnOnce(&mm_meta::domain_queries::MissingAlbumSingleSignalWire, &str) -> Vec<mm_meta::mutations::TagOp>,
        label: &str,
    ) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation};

        let (ops, group_idx) = match &self.view {
            ActiveView::MissingAlbumSingleResolution {
                ref data, current_group, ref suffix, ..
            } => {
                let signal = match data.get(*current_group) {
                    Some(s) => s,
                    None => return,
                };
                (build_ops(signal, suffix), *current_group)
            }
            _ => return,
        };

        if !ops.is_empty() {
            let ctx = MissingAlbumButtonCtx { has_tracks: true, group_index: group_idx };
            let key = MissingAlbumButton::PerTrackTitle.protocol_binding(&ctx)
                .decision_key().unwrap().clone();
            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                ops,
                zone: Zone::Corpus,
            });
            let decision = gesture.decide(label, vec![mutation]);
            let _ = super::super::operator_decisions::stage_decision(self, key, decision);
        }
    }

    /// Advance to next group or go to review (V3 missing album).
    fn advance_missing_album_v3(&mut self) {
        if let ActiveView::MissingAlbumSingleResolution {
            ref data, ref mut current_group, ref mut list, ..
        } = self.view
        {
            if *current_group + 1 < data.len() {
                *current_group += 1;
                list.reset();
            } else {
                self.after_staging_decisions();
            }
        } else {
            self.after_staging_decisions();
        }
    }
}
