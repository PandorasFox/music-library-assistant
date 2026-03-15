//! Tag Canonicity Resolution
//!
//! Handles the V3 tag canonicity modal: single-load packed data with
//! StandardList + DecisionField + ButtonRow.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_meta::decisions::DecisionKey;
use mm_ui::resolutions::tag_canonicity::CanonicityAction;
use crate::{
    insights_view, ActiveView,
};

// =========================================================================
// V3: Single-load canonicity with packed data
// =========================================================================

impl HandleAction for CanonicityAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            CanonicityAction::Confirm => {
                let Some(w) = witness else { return };
                app.stage_canonicity_decision_v3(w);
                app.advance_canonicity_v3();
            }
            CanonicityAction::FlagCanonical => {
                let Some(w) = witness else { return };
                // In album artist mode, FlagCanonical means "flag as non-compilation"
                let mode = match &app.view {
                    ActiveView::TagCanonicityResolution { mode, .. } => *mode,
                    _ => return,
                };
                match mode {
                    mm_ui::resolutions::tag_canonicity::CanonicityMode::InconsistentAlbumArtist => {
                        app.stage_flag_non_compilation_v3(w);
                    }
                    mm_ui::resolutions::tag_canonicity::CanonicityMode::TagCanonicity => {
                        app.stage_flag_canonical_v3(w);
                    }
                }
                app.advance_canonicity_v3();
            }
            CanonicityAction::Cancel => {
                app.cancel_and_return_to_source("Tag canonicity resolution cancelled");
            }
        }
    }
}

impl App {
    /// Start tag canonicity resolution using the packed query (V3).
    pub(crate) fn start_tag_canonicity_resolution_v3(&mut self) {
        let insight_type = match &self.view {
            ActiveView::Insights { ref data, ref interaction } => data.insight_type_at(interaction.list.cursor),
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
        use mm_ui::resolutions::tag_canonicity::CanonicityMode;
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
            tag_name: tag_name.clone(),
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
        let prefill = data.clusters.first()
            .map(|c| c.suggested_canonical.as_deref().unwrap_or(""))
            .unwrap_or("");
        let field_label = match mode {
            CanonicityMode::InconsistentAlbumArtist => "Album artist:",
            CanonicityMode::TagCanonicity => "Squash to:",
        };
        let field = mm_ui::decision_field::DecisionField::new(field_label)
            .with_value(prefill);

        self.view = ActiveView::TagCanonicityResolution {
            data,
            current_cluster: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            field,
            zone,
            focus: mm_ui::geometry::FocusPane::List,
            mode,
        };
    }

    /// Advance to next cluster or go to review (V3).
    fn advance_canonicity_v3(&mut self) {
        if let ActiveView::TagCanonicityResolution {
            ref data, ref mut current_cluster, ref mut list, ref mut field, ..
        } = self.view
        {
            if *current_cluster + 1 < data.clusters.len() {
                *current_cluster += 1;
                list.reset();
                // Pre-fill field with new cluster's canonical candidate
                if let Some(cluster) = data.clusters.get(*current_cluster) {
                    field.set_value(&cluster.suggested_canonical.as_deref().unwrap_or(""));
                }
            } else {
                // Last cluster — go to review
                self.after_staging_decisions();
            }
        } else {
            self.after_staging_decisions();
        }
    }

    /// Stage canonicity squash decision for current cluster (V3).
    fn stage_canonicity_decision_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref data, current_cluster, ref field, ref zone, ..
            } => {
                use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
                use mm_meta::mutations::{Mutation, TagOp};

                let canonical_value = field.value().trim().to_string();
                if canonical_value.is_empty() {
                    return;
                }

                let cluster = match data.clusters.get(*current_cluster) {
                    Some(c) => c,
                    None => return,
                };

                // Build tag ops: for each outlier file, replace its tag value with canonical
                let mut ops = Vec::new();
                for variant in &cluster.variants {
                    for file in &variant.files {
                        ops.push(TagOp::replace_tag(
                            file.inode,
                            &data.tag_name,
                            &variant.value,
                            &canonical_value,
                        ));
                    }
                }

                if ops.is_empty() {
                    return;
                }

                let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                    ops,
                    zone: *zone,
                })];

                (mutations, *current_cluster, data.tag_name.clone())
            }
            _ => return,
        };

        let label = format!("Canonicalize {}", tag_name);
        let decision = gesture.decide(&label, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            decision,
        );
    }

    /// Stage flag-canonical decision for current cluster (V3).
    fn stage_flag_canonical_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref data, current_cluster, ..
            } => {
                use mm_meta::mutations::indexing::EmitCanonicalTagMutation;
                use mm_meta::mutations::Mutation;

                let cluster = match data.clusters.get(*current_cluster) {
                    Some(c) => c,
                    None => return,
                };

                // Emit canonical tag for each outlier variant + the canonical candidate
                let mut mutations: Vec<Mutation> = cluster
                    .variants
                    .iter()
                    .map(|v| {
                        Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                            tag_name: data.tag_name.clone(),
                            canonical_value: v.value.clone(),
                        })
                    })
                    .collect();

                // Also flag the canonical candidate itself
                mutations.push(Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                    tag_name: data.tag_name.clone(),
                    canonical_value: cluster.suggested_canonical.clone().unwrap_or_default(),
                }));

                if mutations.is_empty() {
                    return;
                }

                (mutations, *current_cluster, data.tag_name.clone())
            }
            _ => return,
        };

        let decision = gesture.decide("Flag canonical", mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            decision,
        );
    }

    /// Stage a "flag as non-compilation" decision for the current cluster (V3).
    ///
    /// Adds COMPILATION=0 to all tracks in the current group, which will
    /// suppress this group in future inconsistent album artist detection runs.
    fn stage_flag_non_compilation_v3(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref data, current_cluster, ref zone, ..
            } => {
                use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
                use mm_meta::mutations::{Mutation, TagOp};

                let cluster = match data.clusters.get(*current_cluster) {
                    Some(c) => c,
                    None => return,
                };

                // Add COMPILATION=0 to every file in this cluster
                let ops: Vec<TagOp> = cluster
                    .variants
                    .iter()
                    .flat_map(|v| &v.files)
                    .map(|f| TagOp::add_tag(f.inode, "COMPILATION", "0"))
                    .collect();

                if ops.is_empty() {
                    return;
                }

                let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                    ops,
                    zone: *zone,
                })];
                (mutations, *current_cluster, data.tag_name.clone())
            }
            _ => return,
        };

        let decision = gesture.decide("Flag non-compilation", mutations);
        let _ = super::super::operator_decisions::stage_decision(
            self,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            decision,
        );
    }
}
