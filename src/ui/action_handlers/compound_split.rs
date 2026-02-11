//! Compound Tag Split Resolution
//!
//! Handles the compound tag split modal: loading signals, navigating between
//! split candidates, staging split/canonicalize decisions, and bulk operations.

use crate::ui::{compound_split_v2, progressive_worker, ActiveView, SuspendedView};
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

        // Load compound signal keys filtered by safety classification and tag
        let signal_keys = read_db.get_compound_signal_keys_by_safety(safe_only, tag_filter)
            .unwrap_or_default();

        if signal_keys.is_empty() {
            let msg = if safe_only {
                "No safe compound splits available"
            } else {
                "No compound splits needing review"
            };
            self.status_message = Some(msg.to_string());
            return;
        }

        // Store signal keys for cluster navigation
        let clusters = compound_split_v2::CompoundSplitClustersV2::new(signal_keys);

        // Start transaction ONCE for entire modal
        if let Some(ref mut witch) = self.witch {
            let mode_str = if safe_only { "safe" } else { "review" };
            let _ = witch.start_transaction(&format!("Compound tag split ({})", mode_str));
        }

        // Load the first signal into modal data (need witch for file info)
        let first_key = &clusters.all_signal_keys()[0];
        let data = {
            let read_db = self.witch.as_mut().unwrap().read_db();
            first_key.parse::<i64>().ok()
                .and_then(|inode| read_db.get_compound_tag_signal(inode).ok())
                .flatten()
                .and_then(|typed_signal| {
                    compound_split_v2::CompoundSplitDataV2::from_compound_tag_signal(&typed_signal, &read_db)
                })
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
                // Ctrl+A - stage ALL splits progressively with progress bar
                self.start_progressive_compound_split_staging();
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

    /// Advance to next compound split signal after confirming current (via Enter).
    fn advance_to_next_compound_split(&mut self) {
        let is_last = matches!(&self.view, ActiveView::CompoundTagSplit { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::CompoundTagSplit { .. }) {
            self.show_transaction_review_for_compound_split();
            return;
        }

        if is_last {
            // At last signal - show review
            self.show_transaction_review_for_compound_split();
        } else {
            let advanced = if let ActiveView::CompoundTagSplit { ref mut clusters, .. } = self.view {
                clusters.next()
            } else {
                false
            };

            if advanced {
                // Load next signal
                if !self.load_current_compound_split_signal() {
                    // Signal load failed - show review with what we have
                    self.show_transaction_review_for_compound_split();
                }
            } else {
                // No more signals - show review
                self.show_transaction_review_for_compound_split();
            }
        }
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
        // Extract clusters and safe_mode from current view, replacing with a temporary
        let (signal_keys, is_safe_mode, clusters, safe_mode) = match &self.view {
            ActiveView::CompoundTagSplit { clusters, safe_mode, .. } => {
                let keys = clusters.all_signal_keys().to_vec();
                if keys.is_empty() {
                    self.status_message = Some("No compound splits to stage".to_string());
                    return;
                }
                (keys, *safe_mode, clusters.clone(), *safe_mode)
            }
            _ => {
                self.status_message = Some("No compound splits to stage".to_string());
                return;
            }
        };

        // Start progressive worker with return context to restore compound split view
        let worker = progressive_worker::ProgressiveWorkerState::for_compound_splits(
            signal_keys,
            is_safe_mode,
        );
        let return_context = Box::new(SuspendedView::CompoundTagSplitReload {
            clusters,
            safe_mode,
        });
        self.view = ActiveView::ProgressiveWork { worker, return_context };
    }

    /// Confirm all safe compound splits directly from insights (Ctrl+A shortcut).
    ///
    /// This is a one-shot operation: loads safe compound signals, starts the progressive
    /// worker to stage all splits, then shows the transaction review screen.
    /// If `tag_filter` is Some, only processes signals for that specific tag.
    pub(in crate::ui) fn confirm_all_safe_compound_splits_from_insights(&mut self, tag_filter: Option<&str>) {
        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        // Load safe compound signal keys only, filtered by tag if specified
        let signal_keys = read_db.get_compound_signal_keys_by_safety(true, tag_filter).unwrap_or_default();

        if signal_keys.is_empty() {
            self.status_message = Some("No safe compound splits available".to_string());
            return;
        }

        // Store signal keys for cluster tracking
        let clusters = compound_split_v2::CompoundSplitClustersV2::new(signal_keys.clone());

        // Start transaction
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Compound tag split (safe bulk)");
        }

        // Start progressive worker to stage all splits
        let worker = progressive_worker::ProgressiveWorkerState::for_compound_splits(
            signal_keys,
            true, // safe mode
        );
        let return_context = Box::new(SuspendedView::CompoundTagSplitReload {
            clusters,
            safe_mode: true,
        });
        self.view = ActiveView::ProgressiveWork { worker, return_context };
    }

    /// Load the compound split signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_compound_split_signal(&mut self) -> bool {
        // Extract cluster info from current view
        let (signal_key, group_index, total, safe_mode) = match &self.view {
            ActiveView::CompoundTagSplit { clusters, safe_mode, .. } => {
                match clusters.current_signal_key() {
                    Some(key) => (key.to_string(), clusters.current_index(), clusters.total(), *safe_mode),
                    None => return false,
                }
            }
            _ => return false,
        };

        // Parse key as inode and query typed signal
        let inode: i64 = match signal_key.parse() {
            Ok(i) => i,
            Err(_) => {
                self.status_message = Some("Invalid signal key (not an inode)".to_string());
                return false;
            }
        };

        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return false,
            };

            let typed_signal = match read_db.get_compound_tag_signal(inode) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    self.status_message = Some("Signal not found".to_string());
                    return false;
                }
                Err(_) => {
                    self.status_message = Some("Signal not found".to_string());
                    return false;
                }
            };

            compound_split_v2::CompoundSplitDataV2::from_compound_tag_signal(&typed_signal, &read_db)
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
            }
        }

        // Update state in existing view
        if let ActiveView::CompoundTagSplit { state: ref mut s, .. } = self.view {
            *s = state;
        }
        true
    }

    /// Load compound split signal using provided clusters (for restoring from SuspendedView).
    ///
    /// Sets the view to CompoundTagSplit with the provided clusters,
    /// loading the current signal's state from the database.
    pub(in crate::ui) fn load_current_compound_split_signal_with_clusters(
        &mut self,
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
    ) -> bool {
        let signal_key = match clusters.current_signal_key() {
            Some(key) => key.to_string(),
            None => return false,
        };

        let (group_index, total) = (clusters.current_index(), clusters.total());

        // Parse key as inode and query typed signal
        let inode: i64 = match signal_key.parse() {
            Ok(i) => i,
            Err(_) => {
                self.status_message = Some("Invalid signal key (not an inode)".to_string());
                return false;
            }
        };

        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return false,
            };

            let typed_signal = match read_db.get_compound_tag_signal(inode) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    self.status_message = Some("Signal not found".to_string());
                    return false;
                }
                Err(_) => {
                    self.status_message = Some("Signal not found".to_string());
                    return false;
                }
            };

            compound_split_v2::CompoundSplitDataV2::from_compound_tag_signal(&typed_signal, &read_db)
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
            }
        }

        self.view = ActiveView::CompoundTagSplit { state, clusters, safe_mode };
        true
    }
}
