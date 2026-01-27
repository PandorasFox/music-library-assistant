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
//! 5. Update scan_state entry

use anyhow::{Context, Result};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::corpus::db::Database;
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
    db: &Database,
    track_id: i64,
    source_path: &Path,
    target_format: TranscodeTarget,
    stash_name: &str,
    stash_root: Option<&Path>,
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

    // Get the existing track to preserve fields we don't want to change
    let existing_track = db.get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track {} not found in database", track_id))?;

    // Convert new absolute path to relative for storage
    let resolver = paths::get_resolver();
    let relative_new_path = resolver
        .to_relative(&dest_path, &existing_track.source)
        .with_context(|| format!(
            "Path {} does not match {} root. Check config.kdl roots.",
            dest_path.display(),
            existing_track.source
        ))?;

    // Build updated track (preserve fingerprint, duration, sample_rate; update path/inode/size/type)
    let updated_track = crate::corpus::db::Track {
        id: existing_track.id,
        path: relative_new_path.to_string_lossy().to_string(),
        source: existing_track.source.clone(),
        inode: new_inode,
        file_size: new_file_size,
        file_type: new_file_type,
        duration_ms: existing_track.duration_ms,
        bitrate_kbps: existing_track.bitrate_kbps,
        sample_rate: existing_track.sample_rate,
        fingerprint: existing_track.fingerprint.clone(),
    };

    // Update track record in database
    db.update_track_metadata(track_id, &updated_track)
        .with_context(|| format!("Failed to update track {} metadata after transcode", track_id))?;

    // Update scan_state: delete old entry for old inode, upsert new entry for new file
    // The old scan_state entry references the old inode which no longer exists at the old path
    // (it's been stashed). We need a fresh scan_state for the new file.
    let mtime_secs = fs_metadata.mtime();
    let mtime_nanos = fs_metadata.mtime_nsec();

    let scan_entry = crate::corpus::db::ScanStateEntry {
        source: existing_track.source.clone(),
        inode: new_inode as i64,
        path: relative_new_path.to_string_lossy().to_string(),
        mtime_secs,
        mtime_nanos,
        file_size: new_file_size as i64,
    };
    db.upsert_scan_state(&scan_entry)
        .with_context(|| format!("Failed to update scan_state after transcode for track {}", track_id))?;

    // Delete old scan_state entry if inode changed (which it will, since it's a new file)
    if existing_track.inode != new_inode {
        db.delete_scan_state_by_inode(&existing_track.source, existing_track.inode)
            .with_context(|| format!("Failed to delete old scan_state for track {}", track_id))?;
    }

    Ok(())
}

/// Execute a single transcode mutation.
///
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &Database,
    mutation: &Mutation,
    stash_root: Option<&Path>,
    _witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::Transcode {
            track_id,
            source_path,
            target_format,
            stash_name,
        } => execute_transcode(db, *track_id, source_path, *target_format, stash_name, stash_root),

        _ => Err(anyhow::anyhow!("Not a transcode mutation")),
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
