//! Tag Canonicity Resolution
//!
//! Handles the tag canonicity flow: loading signals, navigating between
//! clusters, staging canonicalization decisions.

use crate::ui::{insights_view, tag_canonicity_v2, transaction_review};
use crate::ui::types::UiMode;
use super::super::App;

impl App {
    /// Start tag canonicity resolution from Insights view.
    ///
    /// Uses the selected insight type to determine which signals to load:
    /// - InconsistentAlbumArtist: loads all inconsistent_album_artist signals
    /// - TagCanonicity { tag_name }: loads tag_canonicity signals filtered by tag_name
    ///
    /// Uses the V2 three-pane layout for tag canonicity resolution.
    pub(in crate::ui) fn start_tag_canonicity_resolution(&mut self) {
        use crate::corpus::db::types::AggregateSignalType;

        // Get the selected insight type to determine what to load
        let insight_type = match self.insights_view.as_ref().and_then(|v| v.selected_insight_type()) {
            Some(t) => t,
            None => {
                self.status_message = Some("No insight selected".to_string());
                return;
            }
        };

        // Load signals based on insight type (scoped borrow of read_db)
        let (signals, pre_fill) = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };

            match &insight_type {
                insights_view::InsightType::InconsistentAlbumArtist => {
                    let sigs = read_db.get_aggregate_signals(Some(AggregateSignalType::InconsistentAlbumArtist))
                        .unwrap_or_default();
                    (sigs, false) // No pre-fill for album_artist
                }
                insights_view::InsightType::TagCanonicity { tag_name } => {
                    // Load all TagCanonicity signals, then filter by tag_name prefix
                    let all_sigs = read_db.get_aggregate_signals(Some(AggregateSignalType::TagCanonicity))
                        .unwrap_or_default();
                    let filtered: Vec<_> = all_sigs.into_iter()
                        .filter(|s| s.key.starts_with(&format!("{}:", tag_name)))
                        .collect();
                    (filtered, true) // Pre-fill for tag canonicity
                }
                _ => {
                    self.status_message = Some("Invalid insight type for tag resolution".to_string());
                    return;
                }
            }
        };

        if signals.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        // Store signal IDs for cluster navigation
        let signal_ids: Vec<i64> = signals.iter().filter_map(|s| s.id).collect();
        self.tag_canonicity_clusters = Some(super::super::TagCanonicityClusters::new(signal_ids));

        // Start transaction ONCE for entire flow
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Tag canonicalization");
        }

        // Load the first signal into V2 modal data (with file info) - scoped borrow
        let first_signal = &signals[0];
        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    self.tag_canonicity_clusters = None;
                    return;
                }
            };
            tag_canonicity_v2::TagCanonicalityModalDataV2::from_signal_with_files(first_signal, &read_db)
        };

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                self.tag_canonicity_clusters = None;
                // Discard the transaction we just started via sealed operator decision handler
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters (just set above)
        let (group_index, total_groups) = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| (c.current_index, c.signal_ids.len()))
            .unwrap_or((0, 1));

        let state = tag_canonicity_v2::TagCanonicalityStateV2::new(data, pre_fill, group_index, total_groups);
        self.tag_canonicity_state = Some(state);
        self.mode = UiMode::TagCanonicityResolution;
    }

    /// Handle tag canonicity modal actions (three-pane layout).
    pub(in crate::ui) fn handle_tag_canonicity_action(&mut self, action: tag_canonicity_v2::TagCanonicalityActionV2) {
        match action {
            tag_canonicity_v2::TagCanonicalityActionV2::None => {}
            tag_canonicity_v2::TagCanonicalityActionV2::Confirmed => {
                // Stage decision and advance to next cluster
                self.stage_canonicity_decision();
                self.advance_to_next_cluster();
            }
            tag_canonicity_v2::TagCanonicalityActionV2::Cancelled => {
                // Discard transaction if active via sealed operator decision handler
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                crate::logging::log_general("Tag canonicity resolution cancelled");
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                self.start_insights_view();
            }
            tag_canonicity_v2::TagCanonicalityActionV2::Navigate { forward } => {
                // User navigated to next/prev cluster - do NOT stage decision
                self.navigate_cluster(forward);
            }
            tag_canonicity_v2::TagCanonicalityActionV2::ShowReview => {
                // Ctrl+R - stage current decision and show review
                self.stage_canonicity_decision();
                self.show_transaction_review_for_canonicity();
            }
        }
    }

    /// Navigate to next/prev cluster without staging a decision.
    fn navigate_cluster(&mut self, forward: bool) {
        let Some(ref mut clusters) = self.tag_canonicity_clusters else {
            self.tag_canonicity_state = None;
            self.start_insights_view();
            return;
        };

        if !forward && clusters.current_index == 0 {
            // Shift-Tab from first group = do nothing
            return;
        }

        if forward && clusters.is_last() {
            // Tab from last group = show review screen
            self.show_transaction_review_for_canonicity();
            return;
        }

        // Normal navigation
        let moved = if forward { clusters.next() } else { clusters.prev() };
        if moved && !self.load_current_cluster_signal() {
            // Signal load failed - return to insights
            self.start_insights_view();
        }
    }

    /// Advance to next cluster after confirming current one.
    fn advance_to_next_cluster(&mut self) {
        let Some(ref mut clusters) = self.tag_canonicity_clusters else {
            self.show_transaction_review_for_canonicity();
            return;
        };

        if clusters.is_last() {
            // At last cluster - show review screen
            self.show_transaction_review_for_canonicity();
        } else if clusters.next() {
            // Load next signal
            if !self.load_current_cluster_signal() {
                // Signal load failed - show review with what we have
                self.show_transaction_review_for_canonicity();
            }
        } else {
            // No more clusters - show review
            self.show_transaction_review_for_canonicity();
        }
    }

    /// Show the transaction review screen for tag canonicity.
    fn show_transaction_review_for_canonicity(&mut self) {
        // Clear the resolution modal state (but keep clusters for Cancel navigation)
        self.tag_canonicity_state = None;

        // Always proceed to review - it will show "No changes" if empty
        self.start_transaction_review(transaction_review::TransactionReviewSource::TagCanonicityResolution);
    }

    /// Stage a decision for the current canonicity cluster (V2).
    ///
    /// This adds the decision to the transaction but does NOT confirm it.
    /// The transaction is confirmed when the user completes the review screen.
    fn stage_canonicity_decision(&mut self) {
        let Some(ref state) = self.tag_canonicity_state else {
            return;
        };

        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Generate mutations using the V2 state's cached data
        let mutations = state.mutations();
        if mutations.is_empty() {
            // No mutations for this cluster - that's OK, skip it
            return;
        }

        let cluster_idx = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| c.current_index)
            .unwrap_or(0);

        let label = format!("Canonicalize {}", state.data.tag_name);

        // Add decision to existing transaction via sealed operator decision handler
        let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &label, mutations);
    }

    /// Load the signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_cluster_signal(&mut self) -> bool {
        use crate::corpus::db::types::AggregateSignalType;

        let Some(ref clusters) = self.tag_canonicity_clusters else {
            return false;
        };

        let Some(signal_id) = clusters.current_signal_id() else {
            self.tag_canonicity_state = None;
            self.tag_canonicity_clusters = None;
            return false;
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                return false;
            }
        };

        // Load signal by ID
        let signal = match read_db.get_signal_by_id(signal_id) {
            Ok(Some(s)) => s,
            _ => {
                self.status_message = Some("Signal not found".to_string());
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                return false;
            }
        };

        // Convert to AggregateSignal for modal data loading
        let agg_signal = crate::corpus::db::types::AggregateSignal {
            id: signal.id,
            signal_type: match crate::corpus::db::types::AggregateSignalType::from_str(signal.issue_type.as_str()) {
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

        let data = match tag_canonicity_v2::TagCanonicalityModalDataV2::from_signal_with_files(&agg_signal, &read_db) {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                return false;
            }
        };

        // Determine pre-fill based on signal type
        let pre_fill = agg_signal.signal_type == AggregateSignalType::TagCanonicity;

        // Get group info from clusters
        let (group_index, total_groups) = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| (c.current_index, c.signal_ids.len()))
            .unwrap_or((0, 1));

        let mut state = tag_canonicity_v2::TagCanonicalityStateV2::new(data, pre_fill, group_index, total_groups);

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(group_index) {
                state.restore_from_mutations(&decision.mutations);
            }
        }

        self.tag_canonicity_state = Some(state);
        true
    }
}
