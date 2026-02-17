//! Transaction lifecycle management for witnessed decisions.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;

use crate::meta::decisions::{
    DecisionKey, DiscardSummary, PendingTransaction, TransactionError, WitnessedDecision,
};
use super::types::DecisionWitness;
use crate::corpus::db::types::Zone;
use crate::meta::mutations::{Mutation, TagOp};
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;

/// Coalesce ApplyTagOps mutations into per-zone mutations.
///
/// Multiple decisions may generate overlapping tag operations for the same inode.
/// This function:
/// 1. Extracts all TagOps from ApplyTagOps mutations, grouped by zone
/// 2. Deduplicates by (inode, tag_name, old_value) → last new_value wins
/// 3. Returns one coalesced ApplyTagOps per zone, plus other mutations unchanged
///
/// This ensures that if two signals affect the same track, both fixes are applied
/// rather than the later one clobbering the earlier.
fn coalesce_tag_ops(mutations: Vec<Mutation>) -> Vec<Mutation> {
    let mut ops_by_zone: HashMap<Zone, Vec<TagOp>> = HashMap::new();
    let mut other: Vec<Mutation> = Vec::new();

    for mutation in mutations {
        match mutation {
            Mutation::ApplyTagOps(m) => ops_by_zone.entry(m.zone).or_default().extend(m.ops),
            m => other.push(m),
        }
    }

    if ops_by_zone.is_empty() {
        return other;
    }

    let mut result = Vec::new();

    for (zone, all_ops) in ops_by_zone {
        // Deduplicate: (inode, tag_name, old_value) → last new_value wins
        let mut deduped: HashMap<(i64, String, Option<String>), Option<String>> = HashMap::new();
        for op in all_ops {
            if op.is_nop() {
                continue;
            }
            deduped.insert(
                (op.inode, op.tag_name.clone(), op.old_value.clone()),
                op.new_value,
            );
        }

        let final_ops: Vec<TagOp> = deduped
            .into_iter()
            .map(|((inode, tag_name, old_value), new_value)| TagOp {
                inode,
                tag_name,
                old_value,
                new_value,
            })
            .collect();

        if !final_ops.is_empty() {
            result.push(Mutation::ApplyTagOps(ApplyTagOpsMutation { ops: final_ops, zone }));
        }
    }

    // Tag ops before other mutations
    result.extend(other);
    result
}
impl super::Witch {
    // -------------------------------------------------------------------------
    // Transaction Helpers
    // -------------------------------------------------------------------------

    /// Require an active transaction, returning error if none exists.
    pub(super) fn require_active_transaction(&self, operation: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_none() {
            crate::logging::log_mutation(format!(
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
            crate::logging::log_mutation(format!(
                "[TRANSACTION] start_transaction({:?}) REJECTED: already active",
                label
            ));
            return Err(TransactionError::AlreadyActive);
        }

        crate::logging::log_mutation(format!(
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

    /// Get a summary of the pending transaction for UI display.
    ///
    /// Returns None if no transaction is active.
    pub fn transaction_summary(&self) -> Option<(&str, usize, usize)> {
        self.pending_transaction.as_ref().map(|txn| {
            (txn.label.as_str(), txn.decision_count(), txn.mutation_count())
        })
    }

    /// Add a witnessed decision to the transaction.
    ///
    /// - `key`: Semantic key identifying the decision source and item
    /// - `witness`: Proof of operator confirmation
    /// - `label`: Human-readable description
    /// - `mutations`: The mutations this decision represents
    ///
    /// Overwrites any existing decision at the same key.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        key: DecisionKey,
        _witness: &DecisionWitness,
        label: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<(), TransactionError> {
        let label_str: String = label.into();
        let mutation_count = mutations.len();

        self.require_active_transaction(&format!(
            "add_decision(key={}, label={:?}, mutations={})",
            key, label_str, mutation_count
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] add_decision(key={}, label={:?}, mutations={}) OK - txn now has {} decisions",
            key, label_str, mutation_count, txn.decision_count() + 1
        ));

        // Log each mutation for debugging
        for (i, m) in mutations.iter().enumerate() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION]   mutation[{}]: {:?}",
                i, m
            ));
        }

        txn.decisions.insert(
            key,
            WitnessedDecision {
                label: label_str,
                mutations,
            },
        );

        Ok(())
    }

    /// Fetch a decision by key.
    ///
    /// Returns None if no decision stored at that key.
    pub fn get_decision(&self, key: &DecisionKey) -> Option<&WitnessedDecision> {
        self.pending_transaction
            .as_ref()
            .and_then(|txn| txn.decisions.get(key))
    }

    /// List all decision keys in the current transaction.
    pub fn decision_keys(&self) -> Vec<DecisionKey> {
        self.pending_transaction
            .as_ref()
            .map(|txn| txn.keys())
            .unwrap_or_default()
    }

    /// Remove an entire decision from the active transaction.
    pub fn remove_decision(
        &mut self,
        key: &DecisionKey,
        _witness: &DecisionWitness,
    ) -> Result<(), TransactionError> {
        self.require_active_transaction(&format!("remove_decision(key={})", key))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        if txn.remove_decision(key).is_some() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] remove_decision(key={}) OK - txn now has {} decisions",
                key, txn.decision_count()
            ));
        }
        Ok(())
    }

    /// Remove a single mutation from a decision in the active transaction.
    /// Auto-removes the decision if no mutations remain.
    pub fn remove_mutation_from_decision(
        &mut self,
        key: &DecisionKey,
        mutation_idx: usize,
        _witness: &DecisionWitness,
    ) -> Result<(), TransactionError> {
        self.require_active_transaction(&format!(
            "remove_mutation(key={}, idx={})", key, mutation_idx
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        if txn.remove_mutation(key, mutation_idx) {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] remove_mutation(key={}, idx={}) OK - txn now has {} decisions",
                key, mutation_idx, txn.decision_count()
            ));
        }
        Ok(())
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
    ) -> Result<(), TransactionError> {
        // Gate: mutations must be accepted (eye is Awake, not read-only)
        if !self.accepting_mutations() {
            crate::logging::log_mutation(
                "[TRANSACTION] confirm_transaction REJECTED: not accepting mutations (eyeballing incomplete or read-only mode)"
            );
            return Err(TransactionError::NotAcceptingMutations);
        }

        self.require_active_transaction("confirm_transaction")?;

        let txn = self.pending_transaction.take().unwrap();

        let decision_count = txn.decision_count();
        let mut mutation_count = 0;

        // Collect all mutations from all decisions
        let raw_mutations: Vec<Mutation> = txn
            .decisions
            .into_values()
            .flat_map(|d| {
                mutation_count += d.mutations.len();
                d.mutations
            })
            .collect();

        // Coalesce ApplyTagOps mutations to handle overlapping edits from multiple signals
        let all_mutations = coalesce_tag_ops(raw_mutations);

        crate::logging::log_mutation(format!(
            "[TRANSACTION] confirm_transaction OK - {} decisions, {} mutations (after coalescing: {})",
            decision_count, mutation_count, all_mutations.len()
        ));

        // Queue mutations for execution
        if !all_mutations.is_empty() {
            self.queue_mutations_internal(all_mutations, Some(txn.label));
        } else {
            crate::logging::log_mutation(
                "[TRANSACTION] confirm_transaction: no mutations to queue (empty transaction)"
            );
        }

        Ok(())
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

        crate::logging::log_mutation(format!(
            "[TRANSACTION] discard_transaction OK - discarded {} decisions, {} mutations",
            txn.decision_count(), txn.mutation_count()
        ));

        Ok(DiscardSummary)
    }
}
