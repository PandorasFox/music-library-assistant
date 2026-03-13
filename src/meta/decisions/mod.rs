//! Decision types - re-exported from mm-meta with server-only PendingTransaction.

use std::collections::HashMap;

// Re-export all protocol-level decision types from mm-meta
pub use mm_meta::decisions::{
    Decision, DecisionKey, DecisionKeyKind, DiscardSummary, TransactionError,
};

// ============================================================================
// Server-only: PendingTransaction (in-memory state, not serialized)
// ============================================================================

/// An active transaction accumulating decisions.
///
/// Transactions are ephemeral in-memory state. They are NOT persisted to disk.
/// Only one transaction may be active at a time.
#[derive(Debug)]
pub struct PendingTransaction {
    /// Human-readable label for this transaction
    pub label: String,
    /// Accumulated decisions keyed by semantic DecisionKey.
    pub(crate) decisions: HashMap<DecisionKey, Decision>,
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

    /// Get all decision keys (sorted by Display representation for stable ordering).
    pub fn keys(&self) -> Vec<DecisionKey> {
        let mut keys: Vec<_> = self.decisions.keys().cloned().collect();
        keys.sort_by_key(|a| a.to_string());
        keys
    }

    /// Remove an entire decision. Returns the removed decision if it existed.
    pub fn remove_decision(&mut self, key: &DecisionKey) -> Option<Decision> {
        self.decisions.remove(key)
    }
}
