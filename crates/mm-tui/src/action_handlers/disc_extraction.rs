//! Disc extraction resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::decisions::DecisionKey;
use crate::active_view::ActiveView;
use crate::tag_editor;

impl HandleAction for crate::disc_extraction_modal::DiscExtractionAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation, TagOp};
        use crate::disc_extraction_modal::{DiscExtractionAction, DiscResolution};

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
                            let decision = g.decide("Extract disc value", vec![mutation]);
                            let _ = super::super::operator_decisions::stage_decision(
                                app,
                                DecisionKey::DiscExtraction {
                                    group_index: group_idx,
                                },
                                decision,
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
                app.open_tag_editor_for_inodes(inodes, mm_meta::db_types::Zone::Corpus, key, label, mode);
            }
        }
    }
}

// =========================================================================
// V3: Single-load disc extraction with packed data
// =========================================================================

impl HandleAction for mm_ui::resolutions::disc_extraction::DiscExtractionAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation, TagOp};
        use mm_ui::resolutions::disc_extraction::DiscExtractionAction;

        match self {
            DiscExtractionAction::Apply => {
                let Some(g) = witness else { return };

                let (ops, group_idx) = match &app.view {
                    ActiveView::DiscExtractionResolutionV3 {
                        ref data, current_group, ref disc_tag_name, ..
                    } => {
                        let group = match data.groups.get(*current_group) {
                            Some(g) => g,
                            None => return,
                        };
                        let mut ops = Vec::new();
                        for file in &group.files {
                            ops.push(TagOp::replace_tag(
                                file.inode,
                                &file.source_tag,
                                &file.original_value,
                                &file.cleaned_value,
                            ));
                            ops.push(TagOp::add_tag(
                                file.inode,
                                disc_tag_name,
                                &group.disc_value,
                            ));
                        }
                        (ops, *current_group)
                    }
                    _ => return,
                };

                if !ops.is_empty() {
                    let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: Zone::Corpus,
                    });
                    let decision = g.decide("Extract disc value", vec![mutation]);
                    let _ = super::super::operator_decisions::stage_decision(
                        app,
                        DecisionKey::DiscExtraction { group_index: group_idx },
                        decision,
                    );
                }
                app.advance_disc_extraction_v3();
            }
            DiscExtractionAction::Skip => {
                // No mutations for skip — just advance
                app.advance_disc_extraction_v3();
            }
            DiscExtractionAction::Cancel => {
                app.cancel_and_return_to_source("Disc extraction resolution cancelled");
            }
        }
    }
}

impl App {
    /// Start disc extraction resolution from Insights view.
    ///
    /// Uses the GetDiscExtractionData domain query to load signals, resolve paths,
    /// and apply config in one step, then starts a transaction and opens the modal.
    pub(super) fn start_disc_extraction_resolution(&mut self) {
        use mm_meta::domain_queries::GetDiscExtractionData;
        use crate::disc_extraction_modal;

        let config = self.config().opinions.disc_extraction.clone();

        let data = self
            .query(GetDiscExtractionData {
                map_letters_to_numbers: config.map_letters_to_numbers,
            });

        if data.groups.is_empty() {
            self.status_message = Some("No disc extraction signals found".to_string());
            return;
        }

        let _ = self.start_transaction("Disc extraction");

        let state = disc_extraction_modal::DiscExtractionState::new(data, config.disc_tag_name);
        self.view = ActiveView::DiscExtractionResolution(state);
    }

    /// Start V3 disc extraction resolution.
    pub(super) fn start_disc_extraction_resolution_v3(&mut self) {
        use mm_meta::domain_queries::GetDiscExtractionData;

        let config = self.config().opinions.disc_extraction.clone();

        let data = self
            .query(GetDiscExtractionData {
                map_letters_to_numbers: config.map_letters_to_numbers,
            });

        if data.groups.is_empty() {
            self.status_message = Some("No disc extraction signals found".to_string());
            return;
        }

        let _ = self.start_transaction("Disc extraction");

        self.view = ActiveView::DiscExtractionResolutionV3 {
            data,
            current_group: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            focus: mm_ui::geometry::FocusPane::List,
            disc_tag_name: config.disc_tag_name,
        };
    }

    /// Advance to next group or go to review (V3 disc extraction).
    fn advance_disc_extraction_v3(&mut self) {
        if let ActiveView::DiscExtractionResolutionV3 {
            ref data, ref mut current_group, ref mut list, ..
        } = self.view
        {
            if *current_group + 1 < data.groups.len() {
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
