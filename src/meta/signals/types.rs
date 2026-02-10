//! Signal types for the MLA signal system.
//!
//! These are the core inter-system abstractions for health signals.
//! Signal types are pure enums/structs with no reverse dependencies on
//! other MLA modules (except `DeploymentStats` from `corpus::db::types`).





// ============================================================================
// Signal Key Types
// ============================================================================

/// How a signal type is keyed in the database.
///
// ============================================================================
// Health Issue Types
// ============================================================================

/// Type of health signal detected in the corpus.
///
/// See [`docs/SIGNAL_REFERENCE.md`](../../../../docs/SIGNAL_REFERENCE.md) for the
/// canonical reference of what emits and clears each signal type.
///
/// **Any changes to signal semantics must be reflected in that document.**
///
/// Signals are organized into levels:
/// - **First-level**: Computed directly from corpus + index state (WalkCorpus, etc.)
/// - **Second-level**: Derived from comparing first-level signals
/// - **Third-level**: Triggered when files become healthy (e.g., deploy conflicts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalType {
    // =========================================================================
    // First-level signals (computed from corpus + index state)
    // =========================================================================
    /// File exists in corpus directory (discovered during WalkCorpus)
    FileInCorpus,

    // =========================================================================
    // Second-level signals (derived from first-level signals)
    // =========================================================================
    /// File in corpus but not in index (needs indexing)
    UnindexedFile,
    /// File in corpus + index with matching inode/mtime (healthy state)
    HealthyFile,
    /// File in index with different path but same inode (file was moved)
    /// issue_key: new path, metadata: {"old_path": "...", "inode": i64}
    MovedFile,
    /// File in index but no longer exists in corpus
    MissingFile,
    /// Directory in index but no longer exists in corpus
    MissingDirectory,

    // =========================================================================
    // Third-level signals (computed for healthy files)
    // =========================================================================
    /// Multiple corpus files would deploy to the same library path
    DeployConflict,

    // =========================================================================
    // Library deployment health signals
    // =========================================================================
    /// Library file exists but deployed at wrong path (tags changed since deploy)
    /// issue_key: "library_stale:{library_name}:{library_path}"
    LibraryStale,
    /// Library file exists without corpus backing (leftover)
    /// issue_key: "library_leftover:{library_name}:{library_path}"
    LibraryLeftover,

    // =========================================================================
    // Content-level signals (tag and fingerprint analysis)
    // =========================================================================
    /// Same fingerprint across multiple files (internal overlap detection)
    FingerprintOverlap,
    /// Same metadata (artist/album/title) across multiple files
    MetadataDuplicate,
    /// Missing required tags (e.g., album_artist)
    MissingTag,
    /// Tags on disk have extras in one direction only (syncable)
    OutOfBandTagSync,
    /// Tags on disk conflict with indexed tags (value differences or mixed directions)
    OutOfBandTagConflict,
    /// File mtime changed but tags are identical (requires operator acknowledgement)
    MtimeOnlyMismatch,
    /// Multiple corpus entries share the same inode (hard links or DB inconsistency)
    DuplicateInode,
    /// Tag value collision needing canonicalization
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    InconsistentAlbumArtist,
    /// Tag value contains separators that should be split into multiple values
    CompoundTagValue,
    /// Cross-source fingerprint overlap cluster (derived from FingerprintOverlap signals)
    /// Key: sorted source pair, e.g., "web/releases/bandcamp|web/releases/indie"
    /// Metadata: { source_a, source_b, overlap_count, track_pairs: [...] }
    CrossSourceOverlap,

    // =========================================================================
    // Error signals (discovery-time parse/read failures)
    // =========================================================================
    /// File is corrupt (unreadable tags or waveform decode failure)
    /// Actionable signal with resolution flow (stash + drop)
    CorruptFile,
    /// File is in a non-Vorbis container format (MP3, M4A, AAC, WMA, or lossless needing remux)
    /// Actionable signal with resolution flow (transcode to Opus/FLAC)
    ShitFormat,
    /// Track is an subpar duplicate (lower quality version of another track)
    /// Actionable signal with resolution flow (stash subpar copy)
    SubparDuplicate,
}


// ============================================================================
// Signal Types (Type-Safe Signal System)
// ============================================================================

// NOTE: FileSignal struct has been removed.
// Corpus file signals are now inode-keyed (use CorpusFileSignalType).
// Library signals are now aggregate signals (use AggregateSignalType).

/// Types of corpus file signals.
///
/// These signals track the state of files in the corpus directory.
/// **All corpus file signals are keyed by inode** - use `clear_corpus_signal(type, inode)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorpusFileSignalType {
    /// File exists in corpus directory
    FileInCorpus,
    /// File in corpus but not in index
    UnindexedFile,
    /// File in corpus + index, healthy state
    HealthyFile,
    /// File in index but missing from corpus
    MissingFile,
    /// Directory in index but missing from corpus
    MissingDirectory,
    /// File moved (same inode, different path than indexed)
    /// issue_key: new path, metadata: {"old_path": "...", "inode": i64}
    MovedFile,
    /// Tags on disk have extras in one direction only (syncable)
    OutOfBandTagSync,
    /// Tags on disk conflict with indexed tags (value differences or mixed directions)
    OutOfBandTagConflict,
    /// File mtime changed but tags are identical (requires operator acknowledgement)
    MtimeOnlyMismatch,
    /// File is corrupt (unreadable tags or waveform decode failure)
    CorruptFile,
    /// File is in a non-Vorbis container format (MP3, M4A, AAC, WMA, or lossless needing remux)
    ShitFormat,
    /// Track is an subpar duplicate (lower quality version of another track)
    SubparDuplicate,
    /// Tag value contains separator characters needing split (per-file)
    /// Metadata: { "inode", "compounds": [{ "tag_name", "compound_value", "split_parts", "separator" }] }
    CompoundTag,
    /// Healthy corpus file ready for deployment (not yet in any library)
    /// Metadata: { "deploy_path": "..." }
    DeployReady,
    /// Healthy corpus file deployed at correct library path
    /// Metadata: { "library_path": "..." }
    DeployedHealthy,
}

impl CorpusFileSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileInCorpus => "file_in_corpus",
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::MissingFile => "missing_file",
            Self::MissingDirectory => "missing_directory",
            Self::MovedFile => "moved_file",
            Self::OutOfBandTagSync => "oob_tag_sync",
            Self::OutOfBandTagConflict => "oob_tag_conflict",
            Self::MtimeOnlyMismatch => "mtime_only_mismatch",
            Self::CorruptFile => "corrupt_file",
            Self::ShitFormat => "shit_format",
            Self::SubparDuplicate => "subpar_duplicate",
            Self::CompoundTag => "compound_tag",
            Self::DeployReady => "deploy_ready",
            Self::DeployedHealthy => "deployed_healthy",
        }
    }
}

impl std::fmt::Display for CorpusFileSignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// NOTE: LibraryFileSignalType and FileSignalType have been removed.
//
// - DeployReady/DeployedHealthy moved to CorpusFileSignalType (inode-keyed)
// - LibraryStale/LibraryLeftover moved to AggregateSignalType (semantic-keyed)
//
// This ensures compile-time enforcement of keying semantics:
// - CorpusFileSignalType -> must use clear_corpus_signal(type, inode)
// - AggregateSignalType -> must use clear_aggregate_signal(type, key)


/// Types of aggregate signals (semantic-keyed).
///
/// These signals use arbitrary string keys (not inodes). Use `clear_aggregate_signal(type, key)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateSignalType {
    /// Multiple files with same fingerprint (internal overlap detection, not surfaced directly)
    FingerprintOverlap,
    /// Multiple files with same artist/album/title
    MetadataDuplicate,
    /// Multiple index entries with same inode
    DuplicateInode,
    /// Tracks missing a required tag
    MissingTag,
    /// Multiple corpus files deploy to same library path
    DeployConflict,
    /// Tag value collision needing canonicalization
    /// Key: "{tag_name}:{normalized_key}" (e.g., "artist:dragonforce")
    /// Metadata: { "variants": {"DragonForce": 47, "Dragonforce": 3}, "inodes": [...] }
    TagCanonicity,
    /// Album has tracks with different artists + missing/inconsistent album_artist
    /// Key: "{normalized_album}" (e.g., "clockwork hearts")
    /// Metadata: { "album": "...", "artist_variants": {...}, "album_artist_variants": {...}, "inodes": [...] }
    InconsistentAlbumArtist,
    /// Tag value contains separator characters needing to be split
    /// Key: "{tag_name}:{compound_value_hash}" (e.g., "genre:abc123")
    /// Metadata: { "tag_name", "compound_value", "split_parts": [...], "separator", "inodes": [...] }
    CompoundTagValue,
    /// Cross-source fingerprint overlap cluster (derived from FingerprintOverlap signals)
    /// Key: sorted source pair, e.g., "web/releases/bandcamp|web/releases/indie"
    /// Metadata: { "source_a", "source_b", "overlap_count", "fingerprint_keys": [...], "track_pairs": [...] }
    CrossSourceOverlap,
    /// Operator-confirmed canonical tag value (whitelist - skip split detection)
    /// Key: "{tag_name}:{tag_value}" (e.g., "artist:Rinse & Repeat")
    /// Metadata: { "tag_name", "canonical_value", "created_at" }
    CanonicalTag,
    /// Library file without corpus backing
    /// Key: "library_leftover:{library_name}:{library_path}"
    LibraryLeftover,
    /// Library file at wrong path (tags changed since deploy)
    /// Key: "library_stale:{library_name}:{library_path}"
    /// Metadata: { "library_path", "expected_path" }
    LibraryStale,
}


