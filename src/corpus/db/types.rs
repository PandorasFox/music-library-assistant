//! Core database types for file metadata and audio info.



// ============================================================================
// New Schema Types (inode-based identity)
// ============================================================================

/// Zone classification — which root directory a file lives in.
///
/// Distinct from "source" in the provenance sense (e.g., bandcamp, indie).
/// This enum tracks the *zone* a file occupies within MM's directory hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zone {
    /// File is in the corpus (source of truth)
    Corpus,
    /// File is in a library (deployment target)
    Library,
    /// File is in the inbox (pending triage)
    Inbox,
}

impl Zone {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Corpus => "corpus",
            Self::Library => "library",
            Self::Inbox => "inbox",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "corpus" => Some(Self::Corpus),
            "library" => Some(Self::Library),
            "inbox" => Some(Self::Inbox),
            _ => None,
        }
    }
}

impl std::fmt::Display for Zone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A path manifestation in the files table.
///
/// Represents a single path (file or directory) in one of the source locations.
/// Multiple paths can share the same inode (hard links across corpus + library).
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// The inode number (content identity)
    pub inode: i64,
    /// Which zone this path belongs to (corpus, library, inbox)
    pub zone: Zone,
    /// Relative path within the source root
    pub path: String,
    pub _is_dir: bool,
    pub _mtime_secs: i64,
    pub _mtime_nanos: i64,
    /// File size in bytes
    pub file_size: i64,
    pub _scanned_at: i64,
}

/// Audio-specific metadata for audio files.
///
/// Only audio file inodes have entries in audio_info.
/// Directories do not have audio_info records.
#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub _inode: i64,
    /// Audio format (flac, mp3, opus, ogg, etc.)
    pub file_type: String,
    /// Duration in milliseconds
    pub duration_ms: Option<i64>,
    /// Bitrate in kbps
    pub bitrate_kbps: Option<i32>,
    /// Sample rate in Hz
    pub sample_rate: Option<i32>,
    /// Chromaprint acoustic fingerprint as raw u32 values
    pub fingerprint: Option<Vec<u32>>,
    pub _needs_tag_flush: bool,
}

/// Combined view of a file entry with its audio info.
///
/// Used for audio files that have both a files row and audio_info row.
#[derive(Debug, Clone)]
pub struct AudioFile {
    pub entry: FileEntry,
    pub audio: AudioInfo,
}

impl AudioFile {
    /// Convenience accessor for the path
    pub fn path(&self) -> &str {
        &self.entry.path
    }

    /// Convenience accessor for the inode
    pub fn inode(&self) -> i64 {
        self.entry.inode
    }
}

/// A single tag associated with an audio file.
///
/// Stored in corpus_tags or inbox_tags tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTag {
    pub inode: i64,
    pub tag_name: String,
    pub tag_value: String,
}

// ============================================================================
// Insights View Data Types
// ============================================================================

/// Insights data for the bucketed Insights view.
/// Computed at cache refresh time, never in render.
#[derive(Debug, Clone, Default)]
pub struct InsightsData {
    pub bucket_corpus: CorpusFilesBucket,
    pub bucket_placeholder: PlaceholderBucket,
    pub bucket_library: LibraryDeployBucket,
    pub bucket_other: OtherSignalsBucket,
}

/// Bucket 1: Corpus Files - file state overview
#[derive(Debug, Clone, Default)]
pub struct CorpusFilesBucket {
    // OOB signals at top - highest priority within bucket
    pub oob_tag_sync: usize,
    pub oob_tag_conflict: usize,
    pub mtime_only_mismatch: usize,
    // Standard corpus file signals
    pub files_in_corpus: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub files_missing: usize,
    pub directories_missing: usize,
    pub files_relocated: usize,
    /// Files that are corrupt (unreadable tags or waveform decode failure)
    pub corrupt_files: usize,
    /// Files in non-Vorbis container formats (MP3, M4A, AAC, WMA, etc.)
    pub shit_format_files: usize,
    /// Filetype breakdown for files_in_corpus
    pub file_type_breakdown: Vec<(String, usize)>,
    /// Directory-level aggregation for selected signal
    pub _directory_breakdown: DirectoryBreakdown,
}

/// Bucket 2: Tag Squash - duplicates, tag canonicity, album_artist, and compound tag issues
#[derive(Debug, Clone, Default)]
pub struct TagSquashBucket {
    /// Directory overlap clusters (grouped fingerprint overlaps for bulk resolution)
    pub directory_overlap_cluster_count: usize,
    /// Subpar duplicates (lower quality versions, easy stash candidates)
    pub subpar_duplicate_count: usize,
    /// Redundant duplicates (equal quality, requires operator choice)
    pub redundant_duplicate_count: usize,
    /// Tag canonicity issues grouped by tag name (e.g., "artist": 50 clusters)
    pub tag_canonicity: Vec<TagSquashEntry>,
    /// Inconsistent album_artist issues count
    pub inconsistent_album_artist_count: usize,
    /// Compound tag values grouped by tag name (e.g., "artist", "genre")
    pub compound_tags: Vec<CompoundTagEntry>,
    /// Directories with sidecar album art embeddable into artless audio files
    pub embeddable_album_art: usize,
    /// Missing album singles (tracks without ALBUM but with ARTIST+TITLE)
    pub missing_album_single_count: usize,
}

/// Entry for tag squash signals (grouped by tag name)
#[derive(Debug, Clone)]
pub struct TagSquashEntry {
    /// Tag name (e.g., "artist", "genre", "album")
    pub tag_name: String,
    /// Number of clusters needing resolution
    pub cluster_count: usize,
    /// Total tracks affected (for ordering - higher = more important)
    pub _total_tracks: usize,
}

/// Entry for compound tag signals (grouped by tag name)
#[derive(Debug, Clone)]
pub struct CompoundTagEntry {
    /// Tag name (e.g., "artist", "genre")
    pub tag_name: String,
    /// Number of safe splits (all parts exist in corpus)
    pub safe_count: usize,
    /// Number of splits needing review (some parts are new)
    pub review_count: usize,
}

// Aliases for compatibility
pub type PlaceholderBucket = TagSquashBucket;

/// Bucket 3: Library/Deploy state
#[derive(Debug, Clone, Default)]
pub struct LibraryDeployBucket {
    pub library_stale: usize,
    pub library_leftover: usize,
    /// Healthy files NOT in any library (DeployReady signals)
    pub deploy_ready: usize,
    /// Healthy files with correct library match (DeployedHealthy signals)
    pub deployed_healthy: usize,
}

/// Bucket 4: Other signals (sorted by magnitude)
#[derive(Debug, Clone, Default)]
pub struct OtherSignalsBucket {
    /// Sorted descending by count
    pub entries: Vec<OtherSignalEntry>,
}

/// Entry for other signals bucket
#[derive(Debug, Clone)]
pub struct OtherSignalEntry {
    pub signal_type: String,
    pub display_label: String,
    pub count: usize,
    /// For aggregate signals that track affected files/tracks
    pub affected_count: Option<usize>,
}

/// Directory breakdown for detail pane
#[derive(Debug, Clone, Default)]
pub struct DirectoryBreakdown {
    /// Sorted by count descending
    pub _entries: Vec<DirectoryBreakdownEntry>,
}

/// Single entry in directory breakdown
#[derive(Debug, Clone)]
pub struct DirectoryBreakdownEntry {
    pub _directory: String,
    pub _count: usize,
}

// ============================================================================
// Deploy Modal Data Types
// ============================================================================

/// A file with deploy info (for healthy/new files).
#[derive(Debug, Clone)]
pub struct DeploySignalFile {
    /// Target library name (e.g., "music", "soundtracks")
    pub library_name: String,
    /// Path in the corpus
    pub corpus_path: String,
    /// Computed deploy path in library (relative to library root, no library prefix)
    pub deploy_path: String,
}

/// A stale library file (deployed path differs from expected).
#[derive(Debug, Clone)]
pub struct StaleSignalFile {
    /// Library this file belongs to (e.g., "music")
    pub library_name: String,
    /// Current path in library (wrong) — "{library_name}/path/..."
    pub library_path: String,
    /// Expected path (computed from current tags) — "{library_name}/path/..."
    pub expected_path: String,
}

/// A leftover file (in library but no corpus backing).
#[derive(Debug, Clone)]
pub struct LeftoverSignalFile {
    /// Library this file belongs to (e.g., "music")
    pub library_name: String,
    /// Path in the library — "{library_name}/path/..."
    pub library_path: String,
}

/// A deploy conflict group (multiple corpus files → same library path).
#[derive(Debug, Clone)]
pub struct ConflictGroup {
    /// The library path they all would deploy to
    pub deploy_path: String,
    /// List of conflicting corpus files: (corpus_path, inode)
    pub conflicting_files: Vec<(String, i64)>,
}

// ============================================================================
// Subpar Duplicate Resolution Types
// ============================================================================

/// A subpar duplicate file (lower quality version identified by fingerprint analysis).
#[derive(Debug, Clone)]
pub struct SubparDuplicateEntry {
    /// Path in the corpus (this is the subpar file)
    pub corpus_path: String,
    /// Reason for being subpar (e.g., "SubparBitrate", "SubparFormat", "SubparSampleRate")
    pub reason: String,
    /// Path to the superior version
    pub superior_path: String,
    /// Fingerprint similarity score (0.0-100.0).
    pub similarity_score: f64,
}

// ============================================================================
// OOB Tag Resolution Types
// ============================================================================

/// Direction of a syncable OOB tag mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSyncDirection {
    /// Extra tags exist on disk only (db_value IS NULL) — sync disk → index
    DiskToIndex,
    /// Extra tags exist in DB only (disk_value IS NULL) — sync index → disk
    IndexToDisk,
}

/// A single tag mismatch entry between DB and disk.
///
/// Contains both display strings (for UI) and individual values (for mutations).
/// Multi-value tags (e.g., multiple TRACKNUMBER fields) are stored as individual
/// values in the `*_values` vecs, joined for display in `*_value` fields.
#[derive(Debug, Clone)]
pub struct TagMismatchEntry {
    pub field: String,
    /// Display string (values joined with "; ") - for UI
    pub db_value: Option<String>,
    /// Display string (values joined with "; ") - for UI
    pub disk_value: Option<String>,
    /// Individual tag values from DB (for mutations)
    pub _db_values: Vec<String>,
    /// Individual tag values from disk (for mutations)
    pub _disk_values: Vec<String>,
}

/// A file with purely sync-direction tag mismatches (all extras in one direction).
#[derive(Debug, Clone)]
pub struct OobSyncFile {
    pub inode: i64,
    /// Relative path (as stored in signals/files)
    pub path: String,
    pub direction: OobSyncDirection,
    pub mismatches: Vec<TagMismatchEntry>,
}

/// Classification bucket for OOB signal files.
///
/// Determined by SQL CASE expression against signal type and `tag_mismatches` table:
/// - MtimeOnly: mtime_only_mismatch signal (mtime changed, tags identical) - needs acknowledgement
/// - DbOnly: all mismatches have `disk_value IS NULL`
/// - DiskOnly: all mismatches have `db_value IS NULL`
/// - Conflict: both values present, or mixed null directions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictBucket {
    MtimeOnly,
    DbOnly,
    DiskOnly,
    Conflict,
}

impl ConflictBucket {
    pub fn label(&self) -> &'static str {
        match self {
            Self::MtimeOnly => "Mtime Only",
            Self::DbOnly => "DB Only",
            Self::DiskOnly => "Disk Only",
            Self::Conflict => "Conflicts",
        }
    }

    pub fn index(&self) -> usize {
        *self as usize
    }

    pub fn next(&self) -> Self {
        match self {
            Self::MtimeOnly => Self::DbOnly,
            Self::DbOnly => Self::DiskOnly,
            Self::DiskOnly => Self::Conflict,
            Self::Conflict => Self::MtimeOnly,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::MtimeOnly => Self::Conflict,
            Self::DbOnly => Self::MtimeOnly,
            Self::DiskOnly => Self::DbOnly,
            Self::Conflict => Self::DiskOnly,
        }
    }

    pub const ALL: [ConflictBucket; 4] = [
        ConflictBucket::MtimeOnly,
        ConflictBucket::DbOnly,
        ConflictBucket::DiskOnly,
        ConflictBucket::Conflict,
    ];

    /// Whether this bucket supports bulk resolution buttons (tag sync/conflict).
    pub fn is_resolvable(&self) -> bool {
        matches!(self, Self::DbOnly | Self::DiskOnly | Self::Conflict)
    }

    /// Whether this bucket supports acknowledgement (mtime-only changes).
    pub fn is_acknowledgeable(&self) -> bool {
        matches!(self, Self::MtimeOnly)
    }
}

/// A file with an OOB tag signal, classified into a conflict bucket.
#[derive(Debug, Clone)]
pub struct BucketedOobFile {
    pub inode: i64,
    pub path: String,
    pub bucket: ConflictBucket,
}

/// A file with a MovedFile signal (same inode, different path).
#[derive(Debug, Clone)]
pub struct MovedFileInfo {
    pub inode: i64,
    pub old_path: String,
    pub new_path: String,
}

