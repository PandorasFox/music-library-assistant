//! Review and match modal data types.

use serde::{Deserialize, Serialize};

use crate::external::musicbrainz::{MbArtist, MbRecording, MbRelease};
use crate::views::{InboxCorpusMatchEntry, MatchClassification};

// ============================================================================
// Manual Review Modal Types
// ============================================================================

/// What kind of manual review this modal is performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewKind {
    /// Equivalent-quality duplicates (same fingerprint, same format/metric tier).
    /// Only stashing resolves these — tag edits cannot help.
    RedundantDuplicate,
    /// Multiple corpus files deploy to the same library path.
    /// Tag editing or stashing resolves these.
    DeployConflict,
    /// Files with identical tag signatures (artist/album/title).
    /// Tag editing or stashing resolves these.
    MetadataDuplicate,
}

impl ReviewKind {
    /// Human-readable title for the modal header.
    pub fn title(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant Duplicate Review",
            Self::DeployConflict => "Deploy Conflict Review",
            Self::MetadataDuplicate => "Metadata Duplicate Review",
        }
    }

    /// Stash directory name for this kind.
    pub fn stash_name(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "redundant",
            Self::DeployConflict => "deploy_conflict",
            Self::MetadataDuplicate => "metadata_dup",
        }
    }

    /// Transaction label.
    pub fn transaction_label(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant duplicate resolution",
            Self::DeployConflict => "Deploy conflict resolution",
            Self::MetadataDuplicate => "Metadata duplicate resolution",
        }
    }

    /// Whether tag editing is available for this kind.
    pub fn supports_tag_edit(self) -> bool {
        true
    }
}

/// Audio metadata summary for the detail pane (loaded once at modal init).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMetaSummary {
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub file_size: i64,
    pub has_pictures: bool,
    /// Ordered list of (tag_name, tag_value).
    pub tags: Vec<(String, String)>,
}

/// A single file entry within a review group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFileEntry {
    /// Corpus-relative path.
    pub corpus_path: String,
    /// Inode for DB operations.
    pub inode: i64,
    /// Kind-specific context line (deploy path, quality description, tag signature).
    pub context: String,
    /// Whether this file has been marked for stashing in this session.
    pub stashed: bool,
    /// Audio metadata (loaded at init time).
    pub meta: Option<FileMetaSummary>,
}

/// A group of files requiring review together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewGroup {
    /// Human-readable label for this group (fingerprint, deploy path, tag signature).
    pub label: String,
    /// Files in this group.
    pub files: Vec<ReviewFileEntry>,
    /// Signal key for this group (fingerprint key for RedundantDuplicate, None for others).
    pub signal_key: Option<String>,
}

/// Cached data for the manual review modal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManualReviewData {
    /// All groups to review.
    pub groups: Vec<ReviewGroup>,
}

// ============================================================================
// External Match Modal Types
// ============================================================================

/// Pre-loaded MB recording summary for inline display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub title: String,
    pub artist_credit: String,
    pub length_ms: Option<u64>,
    pub release_count: usize,
}

/// Full recording detail data for building wizard pane lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingDetail {
    pub recording: MbRecording,
    pub artists: Vec<(String, Option<MbArtist>)>,
    pub releases: Vec<(String, Option<MbRelease>)>,
}

// ============================================================================
// Inbox Corpus Match Modal Types
// ============================================================================

/// Cached data for the inbox corpus match resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
/// Entries sorted: Equivalent first, then Subpar, then Better.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InboxCorpusMatchModalData {
    pub entries: Vec<InboxCorpusMatchEntry>,
}

impl InboxCorpusMatchModalData {
    /// Total number of entries.
    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    /// Count of entries safe to stash (Equivalent + Subpar).
    pub fn stashable_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                matches!(
                    e.classification,
                    MatchClassification::Equivalent | MatchClassification::Subpar
                )
            })
            .count()
    }

    /// Count by classification: (better, equivalent, subpar).
    pub fn count_by_class(&self) -> (usize, usize, usize) {
        let mut better = 0;
        let mut equivalent = 0;
        let mut subpar = 0;
        for e in &self.entries {
            match e.classification {
                MatchClassification::Better => better += 1,
                MatchClassification::Equivalent => equivalent += 1,
                MatchClassification::Subpar => subpar += 1,
            }
        }
        (better, equivalent, subpar)
    }
}
