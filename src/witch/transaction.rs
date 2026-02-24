//! Transaction lifecycle management for witnessed decisions.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::{BTreeMap, HashMap};

use crate::meta::decisions::{
    ConfirmationGesture, DecisionKey, DiscardSummary, PendingTransaction, TransactionError,
    WitnessedDecision,
};
use crate::db::types::Zone;
use crate::meta::mutations::{Mutation, MutationExecutionStage, MutationStaging, TagOp};
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
use crate::meta::mutations::dir_config_edit::{ApplyBatchDirConfigEditsMutation, DirConfigEditEntry};

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
/// Coalesce ApplyDirConfigEdit mutations into a single batch write.
///
/// Multiple dir config edits targeting the same dirs.kdl file would race
/// when executed in parallel on rayon. This function:
/// 1. Extracts all ApplyDirConfigEdit mutations
/// 2. If there are 2+, merges them into a single ApplyBatchDirConfigEdits
/// 3. Builds a merged Config by overlaying each edit's changes
///
/// With only 0-1 dir config edits, returns mutations unchanged.
fn coalesce_dir_config_edits(mutations: Vec<Mutation>) -> Vec<Mutation> {
    let mut dir_edits: Vec<crate::meta::mutations::dir_config_edit::ApplyDirConfigEditMutation> = Vec::new();
    let mut other: Vec<Mutation> = Vec::new();

    for mutation in mutations {
        match mutation {
            Mutation::ApplyDirConfigEdit(m) => dir_edits.push(m),
            m => other.push(m),
        }
    }

    if dir_edits.len() <= 1 {
        // Nothing to coalesce — put back as-is
        for m in dir_edits {
            other.push(Mutation::ApplyDirConfigEdit(m));
        }
        return other;
    }

    // Build merged new_config: start from first, overlay subsequent edits
    let mut merged_config = dir_edits[0].new_config.clone();
    for edit in &dir_edits[1..] {
        let mut found = false;
        for sd in &mut merged_config.source_dirs {
            if sd.path == edit.source_path {
                *sd = edit.new_dir.clone();
                found = true;
                break;
            }
        }
        if !found {
            merged_config.source_dirs.push(edit.new_dir.clone());
        }
    }

    // Build batch entries
    let entries: Vec<DirConfigEditEntry> = dir_edits
        .into_iter()
        .map(|m| DirConfigEditEntry {
            source_path: m.source_path,
            old_dir: m.old_dir,
            new_dir: m.new_dir,
        })
        .collect();

    other.push(Mutation::ApplyBatchDirConfigEdits(
        ApplyBatchDirConfigEditsMutation {
            edits: entries,
            new_config: merged_config,
        },
    ));
    other
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
        self.sync_handled_sources();
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
    /// - `decision`: A `WitnessedDecision` (already carries gesture proof)
    ///
    /// Overwrites any existing decision at the same key.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), TransactionError> {
        let mutation_count = decision.mutations.len();

        self.require_active_transaction(&format!(
            "add_decision(key={}, label={:?}, mutations={})",
            key, decision.label, mutation_count
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] add_decision(key={}, label={:?}, mutations={}) OK - txn now has {} decisions",
            key, decision.label, mutation_count, txn.decision_count() + 1
        ));

        // Log each mutation for debugging
        for (i, m) in decision.mutations.iter().enumerate() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION]   mutation[{}]: {:?}",
                i, m
            ));
        }

        txn.decisions.insert(key, decision);

        self.sync_handled_sources();
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
        _gesture: &ConfirmationGesture,
    ) -> Result<(), TransactionError> {
        self.require_active_transaction(&format!("remove_decision(key={})", key))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        if txn.remove_decision(key).is_some() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] remove_decision(key={}) OK - txn now has {} decisions",
                key, txn.decision_count()
            ));
        }
        self.sync_handled_sources();
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
        _gesture: &ConfirmationGesture,
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
        self.sync_handled_sources();

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
        // Coalesce ApplyDirConfigEdit mutations into a single atomic dirs.kdl write
        let all_mutations = coalesce_dir_config_edits(all_mutations);

        crate::logging::log_mutation(format!(
            "[TRANSACTION] confirm_transaction OK - {} decisions, {} mutations (after coalescing: {})",
            decision_count, mutation_count, all_mutations.len()
        ));

        if all_mutations.is_empty() {
            crate::logging::log_mutation(
                "[TRANSACTION] confirm_transaction: no mutations to queue (empty transaction)"
            );
            return Ok(());
        }

        // Bucket mutations by execution stage (BTreeMap gives ordered iteration via Ord)
        let mut by_stage: BTreeMap<MutationExecutionStage, Vec<Mutation>> = BTreeMap::new();
        for mutation in all_mutations {
            let stage = match mutation.as_executor().staging() {
                MutationStaging::Staged(stage) => stage,
                MutationStaging::ChainEmitted => {
                    panic!(
                        "ChainEmitted mutation {:?} found in transaction — \
                         these are only spawned during execution, never directly staged",
                        mutation
                    );
                }
            };
            by_stage.entry(stage).or_default().push(mutation);
        }

        // Convert to ordered VecDeque of phases
        let mut phases: std::collections::VecDeque<(MutationExecutionStage, Vec<Mutation>)> =
            by_stage.into_iter().collect();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] Staged execution: {} phase(s): {:?}",
            phases.len(),
            phases.iter().map(|(s, m)| format!("{:?}({})", s, m.len())).collect::<Vec<_>>()
        ));

        // Generate a timestamped session label so tag_edit_history rows from this
        // transaction are grouped into a unique session (not one giant "Tag edit" bucket).
        let session_label = format!(
            "{} @ {}",
            txn.label,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );

        // Queue first phase immediately, stash remainder for drain-and-advance
        if let Some((stage, mutations)) = phases.pop_front() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] Queueing first phase: {:?} ({} mutations)",
                stage, mutations.len()
            ));
            self.pending_mutation_phases = phases;
            self.queue_mutations_internal(mutations, Some(session_label));
        }

        Ok(())
    }

    /// Discard the transaction - drop all accumulated decisions.
    ///
    /// Does not require a gesture — discarding is a safe, non-mutating operation.
    /// Any code that has access to the Witch can discard (cancel handlers, etc.).
    ///
    /// Returns summary of what was discarded.
    pub fn discard_transaction(
        &mut self,
    ) -> Result<DiscardSummary, TransactionError> {
        self.require_active_transaction("discard_transaction")?;

        let txn = self.pending_transaction.take().unwrap();
        self.sync_handled_sources();

        crate::logging::log_mutation(format!(
            "[TRANSACTION] discard_transaction OK - discarded {} decisions, {} mutations",
            txn.decision_count(), txn.mutation_count()
        ));

        Ok(DiscardSummary)
    }
}
