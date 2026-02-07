//! Indexing Operations
//!
//! Handles execution of indexing-related mutations:
//! - IndexFileFromPath: Extract metadata and index a file

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::types::{CorpusFileSignalType, FileSource};
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::corpus::tags::TagSet;

/// File types that should trigger ShitFormat signal (non-Vorbis containers).
/// Includes lossy formats with poor metadata and lossless needing remux.
const SHIT_FORMAT_TYPES: &[&str] = &["mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv"];

/// Check if a file type is a "shit format" (non-Vorbis container).
fn is_shit_format(file_type: &str) -> bool {
    let file_type_lower = file_type.to_lowercase();
    SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str())
}
use crate::witch::MutationExecutionWitness;

use super::types::{ExtractedMetadata, Mutation, MutationResult, PendingSignal};

/// Index a track from extracted metadata (internal helper).
///
/// Inserts or updates a track in the database from extracted metadata.
/// Tags are stored in corpus_tags table (or inbox_tags for inbox files).
///
/// Routes write through signal_sender (fire-and-forget).
fn index_track_from_metadata(
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
///
/// Returns pending signals to emit post-execution. Signals are determined from
/// extracted metadata BEFORE the async DB write, avoiding race conditions where
/// a post-execution DB read might not see the write yet.
pub fn execute_index_file_from_path(_db: &ReadOnlyDb<'_>, path: &Path, source: &str, witness: &MutationExecutionWitness) -> Result<Vec<PendingSignal>> {
    use crate::corpus::metadata;

    // Extract audio properties (returns ExtractedMetadata with empty tags)
    let mut extracted = metadata::extract_metadata(path, source)
        .with_context(|| format!("Failed to extract metadata from {:?}", path))?;

    // Read tags using TagSet and populate the extracted metadata
    extracted.tags = TagSet::from_file(path)
        .with_context(|| format!("Failed to read tags from {:?}", path))?;

    // Build pending signals from extracted metadata BEFORE the async DB write.
    // This avoids the race condition where post-execution DB queries don't see
    // the write yet (fire-and-forget pattern).
    let mut pending_signals = Vec::new();
    let resolver = paths::get_resolver();
    if let Some(rel) = resolver.to_relative(path) {
        let rel_str = rel.to_string_lossy().to_string();

        // CorruptFile if fingerprint extraction failed
        if extracted.fingerprint.is_none() {
            pending_signals.push(PendingSignal::FileSignal {
                signal_type: CorpusFileSignalType::CorruptFile,
                path: rel_str.clone(),
            });
        }

        // ShitFormat if non-Vorbis container
        if is_shit_format(&extracted.file_type) {
            let metadata_json = serde_json::json!({ "file_type": extracted.file_type }).to_string();
            pending_signals.push(PendingSignal::FileSignalWithMetadata {
                signal_type: CorpusFileSignalType::ShitFormat,
                path: rel_str,
                metadata_json,
            });
        }
    }

    // Note: _db is unused - index_track_from_metadata routes through signal_sender
    index_track_from_metadata(_db, path, source, &extracted, witness)?;

    Ok(pending_signals)
}

// ============================================================================
// Signal Resolution Executors
// ============================================================================

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

/// Execute DropDirectoryFromIndex mutation - remove directory and contents from index.
///
/// Removes:
/// - Directory entry from files table
/// - All file entries under the directory from files table
/// - All audio_info entries for those files
/// - MissingDirectory signal for the directory
/// - MissingFile signals for files within
pub fn execute_drop_directory_from_index(
    db: &ReadOnlyDb<'_>,
    directory_path: &Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;
    use crate::corpus::db::types::CorpusFileSignalType;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    let dir_str = directory_path.to_string_lossy().to_string();

    // Get all files under this directory from the index
    let audio_files = db.get_audio_files_by_path_prefix(&dir_str).unwrap_or_default();

    crate::logging::log_general(format!(
        "[MUTATION] DropDirectoryFromIndex: removing {} files from {}",
        audio_files.len(),
        dir_str
    ));

    // Drop each file from the index
    for audio_file in &audio_files {
        let file_path = audio_file.path();
        let inode = audio_file.entry.inode;
        sender.drop_from_index(file_path, witness);
        sender.drop_file_index_by_inode("corpus", inode, witness);
        // Clear all corpus signals for this inode (MissingFile, CorruptFile, etc.)
        sender.clear_all_corpus_signals(inode, witness);
    }

    // Drop the directory entry itself from files table
    // We need to get the directory's inode first
    if let Ok(Some(dir_entry)) = db.get_file_entry_by_path(&dir_str, "corpus") {
        sender.drop_file_index_by_inode("corpus", dir_entry.inode, witness);
    }

    // Clear MissingDirectory signal
    sender.clear_file_signal(
        CorpusFileSignalType::MissingDirectory.into(),
        &dir_str,
        witness,
    );

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

/// A single tag mismatch between DB and disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagMismatch {
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_value: Option<String>,
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
    /// All detected mismatches with their values.
    pub mismatches: Vec<TagMismatch>,
}

impl TagVerifyResult {
    fn empty() -> Self {
        Self { has_conflict: false, has_extra_disk: false, has_extra_db: false, mismatches: Vec::new() }
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

/// Execute tag verification - compare in-file tags with database.
///
/// Returns a `TagVerifyResult` with classification flags and the actual mismatches.
/// Computations use this for in-memory classification, then emit signals with
/// the mismatch details serialized as metadata.
///
/// Note: This function is public because it's called from corpus::computations.
pub fn execute_verify_tags(
    db: &crate::corpus::db::ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
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

            // Collect mismatch for signal metadata
            result.mismatches.push(TagMismatch {
                field: tag_name.clone(),
                db_value: db_display,
                disk_value: disk_display,
            });
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
/// - Spawned from ApplyTagOps (incremental tag edit step 2)
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
    sender.set_index_track_tags(&rel_path_str, disk_tagset, witness);

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
// Signal Emission Executors
// ============================================================================

/// Execute EmitCanonicalTag mutation - emit a CanonicalTag signal to whitelist a value.
///
/// Creates a CanonicalTag aggregate signal that marks a compound-looking value as
/// a single canonical entity (e.g., "Rinse & Repeat" is a band name, not a collaboration).
/// Also clears the CompoundTagValue signal for this value so it won't be flagged again.
pub fn execute_emit_canonical_tag(
    tag_name: &str,
    canonical_value: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::db::types::AggregateSignalType;
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Key format: "{tag_name}:{tag_value}" (e.g., "artist:Rinse & Repeat")
    let canonical_key = format!("{}:{}", tag_name, canonical_value);

    // Metadata for the CanonicalTag signal
    let metadata = serde_json::json!({
        "tag_name": tag_name,
        "canonical_value": canonical_value,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    // Emit CanonicalTag signal
    sender.ensure_aggregate_signal(
        AggregateSignalType::CanonicalTag,
        &canonical_key,
        Some(&metadata.to_string()),
        witness,
    );

    // Clear CompoundTagValue signal for this value (it's now whitelisted)
    // Key format for CompoundTagValue: "{tag_name}:{compound_value}"
    let compound_key = format!("{}:{}", tag_name, canonical_value);
    sender.clear_aggregate_signal(
        AggregateSignalType::CompoundTagValue,
        &compound_key,
        witness,
    );

    crate::logging::log_general(format!(
        "[MUTATION] EmitCanonicalTag: {} = {:?}",
        tag_name, canonical_value
    ));

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
    db: &ReadOnlyDb<'_>,
    mutation: &Mutation,
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    // IndexFileFromPath returns pending signals; other mutations don't.
    // Handle it specially to capture the signals.
    if let Mutation::IndexFileFromPath { path, source } = mutation {
        return match execute_index_file_from_path(db, path, source, witness) {
            Ok(pending_signals) => MutationResult {
                _mutation: mutation.clone(),
                success: true,
                error: None,
                _duration_ms: start.elapsed().as_millis() as u64,
                spawn_mutations: Vec::new(),
                pending_signals,
            },
            Err(e) => MutationResult {
                _mutation: mutation.clone(),
                success: false,
                error: Some(format!("{:#}", e)),
                _duration_ms: start.elapsed().as_millis() as u64,
                spawn_mutations: Vec::new(),
                pending_signals: Vec::new(),
            },
        };
    }

    let result = match mutation {
        // Already handled above with early return
        Mutation::IndexFileFromPath { .. } => unreachable!(),

        // Signal resolution mutations
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

        Mutation::DropDirectoryFromIndex { directory_path } => {
            execute_drop_directory_from_index(db, directory_path, witness)
        }

        // OOB resolution mutations (batch, for legacy support)
        Mutation::AcknowledgeMtimeOnly { tracks } => {
            execute_acknowledge_mtime_only(db, tracks, witness).map(|_| ())
        }

        // OBSOLETE: InodeChanged signals removed in v3 migration
        // Now exposed as MissingFile + UnindexedFile pair
        #[allow(deprecated)]
        Mutation::AcknowledgeInodeChanged { .. } => {
            // No-op - signal type no longer exists
            Ok(())
        }

        // Single-file tag sync mutations
        Mutation::ApplyDbTagsToDisk { inode, path } => {
            execute_apply_db_tags_to_disk(db, *inode, path, witness)
        }

        Mutation::AssimilateDiskTagsToDb { inode, path } => {
            execute_assimilate_disk_tags_to_db(db, *inode, path, witness)
        }

        // Signal emission mutations
        Mutation::EmitCanonicalTag { tag_name, canonical_value } => {
            execute_emit_canonical_tag(tag_name, canonical_value, witness)
        }

        // Note: ApplyTagOps is handled by tag_edit.rs (spawns ApplyDbTagsToDisk)
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
        pending_signals: Vec::new(), // Only IndexFileFromPath carries pending signals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            tags: TagSet::new(vec![
                ("artist".to_string(), "Test Artist".to_string()),
                ("album".to_string(), "Test Album".to_string()),
            ]),
        };

        assert_eq!(metadata.get_tag("artist"), Some("Test Artist"));
        assert_eq!(metadata.get_tag("album"), Some("Test Album"));
        assert_eq!(metadata.get_tag("missing"), None);
    }
}
