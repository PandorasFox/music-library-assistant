//! Transcode Operations
//!
//! Handles execution of transcode mutations: converting audio files between
//! container/codec formats while preserving the associated track record.
//!
//! Process:
//! 1. Validate source file and target format
//! 2. Transcode via native decode/encode pipeline
//! 3. Stash original file under stash_name
//! 4. Update track record (path, inode, file_size, file_type)
//! 5. Update files table entry
//! 6. Spawn AssimilateDiskTagsToDb to sync tags from new file to index

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::corpus::transcode::{self, TranscodeTarget};
use crate::witch::MutationExecutionWitness;

use crate::meta::recomputation::RecomputationScope;

use super::file_ops;
use super::indexing::AssimilateDiskTagsToDbMutation;
use super::traits::{MutationContext, MutationExecutor};
use super::types::{DiffEntry, Mutation, MutationResult, SignalClearScope, path_filename};

/// Transcode a file to a different container/codec format.
///
/// On success: creates new file at same path with different extension,
/// stashes original under stash_name, and updates the track record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscodeMutation {
    pub inode: i64,
    pub source_path: PathBuf,
    pub target_format: TranscodeTarget,
    pub stash_name: String,
}

impl MutationExecutor for TranscodeMutation {
    fn label(&self) -> &'static str { "Transcode" }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::DiskFlush) }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        let result = execute_transcode_impl(
            ctx.read_db,
            self.inode,
            &self.source_path,
            self.target_format,
            &self.stash_name,
            ctx.stash_root,
            ctx.witness,
        );

        let (success, error, spawn_mutations) = match result {
            Ok(source) => {
                // On success, spawn AssimilateDiskTagsToDb to read tags from the new file
                // and update the index. This picks up the tags copied by the
                // native transcode pipeline's tag-copy step.
                let new_path = self.target_format.dest_path(&self.source_path);
                let spawn = vec![ctx.witness.spawn_mutation(Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation {
                    inode: self.inode,
                    path: new_path,
                    zone: Some(source),
                }))];
                (true, None, spawn)
            }
            Err(e) => (false, Some(format!("{:#}", e)), Vec::new()),
        };

        MutationResult {
            _mutation: Mutation::Transcode(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations,
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::All }

    fn affected_inodes(&self) -> Vec<i64> { vec![self.inode] }
    fn recomputation_scope(&self) -> RecomputationScope { RecomputationScope::FILES | RecomputationScope::TAGS }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        // Transcode: only spawn for NEW path (source is stashed, would race)
        vec![self.target_format.dest_path(&self.source_path)]
    }

    fn diff_entries(&self) -> Vec<DiffEntry> {
        let source_ext = self.source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("?")
            .to_uppercase();
        let target_label = match self.target_format {
            TranscodeTarget::Opus { bitrate_kbps } => format!("Opus ({}kbps)", bitrate_kbps),
            TranscodeTarget::Flac => "FLAC".to_string(),
            TranscodeTarget::FlacLossyCapture => "FLAC (lossy capture)".to_string(),
        };
        vec![DiffEntry::new(path_filename(&self.source_path), source_ext, target_label)]
    }
}

// ============================================================================
// Execution Helper (unchanged)
// ============================================================================

/// Execute a Transcode mutation.
///
/// Transcodes the source file to the target format, stashes the original,
/// and updates the audio file record in the database.
/// Returns the file zone on success (for in-band plumbing to spawned mutations).
fn execute_transcode_impl(
    db: &ReadOnlyDb<'_>,
    inode: i64,
    source_path: &Path,
    target_format: TranscodeTarget,
    stash_name: &str,
    stash_root: Option<&Path>,
    witness: &MutationExecutionWitness,
) -> Result<String> {
    // Validate stash is configured
    let stash_root = stash_root.ok_or_else(|| {
        anyhow::anyhow!("Stash directory not configured. Cannot transcode without stash for originals.")
    })?;

    // Validate source exists
    if !source_path.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source_path.display()
        ));
    }

    // Check source isn't already in target format
    let source_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if source_ext == target_format.extension() {
        return Err(anyhow::anyhow!(
            "Source file is already in target format ({}): {}",
            target_format.extension(),
            source_path.display()
        ));
    }

    // Compute destination path
    let dest_path = target_format.dest_path(source_path);

    // If target already exists (e.g., from a previous incomplete transcode),
    // stash it first so we can create a fresh transcode
    if dest_path.exists() {
        crate::logging::log_general(format!(
            "[TRANSCODE] Target already exists, stashing old file: {}",
            dest_path.display()
        ));
        file_ops::execute_move_to_stash(&dest_path, stash_name, stash_root)
            .with_context(|| format!(
                "Failed to stash existing target file: {}",
                dest_path.display()
            ))?;
    }

    // Transcode via native pipeline
    transcode::transcode(source_path, &dest_path, target_format, witness)
        .with_context(|| format!(
            "Transcode failed: {} -> {}",
            source_path.display(),
            dest_path.display()
        ))?;

    // Stash the original file
    file_ops::execute_move_to_stash(source_path, stash_name, stash_root)
        .with_context(|| format!(
            "Failed to stash original file: {}",
            source_path.display()
        ))?;

    // Get new file's filesystem metadata
    let fs_metadata = std::fs::metadata(&dest_path)
        .with_context(|| format!(
            "Failed to read metadata of transcoded file: {}",
            dest_path.display()
        ))?;

    let new_inode = fs_metadata.ino() as i64;
    let new_file_size = fs_metadata.len() as i64;
    let new_file_type = target_format.extension().to_string();

    // Get the existing audio file to preserve fields we don't want to change
    // Transcode only operates on corpus files
    let existing_file = db.get_audio_file_by_inode(inode, Zone::Corpus)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", inode))?;

    // Convert new absolute path to relative for storage
    let resolver = paths::get_resolver();
    let relative_new_path = resolver
        .to_relative(&dest_path)
        .with_context(|| format!(
            "Path {} does not match any configured root. Check config.kdl roots.",
            dest_path.display(),
        ))?;

    // Get signal_sender for DB writes
    use crate::db::write_thread::{self, FileEntryData};

    let sender = write_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    let relative_path_str = relative_new_path.to_string_lossy().to_string();

    let old_path = existing_file.path();
    let old_inode = existing_file.inode();
    let zone_str = existing_file.entry.zone.as_str();

    // Update audio_info record: path, inode, file_size, file_type
    // Use old path for lookup, update to new path
    sender.update_track_path_with_metadata(
        old_path,              // Old path (already relative in DB)
        &relative_path_str,    // New path
        new_inode,
        new_file_size,
        &new_file_type,
        witness,
    );

    // Update files table: upsert new entry for new file
    let mtime_secs = fs_metadata.mtime();
    let mtime_nanos = fs_metadata.mtime_nsec();

    let file_entry = FileEntryData {
        inode: new_inode,
        mtime_secs,
        mtime_nanos,
        file_size: new_file_size,
    };
    sender.upsert_file_entry(
        &relative_path_str,
        zone_str,
        file_entry,
        witness,
    );

    // Delete old files table entry if inode changed (which it will, since it's a new file)
    if old_inode != new_inode {
        sender.drop_file_index_by_inode(zone_str, old_inode, witness);
    }

    Ok(zone_str.to_string())
}

// ============================================================================
// (execute_single removed — trait dispatch via MutationExecutor::execute())
