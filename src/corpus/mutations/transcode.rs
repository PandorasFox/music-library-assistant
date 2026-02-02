//! Transcode Operations
//!
//! Handles execution of transcode mutations: converting audio files between
//! container/codec formats while preserving the associated track record.
//!
//! Flow:
//! 1. Validate source file and target format
//! 2. Transcode via ffmpeg subprocess
//! 3. Stash original file under stash_name
//! 4. Update track record (path, inode, file_size, file_type)
//! 5. Update files table entry
//! 6. Spawn AssimilateDiskTagsToDb to sync tags from new file to index

use anyhow::{Context, Result};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::corpus::db::types::FileSource;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::corpus::transcode::{self, TranscodeTarget};
use crate::witch::MutationExecutionWitness;

use super::file_ops;
use super::types::{Mutation, MutationResult};

/// Execute a Transcode mutation.
///
/// Transcodes the source file to the target format, stashes the original,
/// and updates the track record in the database.
fn execute_transcode(
    db: &ReadOnlyDb<'_>,
    track_id: i64,
    source_path: &Path,
    target_format: TranscodeTarget,
    stash_name: &str,
    stash_root: Option<&Path>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
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

    // Compute destination path: same directory, same stem, new extension
    let dest_path = source_path.with_extension(target_format.extension());

    if dest_path.exists() {
        return Err(anyhow::anyhow!(
            "Target file already exists: {}",
            dest_path.display()
        ));
    }

    // Transcode via ffmpeg
    transcode::transcode(source_path, &dest_path, target_format)
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

    // Get the existing audio file to preserve fields we don't want to change - track_id is actually inode
    // Transcode only operates on corpus files
    let existing_file = db.get_audio_file_by_inode(track_id, FileSource::Corpus)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", track_id))?;

    // Convert new absolute path to relative for storage
    let resolver = paths::get_resolver();
    let relative_new_path = resolver
        .to_relative(&dest_path)
        .with_context(|| format!(
            "Path {} does not match any configured root. Check config.kdl roots.",
            dest_path.display(),
        ))?;

    // Get signal_sender for DB writes
    use crate::db_thread::{self, FileEntryData};

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    let relative_path_str = relative_new_path.to_string_lossy().to_string();

    let old_path = existing_file.path();
    let old_inode = existing_file.inode();
    let source_str = existing_file.entry.source.as_str();

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
        source_str,
        file_entry,
        witness,
    );

    // Delete old files table entry if inode changed (which it will, since it's a new file)
    if old_inode != new_inode {
        sender.drop_file_index_by_inode(source_str, old_inode, witness);
    }

    Ok(())
}

/// Execute a single transcode mutation.
///
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
/// On success, spawns AssimilateDiskTagsToDb to sync tags from the new file.
pub fn execute_single(
    db: &ReadOnlyDb<'_>,
    mutation: &Mutation,
    stash_root: Option<&Path>,
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    // Extract mutation parameters before execution for spawn_mutations
    let (track_id, source_path, target_format) = match mutation {
        Mutation::Transcode {
            track_id,
            source_path,
            target_format,
            ..
        } => (*track_id, source_path.clone(), *target_format),
        _ => {
            return MutationResult {
                _mutation: mutation.clone(),
                success: false,
                error: Some("Not a transcode mutation".to_string()),
                _duration_ms: start.elapsed().as_millis() as u64,
                spawn_mutations: Vec::new(),
            };
        }
    };

    let result = match mutation {
        Mutation::Transcode {
            track_id,
            source_path,
            target_format,
            stash_name,
        } => execute_transcode(db, *track_id, source_path, *target_format, stash_name, stash_root, witness),

        _ => Err(anyhow::anyhow!("Not a transcode mutation")),
    };

    let (success, error, spawn_mutations) = match result {
        Ok(()) => {
            // On success, spawn AssimilateDiskTagsToDb to read tags from the new file
            // and update the index. This picks up any encoder tags added by ffmpeg
            // while preserving the original metadata that ffmpeg copies.
            let new_path = source_path.with_extension(target_format.extension());
            let spawn = vec![witness.spawn_mutation(Mutation::AssimilateDiskTagsToDb {
                track_id,
                path: new_path,
            })];
            (true, None, spawn)
        }
        Err(e) => (false, Some(format!("{:#}", e)), Vec::new()),
    };

    MutationResult {
        _mutation: mutation.clone(),
        success,
        error,
        _duration_ms: start.elapsed().as_millis() as u64,
        spawn_mutations,
    }
}
