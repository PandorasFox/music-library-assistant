//! Status publishing and utility queries for the Witch.
//!
//! Provides read-only snapshots of Witch state for UI rendering
//! and protocol responses.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use super::{
    TransactionSnapshot, WitchStatus, WorkStateSnapshot, WorkStatus,
};

impl super::Witch {
    /// Get current Witch status snapshot (readonly, does not advance state).
    ///
    /// Safe to call from render code, utility functions, etc.
    /// Use `tick()` only from the main event loop to advance state.
    pub fn status(&self) -> WorkStatus {
        WorkStatus {
            state: WorkStateSnapshot::from(&self.work_state),
            pending: self.work_state.in_flight(),
            total_processed: self.work_state.processed(),
            session_queued: self.work_state.queued(),
            pending_by_label: self.work_state.by_label().cloned().unwrap_or_default(),
        }
    }

    /// Build the comprehensive state machine snapshot.
    ///
    /// This captures all observable Witch state in a single struct.
    /// Returned synchronously when a client sends `StatusQuery`.
    pub fn publish_status(&self) -> WitchStatus {
        let transaction = self.pending_transaction.as_ref().map(|txn| {
            TransactionSnapshot {
                label: txn.label.clone(),
                decision_count: txn.decision_count(),
                mutation_count: txn.mutation_count(),
                decision_keys: txn.keys(),
                decision_labels: txn
                    .decisions
                    .iter()
                    .map(|(k, d)| (k.clone(), d.label.clone()))
                    .collect(),
            }
        });

        WitchStatus {
            startup_state: self.startup_state,
            work: self.status(),
            reasoning_level: self.reasoning_level(),
            has_pending: self.has_pending(),
            is_initial_scanning: self.is_initial_scanning(),
            db_queue_depth: self.db_queue_depth(),
            transaction,
            handled_decision_kinds: self.handled_sources.clone(),
            is_external_fetch_active: self.is_external_fetch_active(),
            external_fetch_progress: self.external_fetch_progress().cloned(),
            has_acoustid_api_key: self.has_acoustid_api_key(),
            is_cover_art_fetch_active: self.is_cover_art_fetch_active(),
            cover_art_progress: self.cover_art_progress.clone(),
            is_deezer_fetch_active: self.is_deezer_fetch_active(),
            deezer_progress: self.deezer_progress.clone(),
            mutations_generation: self.mutations_generation,
            computations_generation: self.computations_generation,
            last_error: self.recent_errors.back().cloned(),
            error_generation: self.error_generation,
            config_generation: self.config_generation,
        }
    }

    /// Check if there's pending work (tasks queued or in-flight).
    pub fn has_pending(&self) -> bool {
        // Check rayon in-flight tasks
        if self.work_state.in_flight() > 0 {
            return true;
        }
        // Check db_thread queue
        if !self.db_thread_handle.queue_empty() {
            return true;
        }
        // Check pending mutation/computation pipeline phases
        if !self.pending_mutation_phases.is_empty()
            || !self.pending_soft_mutation_phases.is_empty()
            || !self.pending_computation_phases.is_empty()
        {
            return true;
        }
        false
    }

    /// Get pending DB write queue depth.
    pub fn db_queue_depth(&self) -> u64 {
        self.db_thread_handle.queue_depth()
    }

    /// Rebuild handled_sources from current transaction state.
    ///
    /// Called internally after transaction mutations (add, remove, confirm, discard).
    pub(crate) fn sync_handled_sources(&mut self) {
        self.handled_sources = self
            .pending_transaction
            .as_ref()
            .map(|txn| txn.decisions.keys().filter_map(|k| k.kind()).collect())
            .unwrap_or_default();
    }
}
