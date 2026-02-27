//! Compound Tag Split Resolution
//!
//! Handles the compound tag split modal: loading signals, navigating between
//! split candidates, staging split/canonicalize decisions, and bulk operations.

use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::ui::{compound_split_v2, helpers, progressive_worker, tag_editor, ActiveView};
use crate::ui::suspended_views::SuspendTarget;
use super::witness;
use super::super::App;

impl App {
    /// Start compound tag split resolution modal.
    ///
    /// Loads CompoundTagValue signals filtered by safety and enters the split modal.
    /// If `safe_only` is true, loads only signals where all split parts exist in corpus.
    /// If `tag_filter` is Some, only loads signals for that specific tag name.
    pub(in crate::ui) fn start_compound_split_resolution(&mut self, safe_only: bool, tag_filter: Option<&str>) {
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
        let tag_filter_owned = tag_filter.map(|s| s.to_string());
        let groups = if zone == Zone::Inbox {
            self.cache.query(move |db| {
                db.get_inbox_compound_signal_groups().unwrap_or_default()
            }).recv()
        } else {
            self.cache.query(move |db| {
                db.get_compound_signal_groups_by_safety(safe_only, tag_filter_owned.as_deref())
                    .unwrap_or_default()
            }).recv()
        };

        if groups.is_empty() {
            self.status_message = Some("No compound tag signals to resolve".to_string());
            return;
        }

        // Store groups for cluster navigation
        let clusters = compound_split_v2::CompoundSplitClustersV2::new(groups);

        // Start transaction ONCE for entire modal
        let mode_str = if zone == Zone::Inbox { "inbox" } else if safe_only { "safe" } else { "review" };
        let _ = self.witch.start_transaction(&format!("Compound tag split ({})", mode_str));

        // Load the first group into modal data
        let first_group = clusters.all_groups()[0].clone();
        let data = self.cache.query(move |db| {
            compound_split_v2::CompoundSplitDataV2::from_compound_group(&first_group, &db, zone)
        }).recv();

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

        let state = compound_split_v2::CompoundSplitStateV2::new(data, safe_only, group_index, total_groups, zone);
        self.view = ActiveView::CompoundTagSplit { state, clusters, safe_mode: safe_only, zone };
    }

    /// Handle compound tag split modal actions (v2).
    pub(super) fn handle_compound_split_action(&mut self, action: compound_split_v2::CompoundSplitActionV2, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            compound_split_v2::CompoundSplitActionV2::None => {}
            compound_split_v2::CompoundSplitActionV2::Confirmed => {
                let Some(w) = witness else { return };
                // Stage decision and advance to next signal
                self.stage_compound_split_decision(w);
                self.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Canonicalize => {
                let Some(w) = witness else { return };
                // Mark as canonical (don't split) and advance
                self.stage_compound_canonicalize_decision(w);
                self.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Cancelled => {
                // Discard transaction if active
                self.cancel_and_return_to_source("Compound tag split cancelled");
            }
            compound_split_v2::CompoundSplitActionV2::Navigate { forward } => {
                // Navigate to next/prev signal without staging
                self.navigate_compound_split(forward);
            }
            compound_split_v2::CompoundSplitActionV2::ShowReview => {
                // Ctrl+R - show review with whatever has already been staged
                self.show_transaction_review_for_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::StageAllAndReview => {
                let Some(g) = witness else { return };
                // Ctrl+A - stage ALL splits progressively with progress bar
                self.start_progressive_compound_split_staging(*g);
            }
            compound_split_v2::CompoundSplitActionV2::OpenTagEditorIndividual => {
                self.launch_tag_editor_from_compound_split(tag_editor::TagEditorMode::Individual);
            }
            compound_split_v2::CompoundSplitActionV2::OpenTagEditorAggregated => {
                self.launch_tag_editor_from_compound_split(tag_editor::TagEditorMode::Aggregated);
            }
        }
    }

    /// Launch embedded tag editor from the compound split modal.
    ///
    /// Extracts the current group's file inodes, queries for AudioFile objects,
    /// and opens an embedded tag editor. The editor's file cursor is positioned
    /// to match the health modal's current file selection.
    fn launch_tag_editor_from_compound_split(&mut self, mode: tag_editor::TagEditorMode) {
        // Extract data from current view
        let (inodes, decision_key, decision_label, file_cursor_inode, zone) =
            if let ActiveView::CompoundTagSplit { ref state, ref clusters, safe_mode, zone, .. } = self.view {
                let inodes: Vec<i64> = state.data.files.iter().map(|f| f.inode).collect();
                let tag_name = state.data.compound.tag_name.clone();
                let cluster_index = clusters.current_index();
                let decision_key = compound_split_key(zone, safe_mode, tag_name, cluster_index);
                let label = format!(
                    "Tag edit: {} \"{}\"",
                    state.data.compound.tag_name,
                    state.data.compound.compound_value,
                );
                let cursor_inode = state.data.files.get(state.file_cursor)
                    .map(|f| f.inode);
                (inodes, decision_key, label, cursor_inode, zone)
            } else {
                return;
            };

        // Query audio files by inodes
        let audio_files = self.cache.query(move |db| {
            db.get_audio_files_by_inodes(&inodes, zone)
                .unwrap_or_default()
        }).recv();

        if audio_files.is_empty() {
            self.status_message = Some("No indexed files found for this group".to_string());
            return;
        }

        // Open embedded tag editor (suspends current view on stack)
        self.open_embedded_tag_editor(mode, audio_files, decision_key, decision_label);

        // Position editor cursor on the file matching the health modal's selection
        if let Some(target_inode) = file_cursor_inode {
            if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                if let tag_editor::types::TagEditContext::BulkEdit { ref audio_files, .. } = editor.context {
                    if let Some(idx) = audio_files.iter().position(|af| af.inode() == target_inode) {
                        editor.current_item_idx = idx;
                    }
                }
            }
        }
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
            self.show_transaction_review_for_compound_split();
            return;
        }

        // Normal navigation
        let moved = if let ActiveView::CompoundTagSplit { ref mut clusters, .. } = self.view {
            if forward { clusters.next() } else { clusters.prev() }
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
            self.show_transaction_review_for_compound_split();
            return;
        }

        // Collect staged canonical values for auto-skip
        let staged_canonicals = self.staged_canonical_values();

        loop {
            let is_last = matches!(&self.view, ActiveView::CompoundTagSplit { clusters, .. } if clusters.is_last());

            if is_last {
                self.show_transaction_review_for_compound_split();
                return;
            }

            let advanced = if let ActiveView::CompoundTagSplit { ref mut clusters, .. } = self.view {
                clusters.next()
            } else {
                false
            };

            if !advanced {
                self.show_transaction_review_for_compound_split();
                return;
            }

            if !self.load_current_compound_split_signal() {
                self.show_transaction_review_for_compound_split();
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
        self.witch.decision_keys().iter().filter_map(|key| {
            let decision = self.witch.get_decision(key)?;
            decision.mutations.iter().find_map(|m| {
                if let crate::meta::mutations::Mutation::EmitCanonicalTag(ref ct) = m {
                    Some((ct.tag_name.clone(), ct.canonical_value.clone()))
                } else {
                    None
                }
            })
        }).collect()
    }

    /// Show the transaction review screen for compound tag splits.
    pub(in crate::ui) fn show_transaction_review_for_compound_split(&mut self) {
        self.after_staging_decisions();
    }

    /// Stage the current compound split decision (v2).
    fn stage_compound_split_decision(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, key, description) = match &self.view {
            ActiveView::CompoundTagSplit { ref state, ref clusters, safe_mode, zone, .. } => {
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

        let _ = super::super::operator_decisions::stage_decision(&mut self.witch, key, &description, mutations, gesture);
    }

    /// Stage a canonicalize decision (mark compound value as canonical, don't split).
    fn stage_compound_canonicalize_decision(&mut self, gesture: &witness::ConfirmationGesture) {
        let (mutations, key, description) = match &self.view {
            ActiveView::CompoundTagSplit { ref state, ref clusters, safe_mode, zone, .. } => {
                let mutation = state.data.create_canonical_signal();
                let desc = format!(
                    "Keep \"{}\" in {} as canonical",
                    state.data.compound.compound_value,
                    state.data.compound.tag_name,
                );
                let tag_name = state.data.compound.tag_name.clone();
                let key = compound_split_key(*zone, *safe_mode, tag_name, clusters.current_index());
                (vec![mutation], key, desc)
            }
            _ => return,
        };

        let _ = super::super::operator_decisions::stage_decision(&mut self.witch, key, &description, mutations, gesture);
    }

    /// Start progressive worker to stage ALL compound splits.
    ///
    /// Called when user presses Ctrl+A in the compound split modal.
    /// Uses the progressive worker to process items in timed chunks with progress bar.
    fn start_progressive_compound_split_staging(&mut self, gesture: witness::ConfirmationGesture) {
        // Extract groups and safe_mode from current view
        let (groups, is_safe_mode) = match &self.view {
            ActiveView::CompoundTagSplit { clusters, safe_mode, .. } => {
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
            ActiveView::CompoundTagSplit { clusters, safe_mode, zone, .. } => {
                match clusters.current_group() {
                    Some(g) => (g.clone(), clusters.current_index(), clusters.total(), *safe_mode, *zone),
                    None => return false,
                }
            }
            _ => return false,
        };

        let data = self.cache.query(move |db| {
            compound_split_v2::CompoundSplitDataV2::from_compound_group(&group, &db, zone)
        }).recv();

        let Some(data) = data else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        // Create modal state
        let mut state = compound_split_v2::CompoundSplitStateV2::new(
            data,
            safe_mode,
            group_index,
            total,
            zone,
        );

        // Back-fill UI state from staged decision if one exists for this cluster
        let backfill_key = compound_split_key(zone, safe_mode, state.data.compound.tag_name.clone(), group_index);
        if let Some(decision) = self.witch.get_decision(&backfill_key) {
            state.restore_from_mutations(&decision.mutations);
            state.pending_tag_edits = Some(helpers::pending_edits_from_mutations(&decision.mutations));
        }

        // Update state in existing view
        if let ActiveView::CompoundTagSplit { state: ref mut s, .. } = self.view {
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

        let data = self.cache.query(move |db| {
            compound_split_v2::CompoundSplitDataV2::from_compound_group(&group, &db, zone)
        }).recv();

        let Some(data) = data else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        // Create modal state
        let mut state = compound_split_v2::CompoundSplitStateV2::new(
            data,
            safe_mode,
            group_index,
            total,
            zone,
        );

        // Back-fill UI state from staged decision if one exists for this cluster
        let backfill_key = compound_split_key(zone, safe_mode, state.data.compound.tag_name.clone(), group_index);
        if let Some(decision) = self.witch.get_decision(&backfill_key) {
            state.restore_from_mutations(&decision.mutations);
            state.pending_tag_edits = Some(helpers::pending_edits_from_mutations(&decision.mutations));
        }

        self.view = ActiveView::CompoundTagSplit { state, clusters, safe_mode, zone };
        true
    }
}

/// Build the appropriate compound split DecisionKey from zone, safe_mode, tag_name, and cluster index.
fn compound_split_key(zone: Zone, safe_mode: bool, tag_name: String, cluster_index: usize) -> DecisionKey {
    if zone == Zone::Inbox {
        DecisionKey::CompoundSplitInbox { tag_name, cluster_index }
    } else if safe_mode {
        DecisionKey::CompoundSplitSafe { tag_name, cluster_index }
    } else {
        DecisionKey::CompoundSplitReview { tag_name, cluster_index }
    }
}
