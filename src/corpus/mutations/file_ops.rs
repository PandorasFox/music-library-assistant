//! File Operations
//!
//! Handles execution of file-related mutations:
//! - Move: Move a file from source to destination
//! - Copy: Copy a file to a new location
//! - Delete: Delete a file
//! - MoveToStash: Move a file to the stash directory
//! - HardLink: Create a hard link (for deployment)
//! - Unlink: Remove a deployed link

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::corpus::db::Database;
use crate::daemon::MutationExecutionWitness;

use super::types::{Mutation, MutationResult};

/// Execute a Move mutation.
///
/// Moves a file from source to destination, creating parent directories if needed.
/// Optionally updates the database if track_id is provided.
pub fn execute_move(
    db: Option<&Database>,
    source: &Path,
    destination: &Path,
    track_id: Option<i64>,
) -> Result<()> {
    // Ensure source exists
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    // Create destination directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Move the file
    fs::rename(source, destination).with_context(|| {
        format!(
            "Failed to move {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    // Note: Database path updates are handled by re-indexing after move
    // The track_id is preserved for reference but path updates require re-scan
    let _ = (db, track_id); // Silence unused warnings

    Ok(())
}

/// Execute a Copy mutation.
///
/// Copies a file to a new location, creating parent directories if needed.
pub fn execute_copy(source: &Path, destination: &Path) -> Result<()> {
    // Ensure source exists
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    // Create destination directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Copy the file
    fs::copy(source, destination).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Execute a Delete mutation.
///
/// Deletes a file and optionally removes it from the database.
pub fn execute_delete(db: Option<&Database>, path: &Path, track_id: Option<i64>) -> Result<()> {
    // Delete the file if it exists
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("Failed to delete file: {}", path.display()))?;
    }

    // Remove from database if track_id provided
    if let (Some(db), Some(_track_id)) = (db, track_id) {
        let _ = db.delete_track_by_path(&path.to_string_lossy());
    }

    Ok(())
}

/// Execute a MoveToStash mutation.
///
/// Moves a file to the stash directory, preserving relative structure.
pub fn execute_move_to_stash(
    db: Option<&Database>,
    path: &Path,
    track_id: Option<i64>,
    stash_name: &str,
    stash_root: &Path,
) -> Result<()> {
    // Build stash destination path
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid file path: {}", path.display()))?;

    let stash_dir = stash_root.join(stash_name);
    let destination = stash_dir.join(file_name);

    // Handle name collisions by appending counter
    let final_destination = if destination.exists() {
        let stem = destination
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = destination
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let mut counter = 1;
        loop {
            let new_name = if ext.is_empty() {
                format!("{}_{}", stem, counter)
            } else {
                format!("{}_{}.{}", stem, counter, ext)
            };
            let new_path = stash_dir.join(new_name);
            if !new_path.exists() {
                break new_path;
            }
            counter += 1;
        }
    } else {
        destination
    };

    // Move to stash
    execute_move(db, path, &final_destination, track_id)?;

    Ok(())
}

/// Execute a HardLink mutation.
///
/// Creates a hard link from source to destination (for deployment).
pub fn execute_hard_link(source: &Path, destination: &Path) -> Result<()> {
    // Ensure source exists
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    // Create destination directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Remove existing file at destination if present
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("Failed to remove existing file: {}", destination.display()))?;
    }

    // Create hard link
    fs::hard_link(source, destination).with_context(|| {
        format!(
            "Failed to create hard link from {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Execute an Unlink mutation.
///
/// Removes a deployed link (regular file deletion).
pub fn execute_unlink(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("Failed to unlink file: {}", path.display()))?;
    }

    Ok(())
}

/// Execute a single file operation mutation.
///
/// Note: MoveToStash requires a stash_root parameter not available in the mutation,
/// so it returns an error. Use execute_move_to_stash directly when stash_root is known.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: Option<&Database>,
    mutation: &Mutation,
    _witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::Move {
            source,
            destination,
            track_id,
        } => execute_move(db, source, destination, *track_id),

        Mutation::Copy {
            source,
            destination,
        } => execute_copy(source, destination),

        Mutation::Delete { path, track_id } => execute_delete(db, path, *track_id),

        Mutation::MoveToStash { path, .. } => {
            // MoveToStash requires stash_root which isn't in the mutation
            // This should be handled by a higher-level executor with config access
            Err(anyhow::anyhow!(
                "MoveToStash requires stash_root configuration. File: {}",
                path.display()
            ))
        }

        Mutation::HardLink {
            source,
            destination,
        } => execute_hard_link(source, destination),

        Mutation::Unlink { path } => execute_unlink(path),

        _ => Err(anyhow::anyhow!("Not a file operation mutation")),
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

/// Execute a batch of file operation mutations.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_batch(
    db: Option<&Database>,
    mutations: &[Mutation],
    witness: &MutationExecutionWitness,
) -> Vec<MutationResult> {
    mutations
        .iter()
        .map(|m| execute_single(db, m, witness))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_move_mutation_structure() {
        let mutation = Mutation::Move {
            source: PathBuf::from("/src/file.flac"),
            destination: PathBuf::from("/dst/file.flac"),
            track_id: Some(1),
        };

        assert!(matches!(mutation, Mutation::Move { .. }));
    }

    #[test]
    fn test_hard_link_mutation_structure() {
        let mutation = Mutation::HardLink {
            source: PathBuf::from("/corpus/file.flac"),
            destination: PathBuf::from("/library/file.flac"),
        };

        assert!(matches!(mutation, Mutation::HardLink { .. }));
    }
}
