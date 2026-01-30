//! Indexing Operations
//!
//! Handles execution of indexing-related mutations:
//! - IndexTrack: Insert or update a track in the database
//! - UpdateScanState: Update scan state for incremental scanning
//! - CleanupStaleScanState: Remove stale scan state entries

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::paths;
use crate::witch::MutationExecutionWitness;

use super::types::{ExtractedMetadata, Mutation, MutationResult};

/// Execute an IndexTrack mutation.
///
/// Inserts or updates a track in the database from extracted metadata.
/// Tags are stored in the separate track_tags table.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_index_track(
    _db: &Database,
    path: &Path,
    source: &str,
    metadata: &ExtractedMetadata,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread::{self, TrackData, ScanStateData};
    use std::os::unix::fs::MetadataExt;

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;
    let rel_path_str = relative_path.to_string_lossy();

    // Build TrackData from ExtractedMetadata
    let track_data = TrackData {
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    // Get file metadata for scan_state
    let file_metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read file metadata: {}", path.display()))?;
    let scan_state = ScanStateData {
        inode: metadata.inode,
        mtime_secs: file_metadata.mtime(),
        mtime_nanos: file_metadata.mtime_nsec() as i64,
        file_size: metadata.file_size,
    };

    // Route write through signal_sender (fire-and-forget)
    sender.index_track(
        &rel_path_str,
        source,
        track_data,
        metadata.tags.clone(),
        scan_state,
        witness,
    );

    Ok(())
}

/// Execute an IndexFileFromPath mutation.
///
/// Extracts metadata from the file and indexes it. This does the heavy lifting
/// on the worker thread rather than the UI thread.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_index_file_from_path(_db: &Database, path: &Path, source: &str, witness: &MutationExecutionWitness) -> Result<()> {
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

    // Note: _db is unused - execute_index_track routes through signal_sender
    execute_index_track(_db, path, source, &extracted, witness)
}

/// Execute an UpdateScanState mutation.
///
/// Updates the scan state entry for a file, enabling incremental scanning.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_update_scan_state(
    _db: &Database,
    source: &str,
    inode: u64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: u64,
    path: &Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread::{self, ScanStateData};

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;

    let scan_state = ScanStateData {
        inode: inode as i64,
        mtime_secs,
        mtime_nanos,
        file_size: file_size as i64,
    };

    // Route write through signal_sender
    sender.upsert_scan_state(&relative_path.to_string_lossy(), source, scan_state, witness);

    Ok(())
}

/// Execute a CleanupStaleScanState mutation.
///
/// Removes scan state entries for files that no longer exist.
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_cleanup_stale(
    _db: &Database,
    source: &str,
    valid_inodes: &[u64],
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    let valid_i64: Vec<i64> = valid_inodes.iter().map(|&i| i as i64).collect();
    sender.cleanup_stale_scan_state(source, valid_i64, witness);

    Ok(())
}

// ============================================================================
// Signal Resolution Executors
// ============================================================================

/// Execute UpdateTrackPath mutation - update path for relocated file.
///
/// Note: This function needs the source to determine which root to use for
/// relative path conversion. It fetches the track's source from the database.
///
/// Uses read-only DB for lookup, routes write through signal_sender.
pub fn execute_update_track_path(db: &Database, track_id: i64, new_path: &Path, witness: &MutationExecutionWitness) -> Result<()> {
    use crate::db_thread;

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get old path from DB (read-only)
    let track = db.get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;
    let old_path = track.path;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(new_path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                new_path.display(),
            )
        })?;

    // Route write through signal_sender
    sender.update_track_path(&old_path, &relative_path.to_string_lossy(), witness);

    Ok(())
}

/// Execute UpdateScanStatePath mutation - update scan state for relocated file.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_update_scan_state_path(
    _db: &Database,
    source: &str,
    inode: i64,
    new_path: &Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(new_path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                new_path.display(),
            )
        })?;

    // Route write through signal_sender
    sender.update_scan_state_path(source, inode, &relative_path.to_string_lossy(), witness);

    Ok(())
}

/// Execute DropFromIndex mutation - remove track from index.
///
/// Uses read-only DB for lookup, routes writes through signal_sender.
pub fn execute_drop_from_index(
    db: &Database,
    track_id: i64,
    inode: Option<i64>,
    source: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get track path from DB (read-only)
    let track = db.get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Delete the track via signal_sender
    sender.drop_from_index(&track.path, witness);

    // Also delete scan_state entry if inode/source provided
    if let (Some(inode), Some(source)) = (inode, source) {
        sender.delete_scan_state_by_inode(source, inode, witness);
    }

    Ok(())
}

/// Execute UpdateTrack mutation - full metadata update for out-of-band changes.
///
/// Uses read-only DB for lookup, routes write through signal_sender.
pub fn execute_update_track(
    db: &Database,
    _track_id: i64,
    path: &Path,
    metadata: &ExtractedMetadata,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread::{self, TrackData};

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert absolute path to relative for storage
    let relative_path = resolver
        .to_relative(path)
        .with_context(|| {
            format!(
                "Path {} does not match root. Check config.kdl roots.",
                path.display(),
            )
        })?;
    let rel_path_str = relative_path.to_string_lossy();

    // Verify track exists (read-only check)
    let _existing = db.get_track_by_path(&rel_path_str)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", rel_path_str))?;

    // Build TrackData from ExtractedMetadata
    let track_data = TrackData {
        inode: metadata.inode,
        file_size: metadata.file_size,
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    // Route write through signal_sender
    sender.update_track_metadata(&rel_path_str, track_data, metadata.tags.clone(), witness);

    Ok(())
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
/// Writes are routed through the db_thread's write connection via the sender.
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
    sender: &crate::db_thread::SignalWriteSender,
    witness: &crate::corpus::computations::ComputationWitness,
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
            // Track mismatch direction for in-memory classification.
            //
            // IMPORTANT: When both sides have non-empty values but differ, we MUST treat
            // this as a conflict - even if one is a subset of the other. The mismatch
            // storage format (tag-level with joined strings) cannot represent subset
            // relationships, and the OobSync resolution query expects one side to be NULL
            // for true one-direction syncs.
            //
            // One-direction sync classification is only valid when one side is EMPTY
            // (tag exists on one side only, not a value difference).
            if !db_values.is_empty() && !disk_values.is_empty() {
                // Both sides have values - always a conflict regardless of subset relationship
                result.has_conflict = true;
            } else {
                // True one-direction: tag exists on only one side
                let db_has_extras = !db_set.difference(&disk_set).collect::<Vec<_>>().is_empty();
                let disk_has_extras = !disk_set.difference(&db_set).collect::<Vec<_>>().is_empty();

                match (db_has_extras, disk_has_extras) {
                    (true, true) => result.has_conflict = true,
                    (false, true) => result.has_extra_disk = true,
                    (true, false) => result.has_extra_db = true,
                    (false, false) => {} // Shouldn't happen if sets differ
                }
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

            // Record mismatch via db_thread
            sender.record_tag_mismatch(
                track_id,
                &tag_name,
                db_display.as_deref(),
                disk_display.as_deref(),
                witness,
            );
        } else {
            // Clear any existing mismatch for this field (now in sync)
            sender.clear_tag_mismatch(track_id, &tag_name, witness);
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
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::db_thread;
    use std::os::unix::fs::MetadataExt;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let mut affected_paths = Vec::new();

    for (track_id, abs_path) in tracks {
        // Get track info (need relative path for DB operations)
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue, // Skip missing tracks
        };

        // Read current disk mtime
        let metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = metadata.mtime_nsec() as i64;

        // Update scan_state mtime via db_thread (fire-and-forget)
        sender.update_scan_state_mtime(
            &track.path,
            mtime_secs,
            mtime_nanos,
            witness,
        );

        // Clear MtimeOnlyMismatch signal via db_thread
        sender.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            &track.path,
            witness,
        );

        affected_paths.push(abs_path.clone());
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
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::db_thread::{self, ScanStateData};
    use std::os::unix::fs::MetadataExt;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let mut affected_paths = Vec::new();

    for (track_id, abs_path) in tracks {
        // Get track info (need relative path and old inode for DB operations)
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue, // Skip missing tracks
        };

        // Read current disk metadata
        let metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let new_inode = metadata.ino() as i64;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = metadata.mtime_nsec() as i64;
        let file_size = metadata.len() as i64;

        // Update track.inode to the new value via db_thread
        sender.update_track_inode(&track.path, new_inode, witness);

        // Delete old scan_state entry (keyed by old inode) via db_thread
        sender.delete_scan_state_by_inode(&track.source, track.inode, witness);

        // Insert new scan_state entry with new inode and current mtime via db_thread
        sender.upsert_scan_state(
            &track.path,
            &track.source,
            ScanStateData {
                inode: new_inode,
                mtime_secs,
                mtime_nanos,
                file_size,
            },
            witness,
        );

        // Clear InodeChanged signal via db_thread
        sender.clear_file_signal(
            CorpusFileSignalType::InodeChanged.into(),
            &track.path,
            witness,
        );

        affected_paths.push(abs_path.clone());
    }

    Ok(affected_paths)
}

/// Execute ApplyDbTagsToDisk mutation.
///
/// For each track: writes DB tags to disk file via `write_file_tags()`,
/// which handles scan_state mtime, tag_mismatches, and OOB signal cleanup.
/// Used to reject disk-side changes and restore DB state to disk.
pub fn execute_apply_db_tags_to_disk(
    db: &Database,
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::tags::{write_file_tags, TagSet};

    let token = super::sealed::MutationToken::new();
    let mut affected_paths = Vec::new();

    for (track_id, abs_path) in tracks {
        // Get DB tags and convert to TagSet
        let db_tags = db.get_track_tags(*track_id)?;
        let tag_set = TagSet::new(
            db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
        );

        // Write tags to disk using the consolidated write path
        // (also clears OOB signals, tag_mismatches, and updates scan_state mtime)
        write_file_tags(abs_path, &tag_set, &token, witness)
            .with_context(|| format!("Failed to write tags to {}", abs_path.display()))?;

        affected_paths.push(abs_path.clone());
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
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::corpus::tags::TagSet;
    use crate::db_thread;
    use std::os::unix::fs::MetadataExt;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let mut affected_paths = Vec::new();

    for (track_id, abs_path) in tracks {
        // Get track info (need relative path for DB operations)
        let track = match db.get_track_by_id(*track_id)? {
            Some(t) => t,
            None => continue,
        };

        // Read disk tags using TagSet
        let disk_tagset = TagSet::from_file(abs_path)
            .with_context(|| format!("Failed to read tags from {}", abs_path.display()))?;

        // Update DB with disk tags via db_thread
        sender.set_track_tags(&track.path, disk_tagset.into_vec(), witness);

        // Read disk mtime
        let file_metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let mtime_secs = file_metadata.mtime();
        let mtime_nanos = file_metadata.mtime_nsec() as i64;

        // Update scan_state mtime via db_thread
        sender.update_scan_state_mtime(
            &track.path,
            mtime_secs,
            mtime_nanos,
            witness,
        );

        // Clear tag_mismatches for this track via db_thread
        sender.clear_tag_mismatches_for_track(&track.path, witness);

        // Clear OOB signals via db_thread
        sender.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagSync.into(),
            &track.path,
            witness,
        );
        sender.clear_file_signal(
            CorpusFileSignalType::OutOfBandTagConflict.into(),
            &track.path,
            witness,
        );
        sender.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            &track.path,
            witness,
        );

        affected_paths.push(abs_path.clone());
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
        } => execute_update_scan_state(db, source, *inode, *mtime_secs, *mtime_nanos, *file_size, path, witness),

        Mutation::CleanupStaleScanState {
            source,
            valid_inodes,
        } => execute_cleanup_stale(db, source, valid_inodes, witness),

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
        } => execute_update_scan_state_path(db, source, *inode, new_path, witness),

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
        Mutation::AcknowledgeMtimeOnly { tracks } => {
            execute_acknowledge_mtime_only(db, tracks, witness).map(|_| ())
        }

        Mutation::AcknowledgeInodeChanged { tracks } => {
            execute_acknowledge_inode_changed(db, tracks, witness).map(|_| ())
        }

        Mutation::ApplyDbTagsToDisk { tracks } => {
            execute_apply_db_tags_to_disk(db, tracks, witness).map(|_| ())
        }

        Mutation::AssimilateDiskTagsToDb { tracks } => {
            execute_assimilate_disk_tags_to_db(db, tracks, witness).map(|_| ())
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
