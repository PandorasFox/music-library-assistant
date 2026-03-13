//! Decision types - first-class meta concepts for operator decisions.
//!
//! A `Decision` is the serializable unit of operator intent: a label plus
//! the mutations it produces. Decisions are protocol-level objects that
//! cross the client/server boundary.

use serde::{Deserialize, Serialize};

use crate::mutations::Mutation;

// ============================================================================
// Decision Key Types
// ============================================================================

/// Semantic key for a decision within a transaction.
///
/// Each variant carries its own typed keying data, ensuring decisions from
/// different workflows can never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecisionKey {
    TagCanonicity {
        tag_name: String,
        cluster_index: usize,
    },
    CompoundSplitSafe {
        tag_name: String,
        cluster_index: usize,
    },
    CompoundSplitReview {
        tag_name: String,
        cluster_index: usize,
    },
    CompoundSplitInbox {
        tag_name: String,
        cluster_index: usize,
    },
    Deploy,
    DeploySidecars,
    TagEdit { key_item: String },
    OobSync,
    OobConflict,
    MtimeAck,
    MovedFile,
    MissingFile,
    MissingDirectory,
    CorruptFile,
    ShitFormat,
    SubparDuplicate,
    DirectoryCluster { cluster_index: usize },
    InboxCorpusMatch,
    InboxOrganize,
    MissingAlbum { group_index: usize },
    ManualReview { group_index: usize },
    IntakeIndex,
    DiscExtraction { group_index: usize },
    EditReversal { session_label: String },
    ConfigEdit,
    DirConfigEdit { source_path: std::path::PathBuf },
    MbReleaseApproval { release_id: String },
}

impl DecisionKey {
    /// If this key is a singleton (no per-item data), return its kind.
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
            DecisionKey::InboxCorpusMatch => Some(DecisionKeyKind::InboxCorpusMatch),
            DecisionKey::IntakeIndex => Some(DecisionKeyKind::IntakeIndex),
            _ => None,
        }
    }
}

impl std::fmt::Display for DecisionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionKey::TagCanonicity {
                tag_name,
                cluster_index,
            } => write!(f, "Tag Canonicity:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitSafe {
                tag_name,
                cluster_index,
            } => write!(f, "Compound Split Safe:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitReview {
                tag_name,
                cluster_index,
            } => write!(f, "Compound Split Review:{}:{}", tag_name, cluster_index),
            DecisionKey::CompoundSplitInbox {
                tag_name,
                cluster_index,
            } => write!(f, "Compound Split Inbox:{}:{}", tag_name, cluster_index),
            DecisionKey::Deploy => write!(f, "Deploy"),
            DecisionKey::DeploySidecars => write!(f, "Deploy Sidecars"),
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
            DecisionKey::DirectoryCluster { cluster_index } => {
                write!(f, "Directory Cluster:{}", cluster_index)
            }
            DecisionKey::InboxCorpusMatch => write!(f, "Inbox Corpus Match"),
            DecisionKey::InboxOrganize => write!(f, "Inbox Organize"),
            DecisionKey::MissingAlbum { group_index } => {
                write!(f, "Missing Album:{}", group_index)
            }
            DecisionKey::ManualReview { group_index } => {
                write!(f, "Manual Review:{}", group_index)
            }
            DecisionKey::IntakeIndex => write!(f, "Intake Index"),
            DecisionKey::DiscExtraction { group_index } => {
                write!(f, "Disc Extraction:{}", group_index)
            }
            DecisionKey::EditReversal { session_label } => {
                write!(f, "Edit Reversal:{}", session_label)
            }
            DecisionKey::ConfigEdit => write!(f, "Config Edit"),
            DecisionKey::DirConfigEdit { source_path } => {
                write!(f, "Dir Config Edit:{}", source_path.display())
            }
            DecisionKey::MbReleaseApproval { release_id } => {
                write!(f, "MB Release Approval:{}", release_id)
            }
        }
    }
}

/// Fieldless mirror of DecisionKey for insight filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    InboxCorpusMatch,
    IntakeIndex,
}

// ============================================================================
// Transaction Types
// ============================================================================

/// A decision with its associated pending mutations.
///
/// Serializable unit of operator intent that crosses the protocol boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// Human-readable label for this decision
    pub label: String,
    /// The mutations this decision will produce when committed
    pub mutations: Vec<Mutation>,
}

/// Errors that can occur during transaction operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionError {
    AlreadyActive,
    NoActiveTransaction,
    NotAcceptingMutations,
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionError::AlreadyActive => write!(f, "Transaction already active"),
            TransactionError::NoActiveTransaction => write!(f, "No active transaction"),
            TransactionError::NotAcceptingMutations => write!(
                f,
                "Not accepting mutations (eyeballing incomplete or read-only mode)"
            ),
        }
    }
}

impl std::error::Error for TransactionError {}

/// Summary returned when a transaction is discarded.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DiscardSummary;
