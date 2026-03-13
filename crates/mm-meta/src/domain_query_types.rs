//! Domain query response types defined alongside the query system.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ============================================================================
// Packing Directory Data
// ============================================================================

/// Cached packing directory data for tree browser markers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackingDirsData {
    /// Paths of files with release packing assignments.
    pub file_paths: HashSet<PathBuf>,
    /// Parent directories mapped to their best packing category.
    pub dir_categories: HashMap<PathBuf, crate::signals::PackingCategory>,
}

// ============================================================================
// Packing Browser Data
// ============================================================================

/// All data needed to build a release packing browser view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackingBrowserData {
    pub packed: Vec<crate::signals::data::PackedReleaseData>,
    pub packing: Vec<(i64, String, crate::signals::data::ReleasePackingData)>,
    pub unfilled: Vec<crate::signals::data::UnfilledReleaseSlotData>,
    pub alternatives: Vec<crate::signals::data::AlternativeReleasePackingData>,
    pub va_overrides: Vec<crate::signals::data::VariousArtistsOverrideData>,
}

// ============================================================================
// Session Edit Detail
// ============================================================================

/// Response for session edit detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEditDetail {
    pub edits: Vec<crate::views::EditRecord>,
    pub inode_paths: HashMap<i64, String>,
}

// ============================================================================
// Recording Batch Result
// ============================================================================

/// Response type for batch recording data loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingBatchResult {
    pub summaries: Vec<(String, crate::views::review_match::RecordingSummary)>,
    pub details: Vec<(String, crate::views::review_match::RecordingDetail)>,
}

// ============================================================================
// Release Staging Data
// ============================================================================

/// Response type for release staging data loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseStagingData {
    pub bundle: crate::external::musicbrainz::MbCacheBundle,
    pub inode_tags: HashMap<i64, Vec<(String, String)>>,
}

// ============================================================================
// Tag Editor Load Mode
// ============================================================================

/// Tag editor file loading mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TagEditorLoadMode {
    /// Load all files in directory tree (recursive).
    Directory,
    /// Load siblings in parent directory, select the target file.
    SingleFile,
}

// ============================================================================
// Audio File With Tags
// ============================================================================

/// An audio file paired with its tag map (tag name -> values).
/// Tag names are uppercased; values are collected into Vec since a
/// single tag name can have multiple values (e.g. multiple genres).
pub type AudioFileWithTags = (crate::db_types::AudioFile, HashMap<String, Vec<String>>);
