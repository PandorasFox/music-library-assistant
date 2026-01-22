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
use crate::witch::MutationExecutionWitness;

use super::types::{ExtractedMetadata, Mutation, MutationResult};

/// Execute an IndexTrack mutation.
///
/// Inserts or updates a track in the database from extracted metadata.
/// Tags are stored in the separate track_tags table.
/// Returns the track_id of the inserted/updated track.
pub fn execute_index_track(
    db: &Database,
    path: &Path,
    source: &str,
    metadata: &ExtractedMetadata,
) -> Result<i64> {
    // Build Track from ExtractedMetadata (audio/file metadata only)
    let track = Track {
        id: None,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    // Insert track with tags into database
    let track_id = db
        .insert_track_with_tags(&track, &metadata.tags)
        .with_context(|| format!("Failed to insert track into database: {}", path.display()))?;

    Ok(track_id)
}

/// Execute an IndexFileFromPath mutation.
///
/// Extracts metadata from the file and indexes it. This does the heavy lifting
/// on the worker thread rather than the UI thread.
pub fn execute_index_file_from_path(db: &Database, path: &Path, source: &str) -> Result<()> {
    use crate::corpus::metadata;

    // Extract audio properties
    let track = metadata::extract_metadata(path, source)
        .with_context(|| format!("Failed to extract metadata from {:?}", path))?;

    // Read tags
    let tags = metadata::read_all_tags(path)
        .with_context(|| format!("Failed to read tags from {:?}", path))?;

    let extracted = ExtractedMetadata {
        inode: track.inode,
        file_size: track.file_size,
        file_type: track.file_type,
        duration_ms: track.duration_ms,
        bitrate_kbps: track.bitrate_kbps,
        sample_rate: track.sample_rate,
        fingerprint: track.fingerprint,
        tags,
    };

    execute_index_track(db, path, source, &extracted)?;
    Ok(())
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

// ============================================================================
// Signal Resolution Executors
// ============================================================================

/// Execute UpdateTrackPath mutation - update path for relocated file.
pub fn execute_update_track_path(db: &Database, track_id: i64, new_path: &Path) -> Result<()> {
    db.update_track_path(track_id, new_path.to_string_lossy().as_ref())
        .context("Failed to update track path")
}

/// Execute UpdateScanStatePath mutation - update scan state for relocated file.
pub fn execute_update_scan_state_path(
    db: &Database,
    source: &str,
    inode: i64,
    new_path: &Path,
) -> Result<()> {
    db.update_scan_state_path(source, inode, new_path.to_string_lossy().as_ref())
        .context("Failed to update scan state path")
}

/// Execute DropFromIndex mutation - remove track from index.
pub fn execute_drop_from_index(
    db: &Database,
    track_id: i64,
    inode: Option<i64>,
    source: Option<&str>,
) -> Result<()> {
    // Delete the track
    db.delete_track(track_id)
        .context("Failed to delete track from index")?;

    // Also delete scan_state entry if inode/source provided
    if let (Some(inode), Some(source)) = (inode, source) {
        db.delete_scan_state_by_inode(source, inode)
            .context("Failed to delete scan state entry")?;
    }

    Ok(())
}

/// Execute UpdateTrack mutation - full metadata update for out-of-band changes.
pub fn execute_update_track(
    db: &Database,
    track_id: i64,
    path: &Path,
    metadata: &ExtractedMetadata,
) -> Result<()> {
    // Build Track from ExtractedMetadata (audio/file metadata only)
    let track = Track {
        id: Some(track_id),
        path: path.to_string_lossy().to_string(),
        source: "corpus".to_string(), // Will be overwritten by existing
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    db.update_track_metadata_with_tags(track_id, &track, &metadata.tags)
        .context("Failed to update track metadata")
}

/// Execute tag verification - compare in-file tags with database, record mismatches.
///
/// This is a read-mostly operation that only writes to the tag_mismatches table.
/// It's used by both the Mutation system (legacy) and the Computation system.
///
/// Note: This function is public because it's called from corpus::computations.
pub fn execute_verify_tags(db: &Database, track_id: i64, path: &Path) -> Result<()> {
    use crate::corpus::metadata;
    use std::collections::HashMap;

    // Verify track exists
    let _track = db
        .get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Get tags from database
    let db_tags = db.get_track_tags(track_id)?;
    let db_map: HashMap<String, String> = db_tags
        .into_iter()
        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
        .collect();

    // Read tags from file
    let disk_tags = match metadata::read_all_tags(path) {
        Ok(tags) => tags,
        Err(e) => {
            // File might not exist or be unreadable - log but don't fail
            let _ = crate::config::log_message(&format!(
                "VerifyTags: Could not read tags from {}: {}",
                path.display(),
                e
            ));
            return Ok(());
        }
    };

    // Build map of disk tags for easier lookup (normalized to lowercase keys)
    let disk_map: HashMap<String, String> = disk_tags
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();

    // Collect all unique tag names from both sources
    let mut all_tags: std::collections::HashSet<String> = db_map.keys().cloned().collect();
    all_tags.extend(disk_map.keys().cloned());

    for tag_name in all_tags {
        let db_value = db_map.get(&tag_name).map(|s| s.as_str());
        let disk_value = disk_map.get(&tag_name).map(|s| s.as_str());

        // Normalize: treat empty string as None
        let db_normalized = db_value.filter(|s| !s.is_empty());
        let disk_normalized = disk_value.filter(|s| !s.is_empty());

        if db_normalized != disk_normalized {
            // Record mismatch
            db.record_tag_mismatch(
                track_id,
                &tag_name,
                db_normalized,
                disk_normalized,
            )?;
        } else {
            // Clear any existing mismatch for this field (now in sync)
            db.clear_tag_mismatch(track_id, &tag_name)?;
        }
    }

    Ok(())
}

// ============================================================================
// Single Mutation Dispatch
// ============================================================================

/// Execute a single indexing mutation.
///
/// Convenience function for executing individual mutations.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &Database,
    mutation: &Mutation,
    _witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::IndexTrack {
            path,
            source,
            metadata,
        } => execute_index_track(db, path, source, metadata).map(|_| ()),

        Mutation::IndexFileFromPath { path, source } => {
            execute_index_file_from_path(db, path, source)
        }

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

        // Signal resolution mutations
        Mutation::UpdateTrackPath {
            track_id,
            new_path,
            ..
        } => execute_update_track_path(db, *track_id, new_path),

        Mutation::UpdateScanStatePath {
            source,
            inode,
            new_path,
        } => execute_update_scan_state_path(db, source, *inode, new_path),

        Mutation::DropFromIndex {
            track_id,
            inode,
            source,
            ..
        } => execute_drop_from_index(db, *track_id, *inode, source.as_deref()),

        Mutation::UpdateTrack {
            track_id,
            path,
            metadata,
        } => execute_update_track(db, *track_id, path, metadata),

        // Note: VerifyTags is now a Computation, not a Mutation.
        // Use corpus::computations::execute_single() instead.

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
            fingerprint: Some(vec![0xabc123]),
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
