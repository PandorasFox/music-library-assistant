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
use crate::corpus::paths;
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
    witness: &MutationExecutionWitness,
) -> Result<i64> {
    let resolver = paths::get_resolver();

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;

    // Build Track from ExtractedMetadata (audio/file metadata only)
    let track = Track {
        id: None,
        path: relative_path.to_string_lossy().to_string(),
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
        .insert_track_with_tags(&track, &metadata.tags, witness)
        .with_context(|| format!("Failed to insert track into database: {}", path.display()))?;

    Ok(track_id)
}

/// Execute an IndexFileFromPath mutation.
///
/// Extracts metadata from the file and indexes it. This does the heavy lifting
/// on the worker thread rather than the UI thread.
pub fn execute_index_file_from_path(db: &Database, path: &Path, source: &str, witness: &MutationExecutionWitness) -> Result<()> {
    use crate::corpus::metadata;
    use crate::corpus::tags::TagSet;

    // Extract audio properties
    let track = metadata::extract_metadata(path, source)
        .with_context(|| format!("Failed to extract metadata from {:?}", path))?;

    // Read tags using TagSet
    let tag_set = TagSet::from_file(path)
        .with_context(|| format!("Failed to read tags from {:?}", path))?;

    let extracted = ExtractedMetadata {
        inode: track.inode,
        file_size: track.file_size,
        file_type: track.file_type,
        duration_ms: track.duration_ms,
        bitrate_kbps: track.bitrate_kbps,
        sample_rate: track.sample_rate,
        fingerprint: track.fingerprint,
        tags: tag_set.into_vec(),
    };

    execute_index_track(db, path, source, &extracted, witness)?;
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
    let resolver = paths::get_resolver();

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;

    let entry = ScanStateEntry {
        source: source.to_string(),
        inode: inode as i64,
        path: relative_path.to_string_lossy().to_string(),
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
///
/// Note: This function needs the source to determine which root to use for
/// relative path conversion. It fetches the track's source from the database.
pub fn execute_update_track_path(db: &Database, track_id: i64, new_path: &Path, witness: &MutationExecutionWitness) -> Result<()> {
    let resolver = paths::get_resolver();

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(new_path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                new_path.display(),
            )
        })?;

    db.update_track_path(track_id, relative_path.to_string_lossy().as_ref(), witness)
        .context("Failed to update track path")
}

/// Execute UpdateScanStatePath mutation - update scan state for relocated file.
pub fn execute_update_scan_state_path(
    db: &Database,
    source: &str,
    inode: i64,
    new_path: &Path,
) -> Result<()> {
    let resolver = paths::get_resolver();

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(new_path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                new_path.display(),
            )
        })?;

    db.update_scan_state_path(source, inode, relative_path.to_string_lossy().as_ref())
        .context("Failed to update scan state path")
}

/// Execute DropFromIndex mutation - remove track from index.
pub fn execute_drop_from_index(
    db: &Database,
    track_id: i64,
    inode: Option<i64>,
    source: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    // Delete the track
    db.delete_track(track_id, witness)
        .with_context(|| format!("Failed to delete track {} from index", track_id))?;

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
    witness: &MutationExecutionWitness,
) -> Result<()> {
    let resolver = paths::get_resolver();

    // Get existing track to preserve source
    let existing = db
        .get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;

    // Build Track from ExtractedMetadata (audio/file metadata only)
    let track = Track {
        id: Some(track_id),
        path: relative_path.to_string_lossy().to_string(),
        source: existing.source, // Preserve original source
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    db.update_track_metadata_with_tags(track_id, &track, &metadata.tags, witness)
        .context("Failed to update track metadata")
}

/// Result of tag verification — indicates which types of mismatches were found.
///
/// Used by computations to classify signals without querying `tag_mismatches`
/// (which may not have committed async writes yet).
pub struct TagVerifyResult {
    /// At least one tag where both DB and disk have different non-empty values.
    pub has_conflict: bool,
    /// At least one tag present on disk but absent in DB.
    pub has_extra_disk: bool,
    /// At least one tag present in DB but absent on disk.
    pub has_extra_db: bool,
}

impl TagVerifyResult {
    fn empty() -> Self {
        Self { has_conflict: false, has_extra_disk: false, has_extra_db: false }
    }

    /// True if no mismatches at all.
    pub fn is_clean(&self) -> bool {
        !self.has_conflict && !self.has_extra_disk && !self.has_extra_db
    }

    /// True if value conflicts or mixed-direction extras.
    pub fn is_conflict(&self) -> bool {
        self.has_conflict || (self.has_extra_disk && self.has_extra_db)
    }
}

/// Execute tag verification - compare in-file tags with database, record mismatches.
///
/// This is a read-mostly operation that only writes to the tag_mismatches table.
/// It's used by both the Mutation system (direct DB writes) and the Computation
/// system (routed through db_thread for write access on read-only connections).
///
/// When `mismatch_sender` is provided, tag mismatch writes are routed through
/// the db_thread's write connection. When `None`, writes go directly to `db`
/// (requires a writable connection, e.g. mutation execution context).
///
/// Returns a `TagVerifyResult` summarizing the mismatch directions found.
/// Computations use this for in-memory classification instead of querying
/// `tag_mismatches` (which may lag due to async db_thread writes).
///
/// Note: This function is public because it's called from corpus::computations.
pub fn execute_verify_tags(
    db: &Database,
    track_id: i64,
    path: &Path,
    mismatch_sender: Option<(&crate::db_thread::SignalWriteSender, &crate::corpus::computations::ComputationWitness)>,
) -> Result<TagVerifyResult> {
    use crate::corpus::tags::TagSet;
    use std::collections::HashSet;

    // Verify track exists
    let _track = db
        .get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Get tags from database as TagSet
    let db_tags = db.get_track_tags(track_id)?;
    let db_tagset = TagSet::new(
        db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
    );

    // Read tags from file as TagSet
    let disk_tagset = match TagSet::from_file(path) {
        Ok(tags) => tags,
        Err(e) => {
            // File might not exist or be unreadable - log but don't fail
            crate::logging::log_error(format!(
                "VerifyTags: Could not read tags from {}: {}",
                path.display(),
                e
            ));
            return Ok(TagVerifyResult::empty());
        }
    };

    // Get all unique tag names from both sources
    let mut all_tag_names: HashSet<String> = HashSet::new();
    for (k, _) in db_tagset.iter() {
        all_tag_names.insert(k.to_string());
    }
    for (k, _) in disk_tagset.iter() {
        all_tag_names.insert(k.to_string());
    }

    let mut result = TagVerifyResult::empty();

    // Compare per-tag-name, handling multi-value properly
    for tag_name in all_tag_names {
        // Collect all values for this tag from each source
        let db_values: Vec<&str> = db_tagset.values_for(&tag_name).collect();
        let disk_values: Vec<&str> = disk_tagset.values_for(&tag_name).collect();

        // Convert to sets for proper comparison (order doesn't matter)
        let db_set: HashSet<&str> = db_values.iter().copied().collect();
        let disk_set: HashSet<&str> = disk_values.iter().copied().collect();

        if db_set != disk_set {
            // Track mismatch direction for in-memory classification
            let db_has_extras = !db_set.difference(&disk_set).collect::<Vec<_>>().is_empty();
            let disk_has_extras = !disk_set.difference(&db_set).collect::<Vec<_>>().is_empty();

            match (db_has_extras, disk_has_extras) {
                (true, true) => result.has_conflict = true,
                (false, true) => result.has_extra_disk = true,
                (true, false) => result.has_extra_db = true,
                (false, false) => {} // Shouldn't happen if sets differ
            }

            // For mismatch recording, aggregate multi-values into semicolon-separated string
            // This maintains backward compatibility with the UI
            let db_display = if db_values.is_empty() {
                None
            } else {
                Some(db_values.join("; "))
            };
            let disk_display = if disk_values.is_empty() {
                None
            } else {
                Some(disk_values.join("; "))
            };

            // Record mismatch — route through sender if available (read-only context)
            if let Some((sender, witness)) = mismatch_sender {
                sender.record_tag_mismatch(
                    track_id,
                    &tag_name,
                    db_display.as_deref(),
                    disk_display.as_deref(),
                    witness,
                );
            } else {
                db.record_tag_mismatch(
                    track_id,
                    &tag_name,
                    db_display.as_deref(),
                    disk_display.as_deref(),
                )?;
            }
        } else {
            // Clear any existing mismatch for this field (now in sync)
            if let Some((sender, witness)) = mismatch_sender {
                sender.clear_tag_mismatch(track_id, &tag_name, witness);
            } else {
                db.clear_tag_mismatch(track_id, &tag_name)?;
            }
        }
    }

    Ok(result)
}

// ============================================================================
// OOB Resolution Executors
// ============================================================================

/// Execute AcknowledgeMtimeOnly mutation.
///
/// For each track: updates scan_state mtime to match current disk mtime,
/// then clears the MtimeOnlyMismatch signal. Used when disk file mtime
/// changed but tags are identical.
pub fn execute_acknowledge_mtime_only(
    db: &Database,
    track_ids: &[i64],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use std::os::unix::fs::MetadataExt;
    use rusqlite::params;

    let resolver = paths::get_resolver();
    let mut affected_paths = Vec::new();

    for track_id in track_ids {
        // Get track info
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue, // Skip missing tracks
        };

        // Resolve absolute path for filesystem access
        let abs_path = resolver.resolve(std::path::Path::new(&track.path));

        // Read current disk mtime
        let metadata = std::fs::metadata(&abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = metadata.mtime_nsec() as i64;

        // Update scan_state mtime
        db.conn.execute(
            "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
            params![mtime_secs, mtime_nanos, track.path],
        ).context("Failed to update scan_state mtime")?;

        // Clear MtimeOnlyMismatch signal
        db.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            &track.path,
            witness,
        )?;

        affected_paths.push(abs_path);
    }

    Ok(affected_paths)
}

/// Execute AcknowledgeInodeChanged mutation.
///
/// For each track: updates track.inode to the new inode, deletes old scan_state
/// entry, creates new scan_state with current mtime, and clears the InodeChanged signal.
/// Tag differences are handled separately through the OOB tag resolution flow.
pub fn execute_acknowledge_inode_changed(
    db: &Database,
    track_ids: &[i64],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::{CorpusFileSignalType, ScanStateEntry};
    use std::os::unix::fs::MetadataExt;

    let resolver = paths::get_resolver();
    let mut affected_paths = Vec::new();

    for track_id in track_ids {
        // Get track info
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue, // Skip missing tracks
        };

        // Resolve absolute path for filesystem access
        let abs_path = resolver.resolve(std::path::Path::new(&track.path));

        // Read current disk metadata
        let metadata = std::fs::metadata(&abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let new_inode = metadata.ino() as i64;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = metadata.mtime_nsec() as i64;
        let file_size = metadata.len() as i64;

        // Update track.inode to the new value
        db.update_track_inode(*track_id, new_inode, witness)?;

        // Delete old scan_state entry (keyed by old inode)
        db.delete_scan_state_by_inode(&track.source, track.inode)?;

        // Insert new scan_state entry with new inode and current mtime
        db.upsert_scan_state(&ScanStateEntry {
            source: track.source.clone(),
            inode: new_inode,
            path: track.path.clone(),
            mtime_secs,
            mtime_nanos,
            file_size,
        })?;

        // Clear InodeChanged signal
        db.clear_file_signal(
            CorpusFileSignalType::InodeChanged.into(),
            &track.path,
            witness,
        )?;

        affected_paths.push(abs_path);
    }

    Ok(affected_paths)
}

/// Execute ApplyDbTagsToDisk mutation.
///
/// For each track: writes DB tags to disk file, updates scan_state mtime,
/// clears tag_mismatches and OOB signals. Used to reject disk-side changes
/// and restore DB state to disk.
pub fn execute_apply_db_tags_to_disk(
    db: &Database,
    track_ids: &[i64],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::corpus::tags::{write_file_tags, TagSet};
    use std::os::unix::fs::MetadataExt;
    use rusqlite::params;

    let resolver = paths::get_resolver();
    let token = super::sealed::MutationToken::new();
    let mut affected_paths = Vec::new();

    for track_id in track_ids {
        // Get track info
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue,
        };

        // Resolve absolute path for filesystem access
        let abs_path = resolver.resolve(std::path::Path::new(&track.path));

        // Get DB tags and convert to TagSet
        let db_tags = db.get_track_tags(*track_id)?;
        let tag_set = TagSet::new(
            db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
        );

        // Write tags to disk using the consolidated write path
        write_file_tags(&abs_path, &tag_set, &token)
            .with_context(|| format!("Failed to write tags to {}", abs_path.display()))?;

        // Read new disk mtime after write
        let file_metadata = std::fs::metadata(&abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let mtime_secs = file_metadata.mtime();
        let mtime_nanos = file_metadata.mtime_nsec() as i64;

        // Update scan_state mtime
        db.conn.execute(
            "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
            params![mtime_secs, mtime_nanos, track.path],
        ).context("Failed to update scan_state mtime")?;

        // Clear tag_mismatches for this track
        db.clear_tag_mismatches_for_track(*track_id)?;

        // Clear OOB signals
        db.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagSync.into(),
            &track.path,
            witness,
        )?;
        db.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagConflict.into(),
            &track.path,
            witness,
        )?;
        db.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            &track.path,
            witness,
        )?;

        affected_paths.push(abs_path);
    }

    Ok(affected_paths)
}

/// Execute AssimilateDiskTagsToDb mutation.
///
/// For each track: reads disk tags into DB, updates scan_state mtime,
/// clears tag_mismatches and OOB signals. Used to accept disk-side changes
/// and update DB to match disk.
pub fn execute_assimilate_disk_tags_to_db(
    db: &Database,
    track_ids: &[i64],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::corpus::tags::TagSet;
    use std::os::unix::fs::MetadataExt;
    use rusqlite::params;

    let resolver = paths::get_resolver();
    let mut affected_paths = Vec::new();

    for track_id in track_ids {
        // Get track info
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue,
        };

        // Resolve absolute path for filesystem access
        let abs_path = resolver.resolve(std::path::Path::new(&track.path));

        // Read disk tags using TagSet
        let disk_tagset = TagSet::from_file(&abs_path)
            .with_context(|| format!("Failed to read tags from {}", abs_path.display()))?;

        // Update DB with disk tags
        db.set_track_tags(*track_id, disk_tagset.as_slice(), witness)
            .with_context(|| format!("Failed to update track tags for track {}", track_id))?;

        // Read disk mtime
        let file_metadata = std::fs::metadata(&abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let mtime_secs = file_metadata.mtime();
        let mtime_nanos = file_metadata.mtime_nsec() as i64;

        // Update scan_state mtime
        db.conn.execute(
            "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
            params![mtime_secs, mtime_nanos, track.path],
        ).context("Failed to update scan_state mtime")?;

        // Clear tag_mismatches for this track
        db.clear_tag_mismatches_for_track(*track_id)?;

        // Clear OOB signals
        db.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagSync.into(),
            &track.path,
            witness,
        )?;
        db.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagConflict.into(),
            &track.path,
            witness,
        )?;
        db.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            &track.path,
            witness,
        )?;

        affected_paths.push(abs_path);
    }

    Ok(affected_paths)
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
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::IndexTrack {
            path,
            source,
            metadata,
        } => execute_index_track(db, path, source, metadata, witness).map(|_| ()),

        Mutation::IndexFileFromPath { path, source } => {
            execute_index_file_from_path(db, path, source, witness)
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
        } => execute_update_track_path(db, *track_id, new_path, witness),

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
        } => execute_drop_from_index(db, *track_id, *inode, source.as_deref(), witness),

        Mutation::UpdateTrack {
            track_id,
            path,
            metadata,
        } => execute_update_track(db, *track_id, path, metadata, witness),

        // OOB resolution mutations
        Mutation::AcknowledgeMtimeOnly { track_ids } => {
            execute_acknowledge_mtime_only(db, track_ids, witness).map(|_| ())
        }

        Mutation::AcknowledgeInodeChanged { track_ids } => {
            execute_acknowledge_inode_changed(db, track_ids, witness).map(|_| ())
        }

        Mutation::ApplyDbTagsToDisk { track_ids } => {
            execute_apply_db_tags_to_disk(db, track_ids, witness).map(|_| ())
        }

        Mutation::AssimilateDiskTagsToDb { track_ids } => {
            execute_assimilate_disk_tags_to_db(db, track_ids, witness).map(|_| ())
        }

        // Note: VerifyTags is now a Computation, not a Mutation.
        // Use corpus::computations::execute_single() instead.

        _ => Err(anyhow::anyhow!("Not an indexing mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
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
