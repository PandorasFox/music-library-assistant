//! Review and match modal data types.

use serde::{Deserialize, Serialize};

use crate::external::musicbrainz::{MbArtist, MbRecording, MbRelease};

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
    /// Same MusicBrainz recording on different releases.
    /// Informational for dedup reasoning and release packing.
    SameRecordingDifferentRelease,
}

impl ReviewKind {
    /// Human-readable title for the modal header.
    pub fn title(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant Duplicate Review",
            Self::DeployConflict => "Deploy Conflict Review",
            Self::MetadataDuplicate => "Metadata Duplicate Review",
            Self::SameRecordingDifferentRelease => "Same Recording, Different Release Review",
        }
    }

    /// Stash directory name for this kind.
    pub fn stash_name(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "redundant",
            Self::DeployConflict => "deploy_conflict",
            Self::MetadataDuplicate => "metadata_duplicate",
            Self::SameRecordingDifferentRelease => "cross_release",
        }
    }

    /// Transaction label.
    pub fn transaction_label(self) -> &'static str {
        match self {
            Self::RedundantDuplicate => "Redundant duplicate resolution",
            Self::DeployConflict => "Deploy conflict resolution",
            Self::MetadataDuplicate => "Metadata duplicate resolution",
            Self::SameRecordingDifferentRelease => "Same recording, different release resolution",
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

