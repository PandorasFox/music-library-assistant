//! Indexing Operations
//!
//! Handles execution of indexing-related mutations:
//! - IndexTrack: Insert or update a track in the database
//! - UpdateScanState: Update scan state for incremental scanning
//! - CleanupStaleScanState: Remove stale scan state entries

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;

use crate::corpus::db::{Database, ScanStateEntry, Track};

use super::types::{ExtractedMetadata, Mutation, MutationResult};

/// Execute an IndexTrack mutation.
///
/// Inserts or updates a track in the database from extracted metadata.
/// Returns the track_id of the inserted/updated track.
pub fn execute_index_track(
    db: &Database,
    path: &Path,
    source: &str,
    metadata: &ExtractedMetadata,
) -> Result<i64> {
    // Build Track from ExtractedMetadata
    let track = Track {
        id: None,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        artist: metadata.get_tag("artist").map(String::from),
        album: metadata.get_tag("album").map(String::from),
        album_artist: metadata.get_tag("album_artist").map(String::from),
        title: metadata.get_tag("title").map(String::from),
        track_number: metadata
            .get_tag("track_number")
            .and_then(|s| s.parse().ok()),
        genre: metadata.get_tag("genre").map(String::from),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
        isrc: metadata.get_tag("isrc").map(String::from),
    };

    // Insert track into database
    let track_id = db
        .insert_track(&track)
        .context("Failed to insert track into database")?;

    // TODO: Future - also insert into track_tags table for arbitrary tag storage
    // For now, tags are stored in the legacy columns

    Ok(track_id)
}

/// Execute an UpdateScanState mutation.
///
/// Updates the scan state entry for a file, enabling incremental scanning.
pub fn execute_update_scan_state(
    db: &Database,
    source: &str,
    inode: u64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: u64,
    path: &Path,
) -> Result<()> {
    let entry = ScanStateEntry {
        source: source.to_string(),
        inode: inode as i64,
        path: path.to_string_lossy().to_string(),
        mtime_secs,
        mtime_nanos,
        file_size: file_size as i64,
    };

    db.upsert_scan_state(&entry)
        .context("Failed to update scan state")?;

    Ok(())
}

/// Execute a CleanupStaleScanState mutation.
///
/// Removes scan state entries for files that no longer exist.
/// Returns the number of entries removed.
pub fn execute_cleanup_stale(
    db: &Database,
    source: &str,
    valid_inodes: &[u64],
) -> Result<usize> {
    let valid_set: HashSet<i64> = valid_inodes.iter().map(|&i| i as i64).collect();

    db.cleanup_stale_scan_state(source, &valid_set)
        .context("Failed to cleanup stale scan state")
}

/// Execute a single indexing mutation.
///
/// Convenience function for executing individual mutations.
pub fn execute_single(db: &Database, mutation: &Mutation) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::IndexTrack {
            path,
            source,
            metadata,
        } => execute_index_track(db, path, source, metadata).map(|_| ()),

        Mutation::UpdateScanState {
            source,
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
            path,
        } => execute_update_scan_state(db, source, *inode, *mtime_secs, *mtime_nanos, *file_size, path),

        Mutation::CleanupStaleScanState {
            source,
            valid_inodes,
        } => execute_cleanup_stale(db, source, valid_inodes).map(|_| ()),

        _ => Err(anyhow::anyhow!("Not an indexing mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    MutationResult {
        mutation: mutation.clone(),
        success,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

/// Execute a batch of indexing mutations.
///
/// Processes multiple indexing operations, typically for bulk scanning.
pub fn execute_batch(db: &Database, mutations: &[Mutation]) -> Vec<MutationResult> {
    mutations.iter().map(|m| execute_single(db, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extracted_metadata_creation() {
        let metadata = ExtractedMetadata {
            inode: 12345,
            file_size: 1024 * 1024,
            file_type: "FLAC".to_string(),
            duration_ms: Some(180000),
            bitrate_kbps: Some(1411),
            sample_rate: Some(44100),
            fingerprint: Some("abc123".to_string()),
            tags: vec![
                ("artist".to_string(), "Test Artist".to_string()),
                ("album".to_string(), "Test Album".to_string()),
            ],
        };

        assert_eq!(metadata.get_tag("artist"), Some("Test Artist"));
        assert_eq!(metadata.get_tag("album"), Some("Test Album"));
        assert_eq!(metadata.get_tag("missing"), None);
    }

    #[test]
    fn test_index_track_mutation_structure() {
        let metadata = ExtractedMetadata {
            inode: 12345,
            file_size: 1024 * 1024,
            file_type: "FLAC".to_string(),
            duration_ms: Some(180000),
            bitrate_kbps: Some(1411),
            sample_rate: Some(44100),
            fingerprint: None,
            tags: vec![],
        };

        let mutation = Mutation::IndexTrack {
            path: PathBuf::from("/test/file.flac"),
            source: "corpus".to_string(),
            metadata,
        };

        assert!(matches!(mutation, Mutation::IndexTrack { .. }));
    }
}
