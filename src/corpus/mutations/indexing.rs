//! Indexing Operations
//!
//! Handles execution of indexing-related mutations:
//! - IndexTrack: Insert or update a track in the database
//! - UpdateFileEntry: Update file entry for incremental scanning
//! - CleanupStaleFiles: Remove stale file entries

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::types::FileSource;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::witch::MutationExecutionWitness;

use super::types::{ExtractedMetadata, Mutation, MutationResult};

/// Execute an IndexTrack mutation.
///
/// Inserts or updates a track in the database from extracted metadata.
/// Tags are stored in corpus_tags table (or inbox_tags for inbox files).
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_index_track(
    _db: &ReadOnlyDb<'_>,
    path: &Path,
    source: &str,
    metadata: &ExtractedMetadata,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread::{self, FileData, AudioData};
    use std::time::UNIX_EPOCH;

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

    // Get file metadata using portable API (consistent with comparison code)
    let file_metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read file metadata: {}", path.display()))?;
    let (mtime_secs, mtime_nanos) = file_metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0));

    // Build FileData from metadata
    let file_data = FileData {
        inode: metadata.inode,
        source: source.to_string(),
        is_dir: false,
        mtime_secs,
        mtime_nanos,
        file_size: metadata.file_size,
    };

    // Build AudioData from ExtractedMetadata
    let audio_data = AudioData {
        file_type: metadata.file_type.clone(),
        duration_ms: metadata.duration_ms,
        bitrate_kbps: metadata.bitrate_kbps,
        sample_rate: metadata.sample_rate,
        fingerprint: metadata.fingerprint.clone(),
    };

    // Route write through signal_sender (fire-and-forget)
    sender.index_audio_file(
        &rel_path_str,
        file_data,
        audio_data,
        metadata.tags.clone(),
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
pub fn execute_index_file_from_path(_db: &ReadOnlyDb<'_>, path: &Path, source: &str, witness: &MutationExecutionWitness) -> Result<()> {
    use crate::corpus::metadata;
    use crate::corpus::tags::TagSet;

    // Extract audio properties (returns ExtractedMetadata with empty tags)
    let mut extracted = metadata::extract_metadata(path, source)
        .with_context(|| format!("Failed to extract metadata from {:?}", path))?;

    // Read tags using TagSet and populate the extracted metadata
    let tag_set = TagSet::from_file(path)
        .with_context(|| format!("Failed to read tags from {:?}", path))?;
    extracted.tags = tag_set.into_vec();

    // Note: _db is unused - execute_index_track routes through signal_sender
    execute_index_track(_db, path, source, &extracted, witness)
}

/// Execute an UpdateFileEntry mutation.
///
/// Updates the file entry for a file, enabling incremental scanning.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_update_file_entry(
    _db: &ReadOnlyDb<'_>,
    source: &str,
    inode: u64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: u64,
    path: &Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread::{self, FileEntryData};

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

    let file_entry = FileEntryData {
        inode: inode as i64,
        mtime_secs,
        mtime_nanos,
        file_size: file_size as i64,
    };

    // Route write through signal_sender
    sender.upsert_file_entry(&relative_path.to_string_lossy(), source, file_entry, witness);

    Ok(())
}

/// Execute a CleanupStaleFiles mutation.
///
/// Removes stale file entries for files that no longer exist.
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_cleanup_stale_files(
    _db: &ReadOnlyDb<'_>,
    source: &str,
    valid_inodes: &[u64],
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    let valid_i64: Vec<i64> = valid_inodes.iter().map(|&i| i as i64).collect();
    sender.cleanup_stale_files(source, valid_i64, witness);

    Ok(())
}

// ============================================================================
// Signal Resolution Executors
// ============================================================================

/// Execute UpdateTrackPath mutation - update path for relocated file.
///
/// Note: This function needs the source to determine which root to use for
/// relative path conversion. It fetches the file's source from the database.
///
/// Uses read-only DB for lookup, routes write through signal_sender.
pub fn execute_update_track_path(db: &ReadOnlyDb<'_>, inode: i64, new_path: &Path, witness: &MutationExecutionWitness) -> Result<()> {
    use crate::db_thread;

    let resolver = paths::get_resolver();
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get old path from DB (read-only)
    // UpdateTrackPath only operates on corpus files
    let audio_file = db.get_audio_file_by_inode(inode, FileSource::Corpus)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;
    let old_path = audio_file.path().to_string();

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

/// Execute UpdateFilePath mutation - update file path for relocated file.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_update_file_path(
    _db: &ReadOnlyDb<'_>,
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
    sender.update_file_path(source, inode, &relative_path.to_string_lossy(), witness);

    Ok(())
}

/// Execute DropFromIndex mutation - remove track from index.
///
/// Routes writes through signal_sender. Path is passed directly from mutation.
/// For orphaned signals (inode=None), just clears audio_info/tags - no files table entry to drop.
pub fn execute_drop_from_index(
    _db: &ReadOnlyDb<'_>,
    path: &Path,
    inode: Option<i64>,
    source: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert path to string for DB operations
    let path_str = path.to_string_lossy();

    // Delete audio_info and corpus_tags entries via signal_sender
    sender.drop_from_index(&path_str, witness);

    // Also delete files table entry if inode and source are provided
    if let (Some(inode), Some(source)) = (inode, source) {
        sender.drop_file_index_by_inode(source, inode, witness);
    }

    Ok(())
}

/// Execute UpdateTrack mutation - full metadata update for out-of-band changes.
///
/// Uses read-only DB for lookup, routes write through signal_sender.
pub fn execute_update_track(
    db: &ReadOnlyDb<'_>,
    _inode: i64,
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

    // Verify audio file exists (read-only check)
    let _existing = db.get_audio_file_by_path(&rel_path_str)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found: {}", rel_path_str))?;

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
/// Used by computations to classify OOB signals without querying persisted state
/// (in-memory classification for immediate use before async db_thread writes commit).
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
/// This is a read-mostly operation that emits OOB signals for detected mismatches.
/// Writes are routed through the db_thread's write connection via the sender.
///
/// Returns a `TagVerifyResult` summarizing the mismatch directions found.
/// Computations use this for in-memory classification (async db_thread writes may lag).
///
/// Note: This function is public because it's called from corpus::computations.
pub fn execute_verify_tags(
    db: &crate::corpus::db::ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    sender: &crate::db_thread::SignalWriteSender,
    witness: &crate::corpus::computations::ComputationWitness,
) -> Result<TagVerifyResult> {
    use crate::corpus::tags::TagSet;
    use std::collections::HashSet;

    // Verify audio file exists in the index
    let _audio_info = db
        .get_audio_info(inode)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;

    // Get tags from database as TagSet
    let db_tags = db.get_corpus_tags(inode)?;
    let db_tagset = TagSet::new(
        db_tags.into_iter().map(|t: crate::corpus::db::types::AudioTag| (t.tag_name, t.tag_value))
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
                inode,
                &tag_name,
                db_display.as_deref(),
                disk_display.as_deref(),
                witness,
            );
        } else {
            // Clear any existing mismatch for this field (now in sync)
            sender.clear_tag_mismatch(inode, &tag_name, witness);
        }
    }

    Ok(result)
}

// ============================================================================
// OOB Resolution Executors
// ============================================================================

/// Execute AcknowledgeMtimeOnly mutation.
///
/// For each track: updates file mtime in files table to match current disk mtime,
/// then clears the MtimeOnlyMismatch signal. Used when disk file mtime
/// changed but tags are identical.
pub fn execute_acknowledge_mtime_only(
    db: &ReadOnlyDb<'_>,
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::db_thread;
    use std::time::UNIX_EPOCH;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let mut affected_paths = Vec::new();

    for (inode, abs_path) in tracks {
        // Get audio file info (need relative path for DB operations)
        // OOB mtime resolution only operates on corpus files
        let audio_file = match db.get_audio_file_by_inode(*inode, FileSource::Corpus)? {
            Some(af) => af,
            None => continue, // Skip missing files
        };

        // Read current disk mtime using portable API (consistent with comparison code)
        let metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let (mtime_secs, mtime_nanos) = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
            .unwrap_or((0, 0));

        // Update file mtime via db_thread using (source, inode) key
        sender.update_file_mtime(
            audio_file.entry.source.as_str(),
            audio_file.inode(),
            mtime_secs,
            mtime_nanos,
            witness,
        );

        // Clear MtimeOnlyMismatch signal via db_thread
        sender.clear_file_signal(
            CorpusFileSignalType::MtimeOnlyMismatch.into(),
            audio_file.path(),
            witness,
        );

        affected_paths.push(abs_path.clone());
    }

    Ok(affected_paths)
}

/// Execute AcknowledgeInodeChanged mutation.
///
/// For each track: updates track.inode to the new inode, deletes old files table
/// entry, creates new file entry with current mtime, and clears the InodeChanged signal.
/// Tag differences are handled separately through the OOB tag resolution flow.
pub fn execute_acknowledge_inode_changed(
    db: &ReadOnlyDb<'_>,
    tracks: &[(i64, std::path::PathBuf)],
    witness: &MutationExecutionWitness,
) -> Result<Vec<std::path::PathBuf>> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::db_thread::{self, FileEntryData};
    use std::os::unix::fs::MetadataExt;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let mut affected_paths = Vec::new();

    for (old_inode, abs_path) in tracks {
        // Get audio file info (need relative path and old inode for DB operations)
        // OOB inode changed resolution only operates on corpus files
        let audio_file = match db.get_audio_file_by_inode(*old_inode, FileSource::Corpus)? {
            Some(af) => af,
            None => continue, // Skip missing files
        };
        let file_path = audio_file.path();
        let source_str = audio_file.entry.source.as_str();

        // Read current disk metadata
        let metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let new_inode = metadata.ino() as i64;
        let mtime_secs = metadata.mtime();
        let mtime_nanos = metadata.mtime_nsec() as i64;
        let file_size = metadata.len() as i64;

        // Update audio_info.inode to the new value via db_thread
        sender.update_track_inode(file_path, new_inode, witness);

        // Delete old files table entry (keyed by old inode) via db_thread
        sender.drop_file_index_by_inode(source_str, *old_inode, witness);

        // Insert new file entry with new inode and current mtime via db_thread
        sender.upsert_file_entry(
            file_path,
            source_str,
            FileEntryData {
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
            file_path,
            witness,
        );

        affected_paths.push(abs_path.clone());
    }

    Ok(affected_paths)
}

/// Execute ApplyDbTagsToDisk mutation (single-track).
///
/// - Reads tags from database (source of truth)
/// - Writes to disk via write_file_tags()
/// - Updates file mtime after write (handled by write_file_tags)
/// - Clears needs_disk_flush flag
/// - Clears OOB signals (handled by write_file_tags)
///
/// Used for:
/// - OOB sync resolution (reject disk changes, restore DB state to disk)
/// - Spawned from SetTrackTagsDb (DB-first pattern step 2)
pub fn execute_apply_db_tags_to_disk(
    db: &ReadOnlyDb<'_>,
    inode: i64,
    abs_path: &std::path::Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::corpus::tags::{write_file_tags, TagSet};
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert abs_path to relative for DB operations.
    // Use mutation's abs_path parameter, not file's path from DB (may be stale).
    let resolver = paths::get_resolver();
    let relative_path = resolver
        .to_relative(abs_path)
        .with_context(|| format!(
            "Path {} does not match root. Check config.kdl roots.",
            abs_path.display(),
        ))?;
    let rel_path_str = relative_path.to_string_lossy();

    // Get DB tags and convert to TagSet
    let db_tags = db.get_corpus_tags(inode)?;
    let tag_set = TagSet::new(
        db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
    );

    // Write tags to disk using the consolidated write path
    // (also clears OOB signals and updates file mtime)
    let token = super::sealed::MutationToken::new();
    write_file_tags(abs_path, &tag_set, &token, witness)
        .with_context(|| format!("Failed to write tags to {}", abs_path.display()))?;

    // Clear needs_disk_flush flag
    sender.set_needs_disk_flush(&rel_path_str, false, witness);

    Ok(())
}

/// Execute AssimilateDiskTagsToDb mutation (single-track).
///
/// - Reads tags from disk file
/// - Writes to database, overwriting DB values
/// - Updates file mtime to match disk
/// - Clears OOB signals
///
/// Used for OOB sync resolution (accept disk changes, update DB to match disk).
pub fn execute_assimilate_disk_tags_to_db(
    db: &ReadOnlyDb<'_>,
    inode: i64,
    abs_path: &std::path::Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::db::types::CorpusFileSignalType;
    use crate::corpus::paths;
    use crate::corpus::tags::TagSet;
    use crate::db_thread;
    use std::os::unix::fs::MetadataExt;
    use std::time::UNIX_EPOCH;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Convert abs_path to relative for DB operations.
    // IMPORTANT: We use the mutation's abs_path parameter, NOT file's path from DB.
    // When spawned from Transcode, the DB read connection may not have seen the
    // path update yet (async write via db_thread), causing stale reads.
    let resolver = paths::get_resolver();
    let relative_path = resolver
        .to_relative(abs_path)
        .with_context(|| format!(
            "Path {} does not match root. Check config.kdl roots.",
            abs_path.display(),
        ))?;
    let rel_path_str = relative_path.to_string_lossy();

    // Get audio file source (needed for files table key).
    // Source doesn't change during transcode, so this read is safe.
    // Tag updates only operate on corpus files.
    let audio_file = db.get_audio_file_by_inode(inode, FileSource::Corpus)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;
    let source = audio_file.entry.source.as_str();

    // Read disk tags using TagSet
    let disk_tagset = TagSet::from_file(abs_path)
        .with_context(|| format!("Failed to read tags from {}", abs_path.display()))?;

    // Update DB with disk tags via db_thread
    // Use rel_path_str (from mutation param), not track.path (potentially stale)
    sender.set_track_tags(&rel_path_str, disk_tagset.into_vec(), witness);

    // Read disk metadata using portable API
    let file_metadata = std::fs::metadata(abs_path)
        .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
    let (mtime_secs, mtime_nanos) = file_metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0));

    // Get current inode from filesystem (not from potentially stale DB read).
    // After transcode, the inode changed and we need the NEW inode for files table lookup.
    let current_inode = file_metadata.ino() as i64;

    // Update file mtime via db_thread using (source, inode) key
    sender.update_file_mtime(
        source,
        current_inode,
        mtime_secs,
        mtime_nanos,
        witness,
    );

    // Clear tag_mismatches for this track via db_thread
    sender.clear_tag_mismatches_for_track(&rel_path_str, witness);

    // Clear OOB signals via db_thread
    sender.clear_file_signal(
        CorpusFileSignalType::OutOfBandTagSync.into(),
        &rel_path_str,
        witness,
    );
    sender.clear_file_signal(
        CorpusFileSignalType::OutOfBandTagConflict.into(),
        &rel_path_str,
        witness,
    );
    sender.clear_file_signal(
        CorpusFileSignalType::MtimeOnlyMismatch.into(),
        &rel_path_str,
        witness,
    );

    Ok(())
}

// ============================================================================
// Fingerprint Rebuild Chain
// ============================================================================
//
// Three-mutation chain for fingerprint regeneration:
// 1. ClearAllFingerprints → spawns ScheduleFingerprintRefill
// 2. ScheduleFingerprintRefill → spawns N RefillSingleFingerprint mutations
// 3. RefillSingleFingerprint → fingerprints one track

/// Execute ClearAllFingerprints mutation.
///
/// Clears all fingerprints from the database in one SQL UPDATE.
/// Returns a spawned ScheduleFingerprintRefill mutation to queue the re-fingerprinting.
pub fn execute_clear_all_fingerprints(
    _db: &ReadOnlyDb<'_>,
    _witness: &MutationExecutionWitness,
) -> Result<Vec<crate::witch::SpawnedMutation>> {
    // TODO: This operation needs to be routed through signal_sender and use audio_info table.
    // The old code wrote directly to a non-existent 'tracks' table.
    todo!("ClearAllFingerprints needs migration to audio_info table and signal_sender pattern")
}

/// Execute ScheduleFingerprintRefill mutation.
///
/// Queries all audio files and spawns a RefillSingleFingerprint mutation for each.
pub fn execute_schedule_fingerprint_refill(
    db: &ReadOnlyDb<'_>,
    witness: &MutationExecutionWitness,
) -> Result<Vec<crate::witch::SpawnedMutation>> {
    use crate::logging::log_general;
    use crate::corpus::db::types::FileSource;

    let audio_files = db.get_all_audio_files(FileSource::Corpus)?;
    let count = audio_files.len();

    log_general(format!(
        "[MUTATION] ScheduleFingerprintRefill: spawning {} individual fingerprint mutations",
        count
    ));

    let spawned: Vec<_> = audio_files
        .into_iter()
        .map(|audio_file| {
            witness.spawn_mutation(super::types::Mutation::RefillSingleFingerprint {
                inode: audio_file.inode(),
                path: audio_file.entry.path,
            })
        })
        .collect();

    Ok(spawned)
}

/// Execute RefillSingleFingerprint mutation.
///
/// Fingerprints one file from full audio. Emits CorruptFile signal on failure,
/// clears CorruptFile signal on success.
pub fn execute_refill_single_fingerprint(
    _db: &ReadOnlyDb<'_>,
    _inode: i64,
    _path: &str,
    _witness: &MutationExecutionWitness,
) -> Result<()> {
    // TODO: This operation needs to be routed through signal_sender and use audio_info table.
    // The old code wrote directly to a non-existent 'tracks' table.
    todo!("RefillSingleFingerprint needs migration to audio_info table and signal_sender pattern")
}

// ============================================================================
// Single Mutation Dispatch
// ============================================================================

/// Execute a single indexing mutation.
///
/// Convenience function for executing individual mutations.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &ReadOnlyDb<'_>,
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

        Mutation::UpdateFileEntry {
            source,
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
            path,
        } => execute_update_file_entry(db, source, *inode, *mtime_secs, *mtime_nanos, *file_size, path, witness),

        Mutation::CleanupStaleFiles {
            source,
            valid_inodes,
        } => execute_cleanup_stale_files(db, source, valid_inodes, witness),

        // Signal resolution mutations
        Mutation::UpdateTrackPath {
            inode,
            new_path,
            ..
        } => execute_update_track_path(db, *inode, new_path, witness),

        Mutation::UpdateFilePath {
            source,
            inode,
            new_path,
        } => execute_update_file_path(db, source, *inode, new_path, witness),

        Mutation::DropFromIndex {
            path,
            inode,
            source,
        } => execute_drop_from_index(db, path, *inode, source.as_deref(), witness),

        Mutation::UpdateTrack {
            inode,
            path,
            metadata,
        } => execute_update_track(db, *inode, path, metadata, witness),

        // OOB resolution mutations (batch, for legacy support)
        Mutation::AcknowledgeMtimeOnly { tracks } => {
            execute_acknowledge_mtime_only(db, tracks, witness).map(|_| ())
        }

        Mutation::AcknowledgeInodeChanged { tracks } => {
            execute_acknowledge_inode_changed(db, tracks, witness).map(|_| ())
        }

        // Single-file tag sync mutations
        Mutation::ApplyDbTagsToDisk { inode, path } => {
            execute_apply_db_tags_to_disk(db, *inode, path, witness)
        }

        Mutation::AssimilateDiskTagsToDb { inode, path } => {
            execute_assimilate_disk_tags_to_db(db, *inode, path, witness)
        }

        // Fingerprint rebuild chain (these spawn follow-up mutations)
        Mutation::ClearAllFingerprints => {
            return match execute_clear_all_fingerprints(db, witness) {
                Ok(spawned) => MutationResult {
                    _mutation: mutation.clone(),
                    success: true,
                    error: None,
                    _duration_ms: start.elapsed().as_millis() as u64,
                    spawn_mutations: spawned,
                },
                Err(e) => MutationResult {
                    _mutation: mutation.clone(),
                    success: false,
                    error: Some(format!("{:#}", e)),
                    _duration_ms: start.elapsed().as_millis() as u64,
                    spawn_mutations: Vec::new(),
                },
            };
        }

        Mutation::ScheduleFingerprintRefill => {
            return match execute_schedule_fingerprint_refill(db, witness) {
                Ok(spawned) => MutationResult {
                    _mutation: mutation.clone(),
                    success: true,
                    error: None,
                    _duration_ms: start.elapsed().as_millis() as u64,
                    spawn_mutations: spawned,
                },
                Err(e) => MutationResult {
                    _mutation: mutation.clone(),
                    success: false,
                    error: Some(format!("{:#}", e)),
                    _duration_ms: start.elapsed().as_millis() as u64,
                    spawn_mutations: Vec::new(),
                },
            };
        }

        Mutation::RefillSingleFingerprint { inode, path } => {
            execute_refill_single_fingerprint(db, *inode, path, witness).map(|_| ())
        }

        // Note: SetTrackTagsDb is now handled by tag_edit.rs (spawns ApplyDbTagsToDisk)
        // Note: VerifyTags is now a Computation, not a Mutation.

        _ => Err(anyhow::anyhow!("Not an indexing mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
    };

    MutationResult {
        _mutation: mutation.clone(),
        success,
        error,
        _duration_ms: start.elapsed().as_millis() as u64,
        spawn_mutations: Vec::new(), // Non-spawning indexing mutations
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
