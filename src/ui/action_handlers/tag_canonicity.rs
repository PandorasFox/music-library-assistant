//! Tag Canonicity Resolution
//!
//! Handles the tag canonicity flow: loading signals, navigating between
//! clusters, staging canonicalization decisions.

use crate::ui::{
    insights_view, tag_canonicity_v2, transaction_review, ActiveView,
    CanonicitySignalKind, TagCanonicityClusters,
};
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
        use crate::meta::signals::AggregateSignalType;

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

        // Determine signal kind and load keys (scoped borrow of read_db)
        let (signal_keys, kind) = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };

            match &insight_type {
                insights_view::InsightType::InconsistentAlbumArtist => {
                    let keys = read_db.get_aggregate_signal_keys(AggregateSignalType::InconsistentAlbumArtist)
                        .unwrap_or_default();
                    (keys, CanonicitySignalKind::InconsistentAlbumArtist)
                }
                insights_view::InsightType::TagCanonicity { tag_name } => {
                    let all_keys = read_db.get_aggregate_signal_keys(AggregateSignalType::TagCanonicity)
                        .unwrap_or_default();
                    let keys: Vec<String> = all_keys.into_iter()
                        .filter(|k| k.starts_with(&format!("{}:", tag_name)))
                        .collect();
                    (keys, CanonicitySignalKind::TagCanonicity)
                }
                _ => {
                    self.status_message = Some("Invalid insight type for tag resolution".to_string());
                    return;
                }
            }
        };

        if signal_keys.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        let clusters = TagCanonicityClusters::new(signal_keys, kind);

        // Start transaction ONCE for entire flow
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Tag canonicalization");
        }

        // Load the first signal into V2 modal data using typed query
        let first_key = clusters.signal_keys[0].clone();
        let data = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };
            Self::load_typed_signal_data(&first_key, kind, &read_db)
        };

        let data = match data {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to load signal data".to_string());
                // Discard the transaction we just started via sealed operator decision handler
                if let Some(ref mut witch) = self.witch {
                    let _ = super::super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters
        let pre_fill = clusters.pre_fill();
        let (group_index, total_groups) = (clusters.current_index, clusters.signal_keys.len());

        let state = tag_canonicity_v2::TagCanonicalityStateV2::new(data, pre_fill, group_index, total_groups);
        self.view = ActiveView::TagCanonicityResolution { state, clusters };
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
        let is_first = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.current_index == 0);
        let is_last = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::TagCanonicityResolution { .. }) {
            self.start_insights_view();
            return;
        }

        if !forward && is_first {
            // Shift-Tab from first group = do nothing
            return;
        }

        if forward && is_last {
            // Tab from last group = show review screen
            self.show_transaction_review_for_canonicity();
            return;
        }

        // Normal navigation
        let moved = if let ActiveView::TagCanonicityResolution { ref mut clusters, .. } = self.view {
            if forward { clusters.next() } else { clusters.prev() }
        } else {
            false
        };

        if moved && !self.load_current_cluster_signal() {
            // Signal load failed - return to insights
            self.start_insights_view();
        }
    }

    /// Advance to next cluster after confirming current one.
    fn advance_to_next_cluster(&mut self) {
        let is_last = matches!(&self.view, ActiveView::TagCanonicityResolution { clusters, .. } if clusters.is_last());

        if !matches!(&self.view, ActiveView::TagCanonicityResolution { .. }) {
            self.show_transaction_review_for_canonicity();
            return;
        }

        if is_last {
            // At last cluster - show review screen
            self.show_transaction_review_for_canonicity();
        } else {
            let advanced = if let ActiveView::TagCanonicityResolution { ref mut clusters, .. } = self.view {
                clusters.next()
            } else {
                false
            };

            if advanced {
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
    }

    /// Show the transaction review screen for tag canonicity.
    fn show_transaction_review_for_canonicity(&mut self) {
        self.start_transaction_review(transaction_review::TransactionReviewSource::TagCanonicityResolution);
    }

    /// Stage a decision for the current canonicity cluster (V2).
    ///
    /// This adds the decision to the transaction but does NOT confirm it.
    /// The transaction is confirmed when the user completes the review screen.
    fn stage_canonicity_decision(&mut self) {
        let (mutations, cluster_idx, tag_name) = match &self.view {
            ActiveView::TagCanonicityResolution { ref state, ref clusters } => {
                let mutations = state.mutations();
                if mutations.is_empty() {
                    return;
                }
                (mutations, clusters.current_index, state.data.tag_name.clone())
            }
            _ => return,
        };

        let label = format!("Canonicalize {}", tag_name);

        // Add decision to existing transaction via sealed operator decision handler
        if let Some(ref mut witch) = self.witch {
            let _ = super::super::operator_decisions::stage_decision(witch, cluster_idx, &label, mutations);
        }
    }

    /// Load the signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    pub(in crate::ui) fn load_current_cluster_signal(&mut self) -> bool {
        // Extract cluster info from current view
        let (signal_key, kind, current_index, total) = match &self.view {
            ActiveView::TagCanonicityResolution { clusters, .. } => {
                match clusters.current_signal_key() {
                    Some(key) => (
                        key.to_string(),
                        clusters.kind,
                        clusters.current_index,
                        clusters.signal_keys.len(),
                    ),
                    None => return false,
                }
            }
            _ => return false,
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => return false,
        };

        let data = match Self::load_typed_signal_data(&signal_key, kind, &read_db) {
            Some(d) => d,
            None => {
                self.status_message = Some("Signal not found".to_string());
                return false;
            }
        };

        let pre_fill = kind == CanonicitySignalKind::TagCanonicity;
        let mut state = tag_canonicity_v2::TagCanonicalityStateV2::new(data, pre_fill, current_index, total);

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(current_index) {
                state.restore_from_mutations(&decision.mutations);
            }
        }

        // Update state in existing view, or set view if called from restore path
        if let ActiveView::TagCanonicityResolution { state: ref mut s, .. } = self.view {
            *s = state;
        }
        true
    }

    /// Load cluster signal using provided clusters (for restoring from SuspendedView).
    ///
    /// Sets the view to TagCanonicityResolution with the provided clusters,
    /// loading the current signal's state from the database.
    pub(in crate::ui) fn load_current_cluster_signal_with_clusters(&mut self, clusters: TagCanonicityClusters) -> bool {
        let signal_key = match clusters.current_signal_key() {
            Some(key) => key.to_string(),
            None => return false,
        };

        let kind = clusters.kind;
        let (current_index, total) = (clusters.current_index, clusters.signal_keys.len());

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => return false,
        };

        let data = match Self::load_typed_signal_data(&signal_key, kind, &read_db) {
            Some(d) => d,
            None => {
                self.status_message = Some("Signal not found".to_string());
                return false;
            }
        };

        let pre_fill = clusters.pre_fill();
        let mut state = tag_canonicity_v2::TagCanonicalityStateV2::new(data, pre_fill, current_index, total);

        // Back-fill UI state from staged decision if one exists for this cluster
        if let Some(ref witch) = self.witch {
            if let Some(decision) = witch.get_decision(current_index) {
                state.restore_from_mutations(&decision.mutations);
            }
        }

        self.view = ActiveView::TagCanonicityResolution { state, clusters };
        true
    }

    /// Load typed signal data by key and kind, returning modal data.
    fn load_typed_signal_data(
        key: &str,
        kind: CanonicitySignalKind,
        read_db: &crate::corpus::db::ReadOnlyDb,
    ) -> Option<tag_canonicity_v2::TagCanonicalityModalDataV2> {
        match kind {
            CanonicitySignalKind::TagCanonicity => {
                let signal = read_db.get_tag_canonicity_signal(key).ok()??;
                tag_canonicity_v2::TagCanonicalityModalDataV2::from_tag_canonicity(&signal, read_db)
            }
            CanonicitySignalKind::InconsistentAlbumArtist => {
                let signal = read_db.get_inconsistent_album_artist_signal(key).ok()??;
                tag_canonicity_v2::TagCanonicalityModalDataV2::from_inconsistent_album_artist(&signal, read_db)
            }
        }
    }
}
