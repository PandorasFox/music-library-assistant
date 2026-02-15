//! Compound Tag Split Resolution
//!
//! Handles the compound tag split modal: loading signals, navigating between
//! split candidates, staging split/canonicalize decisions, and bulk operations.

use crate::corpus::db::types::Zone;
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
        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        // Load compound signal groups filtered by safety classification and tag
        let groups = read_db.get_compound_signal_groups_by_safety(safe_only, tag_filter)
            .unwrap_or_default();

        if groups.is_empty() {
            let msg = if safe_only {
                "No safe compound splits available"
            } else {
                "No compound splits needing review"
            };
            self.status_message = Some(msg.to_string());
            return;
        }

        // Store groups for cluster navigation
        let clusters = compound_split_v2::CompoundSplitClustersV2::new(groups);

        // Start transaction ONCE for entire modal
        if let Some(ref mut witch) = self.witch {
            let mode_str = if safe_only { "safe" } else { "review" };
            let _ = witch.start_transaction(&format!("Compound tag split ({})", mode_str));
        }

        // Load the first group into modal data
        let first_group = clusters.all_groups()[0].clone();
        let data = {
            let read_db = self.witch.as_mut().unwrap().read_db();
            compound_split_v2::CompoundSplitDataV2::from_compound_group(&first_group, &read_db)
        };

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                // Discard the transaction we just started
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters
        let (group_index, total_groups) = (clusters.current_index(), clusters.total());

        let state = compound_split_v2::CompoundSplitStateV2::new(data, safe_only, group_index, total_groups);
        self.view = ActiveView::CompoundTagSplit { state, clusters, safe_mode: safe_only };
    }

    /// Handle compound tag split modal actions (v2).
    pub(super) fn handle_compound_split_action(&mut self, action: compound_split_v2::CompoundSplitActionV2, witness: Option<&witness::DecisionWitness>) {
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
                self.cancel_and_return_to_insights("Compound tag split cancelled");
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
                let Some(_w) = witness else { return };
                // Ctrl+A - stage ALL splits progressively with progress bar
                self.start_progressive_compound_split_staging();
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
        let (inodes, decision_index, decision_label, file_cursor_inode) =
            if let ActiveView::CompoundTagSplit { ref state, ref clusters, .. } = self.view {
                let inodes: Vec<i64> = state.data.files.iter().map(|f| f.inode).collect();
                let decision_index = clusters.current_index();
                let label = format!(
                    "Tag edit: {} \"{}\"",
                    state.data.compound.tag_name,
                    state.data.compound.compound_value,
                );
                let cursor_inode = state.data.files.get(state.file_cursor)
                    .map(|f| f.inode);
                (inodes, decision_index, label, cursor_inode)
            } else {
                return;
            };

        // Query audio files by inodes
        let audio_files = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return,
            };
            read_db.get_audio_files_by_inodes(&inodes, Zone::Corpus)
                .unwrap_or_default()
        };

        if audio_files.is_empty() {
            self.status_message = Some("No indexed files found for this group".to_string());
            return;
        }

        // Open embedded tag editor (suspends current view on stack)
        self.open_embedded_tag_editor(mode, audio_files, decision_index, decision_label);

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
            self.start_insights_view();
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
            self.start_insights_view();
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
        let Some(ref witch) = self.witch else { return Vec::new() };
        witch.decision_indices().iter().filter_map(|&idx| {
            let decision = witch.get_decision(idx)?;
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
        self.start_transaction_review();
    }

    /// Stage the current compound split decision (v2).
    fn stage_compound_split_decision(&mut self, _witness: &witness::DecisionWitness) {
        let (mutations, cluster_idx, description) = match &self.view {
            ActiveView::CompoundTagSplit { ref state, ref clusters, .. } => {
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
                (mutations, clusters.current_index(), desc)
            }
            _ => return,
        };

        // Stage the decision via operator_decisions
        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &description, mutations);
        }
    }

    /// Stage a canonicalize decision (mark compound value as canonical, don't split).
    fn stage_compound_canonicalize_decision(&mut self, _witness: &witness::DecisionWitness) {
        let (mutations, cluster_idx, description) = match &self.view {
            ActiveView::CompoundTagSplit { ref state, ref clusters, .. } => {
                let mutation = state.data.create_canonical_signal();
                let desc = format!(
                    "Keep \"{}\" in {} as canonical",
                    state.data.compound.compound_value,
                    state.data.compound.tag_name,
                );
                (vec![mutation], clusters.current_index(), desc)
            }
            _ => return,
        };

        // Stage the decision via operator_decisions
        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &description, mutations);
        }
    }

    /// Start progressive worker to stage ALL compound splits.
    ///
    /// Called when user presses Ctrl+A in the compound split modal.
    /// Uses the progressive worker to process items in timed chunks with progress bar.
    fn start_progressive_compound_split_staging(&mut self) {
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
        );
        self.push_and_switch(SuspendTarget::ProgressiveWork(worker));
    }

    /// Load the compound split group at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_compound_split_signal(&mut self) -> bool {
        // Extract cluster info from current view
        let (group, group_index, total, safe_mode) = match &self.view {
            ActiveView::CompoundTagSplit { clusters, safe_mode, .. } => {
                match clusters.current_group() {
                    Some(g) => (g.clone(), clusters.current_index(), clusters.total(), *safe_mode),
                    None => return false,
                }
            }
            _ => return false,
        };

        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return false,
            };

            compound_split_v2::CompoundSplitDataV2::from_compound_group(&group, &read_db)
        };

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
        );

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(group_index) {
                state.restore_from_mutations(&decision.mutations);
                state.pending_tag_edits = Some(helpers::pending_edits_from_mutations(&decision.mutations));
            }
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
    ) -> bool {
        let group = match clusters.current_group() {
            Some(g) => g.clone(),
            None => return false,
        };

        let (group_index, total) = (clusters.current_index(), clusters.total());

        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return false,
            };

            compound_split_v2::CompoundSplitDataV2::from_compound_group(&group, &read_db)
        };

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
        );

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(group_index) {
                state.restore_from_mutations(&decision.mutations);
                state.pending_tag_edits = Some(helpers::pending_edits_from_mutations(&decision.mutations));
            }
        }

        self.view = ActiveView::CompoundTagSplit { state, clusters, safe_mode };
        true
    }
}
