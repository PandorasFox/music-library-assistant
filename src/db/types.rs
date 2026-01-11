//! Core database types for track metadata and scan state.

use serde::{Deserialize, Serialize};

/// Universal audio file representation.
/// Stored in the `tracks` table.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: Option<i64>,
    pub path: String,
    pub source: String, // corpus/library name/legacy
    pub inode: i64,
    pub file_size: i64,
    pub file_type: String, // flac, mp3, ogg, etc.
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub title: Option<String>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub fingerprint: Option<String>, // chromaprint acoustic fingerprint
    pub isrc: Option<String>,        // International Standard Recording Code
}

/// Entry in the scan_state table for incremental scanning.
/// Tracks inode + mtime to detect file changes.
#[derive(Debug, Clone)]
pub struct ScanStateEntry {
    pub source: String,
    pub inode: i64,
    pub path: String,
    pub mtime_secs: i64,
    pub mtime_nanos: i64, // SQLite INTEGER is i64; cast to u32 at comparison time
    pub file_size: i64,
}

/// Deployment statistics for corpus health tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStats {
    pub total_corpus_files: usize,
    pub deployed_files: usize,
    pub deployment_percentage: f64,
    pub last_updated: String,
}
