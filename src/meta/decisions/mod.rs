//! Decision types - first-class meta concepts for operator decisions.
//!
//! Contains the data types for witnessed decisions and transactions.
//! The `ConfirmationGesture` type is defined in `ui/action_handlers/witness.rs`
//! and re-exported here for use in `WitnessedDecision`.

use std::collections::HashMap;

use crate::meta::mutations::Mutation;
pub(crate) use crate::ui::action_handlers::witness::ConfirmationGesture;

// ============================================================================
// Decision Key Types
// ============================================================================

/// Semantic key for a decision within a transaction.
///
/// Each variant carries its own typed keying data, ensuring decisions from
/// different workflows can never collide. Singleton variants (no inner data)
/// represent bulk operations where one decision handles the entire insight type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DecisionKey {
    // === Zone-scoped file signal decisions ===

    /// Tag canonicity resolution (cluster index within tag_name)
    TagCanonicity { tag_name: String, cluster_index: usize },
    /// Compound tag split — safe (all parts exist in corpus)
    CompoundSplitSafe { tag_name: String, cluster_index: usize },
    /// Compound tag split — review (some parts new to corpus)
    CompoundSplitReview { tag_name: String, cluster_index: usize },
    /// Compound tag split — inbox
    CompoundSplitInbox { tag_name: String, cluster_index: usize },
    /// Deploy operations (single decision per transaction)
    Deploy,
    /// Tag edit (keyed by inode set + tag names for dedup)
    TagEdit { key_item: String },
    /// OOB tag sync (bulk, single decision)
    OobSync,
    /// OOB tag conflict (bulk, single decision)
    OobConflict,
    /// Mtime-only ack (bulk, single decision)
    MtimeAck,
    /// Moved file resolution (bulk, single decision)
    MovedFile,
    /// Missing file resolution (bulk, single decision)
    MissingFile,
    /// Missing directory resolution (bulk, single decision)
    MissingDirectory,
    /// Corrupt file resolution (bulk, single decision)
    CorruptFile,
    /// Shit format transcode (bulk, single decision)
    ShitFormat,
    /// Subpar duplicate stash (bulk, single decision)
    SubparDuplicate,
    /// Directory cluster overlap resolution (per cluster)
    DirectoryCluster { cluster_index: usize },
    /// Album art work — embed + upgrade (bulk, single decision)
    AlbumArt,
    /// Inbox corpus match stash (bulk, single decision)
    InboxCorpusMatch,
    /// Inbox organize into corpus (bulk, single decision)
    InboxOrganize,
    /// Missing album single resolution (per group)
    MissingAlbum { group_index: usize },
    /// Manual review resolution (per group)
    ManualReview { group_index: usize },
    /// Intake/index unindexed files (bulk, single decision)
    IntakeIndex,
    /// External match tag acceptance (per inode)
    ExternalMatch { inode: i64 },
    /// Disc extraction resolution (per group)
    DiscExtraction { group_index: usize },
    /// Edit reversal from history (per session)
    EditReversal { session_label: String },

    // === Meta-level decisions (not file-scoped) ===

    /// Config edit (app-level config change)
    ConfigEdit,
    /// Directory config edit (per source dir path)
    DirConfigEdit { source_path: std::path::PathBuf },
}

impl DecisionKey {
    /// If this key is a singleton (no per-item data), return its kind.
    ///
    /// Used by the insights view to determine which insight types are
    /// fully handled by a single staged decision.
    pub fn kind(&self) -> Option<DecisionKeyKind> {
        match self {
            DecisionKey::OobSync => Some(DecisionKeyKind::OobSync),
            DecisionKey::OobConflict => Some(DecisionKeyKind::OobConflict),
            DecisionKey::MtimeAck => Some(DecisionKeyKind::MtimeAck),
            DecisionKey::MovedFile => Some(DecisionKeyKind::MovedFile),
            DecisionKey::MissingFile => Some(DecisionKeyKind::MissingFile),
            DecisionKey::MissingDirectory => Some(DecisionKeyKind::MissingDirectory),
            DecisionKey::CorruptFile => Some(DecisionKeyKind::CorruptFile),
            DecisionKey::ShitFormat => Some(DecisionKeyKind::ShitFormat),
            DecisionKey::SubparDuplicate => Some(DecisionKeyKind::SubparDuplicate),
            DecisionKey::AlbumArt => Some(DecisionKeyKind::AlbumArt),
            DecisionKey::InboxCorpusMatch => Some(DecisionKeyKind::InboxCorpusMatch),
            DecisionKey::IntakeIndex => Some(DecisionKeyKind::IntakeIndex),
            _ => None,
        }
    }
}

impl std::fmt::Display for DecisionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionKey::TagCanonicity { tag_name, cluster_index } =>
                write!(f, "Tag Canonicity:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitSafe { tag_name, cluster_index } =>
                write!(f, "Compound Split Safe:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitReview { tag_name, cluster_index } =>
                write!(f, "Compound Split Review:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitInbox { tag_name, cluster_index } =>
                write!(f, "Compound Split Inbox:{}:{}", tag_name, cluster_index),
            DecisionKey::Deploy => write!(f, "Deploy"),
            DecisionKey::TagEdit { key_item } => write!(f, "Tag Edit:{}", key_item),
            DecisionKey::OobSync => write!(f, "OOB Sync"),
            DecisionKey::OobConflict => write!(f, "OOB Conflict"),
            DecisionKey::MtimeAck => write!(f, "Mtime Ack"),
            DecisionKey::MovedFile => write!(f, "Moved File"),
            DecisionKey::MissingFile => write!(f, "Missing File"),
            DecisionKey::MissingDirectory => write!(f, "Missing Directory"),
            DecisionKey::CorruptFile => write!(f, "Corrupt File"),
            DecisionKey::ShitFormat => write!(f, "Format Conversion"),
            DecisionKey::SubparDuplicate => write!(f, "Subpar Duplicate"),
            DecisionKey::DirectoryCluster { cluster_index } =>
                write!(f, "Directory Cluster:{}", cluster_index),
            DecisionKey::AlbumArt => write!(f, "Embed Album Art"),
            DecisionKey::InboxCorpusMatch => write!(f, "Inbox Corpus Match"),
            DecisionKey::InboxOrganize => write!(f, "Inbox Organize"),
            DecisionKey::MissingAlbum { group_index } =>
                write!(f, "Missing Album:{}", group_index),
            DecisionKey::ManualReview { group_index } =>
                write!(f, "Manual Review:{}", group_index),
            DecisionKey::IntakeIndex => write!(f, "Intake Index"),
            DecisionKey::ExternalMatch { inode } =>
                write!(f, "External Match:{}", inode),
            DecisionKey::DiscExtraction { group_index } =>
                write!(f, "Disc Extraction:{}", group_index),
            DecisionKey::EditReversal { session_label } =>
                write!(f, "Edit Reversal:{}", session_label),
            DecisionKey::ConfigEdit => write!(f, "Config Edit"),
            DecisionKey::DirConfigEdit { source_path } =>
                write!(f, "Dir Config Edit:{}", source_path.display()),
        }
    }
}

/// Fieldless mirror of DecisionKey for insight filtering.
///
/// Only singleton variants (no inner data) are represented. Used by the
/// insights view to hide entries whose single-decision source is already
/// staged in the active transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionKeyKind {
    OobSync,
    OobConflict,
    MtimeAck,
    MovedFile,
    MissingFile,
    MissingDirectory,
    CorruptFile,
    ShitFormat,
    SubparDuplicate,
    AlbumArt,
    InboxCorpusMatch,
    IntakeIndex,
}

// ============================================================================
// Transaction Types
// ============================================================================

/// A witnessed decision with its associated pending mutations.
///
/// Decisions are accumulated in a transaction and committed together.
/// The private `_gesture` field ensures this can only be constructed by
/// code that possesses a `ConfirmationGesture` token.
pub struct WitnessedDecision {
    /// Human-readable label for this decision
    pub label: String,
    /// The mutations this decision will produce when committed
    pub mutations: Vec<Mutation>,
    /// Proof that an operator confirmation gesture authorized this decision.
    _gesture: ConfirmationGesture,
}

impl WitnessedDecision {
    pub fn new(label: impl Into<String>, mutations: Vec<Mutation>, gesture: &ConfirmationGesture) -> Self {
        Self {
            label: label.into(),
            mutations,
            _gesture: *gesture,
        }
    }
}

impl std::fmt::Debug for WitnessedDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WitnessedDecision")
            .field("label", &self.label)
            .field("mutations", &self.mutations)
            .finish()
    }
}

impl Clone for WitnessedDecision {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            mutations: self.mutations.clone(),
            _gesture: self._gesture,
        }
    }
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
        keys.sort_by_key(|a| a.to_string());
        keys
    }

    /// Remove an entire decision. Returns the removed decision if it existed.
    pub fn remove_decision(&mut self, key: &DecisionKey) -> Option<WitnessedDecision> {
        self.decisions.remove(key)
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
