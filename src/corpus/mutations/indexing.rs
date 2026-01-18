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
use crate::daemon::MutationExecutionWitness;

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
    // Build Track from ExtractedMetadata (similar to execute_index_track)
    let track = Track {
        id: Some(track_id),
        path: path.to_string_lossy().to_string(),
        source: "corpus".to_string(), // Will be overwritten by existing
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

    db.update_track_metadata(track_id, &track)
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

    // Get track from database
    let track = db
        .get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

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

    // Build map of disk tags for easier lookup
    // Note: disk_tags uses Debug format keys like "Unknown(\"ARTIST\")" or standard keys
    let disk_map: HashMap<String, String> = disk_tags.into_iter().collect();

    // Helper to get disk tag value, checking various key formats
    let get_disk_tag = |standard_name: &str| -> Option<&str> {
        // Try lowercase standard name first
        if let Some(v) = disk_map.get(standard_name) {
            return Some(v.as_str());
        }
        // lofty often returns tags as Unknown("TAG_NAME") format
        let upper = standard_name.to_uppercase();
        for (k, v) in &disk_map {
            if k.contains(&upper) || k.to_lowercase() == standard_name {
                return Some(v.as_str());
            }
        }
        None
    };

    // Fields to compare: artist, album, album_artist, title, genre
    // track_number is special (stored as Option<i32>)
    let fields_to_check = [
        ("artist", track.artist.as_deref()),
        ("album", track.album.as_deref()),
        ("album_artist", track.album_artist.as_deref()),
        ("title", track.title.as_deref()),
        ("genre", track.genre.as_deref()),
    ];

    for (field_name, db_value) in fields_to_check {
        let disk_value = get_disk_tag(field_name);

        // Normalize: treat empty string as None
        let db_normalized = db_value.filter(|s| !s.is_empty());
        let disk_normalized = disk_value.filter(|s| !s.is_empty());

        if db_normalized != disk_normalized {
            // Record mismatch
            db.record_tag_mismatch(
                track_id,
                field_name,
                db_normalized,
                disk_normalized,
            )?;
        } else {
            // Clear any existing mismatch for this field (now in sync)
            db.clear_tag_mismatch(track_id, field_name)?;
        }
    }

    // Handle track_number separately (i32 vs string)
    let disk_track_num = get_disk_tag("track_number")
        .or_else(|| get_disk_tag("tracknumber"))
        .and_then(|s| s.split('/').next()) // Handle "1/12" format
        .and_then(|s| s.parse::<i32>().ok());

    let db_track_num = track.track_number;

    if db_track_num != disk_track_num {
        db.record_tag_mismatch(
            track_id,
            "track_number",
            db_track_num.map(|n| n.to_string()).as_deref(),
            disk_track_num.map(|n| n.to_string()).as_deref(),
        )?;
    } else {
        db.clear_tag_mismatch(track_id, "track_number")?;
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

/// Execute a batch of indexing mutations.
///
/// Processes multiple indexing operations, typically for bulk scanning.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_batch(
    db: &Database,
    mutations: &[Mutation],
    witness: &MutationExecutionWitness,
) -> Vec<MutationResult> {
    mutations.iter().map(|m| execute_single(db, m, witness)).collect()
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
