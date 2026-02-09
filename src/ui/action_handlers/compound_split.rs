//! Compound Tag Split Resolution
//!
//! Handles the compound tag split flow: loading signals, navigating between
//! split candidates, staging split/canonicalize decisions, and bulk operations.

use crate::ui::{compound_split_v2, transaction_review};
use super::super::App;

impl App {
    /// Start compound tag split resolution flow.
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

        // Load compound signals filtered by safety classification and tag
        let signals = read_db.get_compound_signals_by_safety(safe_only, tag_filter)
            .unwrap_or_default();

        if signals.is_empty() {
            let msg = if safe_only {
                "No safe compound splits available"
            } else {
                "No compound splits needing review"
            };
            self.status_message = Some(msg.to_string());
            return;
        }

        // Store signal IDs for cluster navigation
        let signal_ids: Vec<i64> = signals.iter().filter_map(|s| s.id).collect();
        self.compound_split_clusters = Some(compound_split_v2::CompoundSplitClustersV2::new(signal_ids));
        self.compound_split_safe_mode = safe_only;

        // Start transaction ONCE for entire flow
        if let Some(ref mut witch) = self.witch {
            let mode_str = if safe_only { "safe" } else { "review" };
            let _ = witch.start_transaction(&format!("Compound tag split ({})", mode_str));
        }

        // Load the first signal into modal data (need witch for file info)
        let first_signal = &signals[0];
        let data = {
            let read_db = self.witch.as_mut().unwrap().read_db();
            compound_split_v2::CompoundSplitDataV2::from_signal_with_files(first_signal, &read_db)
        };

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                self.compound_split_clusters = None;
                // Discard the transaction we just started
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters
        let (group_index, total_groups) = self.compound_split_clusters
            .as_ref()
            .map(|c| (c.current_index(), c.total()))
            .unwrap_or((0, 1));

        let state = compound_split_v2::CompoundSplitStateV2::new(data, safe_only, group_index, total_groups);
        self.compound_split_state = Some(state);
        self.mode = super::super::types::UiMode::CompoundTagSplit;
    }

    /// Handle compound tag split modal actions (v2).
    pub(in crate::ui) fn handle_compound_split_action(&mut self, action: compound_split_v2::CompoundSplitActionV2) {
        match action {
            compound_split_v2::CompoundSplitActionV2::None => {}
            compound_split_v2::CompoundSplitActionV2::Confirmed => {
                // Stage decision and advance to next signal
                self.stage_compound_split_decision();
                self.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Canonicalize => {
                // Mark as canonical (don't split) and advance
                self.stage_compound_canonicalize_decision();
                self.advance_to_next_compound_split();
            }
            compound_split_v2::CompoundSplitActionV2::Cancelled => {
                // Discard transaction if active
                self.cancel_and_return_to_insights("Compound tag split cancelled");
                self.compound_split_state = None;
                self.compound_split_clusters = None;
            }
            compound_split_v2::CompoundSplitActionV2::Navigate { forward } => {
                // Navigate to next/prev signal without staging
                self.navigate_compound_split(forward);
            }
            compound_split_v2::CompoundSplitActionV2::ShowReview => {
                // Ctrl+R - stage current decision and show review
                self.stage_compound_split_decision();
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
        let Some(ref mut clusters) = self.compound_split_clusters else {
            self.compound_split_state = None;
            self.start_insights_view();
            return;
        };

        if !forward && clusters.is_first() {
            // Shift-Tab from first = do nothing
            return;
        }

        if forward && clusters.is_last() {
            // Tab from last = show review screen
            self.show_transaction_review_for_compound_split();
            return;
        }

        // Normal navigation
        let moved = if forward { clusters.next() } else { clusters.prev() };
        if moved && !self.load_current_compound_split_signal() {
            // Signal load failed - return to insights
            self.start_insights_view();
        }
    }

    /// Advance to next compound split signal after confirming current (via Enter).
    fn advance_to_next_compound_split(&mut self) {
        let Some(ref mut clusters) = self.compound_split_clusters else {
            self.show_transaction_review_for_compound_split();
            return;
        };

        if clusters.is_last() {
            // At last signal - show review
            self.show_transaction_review_for_compound_split();
        } else if clusters.next() {
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

    /// Show the transaction review screen for compound tag splits.
    pub(in crate::ui) fn show_transaction_review_for_compound_split(&mut self) {
        // Clear the resolution modal state (but keep clusters for Cancel navigation)
        self.compound_split_state = None;

        // Always proceed to review - it will show "No changes" if empty
        self.start_transaction_review(transaction_review::TransactionReviewSource::CompoundTagSplit);
    }

    /// Stage the current compound split decision (v2).
    fn stage_compound_split_decision(&mut self) {
        let Some(ref state) = self.compound_split_state else {
            return;
        };

        let cluster_idx = self.compound_split_clusters
            .as_ref()
            .map(|c| c.current_index())
            .unwrap_or(0);

        // Generate mutations using v2 state
        let mutations = state.mutations();

        if mutations.is_empty() {
            return;
        }

        // Stage the decision via operator_decisions
        let description = format!(
            "Split \"{}\" in {} → [{}]",
            state.data.compound.compound_value,
            state.data.compound.tag_name,
            state.edited_parts.join(", ")
        );

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &description, mutations);
        }
    }

    /// Stage a canonicalize decision (mark compound value as canonical, don't split).
    fn stage_compound_canonicalize_decision(&mut self) {
        let Some(ref state) = self.compound_split_state else {
            return;
        };

        let cluster_idx = self.compound_split_clusters
            .as_ref()
            .map(|c| c.current_index())
            .unwrap_or(0);

        // Create the EmitCanonicalTag mutation
        let mutation = state.data.create_canonical_signal();
        let mutations = vec![mutation];

        // Stage the decision via operator_decisions
        let description = format!(
            "Keep \"{}\" in {} as canonical",
            state.data.compound.compound_value,
            state.data.compound.tag_name,
        );

        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &description, mutations);
        }
    }

    /// Start progressive worker to stage ALL compound splits.
    ///
    /// Called when user presses Ctrl+A in the compound split modal.
    /// Uses the progressive worker to process items in timed chunks with progress bar.
    fn start_progressive_compound_split_staging(&mut self) {
        let Some(ref clusters) = self.compound_split_clusters else {
            self.status_message = Some("No compound splits to stage".to_string());
            return;
        };

        let signal_ids = clusters.all_signal_ids().to_vec();
        if signal_ids.is_empty() {
            self.status_message = Some("No compound splits to stage".to_string());
            return;
        }

        let is_safe_mode = self.compound_split_safe_mode;

        // Start progressive worker
        let worker = super::super::progressive_worker::ProgressiveWorkerState::for_compound_splits(
            signal_ids,
            is_safe_mode,
        );
        self.progressive_worker = Some(worker);
        self.mode = super::super::types::UiMode::ProgressiveWork;
    }

    /// Confirm all safe compound splits directly from insights (Ctrl+A shortcut).
    ///
    /// This is a one-shot flow: loads safe compound signals, starts the progressive
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

        // Load safe compound signals only, filtered by tag if specified
        let signals = read_db.get_compound_signals_by_safety(true, tag_filter).unwrap_or_default();

        if signals.is_empty() {
            self.status_message = Some("No safe compound splits available".to_string());
            return;
        }

        // Store signal IDs for cluster tracking
        let signal_ids: Vec<i64> = signals.iter().filter_map(|s| s.id).collect();
        self.compound_split_clusters = Some(compound_split_v2::CompoundSplitClustersV2::new(signal_ids.clone()));
        self.compound_split_safe_mode = true;

        // Start transaction
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Compound tag split (safe bulk)");
        }

        // Start progressive worker to stage all splits
        let worker = super::super::progressive_worker::ProgressiveWorkerState::for_compound_splits(
            signal_ids,
            true, // safe mode
        );
        self.progressive_worker = Some(worker);
        self.mode = super::super::types::UiMode::ProgressiveWork;
    }

    /// Load the compound split signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_compound_split_signal(&mut self) -> bool {
        use crate::meta::signals::{AggregateSignal, AggregateSignalType};

        let Some(ref clusters) = self.compound_split_clusters else {
            return false;
        };

        let Some(signal_id) = clusters.current_signal_id() else {
            self.compound_split_state = None;
            self.compound_split_clusters = None;
            return false;
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.compound_split_state = None;
                self.compound_split_clusters = None;
                return false;
            }
        };

        // Fetch the signal by ID
        let signal = match read_db.get_signal_by_id(signal_id) {
            Ok(Some(s)) => s,
            _ => {
                self.status_message = Some("Signal not found".to_string());
                self.compound_split_state = None;
                self.compound_split_clusters = None;
                return false;
            }
        };

        // Convert to AggregateSignal
        let agg_signal = AggregateSignal {
            id: signal.id,
            signal_type: match AggregateSignalType::from_str(signal.issue_type.as_str()) {
                Some(t) => t,
                None => {
                    self.status_message = Some("Invalid signal type".to_string());
                    return false;
                }
            },
            key: signal.issue_key,
            discovered_at: signal.discovered_at,
            metadata_json: signal.metadata_json,
        };

        // Verify signal type
        if agg_signal.signal_type != AggregateSignalType::CompoundTagValue {
            self.status_message = Some(format!(
                "Wrong signal type: expected compound_tag_value, got {:?}",
                agg_signal.signal_type
            ));
            return false;
        }

        // Parse modal data from signal (need witch for file info)
        let data = {
            let read_db = self.witch.as_mut().unwrap().read_db();
            compound_split_v2::CompoundSplitDataV2::from_signal_with_files(&agg_signal, &read_db)
        };

        let Some(data) = data else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        let group_index = clusters.current_index();

        // Create modal state
        let mut state = compound_split_v2::CompoundSplitStateV2::new(
            data,
            self.compound_split_safe_mode,
            group_index,
            clusters.total(),
        );

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(group_index) {
                state.restore_from_mutations(&decision.mutations);
            }
        }

        self.compound_split_state = Some(state);
        true
    }
}
