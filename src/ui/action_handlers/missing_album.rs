//! Missing album single resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::meta::decisions::DecisionKey;
use crate::ui::active_view::ActiveView;
use crate::ui::tag_editor;

impl HandleAction for crate::ui::missing_album_modal::MissingAlbumAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use crate::db::types::Zone;
        use crate::meta::mutations::{
            indexing::EmitExpectedMissingTagMutation, tag_edit::ApplyTagOpsMutation, Mutation,
            TagOp,
        };
        use crate::ui::missing_album_modal::{AlbumResolution, MissingAlbumAction};

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
                            let _ = super::super::operator_decisions::stage_decision(
                                &mut app.witch,
                                DecisionKey::MissingAlbum {
                                    group_index: group_idx,
                                },
                                label,
                                vec![mutation],
                                g,
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
                            let _ = super::super::operator_decisions::stage_decision(
                                &mut app.witch,
                                DecisionKey::MissingAlbum {
                                    group_index: group_idx,
                                },
                                "Suppress missing album",
                                vec![mutation],
                                g,
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
                app.open_tag_editor_for_inodes(inodes, crate::db::types::Zone::Corpus, key, label, mode);
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
        use crate::ui::missing_album_modal;

        let signals = self
            .cache
            .domain_query(crate::db::domain::GetMissingAlbumSingleSignals)
            .recv();

        let data = missing_album_modal::MissingAlbumData::from_signals(signals);
        let suffix = self
            .config()
            .opinions
            .health_detection
            .single_album_suffix
            .clone();

        // Start transaction for the resolution session
        let _ = self.witch.start_transaction("Missing album singles");

        let state = missing_album_modal::MissingAlbumState::new(data, suffix);
        self.view = ActiveView::MissingAlbumSingleResolution(state);
    }
}
