//! Decision types - first-class meta concepts for operator decisions.
//!
//! A `Decision` is the serializable unit of operator intent: a label plus
//! the mutations it produces. Decisions are protocol-level objects that
//! cross the client/server boundary.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::mutations::{Mutation, MutationKind};
use crate::views::ConflictBucket;

// ============================================================================
// Decision Key Types
// ============================================================================

/// Semantic key for a decision within a transaction.
///
/// Each variant carries its own typed keying data, ensuring decisions from
/// different workflows can never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
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
    Deploy,
    DeploySidecars,
    TagEdit { key_item: String },
    OobResolution { bucket: ConflictBucket },
    MovedFile,
    MissingFile,
    MissingDirectory,
    CorruptFile,
    LosslessRemux,
    SubparDuplicate,
    DirectoryCluster { cluster_index: usize },
    MissingAlbum { group_index: usize },
    ManualReview { group_index: usize },
    DiscExtraction { group_index: usize },
    EditReversal { session_label: String },
    ConfigEdit,
    ArtistPluralNormalization,
    DirConfigEdit { source_path: std::path::PathBuf },
    MbReleaseApproval { release_id: String },
    JettisonEditHistory,
}

impl DecisionKey {
    /// If this key is a singleton (no per-item data), return its kind.
    pub fn kind(&self) -> Option<DecisionKeyKind> {
        match self {
            DecisionKey::OobResolution { .. } => Some(DecisionKeyKind::OobResolution),
            DecisionKey::MovedFile => Some(DecisionKeyKind::MovedFile),
            DecisionKey::MissingFile => Some(DecisionKeyKind::MissingFile),
            DecisionKey::MissingDirectory => Some(DecisionKeyKind::MissingDirectory),
            DecisionKey::CorruptFile => Some(DecisionKeyKind::CorruptFile),
            DecisionKey::ArtistPluralNormalization => Some(DecisionKeyKind::ArtistPluralNormalization),
            DecisionKey::LosslessRemux => Some(DecisionKeyKind::LosslessRemux),
            DecisionKey::SubparDuplicate => Some(DecisionKeyKind::SubparDuplicate),
            _ => None,
        }
    }

    /// Which mutation kinds are valid for this decision type.
    ///
    /// Exhaustive match forces update when DecisionKey variants are added.
    /// Server-side validation uses this to debug_assert that staged decisions
    /// contain only allowed mutation kinds.
    pub fn allowed_mutation_kinds(&self) -> &'static [MutationKind] {
        use MutationKind::*;
        match self {
            // Tag canonicity → emit whitelist signals + tag ops
            DecisionKey::TagCanonicity { .. } => &[EmitCanonicalTag, ApplyTagOps],
            // Compound splits → tag ops (splitting compound tag values)
            DecisionKey::CompoundSplitSafe { .. }
            | DecisionKey::CompoundSplitReview { .. } => &[ApplyTagOps],
            // Deploy → hard links + library moves + stash leftovers
            DecisionKey::Deploy => &[HardLink, LibraryMove, StashLeftovers],
            // Deploy sidecars → hard links
            DecisionKey::DeploySidecars => &[HardLink],
            // Tag edit → tag ops + DB→disk sync
            DecisionKey::TagEdit { .. } => &[ApplyTagOps, ApplyDbTagsToDisk],
            // OOB resolution: mtime-only → ack, all others → tag sync direction
            DecisionKey::OobResolution { bucket: ConflictBucket::MtimeOnly } => &[AcknowledgeMtimeOnly],
            DecisionKey::OobResolution { .. } => &[ApplyDbTagsToDisk, AssimilateDiskTagsToDb],
            // Moved file → update path in index
            DecisionKey::MovedFile => &[UpdateFilePath],
            // Missing file → drop from index or stash
            DecisionKey::MissingFile => &[DropFromIndex, IndexFileFromPath],
            // Missing directory → drop directory from index
            DecisionKey::MissingDirectory => &[DropDirectoryFromIndex],
            // Corrupt file → stash from zone + drop from index
            DecisionKey::CorruptFile => &[StashFromZone, DropFromIndex],
            // Artist plural normalization → tag ops (restructure singular/plural artist tags)
            DecisionKey::ArtistPluralNormalization => &[ApplyTagOps],
            // Lossless remux → transcode to FLAC
            DecisionKey::LosslessRemux => &[Transcode],
            // Subpar duplicate → stash from zone + drop from index
            DecisionKey::SubparDuplicate => &[StashFromZone, DropFromIndex],
            // Directory cluster → tag ops (organize directory structure)
            DecisionKey::DirectoryCluster { .. } => &[ApplyTagOps],
            // Missing album → tag ops (fill missing album/artist tags)
            DecisionKey::MissingAlbum { .. } => &[ApplyTagOps],
            // Manual review → tag ops
            DecisionKey::ManualReview { .. } => &[ApplyTagOps],
            // Disc extraction → tag ops (disc number assignments)
            DecisionKey::DiscExtraction { .. } => &[ApplyTagOps],
            // Edit reversal → tag ops (undo previous tag edits)
            DecisionKey::EditReversal { .. } => &[ApplyTagOps],
            // Config edit → apply config edits
            DecisionKey::ConfigEdit => &[ApplyConfigEdits],
            // Dir config edit → apply dir config edits or batch edits
            DecisionKey::DirConfigEdit { .. } => &[ApplyDirConfigEdit, ApplyBatchDirConfigEdits],
            // MB release approval → tag ops + dir config edit
            DecisionKey::MbReleaseApproval { .. } => &[ApplyTagOps, ApplyDirConfigEdit],
            // Jettison edit history → export to file (chain-emits clear)
            DecisionKey::JettisonEditHistory => &[ExportEditHistory],
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
            DecisionKey::Deploy => write!(f, "Deploy"),
            DecisionKey::DeploySidecars => write!(f, "Deploy Sidecars"),
            DecisionKey::TagEdit { key_item } => write!(f, "Tag Edit:{}", key_item),
            DecisionKey::OobResolution { bucket } => write!(f, "OOB Resolution:{}", bucket.label()),
            DecisionKey::MovedFile => write!(f, "Moved File"),
            DecisionKey::MissingFile => write!(f, "Missing File"),
            DecisionKey::MissingDirectory => write!(f, "Missing Directory"),
            DecisionKey::CorruptFile => write!(f, "Corrupt File"),
            DecisionKey::ArtistPluralNormalization => write!(f, "Artist Plural Normalization"),
            DecisionKey::LosslessRemux => write!(f, "Lossless Remux"),
            DecisionKey::SubparDuplicate => write!(f, "Subpar Duplicate"),
            DecisionKey::DirectoryCluster { cluster_index } => {
                write!(f, "Directory Cluster:{}", cluster_index)
            }
            DecisionKey::MissingAlbum { group_index } => {
                write!(f, "Missing Album:{}", group_index)
            }
            DecisionKey::ManualReview { group_index } => {
                write!(f, "Manual Review:{}", group_index)
            }
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
            DecisionKey::JettisonEditHistory => write!(f, "Jettison Edit History"),
        }
    }
}

/// Fieldless mirror of DecisionKey for insight filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DecisionKeyKind {
    OobResolution,
    MovedFile,
    MissingFile,
    MissingDirectory,
    CorruptFile,
    ArtistPluralNormalization,
    LosslessRemux,
    SubparDuplicate,
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
