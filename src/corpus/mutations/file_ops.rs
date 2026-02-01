//! File Operations
//!
//! Handles execution of file-related mutations:
//! - Move: Move a file from source to destination
//! - Copy: Copy a file to a new location
//! - MoveToStash: Move a file to the stash directory
//! - HardLink: Create a hard link (for deployment)
//! - LibraryMove: Move a file within a library

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::witch::MutationExecutionWitness;

use super::types::{Mutation, MutationResult};

/// Execute a Move mutation.
///
/// Moves a file from source to destination, creating parent directories if needed.
/// Optionally updates the database if track_id is provided.
pub fn execute_move(
    source: &Path,
    destination: &Path,
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

    // Fail if destination already exists - MLA never overwrites files
    if destination.exists() {
        return Err(anyhow::anyhow!(
            "Destination already exists: {}",
            destination.display()
        ));
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

/// Execute a LibraryMove mutation.
///
/// Moves a file within a library (e.g., stale file to correct location).
/// Unlike corpus moves, this does not update any database records.
///
/// IMPORTANT: MLA never unlinks/destroys data. If destination exists:
/// - Same inode: File already correctly deployed, nothing to do (caller should
///   handle stale source path via separate MoveToStash if cleanup needed)
/// - Different inode: Conflict - fails so caller can stash the conflicting file first
fn execute_library_move(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    // Check if destination already exists
    if destination.exists() {
        let src_inode = source.metadata()?.ino();
        let dst_inode = destination.metadata()?.ino();

        if src_inode == dst_inode {
            // Same file already at destination - nothing to do
            // The source is a stale hard link; if cleanup is desired,
            // caller should issue a separate MoveToStash for the source path
            return Ok(());
        } else {
            // Different file at destination - refuse to clobber
            // Caller must stash the conflicting destination file first
            return Err(anyhow::anyhow!(
                "Destination already exists with different inode: {} (source inode: {}, dest inode: {}). \
                 Stash the conflicting file first, then retry.",
                destination.display(),
                src_inode,
                dst_inode
            ));
        }
    }

    // Create parent directories if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Move the file
    fs::rename(source, destination).with_context(|| {
        format!(
            "Failed to move library file from {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    Ok(())
}

/// Move a file to the stash directory, organized by stash action name.
///
/// Destination: `{stash_root}/{stash_name}/{filename}`
/// Creates the stash subdirectory if it doesn't exist.
pub fn execute_move_to_stash(
    path: &Path,
    stash_name: &str,
    stash_root: &Path,
) -> Result<()> {
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            path.display()
        ));
    }

    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Path has no filename: {}", path.display()))?;

    let stash_dir = stash_root.join(stash_name);
    fs::create_dir_all(&stash_dir)
        .with_context(|| format!("Failed to create stash directory: {}", stash_dir.display()))?;

    let dest = stash_dir.join(filename);

    // Don't overwrite existing stashed files
    if dest.exists() {
        return Err(anyhow::anyhow!(
            "Stash destination already exists: {}",
            dest.display()
        ));
    }

    fs::rename(path, &dest).with_context(|| {
        format!(
            "Failed to move {} to stash at {}",
            path.display(),
            dest.display()
        )
    })?;

    Ok(())
}

/// Execute a single file operation mutation.
///
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    mutation: &Mutation,
    stash_root: Option<&Path>,
    _witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::Move {
            source,
            destination,
        } => execute_move(source, destination),

        Mutation::Copy {
            source,
            destination,
        } => execute_copy(source, destination),

        Mutation::MoveToStash { path, stash_name, .. } => {
            match stash_root {
                Some(root) => execute_move_to_stash(path, stash_name, root),
                None => Err(anyhow::anyhow!(
                    "Stash directory not configured. File: {}",
                    path.display()
                )),
            }
        }

        Mutation::HardLink {
            source,
            destination,
        } => execute_hard_link(source, destination),

        Mutation::LibraryMove {
            source,
            destination,
        } => execute_library_move(source, destination),

        _ => Err(anyhow::anyhow!("Not a file operation mutation")),
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
        spawn_mutations: Vec::new(), // File ops don't spawn
    }
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
