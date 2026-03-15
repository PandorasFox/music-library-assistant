//! Disc extraction resolution action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_ui::modal_buttons::ModalButtons;
use crate::active_view::ActiveView;

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
                    ActiveView::DiscExtractionResolution {
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
                    use mm_ui::resolutions::disc_extraction::{DiscExtractionButton, DiscExtractionButtonCtx};
                    let ctx = DiscExtractionButtonCtx {
                        has_files: true,
                        group_index: group_idx,
                    };
                    let key = DiscExtractionButton::Apply.protocol_binding(&ctx)
                        .decision_key().unwrap().clone();
                    let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: Zone::Corpus,
                    });
                    let decision = g.decide("Extract disc value", vec![mutation]);
                    let _ = super::super::operator_decisions::stage_decision(app, key, decision);
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

        self.view = ActiveView::DiscExtractionResolution {
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
        if let ActiveView::DiscExtractionResolution {
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
