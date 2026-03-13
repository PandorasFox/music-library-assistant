//! Tag data structures.
//!
//! TagSet and PictureInfo are pure data types used across the protocol boundary.
//! File I/O operations (from_file, write_file_tags) stay in the mm crate.

use serde::{Deserialize, Serialize};

// =============================================================================
// PictureInfo - Embedded picture metadata
// =============================================================================

/// Metadata about an embedded picture (album art) in an audio file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PictureInfo {
    /// Format: "jpeg", "png", "gif", "bmp", or "unknown"
    pub format: String,
    /// Width in pixels (0 if unknown/unsupported format)
    pub width: u32,
    /// Height in pixels (0 if unknown/unsupported format)
    pub height: u32,
    /// Total number of embedded pictures
    pub count: u32,
}

// =============================================================================
// TagSet - The canonical representation of tags
// =============================================================================

/// Complete set of tags for a track.
///
/// Semantically a Set<(key, value)> - the same key can appear multiple times
/// with different values (e.g., multiple genre tags).
///
/// Keys are normalized to UPPERCASE, matching VorbisComments convention on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagSet {
    /// Sorted, deduplicated (key, value) pairs.
    tags: Vec<(String, String)>,
}

impl TagSet {
    /// Create from raw (key, value) pairs.
    ///
    /// Normalizes keys to UPPERCASE, deduplicates exact (key, value) pairs,
    /// and sorts for stable comparison.
    pub fn new(raw: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut tags: Vec<(String, String)> = raw
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.to_uppercase(), v))
            .collect();

        tags.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        tags.dedup();

        Self { tags }
    }

    /// Create an empty TagSet.
    pub fn empty() -> Self {
        Self { tags: Vec::new() }
    }

    /// Check if a specific (key, value) pair exists. Key comparison is case-insensitive.
    pub fn contains(&self, key: &str, value: &str) -> bool {
        let key_upper = key.to_uppercase();
        self.tags.iter().any(|(k, v)| k == &key_upper && v == value)
    }

    /// Get all values for a key (case-insensitive).
    pub fn values_for(&self, key: &str) -> impl Iterator<Item = &str> {
        let key_upper = key.to_uppercase();
        self.tags
            .iter()
            .filter(move |(k, _)| k == &key_upper)
            .map(|(_, v)| v.as_str())
    }

    /// Get all values for a key, matching by normalized tag name.
    pub fn values_for_normalized(&self, key: &str) -> Vec<(&str, &str)> {
        let normalized = mm_utils::tag_names::normalize_tag_name(key);
        self.tags
            .iter()
            .filter(|(k, _)| mm_utils::tag_names::normalize_tag_name(k) == normalized)
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// Get the first value for a key (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values_for(key).next()
    }

    /// Iterate over all (key, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.tags.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of tag pairs (not unique keys).
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// True if no tags.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Convert to raw vec.
    pub fn into_vec(self) -> Vec<(String, String)> {
        self.tags
    }

    /// Borrow as slice.
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.tags
    }

    /// Compute what's different between self and other.
    pub fn diff(&self, other: &TagSet) -> TagSetDiff {
        use std::collections::HashSet;

        let self_set: HashSet<(&str, &str)> = self.iter().collect();
        let other_set: HashSet<(&str, &str)> = other.iter().collect();

        let only_left: Vec<(String, String)> = self_set
            .difference(&other_set)
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let only_right: Vec<(String, String)> = other_set
            .difference(&self_set)
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        TagSetDiff {
            only_left: TagSet::new(only_left),
            only_right: TagSet::new(only_right),
        }
    }
}

// =============================================================================
// TagSetDiff
// =============================================================================

/// Result of comparing two TagSets.
#[derive(Debug, Clone)]
pub struct TagSetDiff {
    /// Tags in the first set but not the second.
    pub only_left: TagSet,
    /// Tags in the second set but not the first.
    pub only_right: TagSet,
}

impl TagSetDiff {
    /// Classify the difference for OOB detection.
    pub fn classify(&self) -> DiffClassification {
        match (
            self.only_left.tags.is_empty(),
            self.only_right.tags.is_empty(),
        ) {
            (true, true) => DiffClassification::Identical,
            (false, true) => DiffClassification::LeftOnly,
            (true, false) => DiffClassification::RightOnly,
            (false, false) => DiffClassification::Conflict,
        }
    }
}

/// Classification of tag differences between two sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffClassification {
    Identical,
    LeftOnly,
    RightOnly,
    Conflict,
}

// =============================================================================
// Album Art Constants
// =============================================================================

/// Sidecar image filenames that map to CoverFront.
pub const COVER_FRONT_NAMES: &[&str] = &["cover", "folder", "albumart", "album", "front"];

/// Sidecar image filenames that map to CoverBack.
pub const COVER_BACK_NAMES: &[&str] = &["back"];
