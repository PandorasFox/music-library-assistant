//! Missing album single resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::decisions::DecisionKey;
use crate::active_view::ActiveView;
use crate::tag_editor;

impl HandleAction for crate::missing_album_modal::MissingAlbumAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::{
            indexing::EmitExpectedMissingTagMutation, tag_edit::ApplyTagOpsMutation, Mutation,
            TagOp,
        };
        use crate::missing_album_modal::{AlbumResolution, MissingAlbumAction};

        match self {
            MissingAlbumAction::None => {}

            MissingAlbumAction::Cancel => {
                app.cancel_and_return_to_source("Missing album single resolution cancelled");
            }

            MissingAlbumAction::Confirm(resolution) => {
                let Some(g) = witness else { return };

                let group_idx = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = app.view else {
                        return;
                    };
                    state.current_group
                };

                match resolution {
                    AlbumResolution::PerTrackTitle | AlbumResolution::AllSingles => {
                        let (ops, label): (Vec<TagOp>, &str) = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = app.view
                            else {
                                return;
                            };
                            let Some(group) = state.current_group_data() else {
                                return;
                            };
                            match resolution {
                                AlbumResolution::PerTrackTitle => (
                                    group.tracks.iter().map(|t| {
                                        TagOp::add_tag(t.inode, "ALBUM", format!("{}{}", t.title, state.suffix))
                                    }).collect(),
                                    "Tag as singles",
                                ),
                                _ => (
                                    group.tracks.iter().map(|t| TagOp::add_tag(t.inode, "ALBUM", "Singles")).collect(),
                                    "Tag all as Singles",
                                ),
                            }
                        };
                        if !ops.is_empty() {
                            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                                ops,
                                zone: Zone::Corpus,
                            });
                            let decision = g.decide(label, vec![mutation]);
                            let _ = super::super::operator_decisions::stage_decision(
                                app,
                                DecisionKey::MissingAlbum {
                                    group_index: group_idx,
                                },
                                decision,
                            );
                        }
                    }

                    AlbumResolution::Suppress => {
                        let inodes: Vec<i64> = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = app.view
                            else {
                                return;
                            };
                            state.current_group_inodes()
                        };
                        if !inodes.is_empty() {
                            let mutation =
                                Mutation::EmitExpectedMissingTag(EmitExpectedMissingTagMutation {
                                    inodes,
                                });
                            let decision = g.decide("Suppress missing album", vec![mutation]);
                            let _ = super::super::operator_decisions::stage_decision(
                                app,
                                DecisionKey::MissingAlbum {
                                    group_index: group_idx,
                                },
                                decision,
                            );
                        }
                    }
                }

                // Advance to next unresolved group, or show review if at end
                if let ActiveView::MissingAlbumSingleResolution(ref mut state) = app.view {
                    state.track_cursor = 0;
                    state.track_scroll = 0;
                    if state.current_group + 1 < state.data.groups.len() {
                        state.current_group += 1;
                    } else {
                        // All groups visited — go to review
                        app.after_staging_decisions();
                    }
                }
            }

            MissingAlbumAction::NavigateGroup(forward) => {
                if let ActiveView::MissingAlbumSingleResolution(ref mut state) = app.view {
                    if forward {
                        if state.current_group + 1 < state.data.groups.len() {
                            state.current_group += 1;
                            state.track_cursor = 0;
                            state.track_scroll = 0;
                        }
                    } else if state.current_group > 0 {
                        state.current_group -= 1;
                        state.track_cursor = 0;
                        state.track_scroll = 0;
                    }
                }
            }

            MissingAlbumAction::ShowReview => {
                app.after_staging_decisions();
            }

            MissingAlbumAction::EditTracks | MissingAlbumAction::EditTracksAggregated => {
                let mode = match self {
                    MissingAlbumAction::EditTracks => tag_editor::TagEditorMode::Individual,
                    _ => tag_editor::TagEditorMode::Aggregated,
                };
                let (inodes, key, label) = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = app.view else {
                        return;
                    };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let key = DecisionKey::TagEdit {
                        key_item: format!("missing_album_{}", state.current_group),
                    };
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), key, label)
                };
                app.open_tag_editor_for_inodes(inodes, mm_meta::db_types::Zone::Corpus, key, label, mode);
            }
        }
    }
}

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
                    ActiveView::MissingAlbumSingleResolutionV3 {
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
                    let mutation = Mutation::EmitExpectedMissingTag(EmitExpectedMissingTagMutation {
                        inodes,
                    });
                    let decision = g.decide("Suppress missing album", vec![mutation]);
                    let _ = super::super::operator_decisions::stage_decision(
                        app,
                        DecisionKey::MissingAlbum { group_index: group_idx },
                        decision,
                    );
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
    /// Start missing album single resolution from Insights view.
    ///
    /// Loads MissingAlbumSingle signals, converts to modal data, starts a
    /// transaction, and switches to the MissingAlbumSingleResolution view.
    pub(super) fn start_missing_album_single_resolution(&mut self) {
        use crate::missing_album_modal;

        let signals = self
            .query(mm_meta::domain_queries::GetMissingAlbumSingleSignals);

        let data = missing_album_modal::MissingAlbumData::from_signals(signals);
        let suffix = self
            .config()
            .opinions
            .health_detection
            .single_album_suffix
            .clone();

        // Start transaction for the resolution session
        let _ = self.start_transaction("Missing album singles");

        let state = missing_album_modal::MissingAlbumState::new(data, suffix);
        self.view = ActiveView::MissingAlbumSingleResolution(state);
    }

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

        self.view = ActiveView::MissingAlbumSingleResolutionV3 {
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
            ActiveView::MissingAlbumSingleResolutionV3 {
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
            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                ops,
                zone: Zone::Corpus,
            });
            let decision = gesture.decide(label, vec![mutation]);
            let _ = super::super::operator_decisions::stage_decision(
                self,
                DecisionKey::MissingAlbum { group_index: group_idx },
                decision,
            );
        }
    }

    /// Advance to next group or go to review (V3 missing album).
    fn advance_missing_album_v3(&mut self) {
        if let ActiveView::MissingAlbumSingleResolutionV3 {
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
