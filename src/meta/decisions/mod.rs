//! Decision types - first-class meta concepts for operator decisions.
//!
//! Contains the data types for witnessed decisions and transactions.
//! Authority types (`DecisionWitness`, `DecisionScope`) remain in `witch/types.rs` -
//! the Witch owns the authority mechanism, while these types represent the data.

use std::collections::HashMap;

use crate::meta::mutations::Mutation;

// ============================================================================
// Transaction Types
// ============================================================================

/// A witnessed decision with its associated pending mutations.
///
/// Decisions are accumulated in a transaction and committed together.
#[derive(Debug, Clone)]
pub struct WitnessedDecision {
    /// Human-readable label for this decision
    pub label: String,
    /// The mutations this decision will produce when committed
    pub mutations: Vec<Mutation>,
}

/// An active transaction accumulating decisions.
///
/// Transactions are ephemeral in-memory state. They are NOT persisted to disk.
/// Only one transaction may be active at a time.
#[derive(Debug)]
pub struct PendingTransaction {
    /// Human-readable label for this transaction
    pub label: String,
    /// Accumulated decisions by index (UI-provided, may have gaps)
    pub(crate) decisions: HashMap<usize, WitnessedDecision>,
}

impl PendingTransaction {
    /// Create a new empty transaction.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            decisions: HashMap::new(),
        }
    }

    /// Count of stored decisions.
    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }

    /// Total mutations across all decisions.
    pub fn mutation_count(&self) -> usize {
        self.decisions.values().map(|d| d.mutations.len()).sum()
    }

    /// Get all decision indices (sorted).
    pub fn indices(&self) -> Vec<usize> {
        let mut indices: Vec<_> = self.decisions.keys().copied().collect();
        indices.sort();
        indices
    }
}

/// Errors that can occur during transaction operations.
#[derive(Debug, Clone)]
pub enum TransactionError {
    /// Attempted to start a transaction when one is already active.
    AlreadyActive,
    /// Attempted an operation requiring an active transaction.
    NoActiveTransaction,
    /// Attempted to confirm transaction but the Witch is not accepting mutations.
    /// This happens if eyeballing hasn't completed or read-only mode is enabled.
    NotAcceptingMutations,
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionError::AlreadyActive => write!(f, "Transaction already active"),
            TransactionError::NoActiveTransaction => write!(f, "No active transaction"),
            TransactionError::NotAcceptingMutations => write!(f, "Not accepting mutations (eyeballing incomplete or read-only mode)"),
        }
    }
}

impl std::error::Error for TransactionError {}

/// Summary returned when a transaction is discarded.
///
/// Unit struct - callers typically `let _ = discard_transaction(...)` since
/// discarding always succeeds and the details aren't needed.
#[derive(Debug, Clone, Copy)]
pub struct DiscardSummary;
