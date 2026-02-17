//! Decision types - first-class meta concepts for operator decisions.
//!
//! Contains the data types for witnessed decisions and transactions.
//! Authority types (`DecisionWitness`, `DecisionScope`) remain in `witch/types.rs` -
//! the Witch owns the authority mechanism, while these types represent the data.

use std::collections::HashMap;

use crate::meta::mutations::Mutation;

// ============================================================================
// Decision Key Types
// ============================================================================

/// Source workflow for a decision — which modal/resolution flow produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionSource {
    TagCanonicity,
    CompoundSplit,
    Deploy,
    ConfigEdit,
    TagEdit,
    OobSync,
    OobConflict,
    MtimeAck,
    MovedFile,
    MissingFile,
    MissingDirectory,
    CorruptFile,
    ShitFormat,
    SubparDuplicate,
    DirectoryCluster,
    EmbedAlbumArt,
    InboxCorpusMatch,
    InboxOrganize,
    MissingAlbum,
    ManualReview,
    IntakeIndex,
}

impl std::fmt::Display for DecisionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TagCanonicity => write!(f, "Tag Canonicity"),
            Self::CompoundSplit => write!(f, "Compound Split"),
            Self::Deploy => write!(f, "Deploy"),
            Self::ConfigEdit => write!(f, "Config Edit"),
            Self::TagEdit => write!(f, "Tag Edit"),
            Self::OobSync => write!(f, "OOB Sync"),
            Self::OobConflict => write!(f, "OOB Conflict"),
            Self::MtimeAck => write!(f, "Mtime Ack"),
            Self::MovedFile => write!(f, "Moved File"),
            Self::MissingFile => write!(f, "Missing File"),
            Self::MissingDirectory => write!(f, "Missing Directory"),
            Self::CorruptFile => write!(f, "Corrupt File"),
            Self::ShitFormat => write!(f, "Format Conversion"),
            Self::SubparDuplicate => write!(f, "Subpar Duplicate"),
            Self::DirectoryCluster => write!(f, "Directory Cluster"),
            Self::EmbedAlbumArt => write!(f, "Embed Album Art"),
            Self::InboxCorpusMatch => write!(f, "Inbox Corpus Match"),
            Self::InboxOrganize => write!(f, "Inbox Organize"),
            Self::MissingAlbum => write!(f, "Missing Album"),
            Self::ManualReview => write!(f, "Manual Review"),
            Self::IntakeIndex => write!(f, "Intake Index"),
        }
    }
}

/// Semantic key for a decision within a transaction.
///
/// Replaces the old `usize` index. The `source` identifies which workflow
/// produced the decision, and `item` provides per-source uniqueness (signal
/// key, inode, cluster id, etc.). Single-decision flows use `"0"` for item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecisionKey {
    pub source: DecisionSource,
    /// Item identifier within source (signal key, inode, cluster id, etc.)
    /// Single-decision flows use "0".
    pub item: String,
}

impl DecisionKey {
    pub fn new(source: DecisionSource, item: impl Into<String>) -> Self {
        Self { source, item: item.into() }
    }

    /// Convenience constructor for single-decision flows.
    pub fn single(source: DecisionSource) -> Self {
        Self { source, item: "0".into() }
    }
}

impl std::fmt::Display for DecisionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.item == "0" {
            write!(f, "{}", self.source)
        } else {
            write!(f, "{}:{}", self.source, self.item)
        }
    }
}

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
    /// Accumulated decisions keyed by semantic DecisionKey.
    pub(crate) decisions: HashMap<DecisionKey, WitnessedDecision>,
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
        keys.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
        keys
    }

    /// Get a decision by key.
    pub fn get(&self, key: &DecisionKey) -> Option<&WitnessedDecision> {
        self.decisions.get(key)
    }

    /// Get a mutable decision by key.
    pub fn get_mut(&mut self, key: &DecisionKey) -> Option<&mut WitnessedDecision> {
        self.decisions.get_mut(key)
    }

    /// Remove an entire decision. Returns the removed decision if it existed.
    pub fn remove_decision(&mut self, key: &DecisionKey) -> Option<WitnessedDecision> {
        self.decisions.remove(key)
    }

    /// Remove a single mutation from a decision by mutation index.
    /// Auto-removes the decision if no mutations remain.
    /// Returns true if the mutation was removed.
    pub fn remove_mutation(&mut self, key: &DecisionKey, mutation_idx: usize) -> bool {
        let should_remove_decision = if let Some(decision) = self.decisions.get_mut(key) {
            if mutation_idx < decision.mutations.len() {
                decision.mutations.remove(mutation_idx);
                decision.mutations.is_empty()
            } else {
                return false;
            }
        } else {
            return false;
        };

        if should_remove_decision {
            self.decisions.remove(key);
        }
        true
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
