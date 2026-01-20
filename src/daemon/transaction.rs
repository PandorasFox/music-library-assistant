//! Transaction lifecycle management for witnessed decisions.
//!
//! This module is part of the daemon subsystem. See `daemon/mod.rs` for overview.

use super::types::{
    CommitSummary, DecisionWitness, DiscardSummary, PendingTransaction,
    TransactionError, TransactionInfo, WitnessedDecision,
};
use crate::corpus::mutations::Mutation;
use crate::config;

impl super::TaskDaemon {
    // -------------------------------------------------------------------------
    // Transaction Helpers
    // -------------------------------------------------------------------------

    /// Require an active transaction, returning error if none exists.
    pub(super) fn require_active_transaction(&self, operation: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_none() {
            let _ = config::log_message(&format!(
                "[TRANSACTION] {} REJECTED: no active transaction",
                operation
            ));
            Err(TransactionError::NoActiveTransaction)
        } else {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Transaction API
    // -------------------------------------------------------------------------

    /// Start a new transaction.
    ///
    /// Called by modals when the user is about to be presented with Decisions.
    /// Only one transaction may be active at a time.
    ///
    /// Returns Err if a transaction is already active.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_some() {
            let _ = config::log_message(&format!(
                "[TRANSACTION] start_transaction({:?}) REJECTED: already active",
                label
            ));
            return Err(TransactionError::AlreadyActive);
        }

        let _ = config::log_message(&format!(
            "[TRANSACTION] start_transaction({:?}) OK",
            label
        ));
        self.pending_transaction = Some(PendingTransaction::new(label));
        Ok(())
    }

    /// Check if a transaction is currently active.
    pub fn has_transaction(&self) -> bool {
        self.pending_transaction.is_some()
    }

    /// Get transaction info for UI display.
    pub fn transaction_info(&self) -> Option<TransactionInfo> {
        self.pending_transaction.as_ref().map(|txn| TransactionInfo {
            label: txn.label.clone(),
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
            started_at: txn.started_at,
        })
    }

    /// Add a witnessed decision to the transaction.
    ///
    /// - `idx`: UI-provided index (may have gaps, largely sequential)
    /// - `witness`: Proof of operator confirmation
    /// - `label`: Human-readable description
    /// - `mutations`: The mutations this decision represents
    ///
    /// Overwrites any existing decision at the same index.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        idx: usize,
        _witness: &DecisionWitness,
        label: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<(), TransactionError> {
        let label_str: String = label.into();
        let mutation_count = mutations.len();

        self.require_active_transaction(&format!(
            "add_decision(idx={}, label={:?}, mutations={})",
            idx, label_str, mutation_count
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();

        let _ = config::log_message(&format!(
            "[TRANSACTION] add_decision(idx={}, label={:?}, mutations={}) OK - txn now has {} decisions",
            idx, label_str, mutation_count, txn.decision_count() + 1
        ));

        // Log each mutation for debugging
        for (i, m) in mutations.iter().enumerate() {
            let _ = config::log_message(&format!(
                "[TRANSACTION]   mutation[{}]: {:?}",
                i, m
            ));
        }

        txn.decisions.insert(
            idx,
            WitnessedDecision {
                label: label_str,
                mutations,
            },
        );

        Ok(())
    }

    /// Discard a single decision by index.
    ///
    /// Requires a witness - discarding is also a decision.
    /// Used primarily for review screen before confirm/discard.
    ///
    /// Returns the discarded decision, or None if no decision at that index.
    pub fn discard_decision(
        &mut self,
        idx: usize,
        _witness: &DecisionWitness,
    ) -> Result<Option<WitnessedDecision>, TransactionError> {
        self.require_active_transaction(&format!("discard_decision(idx={})", idx))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        Ok(txn.decisions.remove(&idx))
    }

    /// Fetch a decision by index.
    ///
    /// Returns None if no decision stored at that index.
    pub fn get_decision(&self, idx: usize) -> Option<&WitnessedDecision> {
        self.pending_transaction
            .as_ref()
            .and_then(|txn| txn.decisions.get(&idx))
    }

    /// List all decision indices in the current transaction.
    pub fn decision_indices(&self) -> Vec<usize> {
        self.pending_transaction
            .as_ref()
            .map(|txn| txn.indices())
            .unwrap_or_default()
    }

    /// Confirm the transaction - queue all mutations for execution.
    ///
    /// This is the primary way to add mutations to the execution queue.
    /// Requires a witness for the commit decision itself.
    ///
    /// Returns summary of what was committed, or error if mutations not accepted.
    pub fn confirm_transaction(
        &mut self,
        _witness: &DecisionWitness,
    ) -> Result<CommitSummary, TransactionError> {
        // Gate: mutations must be accepted (eyeballing complete, not read-only)
        if !self.accepting_mutations {
            let _ = config::log_message(
                "[TRANSACTION] confirm_transaction REJECTED: not accepting mutations (eyeballing incomplete or read-only mode)"
            );
            return Err(TransactionError::NotAcceptingMutations);
        }

        self.require_active_transaction("confirm_transaction")?;

        let txn = self.pending_transaction.take().unwrap();

        let decision_count = txn.decision_count();
        let mut mutation_count = 0;

        // Collect all mutations from all decisions
        let all_mutations: Vec<Mutation> = txn
            .decisions
            .into_values()
            .flat_map(|d| {
                mutation_count += d.mutations.len();
                d.mutations
            })
            .collect();

        let _ = config::log_message(&format!(
            "[TRANSACTION] confirm_transaction OK - {} decisions, {} mutations queued",
            decision_count, mutation_count
        ));

        // Queue mutations for execution
        if !all_mutations.is_empty() {
            self.queue_mutations_internal(all_mutations, Some(txn.label));
        } else {
            let _ = config::log_message(
                "[TRANSACTION] confirm_transaction: no mutations to queue (empty transaction)"
            );
        }

        Ok(CommitSummary {
            decision_count,
            mutation_count,
        })
    }

    /// Discard the transaction - drop all accumulated decisions.
    ///
    /// Requires a witness - discarding is also a decision.
    ///
    /// Returns summary of what was discarded.
    pub fn discard_transaction(
        &mut self,
        _witness: &DecisionWitness,
    ) -> Result<DiscardSummary, TransactionError> {
        self.require_active_transaction("discard_transaction")?;

        let txn = self.pending_transaction.take().unwrap();

        let _ = config::log_message(&format!(
            "[TRANSACTION] discard_transaction OK - discarded {} decisions, {} mutations",
            txn.decision_count(), txn.mutation_count()
        ));

        Ok(DiscardSummary {
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
        })
    }
}
