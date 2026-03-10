//! Tag Canonicity Resolution
//!
//! Handles the tag canonicity modal: loading signals, navigating between
//! clusters, staging canonicalization decisions.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::ui::{
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
    pub(in crate::ui) fn start_tag_canonicity_resolution(&mut self) {
        // Get the selected insight type to determine what to load
        let insight_type = match &self.view {
            ActiveView::Insights(v) => v.selected_insight_type(),
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
                    .cache
                    .domain_query(crate::db::domain::GetInconsistentAlbumArtistKeys)
                    .recv();
                (keys, CanonicitySignalKind::InconsistentAlbumArtist)
            }
            insights_view::InsightType::TagCanonicity { tag_name } => {
                let keys = self
                    .cache
                    .domain_query(crate::db::domain::GetTagCanonicityKeys {
                        tag_filter: Some(tag_name.clone()),
                    })
                    .recv();
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
        let _ = self.witch.start_transaction("Tag canonicalization");

        // Fire async load for the first signal — tick handler will complete it
        if !self.start_async_cluster_load(clusters) {
            self.status_message = Some("Failed to load signal data".to_string());
            let _ = super::super::operator_decisions::discard_transaction(&mut self.witch);
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
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            &label,
            mutations,
            gesture,
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
                use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
                use crate::meta::mutations::{Mutation, TagOp};

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

        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            "Flag non-compilation",
            mutations,
            gesture,
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
                use crate::meta::mutations::indexing::EmitCanonicalTagMutation;
                use crate::meta::mutations::Mutation;

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

        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index: cluster_idx,
            },
            "Flag canonical",
            mutations,
            gesture,
        );
    }

    /// Fire async load of the signal at the current cluster index.
    ///
    /// Extracts clusters from the current `TagCanonicityResolution` view, fires
    /// a non-blocking query on the cache thread, and transitions to
    /// `TagCanonicityLoading`. The tick handler will poll for completion.
    ///
    /// Returns true if the async load was started, false on error.
    pub(in crate::ui) fn load_current_cluster_signal(&mut self) -> bool {
        // Extract clusters from current view (take ownership via replace)
        let clusters = match std::mem::replace(
            &mut self.view,
            ActiveView::Insights(insights_view::InsightsViewState::new()),
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

    /// Fire async load with provided clusters (for restoring from SuspendedView).
    ///
    /// Sets the view to `TagCanonicityLoading` with the pending query.
    /// The tick handler will poll for completion and transition to Resolution.
    pub(in crate::ui) fn load_current_cluster_signal_with_clusters(
        &mut self,
        clusters: TagCanonicityClusters,
    ) -> bool {
        self.start_async_cluster_load(clusters)
    }

    /// Common helper: fire the cache query and transition to loading state.
    pub(in crate::ui) fn start_async_cluster_load(
        &mut self,
        clusters: TagCanonicityClusters,
    ) -> bool {
        let signal_key = match clusters.current_signal_key() {
            Some(key) => key.to_string(),
            None => return false,
        };

        let kind = clusters.kind;
        let pending = self
            .cache
            .query(move |db| Self::load_typed_signal_data(&signal_key, kind, db));

        self.view = ActiveView::TagCanonicityLoading { pending, clusters };
        true
    }

    /// Called by the tick loop when `TagCanonicityLoading` is active.
    /// Polls the pending query; on completion, constructs state and transitions
    /// to `TagCanonicityResolution`.
    pub(in crate::ui) fn tick_tag_canonicity_loading(&mut self) {
        // Take the loading view out to get ownership of the pending query
        let old = std::mem::replace(
            &mut self.view,
            ActiveView::Insights(insights_view::InsightsViewState::new()),
        );
        let ActiveView::TagCanonicityLoading { pending, clusters } = old else {
            // Shouldn't happen — put it back
            self.view = old;
            return;
        };

        match pending.try_recv() {
            Err(still_pending) => {
                // Not ready yet — put loading state back
                self.view = ActiveView::TagCanonicityLoading {
                    pending: still_pending,
                    clusters,
                };
            }
            Ok(None) => {
                // Signal not found
                self.status_message = Some("Signal not found".to_string());
                self.start_health_view();
            }
            Ok(Some(data)) => {
                // Build the resolved state
                let kind = clusters.kind;
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
                if let Some(decision) = self.witch.get_decision(&DecisionKey::TagCanonicity {
                    tag_name: tag_name_for_key,
                    cluster_index: current_index,
                }) {
                    state.restore_from_mutations(&decision.mutations);
                    state.pending_tag_edits =
                        Some(helpers::pending_edits_from_mutations(&decision.mutations));
                }

                self.view = ActiveView::TagCanonicityResolution { state, clusters };
            }
        }
    }

    /// Load typed signal data by key and kind, returning modal data.
    fn load_typed_signal_data(
        key: &str,
        kind: CanonicitySignalKind,
        read_db: &crate::db::ReadOnlyDb,
    ) -> Option<tag_canonicity_v2::TagCanonicalityModalDataV2> {
        match kind {
            CanonicitySignalKind::TagCanonicity => {
                let signal = read_db.get_tag_canonicity_signal(key).ok()??;
                tag_canonicity_v2::TagCanonicalityModalDataV2::from_tag_canonicity(&signal, read_db)
            }
            CanonicitySignalKind::InconsistentAlbumArtist => {
                let signal = read_db.get_inconsistent_album_artist_signal(key).ok()??;
                tag_canonicity_v2::TagCanonicalityModalDataV2::from_inconsistent_album_artist(
                    &signal, read_db,
                )
            }
            CanonicitySignalKind::InboxTagCanonicity => {
                let signal = read_db.get_inbox_tag_canonicity_signal(key).ok()??;
                tag_canonicity_v2::TagCanonicalityModalDataV2::from_inbox_tag_canonicity(
                    &signal, read_db,
                )
            }
        }
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
