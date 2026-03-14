//! Tag Canonicity Resolution
//!
//! Handles the tag canonicity modal: loading signals, navigating between
//! clusters, staging canonicalization decisions.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_meta::decisions::DecisionKey;
use mm_ui::resolutions::tag_canonicity::CanonicityAction;
use crate::{
    helpers, insights_view, tag_canonicity_v2, tag_editor, ActiveView, CanonicitySignalKind,
    TagCanonicityClusters,
};

impl App {
    /// Start tag canonicity resolution from Insights view.
    ///
    /// Uses the selected insight type to determine which signals to load:
    /// - InconsistentAlbumArtist: loads all inconsistent_album_artist signals
    /// - TagCanonicity { tag_name }: loads tag_canonicity signals filtered by tag_name
    ///
    /// Uses the V2 three-pane layout for tag canonicity resolution.
    pub(crate) fn start_tag_canonicity_resolution(&mut self) {
        // Get the selected insight type to determine what to load
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

        // Determine signal kind and load keys via cache thread
        let (signal_keys, kind) = match &insight_type {
            insights_view::InsightType::InconsistentAlbumArtist => {
                let keys = self
                    .query(mm_meta::domain_queries::GetInconsistentAlbumArtistKeys);
                (keys, CanonicitySignalKind::InconsistentAlbumArtist)
            }
            insights_view::InsightType::TagCanonicity { tag_name } => {
                let keys = self
                    .query(mm_meta::domain_queries::GetTagCanonicityKeys {
                        zone: Zone::Corpus,
                        tag_filter: Some(tag_name.clone()),
                    });
                (keys, CanonicitySignalKind::TagCanonicity)
            }
            _ => {
                self.status_message = Some("Invalid insight type for tag resolution".to_string());
                return;
            }
        };

        if signal_keys.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        let clusters = TagCanonicityClusters::new(signal_keys, kind);

        // Start transaction ONCE for entire modal
        let _ = self.start_transaction("Tag canonicalization");

        // Fire async load for the first signal — tick handler will complete it
        if !self.start_async_cluster_load(clusters) {
            self.status_message = Some("Failed to load signal data".to_string());
            let _ = super::super::operator_decisions::discard_transaction(self);
        }
    }

}

impl HandleAction for tag_canonicity_v2::TagCanonicalityActionV2 {
    /// Handle tag canonicity modal actions (three-pane layout).
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            tag_canonicity_v2::TagCanonicalityActionV2::None => {}
            tag_canonicity_v2::TagCanonicalityActionV2::Confirmed => {
                let Some(w) = witness else { return };
                // Stage decision and advance to next cluster
                app.stage_canonicity_decision(w);
                app.advance_to_next_cluster();
            }
            tag_canonicity_v2::TagCanonicalityActionV2::Cancelled => {
                app.cancel_and_return_to_source("Tag canonicity resolution cancelled");
            }
            tag_canonicity_v2::TagCanonicalityActionV2::Navigate { forward } => {
                // User navigated to next/prev cluster - do NOT stage decision
                app.navigate_cluster(forward);
            }
            tag_canonicity_v2::TagCanonicalityActionV2::ShowReview => {
                // Ctrl+R - show review with whatever has already been staged
                app.after_staging_decisions();
            }
            tag_canonicity_v2::TagCanonicalityActionV2::OpenTagEditorIndividual => {
                app.launch_tag_editor_from_canonicity(tag_editor::TagEditorMode::Individual);
            }
            tag_canonicity_v2::TagCanonicalityActionV2::OpenTagEditorAggregated => {
                app.launch_tag_editor_from_canonicity(tag_editor::TagEditorMode::Aggregated);
            }
            tag_canonicity_v2::TagCanonicalityActionV2::FlagNonCompilation => {
                let Some(w) = witness else { return };
                app.stage_flag_non_compilation(w);
                app.advance_to_next_cluster();
            }
            tag_canonicity_v2::TagCanonicalityActionV2::FlagCanonical => {
                let Some(w) = witness else { return };
                app.stage_flag_canonical(w);
                app.advance_to_next_cluster();
            }
        }
    }
}

impl App {
    /// Launch embedded tag editor from the tag canonicity modal.
    fn launch_tag_editor_from_canonicity(&mut self, mode: tag_editor::TagEditorMode) {
        let (inodes, decision_key, decision_label, file_cursor_inode, zone) =
            if let ActiveView::TagCanonicityResolution {
                ref state,
                ref clusters,
            } = self.view
            {
                let inodes: Vec<i64> = state.data.inodes.clone();
                let key = DecisionKey::TagCanonicity {
                    tag_name: state.data.tag_name.clone(),
                    cluster_index: clusters.current_index,
                };
                let label = format!("Tag edit: {} canonicity", state.data.tag_name);
                let cursor_inode = state.data.files.get(state.file_cursor).map(|f| f.inode);
                (inodes, key, label, cursor_inode, state.zone)
            } else {
                return;
            };

        self.open_tag_editor_for_inodes(inodes, zone, decision_key, decision_label, mode);
        self.position_editor_cursor(file_cursor_inode);
    }

    /// Navigate to next/prev cluster without staging a decision.
    fn navigate_cluster(&mut self, forward: bool) {
        let is_first = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.current_index == 0);
        let is_last = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::TagCanonicityResolution { .. }) {
            self.start_health_view();
            return;
        }

        if !forward && is_first {
            // Shift-Tab from first group = do nothing
            return;
        }

        if forward && is_last {
            // Tab from last group = show review screen
            self.after_staging_decisions();
            return;
        }

        // Normal navigation
        let moved = if let ActiveView::TagCanonicityResolution {
            ref mut clusters, ..
        } = self.view
        {
            if forward {
                clusters.next()
            } else {
                clusters.prev()
            }
        } else {
            false
        };

        if moved && !self.load_current_cluster_signal() {
            // Signal load failed - return to insights
            self.start_health_view();
        }
    }

    /// Advance to next cluster after confirming current one.
    fn advance_to_next_cluster(&mut self) {
        let is_last = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::TagCanonicityResolution { .. }) {
            self.after_staging_decisions();
            return;
        }

        if is_last {
            // At last cluster - show review screen
            self.after_staging_decisions();
        } else {
            let advanced = if let ActiveView::TagCanonicityResolution {
                ref mut clusters, ..
            } = self.view
            {
                clusters.next()
            } else {
                false
            };

            if advanced {
                // Load next signal
                if !self.load_current_cluster_signal() {
                    // Signal load failed - show review with what we have
                    self.after_staging_decisions();
                }
            } else {
                // No more clusters - show review
                self.after_staging_decisions();
            }
        }
    }

    /// Stage a decision for the current canonicity cluster (V2).
    ///
    /// This adds the decision to the transaction but does NOT confirm it.
    /// The transaction is confirmed when the user completes the review screen.
    fn stage_canonicity_decision(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref state,
                ref clusters,
            } => {
                let mutations = state.mutations();
                if mutations.is_empty() {
                    return;
                }
                (
                    mutations,
                    clusters.current_index,
                    state.data.tag_name.clone(),
                )
            }
            _ => return,
        };

        let label = format!("Canonicalize {}", tag_name);

        // Add decision to existing transaction via sealed operator decision handler
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

    /// Stage a "flag as non-compilation" decision for the current cluster.
    ///
    /// Adds COMPILATION=0 to all tracks in the current group, which will
    /// suppress this group in future inconsistent album artist detection runs.
    fn stage_flag_non_compilation(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref state,
                ref clusters,
            } => {
                use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
                use mm_meta::mutations::{Mutation, TagOp};

                let ops: Vec<TagOp> = state
                    .data
                    .inodes
                    .iter()
                    .map(|&inode| TagOp::add_tag(inode, "COMPILATION", "0"))
                    .collect();

                if ops.is_empty() {
                    return;
                }

                let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                    ops,
                    zone: state.zone,
                })];
                (
                    mutations,
                    clusters.current_index,
                    state.data.tag_name.clone(),
                )
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

    /// Stage a "flag as canonical" decision for the current collision group.
    ///
    /// Emits an EmitCanonicalTag mutation for each variant in the current group,
    /// which will suppress this collision in future DetectTagCanonicalizations runs.
    fn stage_flag_canonical(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution {
                ref state,
                ref clusters,
            } => {
                use mm_meta::mutations::indexing::EmitCanonicalTagMutation;
                use mm_meta::mutations::Mutation;

                let mutations: Vec<Mutation> = state
                    .data
                    .variants
                    .iter()
                    .map(|variant| {
                        Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                            tag_name: state.data.tag_name.clone(),
                            canonical_value: variant.value.clone(),
                        })
                    })
                    .collect();

                if mutations.is_empty() {
                    return;
                }

                (
                    mutations,
                    clusters.current_index,
                    state.data.tag_name.clone(),
                )
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

    /// Load the signal at the current cluster index.
    ///
    /// Extracts clusters from the current `TagCanonicityResolution` view,
    /// queries the DB, and transitions directly to the new resolution state.
    ///
    /// Returns true if the load succeeded, false on error.
    pub(crate) fn load_current_cluster_signal(&mut self) -> bool {
        // Extract clusters from current view (take ownership via replace)
        let clusters = match std::mem::replace(
            &mut self.view,
            ActiveView::Insights {
                data: insights_view::InsightsViewData::new(),
                interaction: insights_view::HealthInteraction::new(),
            },
        ) {
            ActiveView::TagCanonicityResolution { clusters, .. } => clusters,
            other => {
                // Put it back if not the right variant
                self.view = other;
                return false;
            }
        };

        self.start_async_cluster_load(clusters)
    }

    /// Load with provided clusters (for restoring from SuspendedView).
    pub(crate) fn load_current_cluster_signal_with_clusters(
        &mut self,
        clusters: TagCanonicityClusters,
    ) -> bool {
        self.start_async_cluster_load(clusters)
    }

    /// Load cluster data and transition directly to resolution state.
    pub(crate) fn start_async_cluster_load(
        &mut self,
        clusters: TagCanonicityClusters,
    ) -> bool {
        let signal_key = match clusters.current_signal_key() {
            Some(key) => key.to_string(),
            None => return false,
        };

        let kind = clusters.kind;
        let result = self
            .query(mm_meta::domain_queries::GetTagCanonicitySignalData {
                signal_key,
                kind,
            });

        match result {
            None => {
                self.status_message = Some("Signal not found".to_string());
                self.start_health_view();
            }
            Some(data) => {
                let current_index = clusters.current_index;
                let total = clusters.signal_keys.len();
                let pre_fill = clusters.pre_fill();
                let is_album_artist = kind == CanonicitySignalKind::InconsistentAlbumArtist;
                let zone = Self::zone_for_kind(kind);
                let tag_name_for_key = data.tag_name.clone();
                let mut state = tag_canonicity_v2::TagCanonicalityStateV2::new(
                    data,
                    pre_fill,
                    current_index,
                    total,
                    is_album_artist,
                    zone,
                );

                // Back-fill UI state from staged decision if one exists for this cluster
                let backfill_key = DecisionKey::TagCanonicity {
                    tag_name: tag_name_for_key,
                    cluster_index: current_index,
                };
                if let Some(detail) = self.transaction_decision_details().unwrap_or_default().into_iter().find(|d| d.key == backfill_key) {
                    state.restore_from_mutations(&detail.mutations);
                    state.pending_tag_edits =
                        Some(helpers::pending_edits_from_mutations(&detail.mutations));
                }

                self.view = ActiveView::TagCanonicityResolution { state, clusters };
            }
        }
        true
    }

    /// Map signal kind to zone for mutations and file queries.
    fn zone_for_kind(kind: CanonicitySignalKind) -> Zone {
        match kind {
            CanonicitySignalKind::TagCanonicity | CanonicitySignalKind::InconsistentAlbumArtist => {
                Zone::Corpus
            }
            CanonicitySignalKind::InboxTagCanonicity => Zone::Inbox,
        }
    }
}

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
                app.stage_flag_canonical_v3(w);
                app.advance_canonicity_v3();
            }
            CanonicityAction::Cancel => {
                app.cancel_and_return_to_source("Tag canonicity resolution cancelled");
            }
        }
    }
}

impl App {
    /// Start tag canonicity resolution using the new packed query (V3).
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

        // Determine tag_name and zone from insight type
        let (tag_name, zone) = match &insight_type {
            insights_view::InsightType::InconsistentAlbumArtist => {
                ("ALBUMARTIST".to_string(), Zone::Corpus)
            }
            insights_view::InsightType::TagCanonicity { tag_name } => {
                (tag_name.clone(), Zone::Corpus)
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
        });

        if data.clusters.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        // Start transaction
        let _ = self.start_transaction("Tag canonicalization");

        // Pre-fill DecisionField with first cluster's canonical candidate
        let prefill = data.clusters.first()
            .map(|c| c.canonical_candidate.as_str())
            .unwrap_or("");
        let field = mm_ui::decision_field::DecisionField::new("Squash to:")
            .with_value(prefill);

        self.view = ActiveView::TagCanonicityResolutionV3 {
            data,
            current_cluster: 0,
            list: mm_ui::standard_list::StandardListState::new(
                mm_ui::standard_list::StandardListConfig::default(),
            ),
            buttons: mm_ui::modal_buttons::ButtonRowState::new(),
            field,
            zone,
            focus: mm_ui::geometry::FocusPane::List,
        };
    }

    /// Advance to next cluster or go to review (V3).
    fn advance_canonicity_v3(&mut self) {
        if let ActiveView::TagCanonicityResolutionV3 {
            ref data, ref mut current_cluster, ref mut list, ref mut field, ..
        } = self.view
        {
            if *current_cluster + 1 < data.clusters.len() {
                *current_cluster += 1;
                list.reset();
                // Pre-fill field with new cluster's canonical candidate
                if let Some(cluster) = data.clusters.get(*current_cluster) {
                    field.set_value(&cluster.canonical_candidate);
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
            ActiveView::TagCanonicityResolutionV3 {
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
                for variant in &cluster.outlier_variants {
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
            ActiveView::TagCanonicityResolutionV3 {
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
                    .outlier_variants
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
                    canonical_value: cluster.canonical_candidate.clone(),
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
}
