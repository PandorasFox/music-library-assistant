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

impl SignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            // First-level signals
            Self::FileInCorpus => "file_in_corpus",

            // Second-level signals
            Self::UnindexedFile => "unindexed_file",
            Self::HealthyFile => "healthy_file",
            Self::MovedFile => "moved_file",
            Self::MissingFile => "missing_file",
            Self::MissingDirectory => "missing_directory",

            // Third-level signals
            Self::DeployConflict => "deploy_conflict",

            // Library deployment health signals
            Self::LibraryStale => "library_stale",
            Self::LibraryLeftover => "library_leftover",

            // Content-level signals
            Self::FingerprintOverlap => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::MissingTag => "missing_tag",
            Self::OutOfBandTagSync => "oob_tag_sync",
            Self::OutOfBandTagConflict => "oob_tag_conflict",
            Self::MtimeOnlyMismatch => "mtime_only_mismatch",
            Self::DuplicateInode => "duplicate_inode",
            Self::TagCanonicity => "tag_canonicity",
            Self::InconsistentAlbumArtist => "inconsistent_album_artist",
            Self::CompoundTagValue => "compound_tag_value",
            Self::CrossSourceOverlap => "cross_source_overlap",

            // Error signals
            Self::CorruptFile => "corrupt_file",
            Self::ShitFormat => "shit_format",
            Self::SubparDuplicate => "subpar_duplicate",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            // First-level signals
            "file_in_corpus" => Some(Self::FileInCorpus),

            // Second-level signals
            "unindexed_file" => Some(Self::UnindexedFile),
            "healthy_file" => Some(Self::HealthyFile),
            "moved_file" => Some(Self::MovedFile),
            "missing_file" => Some(Self::MissingFile),
            "missing_directory" => Some(Self::MissingDirectory),

            // Third-level signals
            "deploy_conflict" => Some(Self::DeployConflict),

            // Library deployment health signals
            "library_stale" => Some(Self::LibraryStale),
            "library_leftover" => Some(Self::LibraryLeftover),

            // Content-level signals
            "fingerprint_dup" => Some(Self::FingerprintOverlap),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "missing_tag" => Some(Self::MissingTag),
            "oob_tag_sync" => Some(Self::OutOfBandTagSync),
            "oob_tag_conflict" => Some(Self::OutOfBandTagConflict),
            "mtime_only_mismatch" => Some(Self::MtimeOnlyMismatch),
            // Legacy: treat old "oob_tag" as conflict (conservative)
            "oob_tag" => Some(Self::OutOfBandTagConflict),
            "duplicate_inode" => Some(Self::DuplicateInode),
            // Legacy: inode_changed was removed in v3 migration
            // These are now exposed as MissingFile + UnindexedFile pair
            "inode_changed" => Some(Self::MissingFile),
            "tag_canonicity" => Some(Self::TagCanonicity),
            "inconsistent_album_artist" => Some(Self::InconsistentAlbumArtist),
            "compound_tag_value" => Some(Self::CompoundTagValue),
            "compound_tag" => Some(Self::CompoundTagValue),
            "cross_source_overlap" => Some(Self::CrossSourceOverlap),
            // Legacy: map old error signal types to CorruptFile
            "tag_parse_error" => Some(Self::CorruptFile),
            "waveform_read_error" => Some(Self::CorruptFile),
            "corrupt_file" => Some(Self::CorruptFile),
            "shit_format" => Some(Self::ShitFormat),
            "subpar_duplicate" => Some(Self::SubparDuplicate),

            // Legacy DB values → map to new types
            "missing_from_disk" => Some(Self::MissingFile),
            "missing_from_index" => Some(Self::FileInCorpus),
            "file_relocated" => Some(Self::MovedFile),
            // Legacy: oob_file_change was split into OOB trio
            "oob_file_change" => Some(Self::OutOfBandTagConflict),
            "corpus_file_modified_oob" => Some(Self::OutOfBandTagConflict),

            _ => None,
        }
    }
}

// NOTE: From<CorpusFileSignalType> for SignalType has been intentionally removed.
// This prevents accidental coercion that could lead to keying mismatches.
// If you need the string representation, use signal_type.as_str() directly.


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

/// Aggregate health signal.
///
/// Groups multiple tracks under a common key. Metadata contains details.
#[derive(Debug, Clone)]
pub struct AggregateSignal {
    pub id: Option<i64>,
    pub signal_type: AggregateSignalType,
    pub key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
}

impl AggregateSignal {
    /// Create a new aggregate signal with inodes embedded in metadata.
    pub fn with_inodes(mut self, ids: &[i64]) -> Self {
        let mut meta = self
            .metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        meta["inodes"] = serde_json::json!(ids);
        meta["inode_count"] = serde_json::json!(ids.len());
        self.metadata_json = Some(meta.to_string());
        self
    }
}

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

impl AggregateSignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FingerprintOverlap => "fingerprint_dup",
            Self::MetadataDuplicate => "metadata_dup",
            Self::DuplicateInode => "duplicate_inode",
            Self::MissingTag => "missing_tag",
            Self::DeployConflict => "deploy_conflict",
            Self::TagCanonicity => "tag_canonicity",
            Self::InconsistentAlbumArtist => "inconsistent_album_artist",
            Self::CompoundTagValue => "compound_tag_value",
            Self::CrossSourceOverlap => "cross_source_overlap",
            Self::CanonicalTag => "canonical_tag",
            Self::LibraryLeftover => "library_leftover",
            Self::LibraryStale => "library_stale",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fingerprint_dup" => Some(Self::FingerprintOverlap),
            "metadata_dup" => Some(Self::MetadataDuplicate),
            "duplicate_inode" => Some(Self::DuplicateInode),
            "missing_tag" => Some(Self::MissingTag),
            "deploy_conflict" => Some(Self::DeployConflict),
            "tag_canonicity" => Some(Self::TagCanonicity),
            "inconsistent_album_artist" => Some(Self::InconsistentAlbumArtist),
            "compound_tag_value" => Some(Self::CompoundTagValue),
            "cross_source_overlap" => Some(Self::CrossSourceOverlap),
            "canonical_tag" => Some(Self::CanonicalTag),
            "library_leftover" => Some(Self::LibraryLeftover),
            "library_stale" => Some(Self::LibraryStale),
            _ => None,
        }
    }
}

// ============================================================================
// Signal (Unified Signal Type)
// ============================================================================

/// Unified signal representing a fact about corpus state.
///
/// Signals are created by computations and deleted when stale. They record
/// file health, duplicate detection, missing tags, deploy conflicts, etc.
///
/// ## Key Types
///
/// - **File signals** (FileInCorpus, UnindexedFile, HealthyFile, MissingFile, etc.):
///   `issue_key` stores the inode as a string. Path is in `metadata_json.path`.
/// - **Library signals** (LibraryStale, LibraryLeftover):
///   `issue_key` uses compound format like `"library_leftover:{name}:{path}"`.
/// - **Aggregate signals** (FingerprintOverlap, TagCanonicity, etc.):
///   `issue_key` uses semantic string keys.
#[derive(Debug, Clone)]
pub struct Signal {
    pub id: Option<i64>,
    pub issue_type: SignalType,
    pub issue_key: String,
    pub discovered_at: Option<String>,
    pub metadata_json: Option<String>,
    /// Native inode for inode-keyed signals (corpus file signals).
    /// Path-keyed signals (MissingDirectory, library signals) have None.
    pub inode: Option<i64>,
}


impl From<AggregateSignal> for Signal {
    fn from(sig: AggregateSignal) -> Self {
        Self {
            id: sig.id,
            issue_type: SignalType::from_str(sig.signal_type.as_str())
                .unwrap_or(SignalType::FingerprintOverlap),
            issue_key: sig.key,
            discovered_at: sig.discovered_at,
            metadata_json: sig.metadata_json,
            inode: None, // AggregateSignal is semantic-keyed, not inode-keyed
        }
    }
}


