//! Disc extraction resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::meta::decisions::DecisionKey;
use crate::ui::active_view::ActiveView;
use crate::ui::tag_editor;

impl HandleAction for crate::ui::disc_extraction_modal::DiscExtractionAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use crate::db::types::Zone;
        use crate::meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation, TagOp};
        use crate::ui::disc_extraction_modal::{DiscExtractionAction, DiscResolution};

        match self {
            DiscExtractionAction::None => {}

            DiscExtractionAction::Cancel => {
                app.cancel_and_return_to_source("Disc extraction resolution cancelled");
            }

            DiscExtractionAction::Confirm(resolution) => {
                let Some(g) = witness else { return };

                let group_idx = {
                    let ActiveView::DiscExtractionResolution(ref state) = app.view else {
                        return;
                    };
                    state.current_group
                };

                match resolution {
                    DiscResolution::Apply => {
                        let ops: Vec<TagOp> = {
                            let ActiveView::DiscExtractionResolution(ref state) = app.view else {
                                return;
                            };
                            let Some(group) = state.current_group_data() else {
                                return;
                            };
                            let mut ops = Vec::new();
                            for file in &group.files {
                                // Replace source tag value
                                ops.push(TagOp::replace_tag(
                                    file.inode,
                                    &file.source_tag,
                                    &file.original_value,
                                    &file.cleaned_value,
                                ));
                                // Add disc number tag
                                ops.push(TagOp::add_tag(
                                    file.inode,
                                    &state.disc_tag_name,
                                    &group.disc_value,
                                ));
                            }
                            ops
                        };
                        if !ops.is_empty() {
                            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                                ops,
                                zone: Zone::Corpus,
                            });
                            let _ = super::super::operator_decisions::stage_decision(
                                &mut app.witch,
                                DecisionKey::DiscExtraction {
                                    group_index: group_idx,
                                },
                                "Extract disc value",
                                vec![mutation],
                                g,
                            );
                        }
                    }
                    DiscResolution::Skip => {
                        // No mutations for skip — just advance
                    }
                }

                // Advance to next group, or show review if at end
                if let ActiveView::DiscExtractionResolution(ref mut state) = app.view {
                    state.file_cursor = 0;
                    state.file_scroll = 0;
                    if state.current_group + 1 < state.data.groups.len() {
                        state.current_group += 1;
                    } else {
                        app.after_staging_decisions();
                    }
                }
            }

            DiscExtractionAction::NavigateGroup(forward) => {
                if let ActiveView::DiscExtractionResolution(ref mut state) = app.view {
                    if forward {
                        if state.current_group + 1 < state.data.groups.len() {
                            state.current_group += 1;
                            state.file_cursor = 0;
                            state.file_scroll = 0;
                        }
                    } else if state.current_group > 0 {
                        state.current_group -= 1;
                        state.file_cursor = 0;
                        state.file_scroll = 0;
                    }
                }
            }

            DiscExtractionAction::ShowReview => {
                app.after_staging_decisions();
            }

            DiscExtractionAction::EditTracks | DiscExtractionAction::EditTracksAggregated => {
                let mode = match self {
                    DiscExtractionAction::EditTracks => tag_editor::TagEditorMode::Individual,
                    _ => tag_editor::TagEditorMode::Aggregated,
                };
                let (inodes, key, label) = {
                    let ActiveView::DiscExtractionResolution(ref state) = app.view else {
                        return;
                    };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let key = DecisionKey::TagEdit {
                        key_item: format!("disc_extraction_{}", state.current_group),
                    };
                    let label = format!("Manual tag edits: {}", group.description);
                    (state.current_group_inodes(), key, label)
                };
                app.open_tag_editor_for_inodes(inodes, crate::db::types::Zone::Corpus, key, label, mode);
            }
        }
    }
}

impl App {
    /// Start disc extraction resolution from Insights view.
    ///
    /// Loads DiscExtraction signals, resolves file paths, builds modal data,
    /// starts a transaction, and switches to the DiscExtractionResolution view.
    pub(super) fn start_disc_extraction_resolution(&mut self) {
        use crate::ui::disc_extraction_modal;

        let config = self.config().opinions.disc_extraction.clone();

        let (signals, path_map) = self
            .cache
            .query(|db| {
                let sigs = db.get_disc_extraction_signals().unwrap_or_default();
                // Collect all inodes for path lookup
                let all_inodes: Vec<i64> = sigs
                    .iter()
                    .flat_map(|s| s.data.inodes.iter().copied())
                    .collect();
                let paths = db
                    .get_file_paths_batch(crate::db::types::Zone::Corpus, &all_inodes)
                    .unwrap_or_default();
                (sigs, paths)
            })
            .recv();

        if signals.is_empty() {
            self.status_message = Some("No disc extraction signals found".to_string());
            return;
        }

        let data = disc_extraction_modal::DiscExtractionData::from_signals(
            signals,
            |inode| {
                path_map
                    .get(&inode)
                    .cloned()
                    .unwrap_or_else(|| format!("<inode {}>", inode))
            },
            config.map_letters_to_numbers,
        );

        // Start transaction for the resolution session
        let _ = self.witch.start_transaction("Disc extraction");

        let state = disc_extraction_modal::DiscExtractionState::new(data, config.disc_tag_name);
        self.view = ActiveView::DiscExtractionResolution(state);
    }
}
