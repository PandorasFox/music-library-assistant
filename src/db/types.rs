//! Core database types for file metadata and audio info.

// ============================================================================
// New Schema Types (inode-based identity)
// ============================================================================

/// Zone classification — which root directory a file lives in.
///
/// Distinct from "source" in the provenance sense (e.g., bandcamp, indie).
/// This enum tracks the *zone* a file occupies within MM's directory hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

    /// The tag table name for this zone. Library has no tags.
    pub fn tag_table(&self) -> Option<&'static str> {
        match self {
            Self::Corpus => Some("corpus_tags"),
            Self::Inbox => Some("inbox_tags"),
            Self::Library => None,
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
