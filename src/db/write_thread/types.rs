//! Data types used by the write thread channel.
//!
//! These structs are sent through the `DbWriteOp` channel and consumed by
//! executor functions on the DB thread.

use crate::meta::computations::ComputationWitness;
use crate::witch::MutationExecutionWitness;

// ============================================================================
// Signal Witness Trait
// ============================================================================

/// Marker trait for types that authorize signal emission.
///
/// Both `ComputationWitness` (computation context) and `MutationExecutionWitness`
/// (mutation context) implement this trait. Signal emission methods accept
/// `&impl SignalWitness` to work in either context.
///
/// Creation restrictions on each witness type ensure signals can only be
/// emitted from authorized execution contexts.
pub trait SignalWitness {}

impl SignalWitness for ComputationWitness {}
impl SignalWitness for MutationExecutionWitness {}

// ============================================================================
// Index Signal Data Types
// ============================================================================

/// File entry metadata for the files table.
///
/// Used when indexing a file or directory into the files table.
#[derive(Debug, Clone)]
pub struct FileData {
    pub inode: i64,
    pub zone: String, // 'corpus', 'library'
    pub _is_dir: bool,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

/// Audio-specific metadata for the audio_info table.
///
/// Only for audio files (not directories).
#[derive(Debug, Clone)]
pub struct AudioData {
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub fingerprint: Option<Vec<u32>>,
    pub has_pictures: bool,
    /// Picture metadata: format string ("jpeg", "png", etc.), or None if no pictures.
    pub pic_format: Option<String>,
    /// Picture width in pixels, or None if no pictures or unknown.
    pub pic_width: Option<u32>,
    /// Picture height in pixels, or None if no pictures or unknown.
    pub pic_height: Option<u32>,
    /// Total number of embedded pictures.
    pub pic_count: u32,
}

/// File entry data for files table operations.
///
/// Used for upsert operations that don't need full FileData (e.g., UpdateFileEntry).
/// Contains the core file identity and mtime fields stored in the files table.
#[derive(Debug, Clone)]
pub struct FileEntryData {
    pub inode: i64,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

// ============================================================================
// Pipeline Intermediate Data Types
// ============================================================================

/// A row for the release_packing_scores intermediate table.
#[derive(Debug, Clone)]
pub struct PackingScoreRow {
    pub release_id: String,
    pub inode: i64,
    pub recording_id: String,
    pub medium_pos: i32,
    pub track_pos: i32,
    pub track_title: String,
    pub medium_format: Option<String>,
    pub track_number: String,
    pub score: f64,
    pub score_breakdown: Vec<u8>, // bincode-serialized PackingScoreBreakdown
    pub is_optimal: bool,
    pub match_method: i32,               // 0=AcoustId, 1=Elimination
    pub fingerprint_hex: Option<String>, // For elimination submission recording
    pub raw_duration_ms: Option<i64>,    // For elimination submission recording
}

/// A row for the release_packing_candidates intermediate table.
#[derive(Debug, Clone)]
pub struct PackingCandidateRow {
    pub release_id: String,
    pub inode: i64,
    pub recording_id: String,
    pub confidence: f64,
    pub path: String,
    pub parent_dir: String,
    pub duration_ms: Option<i64>,
    pub tag_title: Option<String>,
    pub tag_artist: Option<String>,
    pub tag_album: Option<String>,
    pub tag_tracknumber: Option<String>,
    pub dir_file_count: i32,
}

/// A pending AcoustID submission from elimination matching.
#[derive(Debug, Clone)]
pub struct PendingAcoustIdSubmission {
    pub fingerprint: String,
    pub recording_id: String,
    pub duration_ms: i64,
    pub source: String,
}
