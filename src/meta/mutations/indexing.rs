//! Indexing Operations
//!
//! Handles execution of indexing-related mutations:
//! - IndexFileFromPath: Extract metadata and index a file
//! - UpdateFilePath: Update file path for relocated file
//! - DropFromIndex: Remove track from index
//! - DropDirectoryFromIndex: Remove directory and contents from index
//! - AcknowledgeMtimeOnly: Acknowledge mtime-only change
//! - ApplyDbTagsToDisk: Apply DB tags to disk file
//! - AssimilateDiskTagsToDb: Assimilate disk tags into DB
//! - EmitCanonicalTag: Emit CanonicalTag signal

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::corpus::paths;
use crate::corpus::tags::{self as tags, TagSet};
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::*;
use crate::meta::signals::registry::TypedSignalWrite;
use crate::witch::MutationExecutionWitness;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{
    ExtractedMetadata, Mutation, MutationResult, PendingSignal, SignalClearScope,
};

// Re-export struct definitions from mm-meta
pub use mm_meta::mutations::indexing::{
    AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation, AssimilateDiskTagsToDbMutation,
    DropDirectoryFromIndexMutation, DropExternalMatchMutation, DropFromIndexMutation,
    EmitCanonicalTagMutation, EmitExpectedDuplicateMutation, EmitExpectedMissingTagMutation,
    EmitExpectedOverlapMutation, FlushTagsToDiskMutation, IndexFileFromPathMutation,
    UpdateFilePathMutation,
};

/// Macro to reduce boilerplate for the common `MutationExecutor` pattern where
/// `execute` delegates to a helper function and wraps the result via
/// `MutationResult::from_unit_result`.
///
/// All required trait methods are specified positionally. The optional
/// `paths_for_signal_updates` method is provided via a trailing `key => expr`
/// arm — omit it to keep the trait default.
///
/// Expressions in the body can use `self` (the mutation struct) and `ctx`
/// (the `MutationContext` passed to `execute`).
macro_rules! impl_mutation_executor {
    (
        $struct_name:ident, $variant:ident,
        origin: $origin:expr,
        execution_stage: $stage:expr,
        signal_clear_scope: $scope:expr,
        recomputation_scope: $recomp:expr,
        execute: |$self_:ident, $ctx:ident| $execute_expr:expr,
        affected_inodes: |$self_i:ident| $inodes:expr
        $(, paths_for_signal_updates: |$self_p:ident| $paths:expr)?
        $(,)?
    ) => {
        impl MutationExecutor for $struct_name {
            fn origin(&self) -> super::MutationOrigin { $origin }
            fn execution_stage(&self) -> super::MutationExecutionStage { $stage }

            fn execute(&self, ctx: &MutationContext) -> MutationResult {
                let $self_ = self;
                let $ctx = ctx;
                let start = std::time::Instant::now();
                let result = $execute_expr;
                MutationResult::from_unit_result(Mutation::$variant(self.clone()), result, start)
            }

            fn signal_clear_scope(&self) -> SignalClearScope { $scope }

            fn affected_inodes(&self) -> Vec<i64> {
                let $self_i = self;
                $inodes
            }

            fn recomputation_scope(&self) -> RecomputationScope { $recomp }

            $(
                fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
                    let $self_p = self;
                    $paths
                }
            )?
        }
    };
}

/// File types that should trigger ShitFormat signal (non-Vorbis containers).
/// Includes lossy formats with poor metadata and lossless needing remux.
const SHIT_FORMAT_TYPES: &[&str] = &[
    "mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv",
];

/// Check if a file type is a "shit format" (non-Vorbis container).
fn is_shit_format(file_type: &str) -> bool {
    let file_type_lower = file_type.to_lowercase();
    SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str())
}


// ============================================================================
// MutationExecutor Implementations
// ============================================================================

impl MutationExecutor for IndexFileFromPathMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DB
    }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        match execute_index_file_from_path(
            ctx.read_db,
            &self.path,
            &self.zone,
            ctx.session_id,
            ctx.witness,
        ) {
            Ok(pending_signals) => MutationResult {
                _mutation: Mutation::IndexFileFromPath(self.clone()),
                success: true,
                error: None,
                _duration_ms: start.elapsed().as_millis() as u64,
                spawn_mutations: Vec::new(),
                pending_signals,
                discovered_inodes: Vec::new(),
            },
            Err(e) => MutationResult {
                _mutation: Mutation::IndexFileFromPath(self.clone()),
                success: false,
                error: Some(format!("{:#}", e)),
                _duration_ms: start.elapsed().as_millis() as u64,
                spawn_mutations: Vec::new(),
                pending_signals: Vec::new(),
                discovered_inodes: Vec::new(),
            },
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::MutableOnly
    }
    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }
    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::TAGS | RecomputationScope::FILES
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.path.clone()]
    }
}

impl_mutation_executor!(
    UpdateFilePathMutation, UpdateFilePath,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::MutableOnly,
    recomputation_scope: RecomputationScope::FILES | RecomputationScope::TAGS,
    execute: |s, ctx| execute_update_file_path(
        ctx.read_db, &s.zone, s.inode, &s.new_path,
        s.new_zone.as_deref(), ctx.witness,
    ),
    affected_inodes: |s| vec![s.inode],
);

impl_mutation_executor!(
    DropFromIndexMutation, DropFromIndex,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DiskFlush,
    signal_clear_scope: SignalClearScope::All,
    recomputation_scope: RecomputationScope::FILES | RecomputationScope::DEPLOY,
    execute: |s, ctx| execute_drop_from_index(
        ctx.read_db, &s.path, s.inode, s.zone.as_deref(), ctx.witness,
    ),
    affected_inodes: |s| s.inode.into_iter().collect(),
);

impl_mutation_executor!(
    DropDirectoryFromIndexMutation, DropDirectoryFromIndex,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DiskFlush,
    signal_clear_scope: SignalClearScope::All,
    recomputation_scope: RecomputationScope::FILES | RecomputationScope::DEPLOY,
    execute: |s, ctx| execute_drop_directory_from_index(ctx.read_db, &s.directory_path, ctx.witness),
    affected_inodes: |_s| Vec::new(),
);

impl_mutation_executor!(
    AcknowledgeMtimeOnlyMutation, AcknowledgeMtimeOnly,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::MutableOnly,
    recomputation_scope: RecomputationScope::EMPTY,
    execute: |s, ctx| execute_acknowledge_mtime_only(ctx.read_db, &s.tracks, ctx.witness).map(|_| ()),
    affected_inodes: |s| s.tracks.iter().map(|(inode, _)| *inode).collect(),
    paths_for_signal_updates: |s| s.tracks.iter().map(|(_, path)| path.clone()).collect(),
);

impl_mutation_executor!(
    ApplyDbTagsToDiskMutation, ApplyDbTagsToDisk,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::MutableOnly,
    recomputation_scope: RecomputationScope::TAGS,
    execute: |s, ctx| execute_apply_db_tags_to_disk(
        ctx.read_db, s.inode, &s.path, s.zone, ctx.witness,
    ),
    affected_inodes: |s| vec![s.inode],
    paths_for_signal_updates: |s| vec![s.path.clone()],
);

impl_mutation_executor!(
    FlushTagsToDiskMutation, FlushTagsToDisk,
    origin: super::MutationOrigin::ChainEmitted,
    execution_stage: super::MutationExecutionStage::DiskFlush,
    signal_clear_scope: SignalClearScope::MutableOnly,
    recomputation_scope: RecomputationScope::TAGS,
    execute: |s, ctx| execute_flush_tags_to_disk(
        s.inode, &s.path, &s.expected_tags, s.zone, ctx.read_db, ctx.witness,
    ),
    affected_inodes: |s| vec![s.inode],
    paths_for_signal_updates: |s| vec![s.path.clone()],
);

impl_mutation_executor!(
    AssimilateDiskTagsToDbMutation, AssimilateDiskTagsToDb,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::MutableOnly,
    recomputation_scope: RecomputationScope::TAGS,
    execute: |s, ctx| execute_assimilate_disk_tags_to_db(
        ctx.read_db, s.inode, &s.path, s.zone.as_deref(), ctx.session_id, ctx.witness,
    ),
    affected_inodes: |s| vec![s.inode],
    paths_for_signal_updates: |s| vec![s.path.clone()],
);

impl_mutation_executor!(
    EmitCanonicalTagMutation, EmitCanonicalTag,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::None,
    recomputation_scope: RecomputationScope::TAGS,
    execute: |s, ctx| execute_emit_canonical_tag(
        &s.tag_name, &s.canonical_value, ctx.read_db, ctx.witness,
    ),
    affected_inodes: |_s| Vec::new(),
);

impl_mutation_executor!(
    EmitExpectedOverlapMutation, EmitExpectedOverlap,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::None,
    recomputation_scope: RecomputationScope::FILES,
    execute: |s, ctx| execute_emit_expected_overlap(&s.source_a, &s.source_b, ctx.witness),
    affected_inodes: |_s| Vec::new(),
);

impl_mutation_executor!(
    EmitExpectedDuplicateMutation, EmitExpectedDuplicate,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::None,
    recomputation_scope: RecomputationScope::FILES,
    execute: |s, ctx| execute_emit_expected_duplicate(&s.fingerprint_key, ctx.witness),
    affected_inodes: |_s| Vec::new(),
);

impl_mutation_executor!(
    DropExternalMatchMutation, DropExternalMatch,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::None,
    recomputation_scope: RecomputationScope::EMPTY,
    execute: |s, ctx| execute_drop_external_match(s.inode, ctx.witness),
    affected_inodes: |s| vec![s.inode],
);

impl_mutation_executor!(
    EmitExpectedMissingTagMutation, EmitExpectedMissingTag,
    origin: super::MutationOrigin::Staged,
    execution_stage: super::MutationExecutionStage::DB,
    signal_clear_scope: SignalClearScope::None,
    recomputation_scope: RecomputationScope::TAGS,
    execute: |s, ctx| execute_emit_expected_missing_tag(&s.inodes, ctx.witness),
    affected_inodes: |_s| Vec::new(),
);

/// Index a track from extracted metadata (internal helper).
///
/// Inserts or updates a track in the database from extracted metadata.
/// Tags are stored in corpus_tags table (or inbox_tags for inbox files).
///
/// Routes write through signal_sender (fire-and-forget).
fn index_track_from_metadata(
    _db: &ReadOnlyDb<'_>,
    path: &Path,
    zone: &str,
    metadata: &ExtractedMetadata,
    session_id: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread::{self, AudioData, FileData};

    let sender = write_thread::require_sender()?;

    // Convert absolute path to relative for storage
    let relative_path = paths::resolve_relative(path)?;
    let rel_path_str = relative_path.to_string_lossy();

    // Get file metadata using portable API (consistent with comparison code)
    let file_metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read file metadata: {}", path.display()))?;
    let (mtime_secs, mtime_nanos) = paths::read_mtime(&file_metadata);

    // Build FileData from metadata
    let file_data = FileData {
        inode: metadata.inode,
        zone: zone.to_string(),
        _is_dir: false,
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
        has_pictures: metadata.has_pictures,
        pic_format: metadata.pic_info.as_ref().map(|p| p.format.clone()),
        pic_width: metadata.pic_info.as_ref().map(|p| p.width),
        pic_height: metadata.pic_info.as_ref().map(|p| p.height),
        pic_count: metadata.pic_info.as_ref().map(|p| p.count).unwrap_or(0),
    };

    // Route write through signal_sender (fire-and-forget)
    sender.index_audio_file(
        &rel_path_str,
        file_data,
        audio_data,
        metadata.tags.clone(),
        session_id,
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
pub fn execute_index_file_from_path(
    _db: &ReadOnlyDb<'_>,
    path: &Path,
    zone: &str,
    session_id: &str,
    witness: &MutationExecutionWitness,
) -> Result<Vec<PendingSignal>> {
    use crate::corpus::metadata;

    // Extract audio properties (returns ExtractedMetadata with empty tags)
    let mut extracted = metadata::extract_metadata(path, zone)
        .with_context(|| format!("Failed to extract metadata from {:?}", path))?;

    // Read tags using TagSet and populate the extracted metadata
    extracted.tags =
        tags::from_file(path).with_context(|| format!("Failed to read tags from {:?}", path))?;

    // Build pending signals from extracted metadata BEFORE the async DB write.
    // This avoids the race condition where post-execution DB queries don't see
    // the write yet (fire-and-forget pattern).
    let mut pending_signals = Vec::new();
    let resolver = paths::get_resolver();
    if let Some(rel) = resolver.to_relative(path) {
        let rel_str = rel.to_string_lossy().to_string();

        // CorruptFile if fingerprint extraction failed
        if extracted.fingerprint.is_none() {
            pending_signals.push(TypedSignalWrite::CorruptFile(CorruptFileSignal {
                inode: extracted.inode,
                path: rel_str.clone(),
            }));
        }

        // ShitFormat if non-Vorbis container
        if is_shit_format(&extracted.file_type) {
            pending_signals.push(TypedSignalWrite::ShitFormat(ShitFormatSignal {
                inode: extracted.inode,
                path: rel_str,
                file_type: extracted.file_type.clone(),
            }));
        }
    }

    // Note: _db is unused - index_track_from_metadata routes through signal_sender
    index_track_from_metadata(_db, path, zone, &extracted, session_id, witness)?;

    Ok(pending_signals)
}

// ============================================================================
// Signal Resolution Executors
// ============================================================================

/// Execute UpdateFilePath mutation - update file path for relocated file.
///
/// When `new_zone` is provided and differs from `zone`, also updates the zone
/// column and migrates tags between tag tables.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_update_file_path(
    _db: &ReadOnlyDb<'_>,
    zone: &str,
    inode: i64,
    new_path: &Path,
    new_zone: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    // Convert to relative for storage. The new_path may already be relative
    // (e.g., from signal_moved_file table which stores relative paths like
    // "corpus/web/misc/..."), so skip to_relative() if it's not absolute.
    let relative_path = if new_path.is_absolute() {
        paths::resolve_relative(new_path)?
    } else {
        new_path.to_path_buf()
    };

    // Route write through signal_sender
    sender.update_file_path(
        zone,
        inode,
        &relative_path.to_string_lossy(),
        new_zone,
        witness,
    );

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
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    let dir_str = directory_path.to_string_lossy().to_string();

    // Get all files under this directory from the index
    let audio_files = db
        .get_audio_files_by_path_prefix(&dir_str)
        .unwrap_or_default();

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

    // Drop the directory entry itself from files table and clear its signals
    // We need to get the directory's inode first
    if let Ok(Some(dir_entry)) = db.get_file_entry_by_path(&dir_str, "corpus") {
        sender.drop_file_index_by_inode("corpus", dir_entry.inode, witness);
        // Clear MissingDirectory signal (inode-keyed)
        sender.clear_corpus_signal::<MissingDirectorySignal>(dir_entry.inode, witness);
    }

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
    zone: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    // Convert path to string for DB operations
    let path_str = path.to_string_lossy();

    // Delete audio_info and corpus_tags entries via signal_sender
    sender.drop_from_index(&path_str, witness);

    // Also delete files table entry if inode and zone are provided
    if let (Some(inode), Some(zone)) = (inode, zone) {
        sender.drop_file_index_by_inode(zone, inode, witness);
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
        Self {
            has_conflict: false,
            has_extra_disk: false,
            has_extra_db: false,
            mismatches: Vec::new(),
        }
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
    db: &crate::db::ReadOnlyDb<'_>,
    inode: i64,
    disk_tagset: crate::corpus::tags::TagSet,
) -> Result<TagVerifyResult> {
    use crate::corpus::tags::TagSet;
    use std::collections::HashSet;

    // Verify audio file exists in the index
    let _audio_info = db
        .get_audio_info(inode)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;

    // Get tags from database as TagSet
    let db_tags = db.get_tags::<crate::zones::CorpusZone>(inode)?;
    let db_tagset = TagSet::new(
        db_tags
            .into_iter()
            .map(|t: crate::db::types::AudioTag| (t.tag_name, t.tag_value)),
    );

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
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;
    let mut affected_paths = Vec::new();

    for (inode, abs_path) in tracks {
        // Get audio file info (need relative path for DB operations)
        // OOB mtime resolution only operates on corpus files
        let audio_file = match db.get_audio_file_by_inode(*inode, Zone::Corpus)? {
            Some(af) => af,
            None => continue, // Skip missing files
        };

        // Read current disk mtime using portable API (consistent with comparison code)
        let metadata = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
        let (mtime_secs, mtime_nanos) = paths::read_mtime(&metadata);

        // Update file mtime via db_thread using (zone, inode) key
        sender.update_file_mtime(
            audio_file.entry.zone.as_str(),
            audio_file.inode(),
            mtime_secs,
            mtime_nanos,
            witness,
        );

        // Clear MtimeOnlyMismatch signal via db_thread
        // NOTE: Corpus file signals are keyed by inode, NOT by path
        sender.clear_corpus_signal::<MtimeOnlyMismatchSignal>(*inode, witness);

        affected_paths.push(abs_path.clone());
    }

    Ok(affected_paths)
}

/// Execute ApplyDbTagsToDisk mutation (single-track).
///
/// - Reads tags from database (source of truth)
/// - Writes to disk via write_file_tags() (marks pending_write)
/// - Clears needs_disk_flush flag
///
/// Post-write reconciliation (mtime update, OOB signal clearing) is handled
/// by VerifyTags when the FS watcher detects the change.
///
/// Used for:
/// - OOB sync resolution (reject disk changes, restore DB state to disk)
/// - Spawned from ApplyTagOps (incremental tag edit step 2)
pub fn execute_apply_db_tags_to_disk(
    db: &ReadOnlyDb<'_>,
    inode: i64,
    abs_path: &std::path::Path,
    zone: Zone,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::corpus::tags::{write_file_tags, TagSet};
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    // Convert abs_path to relative for DB operations.
    // Use mutation's abs_path parameter, not file's path from DB (may be stale).
    let relative_path = paths::resolve_relative(abs_path)?;
    let rel_path_str = relative_path.to_string_lossy();

    // Get DB tags from the zone-appropriate table and convert to TagSet
    let db_tags = db.get_tags_for_zone(inode, zone)?;
    let tag_set = TagSet::new(db_tags.into_iter().map(|t| (t.tag_name, t.tag_value)));

    // Write tags to disk (marks pending_write; mtime/signal cleanup via watcher→VerifyTags)
    let token = super::sealed::MutationToken::new();
    write_file_tags(abs_path, &tag_set, &token, witness)
        .with_context(|| format!("Failed to write tags to {}", abs_path.display()))?;

    // Clear needs_disk_flush flag
    sender.set_needs_disk_flush(&rel_path_str, false, witness);

    Ok(())
}

/// Execute FlushTagsToDisk mutation — drain DB queue, validate, then write.
///
/// 1. Blocks until the DB write queue drains (all pending writes commit).
/// 2. Reads committed tags from DB via `read_db`.
/// 3. Compares committed tags against `expected_tags`.
/// 4. On match: writes committed tags to disk (marks pending_write) and clears `needs_disk_flush`.
/// 5. On mismatch: returns error, leaves `needs_disk_flush` raised for OOB
///    resolution.
///
/// Post-write reconciliation (mtime update, OOB signal clearing) is handled
/// by VerifyTags when the FS watcher detects the change.
pub fn execute_flush_tags_to_disk(
    inode: i64,
    abs_path: &std::path::Path,
    expected_tags: &TagSet,
    zone: Zone,
    read_db: &crate::db::queries::ReadOnlyDb<'_>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::corpus::tags::write_file_tags;
    use crate::db::write_thread;

    // 1. Drain: block until all pending DB writes have committed
    write_thread::wait_for_queue_drain();

    let sender = write_thread::require_sender()?;

    let relative_path = paths::resolve_relative(abs_path)?;
    let rel_path_str = relative_path.to_string_lossy();

    // 2. Read committed tags from zone-appropriate table
    let db_tags = read_db.get_tags_for_zone(inode, zone)?;
    let committed_tags = TagSet::new(db_tags.into_iter().map(|t| (t.tag_name, t.tag_value)));

    // 3. Validate: committed DB state must match what we expected to write
    if committed_tags != *expected_tags {
        anyhow::bail!(
            "FlushTagsToDisk validation failed for inode {}: \
             DB tags diverge from expected tags. \
             Leaving needs_disk_flush raised for OOB resolution.",
            inode,
        );
    }

    // 4. Write validated committed tags to disk (marks pending_write; mtime/signal cleanup via watcher→VerifyTags)
    let token = super::sealed::MutationToken::new();
    write_file_tags(abs_path, &committed_tags, &token, witness)
        .with_context(|| format!("Failed to flush tags to {}", abs_path.display()))?;

    // Clear needs_disk_flush flag only on success
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
    in_band_zone: Option<&str>,
    session_id: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::db::write_thread;
    use std::os::unix::fs::MetadataExt;

    let sender = write_thread::require_sender()?;

    // Convert abs_path to relative for DB operations.
    // IMPORTANT: We use the mutation's abs_path parameter, NOT file's path from DB.
    // When spawned from Transcode, the DB read connection may not have seen the
    // path update yet (async write via db_thread), causing stale reads.
    let relative_path = paths::resolve_relative(abs_path)?;
    let rel_path_str = relative_path.to_string_lossy();

    // Use in-band zone when available (chain-spawned from Transcode), otherwise
    // fall back to DB read (standalone OOB resolution where inode is stable).
    let zone: &str = match in_band_zone {
        Some(s) => s,
        None => {
            let audio_file = db
                .get_audio_file_by_inode(inode, Zone::Corpus)?
                .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;
            audio_file.entry.zone.as_str()
        }
    };

    // Read disk tags using TagSet
    let disk_tagset = tags::from_file(abs_path)
        .with_context(|| format!("Failed to read tags from {}", abs_path.display()))?;

    // Determine tag table from zone
    let zone_enum =
        Zone::from_str(zone).ok_or_else(|| anyhow::anyhow!("Unknown zone: {}", zone))?;
    let tag_table = zone_enum
        .tag_table()
        .ok_or_else(|| anyhow::anyhow!("Zone {:?} has no tag table", zone_enum))?;

    // Update DB with disk tags via db_thread
    // Use rel_path_str (from mutation param), not track.path (potentially stale)
    sender.set_index_track_tags(&rel_path_str, disk_tagset, tag_table, session_id, witness);

    // Read disk metadata using portable API
    let file_metadata = std::fs::metadata(abs_path)
        .with_context(|| format!("Failed to read metadata for {}", abs_path.display()))?;
    let (mtime_secs, mtime_nanos) = paths::read_mtime(&file_metadata);

    // Get current inode from filesystem (not from potentially stale DB read).
    // After transcode, the inode changed and we need the NEW inode for files table lookup.
    let current_inode = file_metadata.ino() as i64;

    // Update file mtime via db_thread using (zone, inode) key
    sender.update_file_mtime(zone, current_inode, mtime_secs, mtime_nanos, witness);

    // Clear tag_mismatches for this track via db_thread
    sender.clear_tag_mismatches_for_track(&rel_path_str, witness);

    // Clear OOB signals via db_thread
    // NOTE: New signals are keyed by inode. Clear both inode-keyed (new) and path-keyed (legacy) signals.
    sender.clear_corpus_signal::<OutOfBandTagSyncSignal>(inode, witness);
    sender.clear_corpus_signal::<OutOfBandTagConflictSignal>(inode, witness);
    sender.clear_corpus_signal::<MtimeOnlyMismatchSignal>(inode, witness);

    Ok(())
}

// ============================================================================
// Signal Emission Executors
// ============================================================================

/// Execute EmitCanonicalTag mutation - emit a CanonicalTag signal to whitelist a value.
///
/// Creates a CanonicalTag aggregate signal that marks a compound-looking value as
/// a single canonical entity (e.g., "Rinse & Repeat" is a band name, not a collaboration).
/// Future compound detection runs check CanonicalTag and skip whitelisted values.
///
/// Also clears existing CompoundTag signals for all inodes that contain the
/// now-canonical value, so they don't persist as stale insights.
pub fn execute_emit_canonical_tag(
    tag_name: &str,
    canonical_value: &str,
    read_db: &crate::db::ReadOnlyDb<'_>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    // Key uses normalized tag name (strip separators + uppercase) so that lookups
    // match regardless of compound variant: "album_artist" ≈ "ALBUMARTIST".
    let normalized_tag_name = mm_utils::tag_names::normalize_tag_name(tag_name);
    let canonical_key = format!("{}:{}", normalized_tag_name, canonical_value);

    // Emit CanonicalTag signal via typed path
    sender.write_typed_signal(
        TypedSignalWrite::CanonicalTag(CanonicalTagSignal {
            key: canonical_key,
            tag_name: normalized_tag_name.clone(),
            canonical_value: canonical_value.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }),
        witness,
    );

    // Clear stale CompoundTag signals for all inodes with this compound value
    let affected_inodes = read_db
        .get_inodes_with_compound_value(tag_name, canonical_value)
        .unwrap_or_default();
    for inode in &affected_inodes {
        sender.clear_corpus_signal::<CompoundTagSignal>(*inode, witness);
    }

    crate::logging::log_general(format!(
        "[MUTATION] EmitCanonicalTag: {} = {:?} (cleared {} stale compound signals)",
        tag_name,
        canonical_value,
        affected_inodes.len()
    ));

    Ok(())
}

/// Execute EmitExpectedOverlap mutation - mark a source pair as expected overlap.
///
/// Creates an ExpectedOverlap aggregate signal that suppresses CrossSourceOverlap
/// signal emission for this source pair in future DetectCrossSourceOverlaps runs.
/// Also clears the existing CrossSourceOverlap signal for the pair.
pub fn execute_emit_expected_overlap(
    source_a: &str,
    source_b: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;
    use crate::meta::signals::data::ExpectedOverlapSignal;

    let sender = write_thread::require_sender()?;

    // Key format: sorted "source_a|source_b" (same as CrossSourceOverlap keys)
    let (key_a, key_b) = if source_a < source_b {
        (source_a, source_b)
    } else {
        (source_b, source_a)
    };
    let pair_key = format!("{}|{}", key_a, key_b);

    // Emit ExpectedOverlap signal
    sender.write_typed_signal(
        TypedSignalWrite::ExpectedOverlap(ExpectedOverlapSignal {
            key: pair_key.clone(),
            source_a: key_a.to_string(),
            source_b: key_b.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }),
        witness,
    );

    // Clear the corresponding CrossSourceOverlap signal
    sender.clear_aggregate_signal::<CrossSourceOverlapSignal>(&pair_key, witness);

    crate::logging::log_general(format!(
        "[MUTATION] EmitExpectedOverlap: {} (cleared CrossSourceOverlap)",
        pair_key
    ));

    Ok(())
}

/// Execute EmitExpectedDuplicate mutation - mark a fingerprint group as expected.
///
/// Creates an ExpectedDuplicate aggregate signal that suppresses RedundantDuplicate
/// and SubparDuplicate signal emission for this fingerprint group in future
/// AnalyzeFingerprintOverlaps runs. Also clears the existing RedundantDuplicate
/// signal for the fingerprint key.
pub fn execute_emit_expected_duplicate(
    fingerprint_key: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;
    use crate::meta::signals::data::ExpectedDuplicateSignal;

    let sender = write_thread::require_sender()?;

    // Emit ExpectedDuplicate signal
    sender.write_typed_signal(
        TypedSignalWrite::ExpectedDuplicate(ExpectedDuplicateSignal {
            key: fingerprint_key.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }),
        witness,
    );

    // Clear the corresponding RedundantDuplicate signal
    sender.clear_aggregate_signal::<RedundantDuplicateSignal>(fingerprint_key, witness);

    crate::logging::log_general(format!(
        "[MUTATION] EmitExpectedDuplicate: {} (cleared RedundantDuplicate)",
        fingerprint_key
    ));

    Ok(())
}

/// Execute EmitExpectedMissingTag mutation - mark inodes as expected-missing-tag.
///
/// Creates ExpectedMissingTag corpus signals for each inode, suppressing them
/// from future MissingAlbumSingle signal emission.
pub fn execute_emit_expected_missing_tag(
    inodes: &[i64],
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    for &inode in inodes {
        sender.write_typed_signal(
            TypedSignalWrite::ExpectedMissingTag(ExpectedMissingTagSignal { inode }),
            witness,
        );
    }

    crate::logging::log_general(format!(
        "[MUTATION] EmitExpectedMissingTag: suppressed {} inodes",
        inodes.len()
    ));

    Ok(())
}

/// Execute DropExternalMatch mutation - delete external match data for an inode.
///
/// Routes write through signal_sender (fire-and-forget).
pub fn execute_drop_external_match(inode: i64, witness: &MutationExecutionWitness) -> Result<()> {
    use crate::db::write_thread;

    let sender = write_thread::require_sender()?;

    sender.drop_external_match(inode, witness);

    crate::logging::log_general(format!("[MUTATION] DropExternalMatch: inode {}", inode));

    Ok(())
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
            has_pictures: false,
            pic_info: None,
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
