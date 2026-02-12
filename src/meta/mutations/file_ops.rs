//! File Operations
//!
//! Handles execution of file-related mutations:
//! - Move: Move a file from source to destination
//! - MoveToStash: Move a file to the stash directory
//! - HardLink: Create a hard link (for deployment)
//! - LibraryMove: Move a file within a library

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::meta::computations::{Computation, awakening};
use crate::meta::signals::data::*;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{Mutation, MutationResult, SignalClearScope, SignalToClear};

// ============================================================================
// Mutation Structs
// ============================================================================

/// Move a file from source to destination.
///
/// NOTE: This mutation is fully plumbed but intentionally not yet utilized in UI.
/// It will be used by the inbox intake for moving files from inbox to corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Move a file to the stash directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveToStashMutation {
    pub path: PathBuf,
    pub stash_name: String,
}

/// Create a hard link from source to destination (for deployment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardLinkMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Move a file within a library (e.g., stale file to correct location).
///
/// Unlike corpus Move, this operates only on library paths and triggers
/// library signal updates (clears LibraryStale for old path).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryMoveMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

// ============================================================================
// MutationExecutor Implementations
// ============================================================================

impl MutationExecutor for MoveMutation {
    fn label(&self) -> &'static str { "File move" }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_move(&self.source, &self.destination);
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::Move(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::MutableOnly }

    fn affected_inodes(&self) -> Vec<i64> { Vec::new() }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }
}

impl MutationExecutor for MoveToStashMutation {
    fn label(&self) -> &'static str { "File move" }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        use std::os::unix::fs::MetadataExt;

        let start = std::time::Instant::now();

        // Discover inode before the move (file still exists at original path)
        let discovered = std::fs::metadata(&self.path)
            .map(|m| vec![m.ino() as i64])
            .unwrap_or_default();

        let result = match ctx.stash_root {
            Some(root) => execute_move_to_stash(&self.path, &self.stash_name, root),
            None => Err(anyhow::anyhow!(
                "Stash directory not configured. File: {}",
                self.path.display()
            )),
        };
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::MoveToStash(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: discovered,
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::All }

    fn affected_inodes(&self) -> Vec<i64> { Vec::new() }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        use crate::corpus::paths;
        let resolver = paths::get_resolver();
        if let Some(rel) = resolver.to_relative(&self.path) {
            if let Ok(lib_rel) = rel.strip_prefix("libraries") {
                let library_path = lib_rel.to_string_lossy();
                if let Some(library_name) = library_path.split('/').next() {
                    let key = LibraryLeftoverSignal::make_key(library_name, &library_path);
                    return vec![SignalToClear::exact::<LibraryLeftoverSignal>(key)];
                }
            }
        }
        Vec::new()
    }

    // MoveToStash: no signal updates needed (file is gone)
}

impl MutationExecutor for HardLinkMutation {
    fn label(&self) -> &'static str { "Hard link" }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_hard_link(&self.source, &self.destination);
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::HardLink(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::MutableOnly }

    fn affected_inodes(&self) -> Vec<i64> { Vec::new() }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }

    fn additional_computations(&self) -> Vec<Computation> {
        vec![Computation::Awakening(awakening::Computation::UpdateDeploySignals {
            corpus_path: self.source.clone(),
            library_path: self.destination.clone(),
        })]
    }
}

impl MutationExecutor for LibraryMoveMutation {
    fn label(&self) -> &'static str { "Library move" }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_library_move_impl(&self.source, &self.destination);
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::LibraryMove(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations: Vec::new(),
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::MutableOnly }

    fn affected_inodes(&self) -> Vec<i64> { Vec::new() }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        vec![self.source.clone(), self.destination.clone()]
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        use crate::corpus::paths;
        let resolver = paths::get_resolver();
        if let Some(rel) = resolver.to_relative(&self.source) {
            if let Ok(lib_rel) = rel.strip_prefix("libraries") {
                let library_path = lib_rel.to_string_lossy();
                if let Some(library_name) = library_path.split('/').next() {
                    let key = LibraryStaleSignal::make_key(library_name, &library_path);
                    return vec![SignalToClear::exact::<LibraryStaleSignal>(key)];
                }
            }
        }
        Vec::new()
    }
}

// ============================================================================
// Execution Helpers (unchanged filesystem operations)
// ============================================================================

/// Execute a Move mutation.
///
/// Moves a file from source to destination, creating parent directories if needed.
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

    // Fail if destination already exists - MM never overwrites files
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
/// IMPORTANT: MM never unlinks/destroys data. If destination exists:
/// - Same inode: File already correctly deployed, nothing to do (caller should
///   handle stale source path via separate MoveToStash if cleanup needed)
/// - Different inode: Conflict - fails so caller can stash the conflicting file first
fn execute_library_move_impl(source: &Path, destination: &Path) -> Result<()> {
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

/// Move a file to the stash directory, preserving corpus/library directory structure.
///
/// Destination: `{stash_root}/{stash_name}/{relative_path}`
/// where `relative_path` is the path within corpus/ or libraries/, preserving structure.
///
/// If the destination already exists, appends underscores to the filename stem until
/// a unique path is found (e.g., `track.flac` -> `track_.flac` -> `track__.flac`).
///
/// Examples:
/// - `/archive/corpus/Artist/Album/track.flac` -> `stash/overlaps/Artist/Album/track.flac`
/// - `/archive/libraries/music/Artist/track.mp3` -> `stash/leftovers/music/Artist/track.mp3`
pub fn execute_move_to_stash(
    path: &Path,
    stash_name: &str,
    stash_root: &Path,
) -> Result<()> {
    use crate::corpus::paths;

    if !path.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            path.display()
        ));
    }

    let resolver = paths::get_resolver();

    // Convert absolute path to root-relative (e.g., "corpus/Artist/Album/track.flac")
    let root_relative = resolver
        .to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path not within archive root: {}", path.display()))?;

    // Strip the domain prefix (corpus/ or libraries/) to get the preservable structure
    let preserved_path = root_relative
        .strip_prefix("corpus")
        .or_else(|_| root_relative.strip_prefix("libraries"))
        .unwrap_or(&root_relative);

    // Build destination: stash_root/stash_name/preserved_path
    let stash_base = stash_root.join(stash_name);
    let mut dest = stash_base.join(preserved_path);

    // Create destination directory if needed
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create stash directory: {}", parent.display()))?;
    }

    // If destination exists, append underscores to stem until unique
    if dest.exists() {
        let parent = dest.parent().map(PathBuf::from);
        let stem = dest
            .file_stem()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        let extension = dest.extension().map(|e| e.to_os_string());

        let mut new_stem = stem;
        loop {
            new_stem.push("_");
            let mut new_filename = new_stem.clone();
            if let Some(ref ext) = extension {
                new_filename.push(".");
                new_filename.push(ext);
            }
            dest = match &parent {
                Some(p) => p.join(&new_filename),
                None => PathBuf::from(&new_filename),
            };
            if !dest.exists() {
                break;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_mutation_structure() {
        let mutation = Mutation::Move(MoveMutation {
            source: PathBuf::from("/src/file.flac"),
            destination: PathBuf::from("/dst/file.flac"),
        });

        assert!(matches!(mutation, Mutation::Move(_)));
    }

    #[test]
    fn test_hard_link_mutation_structure() {
        let mutation = Mutation::HardLink(HardLinkMutation {
            source: PathBuf::from("/corpus/file.flac"),
            destination: PathBuf::from("/library/file.flac"),
        });

        assert!(matches!(mutation, Mutation::HardLink(_)));
    }
}
