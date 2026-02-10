//! Tag Operations Module
//!
//! THIS IS THE ONLY MODULE THAT TOUCHES AUDIO FILE TAGS.
//!
//! All tag reading, writing, and comparison goes through this module.
//! Do not use lofty directly elsewhere. Do not duplicate this logic.
//! If you need tag functionality not provided here, ADD IT HERE.
//!
//! ## Architecture
//!
//! Tags are modeled as `TagSet`: a set of (key, value) tuples.
//! This naturally handles multi-value fields (multiple genres, etc).
//!
//! ## Entry Points
//!
//! - [`TagSet::from_file()`] - Read all tags from audio file
//! - [`write_file_tags()`] - Write complete tag set to file
//! - [`TagSet::diff()`] - Compare two tag sets
//!
//! ## DO NOT
//!
//! - Import lofty in other modules
//! - Create "convenience" wrappers elsewhere
//! - Duplicate any of this logic

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::meta::signals::data::*;
use crate::meta::mutations::MutationToken;
use crate::corpus::paths;
use crate::db_thread;
use crate::witch::MutationExecutionWitness;

// =============================================================================
// TagSet - The canonical representation of tags
// =============================================================================

/// Complete set of tags for a track.
///
/// Semantically a Set<(key, value)> - the same key can appear multiple times
/// with different values (e.g., multiple genre tags).
///
/// Keys are normalized to lowercase for comparison but original case is preserved
/// for display and round-trip fidelity where possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagSet {
    /// Sorted, deduplicated (key, value) pairs.
    /// Sorting is by (lowercase_key, value) for stable comparison.
    tags: Vec<(String, String)>,
}

impl TagSet {
    /// Create from raw (key, value) pairs.
    ///
    /// Normalizes keys to lowercase, deduplicates exact (key, value) pairs,
    /// and sorts for stable comparison.
    pub fn new(raw: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut tags: Vec<(String, String)> = raw
            .into_iter()
            .filter(|(_, v)| !v.is_empty()) // Skip empty values
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();

        // Sort by (key, value) for stable comparison
        tags.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));

        // Deduplicate exact (key, value) pairs
        tags.dedup();

        Self { tags }
    }

    /// Create an empty TagSet.
    pub fn empty() -> Self {
        Self { tags: Vec::new() }
    }

    /// Read tags from an audio file.
    ///
    /// Uses lofty to read all text tags from the file's primary tag container.
    /// Binary tags (album art, etc.) are skipped.
    pub fn from_file(path: &Path) -> Result<Self> {
        use lofty::file::TaggedFileExt;
        use lofty::probe::Probe;
        use lofty::tag::Accessor;

        let tagged_file = Probe::open(path)
            .with_context(|| format!("Failed to open file for tag reading: {}", path.display()))?
            .read()
            .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

        // Binary/embedded tags to skip (album art, lyrics, etc.)
        const BINARY_TAG_PATTERNS: &[&str] = &[
            "apic", "pic", "uslt", "sylt", "geob",
            "metadata_block_picture", "picture", "popularimeter",
            "cover", "artwork", "lyrics",
        ];

        let mut all_tags = Vec::new();

        if let Some(tag) = tagged_file.primary_tag() {
            // Standard tags via Accessor trait
            if let Some(artist) = tag.artist() {
                all_tags.push(("artist".to_string(), artist.as_ref().to_string()));
            }
            if let Some(album) = tag.album() {
                all_tags.push(("album".to_string(), album.as_ref().to_string()));
            }
            if let Some(title) = tag.title() {
                all_tags.push(("title".to_string(), title.as_ref().to_string()));
            }
            if let Some(track) = tag.track() {
                all_tags.push(("track_number".to_string(), track.to_string()));
            }
            if let Some(year) = tag.year() {
                all_tags.push(("year".to_string(), year.to_string()));
            }
            if let Some(genre) = tag.genre() {
                all_tags.push(("genre".to_string(), genre.as_ref().to_string()));
            }

            // Additional fields from items (extended tags, including multi-value)
            for item in tag.items() {
                let key = item_key_to_string(item.key());
                let normalized_key = key.to_lowercase();

                // Skip binary patterns
                if BINARY_TAG_PATTERNS.iter().any(|&p| normalized_key.contains(p)) {
                    continue;
                }

                // Extract actual string value from ItemValue
                let value = match item.value() {
                    lofty::tag::ItemValue::Text(s) => s.clone(),
                    lofty::tag::ItemValue::Locator(s) => s.clone(),
                    lofty::tag::ItemValue::Binary(_) => continue,
                };

                if !value.is_empty() {
                    all_tags.push((key, value));
                }
            }
        }

        Ok(Self::new(all_tags))
    }

    /// Check if a specific (key, value) pair exists.
    ///
    /// Key comparison is case-insensitive.
    pub fn contains(&self, key: &str, value: &str) -> bool {
        let key_lower = key.to_lowercase();
        self.tags.iter().any(|(k, v)| k == &key_lower && v == value)
    }

    /// Get all values for a key (case-insensitive).
    pub fn values_for(&self, key: &str) -> impl Iterator<Item = &str> {
        let key_lower = key.to_lowercase();
        self.tags
            .iter()
            .filter(move |(k, _)| k == &key_lower)
            .map(|(_, v)| v.as_str())
    }

    /// Get the first value for a key (case-insensitive).
    ///
    /// For single-value fields, this is THE value.
    /// For multi-value fields, this returns an arbitrary one.
    #[cfg(test)]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values_for(key).next()
    }

    /// Iterate over all (key, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.tags.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of tag pairs (not unique keys).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// True if no tags.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Convert to raw vec (for DB storage, etc).
    pub fn into_vec(self) -> Vec<(String, String)> {
        self.tags
    }

    /// Borrow as slice (for DB storage, etc).
    #[cfg(test)]
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.tags
    }

    /// Compute what's different between self and other.
    ///
    /// Returns a `TagSetDiff` containing:
    /// - `only_left`: tags in self but not in other
    /// - `only_right`: tags in other but not in self
    /// - `common`: tags in both
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
// TagSetDiff - Result of comparing two TagSets
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
    #[cfg(test)]
    pub fn classify(&self) -> DiffClassification {
        match (self.only_left.tags.is_empty(), self.only_right.tags.is_empty()) {
            (true, true) => DiffClassification::Identical,
            (false, true) => DiffClassification::LeftOnly,
            (true, false) => DiffClassification::RightOnly,
            (false, false) => DiffClassification::Conflict,
        }
    }
}

/// Classification of tag differences between two sources.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffClassification {
    /// No differences - tags are identical.
    Identical,
    /// Only the left side has extra tags.
    LeftOnly,
    /// Only the right side has extra tags.
    RightOnly,
    /// Both sides have different tags (true conflict).
    Conflict,
}

// =============================================================================
// Write Function - THE ONLY WAY TO WRITE TAGS
// =============================================================================

/// Write a complete TagSet to an audio file.
///
/// THIS IS THE ONLY FUNCTION THAT WRITES TAGS TO FILES.
///
/// Replaces all text tags in the file with the provided set.
/// Preserves non-text data (pictures, binary fields).
/// Removes legacy ID3v1 tags to avoid encoding issues.
///
/// After successful disk write, automatically:
/// - Updates file mtime in files table to match the new file mtime
/// - Clears all OOB signals (OutOfBandTagSync, OutOfBandTagConflict, MtimeOnlyMismatch)
///
/// Signals will be recomputed by VerifyTags in the next computation cycle.
///
/// # Multi-value Support
///
/// The TagSet naturally supports multiple values per key (e.g., multiple genres).
/// For formats that support it (Vorbis comments in FLAC/OGG), each (key, value)
/// pair becomes a separate tag entry. For formats with limited support (ID3v2),
/// only the last value for each key may be preserved.
///
/// # Authorization
///
/// Requires `MutationToken` proving this is called from mutation context.
/// Requires `MutationExecutionWitness` to authorize DB updates.
/// Do not call from UI code or computations.
pub fn write_file_tags(
    path: &Path,
    tags: &TagSet,
    _token: &MutationToken,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{ItemValue, Tag, TagItem, TagType};

    let mut tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag writing: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    let tag_type = tagged_file.primary_tag_type();

    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Group tags by key to handle multi-value properly
    let mut grouped: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();
    for (key, value) in tags.iter() {
        grouped.entry(key.to_string()).or_default().push(value);
    }

    // Apply each key's values
    for (key, values) in grouped {
        let item_key = string_to_item_key(&key, tag_type);

        // Remove all existing values for this key
        tag.remove_key(&item_key);

        // Add each new value
        for value in values {
            if !value.is_empty() {
                let item = TagItem::new(item_key.clone(), ItemValue::Text(value.to_string()));
                tag.push(item);
            }
        }
    }

    // Remove ID3v1 before saving - it's legacy, has 30-byte field limits,
    // and lofty's ID3v1 encoder panics on UTF-8 boundary issues
    tagged_file.remove(TagType::Id3v1);

    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to file: {}", path.display()))?;

    // Update file mtime after successful disk write
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized during tag write"))?;
    let resolver = paths::get_resolver();
    let rel_path = resolver
        .to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path {} not in corpus root", path.display()))?;
    let rel_path_str = rel_path.to_string_lossy();
    let file_metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata after write: {}", path.display()))?;

    // Determine source from relative path (first component: corpus, legacy, libraries, etc.)
    let source = rel_path
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("corpus");

    // Use portable mtime API (consistent with comparison code)
    use std::os::unix::fs::MetadataExt;
    use std::time::UNIX_EPOCH;
    let inode = file_metadata.ino() as i64;
    let (mtime_secs, mtime_nanos) = file_metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0));

    sender.update_file_mtime(
        source,
        inode,
        mtime_secs,
        mtime_nanos,
        witness,
    );

    // Clear OOB/tag signals after successful write - they'll be recomputed next cycle
    // This ensures mutations don't leave stale signals behind (inode-keyed)
    sender.clear_corpus_signal::<OutOfBandTagSyncSignal>(
        inode,
        witness,
    );
    sender.clear_corpus_signal::<OutOfBandTagConflictSignal>(
        inode,
        witness,
    );
    sender.clear_corpus_signal::<MtimeOnlyMismatchSignal>(
        inode,
        witness,
    );
    // Clear tag mismatches - will be recomputed by VerifyTags
    sender.clear_tag_mismatches_for_track(&rel_path_str, witness);

    Ok(())
}

// =============================================================================
// Internal Helpers - Lofty ItemKey conversion
// =============================================================================

/// Convert a lofty ItemKey to a clean tag name string.
///
/// Handles Unknown("NAME") variants properly instead of using Debug format.
fn item_key_to_string(key: &lofty::tag::ItemKey) -> String {
    use lofty::tag::ItemKey;
    match key {
        ItemKey::Unknown(s) => s.clone(),
        ItemKey::TrackArtist => "artist".to_string(),
        ItemKey::AlbumArtist => "album_artist".to_string(),
        ItemKey::TrackTitle => "title".to_string(),
        ItemKey::AlbumTitle => "album".to_string(),
        ItemKey::TrackNumber => "track_number".to_string(),
        ItemKey::DiscNumber => "disc_number".to_string(),
        ItemKey::Genre => "genre".to_string(),
        ItemKey::Year => "year".to_string(),
        ItemKey::RecordingDate => "date".to_string(),
        ItemKey::Comment => "comment".to_string(),
        ItemKey::Composer => "composer".to_string(),
        ItemKey::Conductor => "conductor".to_string(),
        ItemKey::Label => "label".to_string(),
        ItemKey::Remixer => "remixer".to_string(),
        ItemKey::Lyricist => "lyricist".to_string(),
        ItemKey::Writer => "writer".to_string(),
        ItemKey::Bpm => "bpm".to_string(),
        ItemKey::CatalogNumber => "catalog_number".to_string(),
        ItemKey::Barcode => "barcode".to_string(),
        ItemKey::Isrc => "isrc".to_string(),
        ItemKey::MusicBrainzTrackId => "musicbrainz_trackid".to_string(),
        ItemKey::MusicBrainzRecordingId => "musicbrainz_recordingid".to_string(),
        ItemKey::MusicBrainzReleaseId => "musicbrainz_releaseid".to_string(),
        ItemKey::MusicBrainzArtistId => "musicbrainz_artistid".to_string(),
        ItemKey::MusicBrainzReleaseArtistId => "musicbrainz_releaseartistid".to_string(),
        ItemKey::MusicBrainzReleaseGroupId => "musicbrainz_releasegroupid".to_string(),
        ItemKey::MusicBrainzWorkId => "musicbrainz_workid".to_string(),
        // For any other variants, use Debug format but strip the enum name
        other => {
            let debug = format!("{:?}", other);
            if debug.starts_with("Unknown(") {
                debug
                    .strip_prefix("Unknown(\"")
                    .and_then(|s| s.strip_suffix("\")"))
                    .map(|s| s.to_string())
                    .unwrap_or(debug)
            } else {
                debug.to_lowercase()
            }
        }
    }
}

/// Convert a string tag name to a lofty ItemKey.
///
/// Uses ItemKey::from_key for format-aware mapping.
fn string_to_item_key(key: &str, tag_type: lofty::tag::TagType) -> lofty::tag::ItemKey {
    lofty::tag::ItemKey::from_key(tag_type, key)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tagset_new_deduplicates() {
        let tags = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("artist".to_string(), "Foo".to_string()), // duplicate
            ("Artist".to_string(), "Foo".to_string()), // case variation = same after normalize
        ]);
        assert_eq!(tags.len(), 1);
    }

    #[test]
    fn test_tagset_new_sorts() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let keys: Vec<&str> = tags.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["album", "artist", "genre"]);
    }

    #[test]
    fn test_tagset_multi_value() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);
        assert_eq!(tags.len(), 2);
        let genres: Vec<&str> = tags.values_for("genre").collect();
        assert!(genres.contains(&"Rock"));
        assert!(genres.contains(&"Metal"));
    }

    #[test]
    fn test_tagset_contains() {
        let tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);
        assert!(tags.contains("genre", "Rock"));
        assert!(tags.contains("GENRE", "Rock")); // case insensitive key
        assert!(!tags.contains("genre", "Jazz"));
    }

    #[test]
    fn test_tagset_skips_empty_values() {
        let tags = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "".to_string()), // empty, should be skipped
        ]);
        assert_eq!(tags.len(), 1);
        assert!(tags.get("album").is_none());
    }

    #[test]
    fn test_tagset_diff_identical() {
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let diff = a.diff(&b);
        assert!(diff.is_empty());
        assert_eq!(diff.classify(), DiffClassification::Identical);
    }

    #[test]
    fn test_tagset_diff_left_only() {
        let a = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let b = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let diff = a.diff(&b);
        assert!(!diff.is_empty());
        assert_eq!(diff.classify(), DiffClassification::LeftOnly);
        assert_eq!(diff.only_left.len(), 1);
        assert!(diff.only_left.contains("album", "Bar"));
    }

    #[test]
    fn test_tagset_diff_right_only() {
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("album".to_string(), "Bar".to_string()),
        ]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::RightOnly);
        assert!(diff.only_right.contains("album", "Bar"));
    }

    #[test]
    fn test_tagset_diff_conflict() {
        let a = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("extra_a".to_string(), "A".to_string()),
        ]);
        let b = TagSet::new(vec![
            ("artist".to_string(), "Foo".to_string()),
            ("extra_b".to_string(), "B".to_string()),
        ]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::Conflict);
        assert!(diff.only_left.contains("extra_a", "A"));
        assert!(diff.only_right.contains("extra_b", "B"));
        assert!(diff.common.contains("artist", "Foo"));
    }

    #[test]
    fn test_tagset_diff_value_change() {
        // Same key, different values = conflict (both sides have "extras")
        let a = TagSet::new(vec![("artist".to_string(), "Foo".to_string())]);
        let b = TagSet::new(vec![("artist".to_string(), "Bar".to_string())]);
        let diff = a.diff(&b);
        assert_eq!(diff.classify(), DiffClassification::Conflict);
        assert!(diff.only_left.contains("artist", "Foo"));
        assert!(diff.only_right.contains("artist", "Bar"));
    }
}
