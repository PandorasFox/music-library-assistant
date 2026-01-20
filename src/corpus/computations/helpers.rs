//! Shared utility functions for computations.
//!
//! These helpers are used across multiple computation modules for common tasks
//! like file type detection, path parsing, signal emission, and configuration access.

use std::path::{Path, PathBuf};

use crate::config::AUDIO_EXTENSIONS;
use crate::corpus::db::types::FileSignalType;
use crate::db_thread;

use super::types::ComputationWitness;

// ============================================================================
// File Type Detection
// ============================================================================

/// Check if path has audio file extension.
pub(super) fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ============================================================================
// Parsing Helpers
// ============================================================================

/// Parse comma-separated track IDs into Vec<i64>.
///
/// Filters out invalid integers and whitespace.
pub(super) fn parse_track_ids_csv(s: &str) -> Vec<i64> {
    s.split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// Extract modification time from metadata as (seconds, nanoseconds) tuple.
///
/// Returns (0, 0) if mtime extraction fails.
pub(super) fn extract_mtime(metadata: &std::fs::Metadata) -> (i64, i64) {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0))
}

// ============================================================================
// Directory Enumeration
// ============================================================================

/// Enumerate all directories under root, including root itself.
///
/// Returns (directories, symlink_count). Symlinks are skipped.
pub(super) fn enumerate_all_directories(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut symlink_count = 0;
    enumerate_directories_recursive(root, &mut directories, &mut symlink_count);

    // Include root itself (for files directly in root)
    directories.push(root.to_path_buf());

    (directories, symlink_count)
}

/// Recursively enumerate directories, tracking symlinks.
pub(super) fn enumerate_directories_recursive(dir: &Path, directories: &mut Vec<PathBuf>, symlink_count: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Check for symlinks - we don't follow them
        if path.is_symlink() {
            if path.is_dir() {
                *symlink_count += 1;
            }
            continue;
        }

        if path.is_dir() {
            directories.push(path.clone());
            enumerate_directories_recursive(&path, directories, symlink_count);
        }
    }
}

// ============================================================================
// Configuration Access
// ============================================================================

/// Extract unique library names from deploy mappings.
pub(super) fn get_configured_library_names(config: &crate::config::Config) -> Vec<String> {
    use std::collections::HashSet;
    let mut names = HashSet::new();
    for mapping in &config.deploy_mappings {
        for name in &mapping.library_names {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

// ============================================================================
// Signal Emission Helpers (with freshness checks)
// ============================================================================

/// Ensure a file signal exists, but only queue the write if it doesn't already exist.
///
/// Uses the read-only DB to check freshness before queueing to the write thread.
/// This dramatically reduces redundant writes during re-computation.
pub(super) fn ensure_file_signal_if_missing(
    db: &crate::corpus::db::Database,
    sender: &db_thread::SignalWriteSender,
    signal_type: FileSignalType,
    key: &str,
    witness: &ComputationWitness,
) {
    if !db.file_signal_exists(signal_type, key) {
        sender.ensure_file_signal(signal_type, key, witness);
    }
}

/// Clear a file signal, but only queue the delete if it currently exists.
///
/// Uses the read-only DB to check existence before queueing to the write thread.
/// This dramatically reduces redundant writes during re-computation.
pub(super) fn clear_file_signal_if_present(
    db: &crate::corpus::db::Database,
    sender: &db_thread::SignalWriteSender,
    signal_type: FileSignalType,
    key: &str,
    witness: &ComputationWitness,
) {
    if db.file_signal_exists(signal_type, key) {
        sender.clear_file_signal(signal_type, key, witness);
    }
}
