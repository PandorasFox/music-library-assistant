//! Compound Tag Split Resolution
//!
//! Handles the compound tag split modal: loading signals, navigating between
//! split candidates, staging split/canonicalize decisions, and bulk operations.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::ui::suspended_views::SuspendTarget;
use crate::ui::{compound_split_v2, helpers, progressive_worker, tag_editor, ActiveView};

impl HandleAction for compound_split_v2::CompoundSplitActionV2 {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            compound_split_v2::CompoundSplitActionV2::None => {}
            compound_split_v2::CompoundSplitActionV2::Confirmed => {
                let Some(w) = witness else { return };
                // Stage decision and advance to next signal
                app.stage_compound_split_decision(w);
                app.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Canonicalize => {
                let Some(w) = witness else { return };
                // Mark as canonical (don't split) and advance
                app.stage_compound_canonicalize_decision(w);
                app.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Cancelled => {
                // Discard transaction if active
                app.cancel_and_return_to_source("Compound tag split cancelled");
            }
            compound_split_v2::CompoundSplitActionV2::Navigate { forward } => {
                // Navigate to next/prev signal without staging
                app.navigate_compound_split(forward);
            }
            compound_split_v2::CompoundSplitActionV2::ShowReview => {
                // Ctrl+R - show review with whatever has already been staged
                app.after_staging_decisions();
            }
            compound_split_v2::CompoundSplitActionV2::StageAllAndReview => {
                let Some(g) = witness else { return };
                // Ctrl+A - stage ALL splits progressively with progress bar
                app.start_progressive_compound_split_staging(*g);
            }
            compound_split_v2::CompoundSplitActionV2::OpenTagEditorIndividual => {
                app.launch_tag_editor_from_compound_split(tag_editor::TagEditorMode::Individual);
            }
            compound_split_v2::CompoundSplitActionV2::OpenTagEditorAggregated => {
                app.launch_tag_editor_from_compound_split(tag_editor::TagEditorMode::Aggregated);
            }
        }
    }
}

impl App {
    /// Start compound tag split resolution modal.
    ///
    /// Loads CompoundTagValue signals filtered by safety and enters the split modal.
    /// If `safe_only` is true, loads only signals where all split parts exist in corpus.
    /// If `tag_filter` is Some, only loads signals for that specific tag name.
    pub(in crate::ui) fn start_compound_split_resolution(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
    ) {
        self.start_compound_split_resolution_for_zone(safe_only, tag_filter, Zone::Corpus);
    }

    /// Start compound tag split resolution modal for a given zone.
    pub(in crate::ui) fn start_compound_split_resolution_for_zone(
        &mut self,
        safe_only: bool,
        tag_filter: Option<&str>,
        zone: Zone,
    ) {
        // Load compound signal groups filtered by safety classification and tag
        let groups = if zone == Zone::Inbox {
            self.witch
                .query(crate::db::domain::GetInboxCompoundSignalGroups)
        } else {
            let tag_filter_owned = tag_filter.map(|s| s.to_string());
            self.witch
                .query(crate::db::domain::GetCompoundSignalGroups {
                    safe_only,
                    tag_filter: tag_filter_owned,
                })
        };

        if groups.is_empty() {
            self.status_message = Some("No compound tag signals to resolve".to_string());
            return;
        }

        // Store groups for cluster navigation
        let clusters = compound_split_v2::CompoundSplitClustersV2::new(groups);

        // Start transaction ONCE for entire modal
        let mode_str = if zone == Zone::Inbox {
            "inbox"
        } else if safe_only {
            "safe"
        } else {
            "review"
        };
        let _ = self
            .witch
            .start_transaction(&format!("Compound tag split ({})", mode_str));

        // Load the first group into modal data
        let data = self
            .witch
            .query(crate::db::domain::GetCompoundSplitGroupData {
                group: clusters.all_groups()[0].clone(),
                zone,
            });

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                // Discard the transaction we just started
                let _ = super::super::operator_decisions::discard_transaction(&mut self.witch);
                return;
            }
        };

        // Get group info from clusters
        let (group_index, total_groups) = (clusters.current_index(), clusters.total());

        let state = compound_split_v2::CompoundSplitStateV2::new(
            data,
            safe_only,
            group_index,
            total_groups,
            zone,
        );
        self.view = ActiveView::CompoundTagSplit {
            state,
            clusters,
            safe_mode: safe_only,
            zone,
        };
    }

    /// Launch embedded tag editor from the compound split modal.
    fn launch_tag_editor_from_compound_split(&mut self, mode: tag_editor::TagEditorMode) {
        let (inodes, decision_key, decision_label, file_cursor_inode, zone) =
            if let ActiveView::CompoundTagSplit {
                ref state,
                ref clusters,
                safe_mode,
                zone,
                ..
            } = self.view
            {
                let inodes: Vec<i64> = state.data.files.iter().map(|f| f.inode).collect();
                let tag_name = state.data.compound.tag_name.clone();
                let cluster_index = clusters.current_index();
                let key = compound_split_key(zone, safe_mode, tag_name, cluster_index);
                let label = format!(
                    "Tag edit: {} \"{}\"",
                    state.data.compound.tag_name, state.data.compound.compound_value,
                );
                let cursor_inode = state.data.files.get(state.file_cursor).map(|f| f.inode);
                (inodes, key, label, cursor_inode, zone)
            } else {
                return;
            };

        self.open_tag_editor_for_inodes(inodes, zone, decision_key, decision_label, mode);
        self.position_editor_cursor(file_cursor_inode);
    }

    /// Navigate to next/prev compound split signal without staging.
    fn navigate_compound_split(&mut self, forward: bool) {
        let is_first = matches!(&self.view, ActiveView::CompoundTagSplit { clusters, .. } if clusters.is_first());
        let is_last = matches!(&self.view, ActiveView::CompoundTagSplit { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::CompoundTagSplit { .. }) {
            self.start_health_view();
            return;
        }

        if !forward && is_first {
            // Shift-Tab from first = do nothing
            return;
        }

        if forward && is_last {
            // Tab from last = show review screen
            self.after_staging_decisions();
            return;
        }

        // Normal navigation
        let moved = if let ActiveView::CompoundTagSplit {
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

        if moved && !self.load_current_compound_split_signal() {
            // Signal load failed - return to insights
            self.start_health_view();
        }
    }

    /// Advance to next compound split signal after confirming current (via Enter/Ctrl+Q).
    ///
    /// After a canonicalize decision, automatically skips over subsequent signals
    /// whose compound value matches any staged canonical value (since the single
    /// EmitCanonicalTag mutation covers all inodes with that value globally).
    fn advance_to_next_compound_split(&mut self) {
        if !matches!(&self.view, ActiveView::CompoundTagSplit { .. }) {
            self.after_staging_decisions();
            return;
        }

        // Collect staged canonical values for auto-skip
        let staged_canonicals = self.staged_canonical_values();

        loop {
            let is_last = matches!(&self.view, ActiveView::CompoundTagSplit { clusters, .. } if clusters.is_last());

            if is_last {
                self.after_staging_decisions();
                return;
            }

            let advanced = if let ActiveView::CompoundTagSplit {
                ref mut clusters, ..
            } = self.view
            {
                clusters.next()
            } else {
                false
            };

            if !advanced {
                self.after_staging_decisions();
                return;
            }

            if !self.load_current_compound_split_signal() {
                self.after_staging_decisions();
                return;
            }

            // Check if this signal's compound value is already covered by a staged canonical
            let should_skip = if let ActiveView::CompoundTagSplit { ref state, .. } = self.view {
                staged_canonicals.iter().any(|(tn, cv)| {
                    tn == &state.data.compound.tag_name && cv == &state.data.compound.compound_value
                })
            } else {
                false
            };

            if !should_skip {
                return; // Found a signal that needs operator attention
            }
            // Otherwise loop to skip past this one
        }
    }

    /// Collect (tag_name, canonical_value) pairs from staged EmitCanonicalTag decisions.
    fn staged_canonical_values(&self) -> Vec<(String, String)> {
        let details = self.witch.transaction_decision_details().unwrap_or_default();
        details
            .iter()
            .filter_map(|detail| {
                detail.mutations.iter().find_map(|m| {
                    if let crate::meta::mutations::Mutation::EmitCanonicalTag(ref ct) = m {
                        Some((ct.tag_name.clone(), ct.canonical_value.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    /// Stage the current compound split decision (v2).
    fn stage_compound_split_decision(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, key, description) = match &self.view {
            ActiveView::CompoundTagSplit {
                ref state,
                ref clusters,
                safe_mode,
                zone,
                ..
            } => {
                let mutations = state.mutations();
                if mutations.is_empty() {
                    return;
                }
                let desc = format!(
                    "Split \"{}\" in {} \u{2192} [{}]",
                    state.data.compound.compound_value,
                    state.data.compound.tag_name,
                    state.edited_parts.join(", ")
                );
                let tag_name = state.data.compound.tag_name.clone();
                let key = compound_split_key(*zone, *safe_mode, tag_name, clusters.current_index());
                (mutations, key, desc)
            }
            _ => return,
        };

        let decision = gesture.decide(&description, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            decision,
        );
    }

    /// Stage a canonicalize decision (mark compound value as canonical, don't split).
    fn stage_compound_canonicalize_decision(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, key, description) = match &self.view {
            ActiveView::CompoundTagSplit {
                ref state,
                ref clusters,
                safe_mode,
                zone,
                ..
            } => {
                let mutation = crate::ui::compound_split_v2::types::create_canonical_signal(&state.data);
                let desc = format!(
                    "Keep \"{}\" in {} as canonical",
                    state.data.compound.compound_value, state.data.compound.tag_name,
                );
                let tag_name = state.data.compound.tag_name.clone();
                let key = compound_split_key(*zone, *safe_mode, tag_name, clusters.current_index());
                (vec![mutation], key, desc)
            }
            _ => return,
        };

        let decision = gesture.decide(&description, mutations);
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            decision,
        );
    }

    /// Start progressive worker to stage ALL compound splits.
    ///
    /// Called when user presses Ctrl+A in the compound split modal.
    /// Uses the progressive worker to process items in timed chunks with progress bar.
    fn start_progressive_compound_split_staging(&mut self, gesture: witness::ConfirmationGesture) {
        // Extract groups and safe_mode from current view
        let (groups, is_safe_mode) = match &self.view {
            ActiveView::CompoundTagSplit {
                clusters,
                safe_mode,
                ..
            } => {
                let groups = clusters.all_groups().to_vec();
                if groups.is_empty() {
                    self.status_message = Some("No compound splits to stage".to_string());
                    return;
                }
                (groups, *safe_mode)
            }
            _ => {
                self.status_message = Some("No compound splits to stage".to_string());
                return;
            }
        };

        // Push current view and switch to progressive worker
        let worker = progressive_worker::ProgressiveWorkerState::for_compound_splits(
            groups,
            is_safe_mode,
            gesture,
        );
        self.push_and_switch(SuspendTarget::ProgressiveWork(worker));
    }

    /// Load the compound split group at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_compound_split_signal(&mut self) -> bool {
        // Extract cluster info from current view
        let (group, group_index, total, safe_mode, zone) = match &self.view {
            ActiveView::CompoundTagSplit {
                clusters,
                safe_mode,
                zone,
                ..
            } => match clusters.current_group() {
                Some(g) => (
                    g.clone(),
                    clusters.current_index(),
                    clusters.total(),
                    *safe_mode,
                    *zone,
                ),
                None => return false,
            },
            _ => return false,
        };

        let data = self
            .witch
            .query(crate::db::domain::GetCompoundSplitGroupData {
                group,
                zone,
            });

        let Some(data) = data else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        // Create modal state
        let mut state =
            compound_split_v2::CompoundSplitStateV2::new(data, safe_mode, group_index, total, zone);

        // Back-fill UI state from staged decision if one exists for this cluster
        let backfill_key = compound_split_key(
            zone,
            safe_mode,
            state.data.compound.tag_name.clone(),
            group_index,
        );
        if let Some(detail) = self.witch.transaction_decision_details().unwrap_or_default().into_iter().find(|d| d.key == backfill_key) {
            state.restore_from_mutations(&detail.mutations);
            state.pending_tag_edits =
                Some(helpers::pending_edits_from_mutations(&detail.mutations));
        }

        // Update state in existing view
        if let ActiveView::CompoundTagSplit {
            state: ref mut s, ..
        } = self.view
        {
            *s = state;
        }
        true
    }

    /// Load compound split group using provided clusters (for restoring from SuspendedView).
    ///
    /// Sets the view to CompoundTagSplit with the provided clusters,
    /// loading the current group's state from the database.
    pub(in crate::ui) fn load_current_compound_split_signal_with_clusters(
        &mut self,
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
        zone: Zone,
    ) -> bool {
        let group = match clusters.current_group() {
            Some(g) => g.clone(),
            None => return false,
        };

        let (group_index, total) = (clusters.current_index(), clusters.total());

        let data = self
            .witch
            .query(crate::db::domain::GetCompoundSplitGroupData {
                group,
                zone,
            });

        let Some(data) = data else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        // Create modal state
        let mut state =
            compound_split_v2::CompoundSplitStateV2::new(data, safe_mode, group_index, total, zone);

        // Back-fill UI state from staged decision if one exists for this cluster
        let backfill_key = compound_split_key(
            zone,
            safe_mode,
            state.data.compound.tag_name.clone(),
            group_index,
        );
        if let Some(detail) = self.witch.transaction_decision_details().unwrap_or_default().into_iter().find(|d| d.key == backfill_key) {
            state.restore_from_mutations(&detail.mutations);
            state.pending_tag_edits =
                Some(helpers::pending_edits_from_mutations(&detail.mutations));
        }

        self.view = ActiveView::CompoundTagSplit {
            state,
            clusters,
            safe_mode,
            zone,
        };
        true
    }
}

/// Build the appropriate compound split DecisionKey from zone, safe_mode, tag_name, and cluster index.
fn compound_split_key(
    zone: Zone,
    safe_mode: bool,
    tag_name: String,
    cluster_index: usize,
) -> DecisionKey {
    if zone == Zone::Inbox {
        DecisionKey::CompoundSplitInbox {
            tag_name,
            cluster_index,
        }
    } else if safe_mode {
        DecisionKey::CompoundSplitSafe {
            tag_name,
            cluster_index,
        }
    } else {
        DecisionKey::CompoundSplitReview {
            tag_name,
            cluster_index,
        }
    }
}
