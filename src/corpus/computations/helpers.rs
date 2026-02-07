//! Shared utility functions for computations.
//!
//! These helpers are used across multiple computation modules for common tasks
//! like file type detection, path parsing, signal emission, and configuration access.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::AUDIO_EXTENSIONS;
use crate::corpus::paths;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, CorpusFileSignalType};
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread::{self, SignalWitness};

use super::types::ComputationWitness;

// ============================================================================
// File Type Detection
// ============================================================================

/// Check if a filename is a macOS resource fork (AppleDouble) file.
///
/// These are metadata files created by macOS on non-HFS+ filesystems (NFS, SMB, etc.)
/// with names like `._filename.mp3`. They should be skipped during corpus scanning.
pub(super) fn is_macos_resource_fork(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("._"))
        .unwrap_or(false)
}

/// Check if path has audio file extension.
///
/// Also filters out macOS resource fork files (`._*`) which appear on NFS/SMB mounts.
pub(super) fn is_audio_file(path: &Path) -> bool {
    // Skip macOS resource fork files (._filename.ext)
    if is_macos_resource_fork(path) {
        return false;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ============================================================================
// Parsing Helpers
// ============================================================================

/// Parse comma-separated inodes into Vec<i64>.
///
/// Filters out invalid integers and whitespace.
pub(super) fn parse_inodes_csv(s: &str) -> Vec<i64> {
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

/// Recursively enumerate directories, tracking symlinks and checking mount boundaries.
///
/// If a directory is on a different filesystem (different st_dev) than the expected
/// root filesystem, reports a mount violation via the global flag. The Witch will
/// pick this up on next tick() and latch into read-only mode.
pub(super) fn enumerate_directories_recursive(dir: &Path, directories: &mut Vec<PathBuf>, symlink_count: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    // Get expected device ID for mount boundary checks
    let expected_dev = paths::get_expected_device_id();

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
            // Check for mount boundary violation
            if let Some(expected) = expected_dev {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let actual_dev = metadata.dev();
                    if actual_dev != expected {
                        // Mount boundary crossed! Report violation.
                        crate::witch::report_mount_violation(format!(
                            "Nested mount point detected at {:?}\n\
                             Expected device: {}, found device: {}\n\
                             \n\
                             The corpus and libraries must not contain nested mount points.\n\
                             Please unmount the nested filesystem and restart MLA.",
                            path, expected, actual_dev
                        ));
                        // Skip this directory and its children - don't recurse into different filesystem
                        continue;
                    }
                }
            }

            directories.push(path.clone());
            enumerate_directories_recursive(&path, directories, symlink_count);
        }
    }
}

// ============================================================================
// Configuration Access
// ============================================================================

/// Extract unique library names from source directories.
pub(super) fn get_configured_library_names(config: &crate::config::Config) -> Vec<String> {
    use std::collections::HashSet;
    let mut names = HashSet::new();
    for source in &config.source_dirs {
        for lib in &source.libraries {
            names.insert(lib.clone());
        }
    }
    names.into_iter().collect()
}

// ============================================================================
// Signal Emission Helpers (Native Inode Column)
// ============================================================================
// These helpers use the native `inode` column in the signals table for
// inode-keyed corpus signals. This is the preferred pattern for all
// corpus file signals.

/// Ensure a corpus signal exists using the native inode column.
///
/// Uses `read_only_db.corpus_signal_exists_by_inode()` for efficient freshness check,
/// then queues to the write thread if needed.
pub(crate) fn ensure_corpus_signal(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: CorpusFileSignalType,
    inode: i64,
    path: &str,
    witness: &impl SignalWitness,
) {
    if !read_only_db.corpus_signal_exists_by_inode(signal_type, inode) {
        sender.ensure_corpus_signal(signal_type, inode, path, witness);
    }
}

/// Ensure a corpus signal exists with additional metadata.
///
/// Uses the native inode column and merges path into the metadata.
pub(crate) fn ensure_corpus_signal_with_metadata(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: CorpusFileSignalType,
    inode: i64,
    path: &str,
    extra_metadata: serde_json::Value,
    witness: &impl SignalWitness,
) {
    if !read_only_db.corpus_signal_exists_by_inode(signal_type, inode) {
        sender.ensure_corpus_signal_with_metadata(signal_type, inode, path, extra_metadata, witness);
    }
}

/// Drop a stale corpus signal using the native inode column.
///
/// Use when a computation determines the signal should not exist for this inode.
pub(crate) fn drop_stale_corpus_signal(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: CorpusFileSignalType,
    inode: i64,
    witness: &impl SignalWitness,
) {
    if read_only_db.corpus_signal_exists_by_inode(signal_type, inode) {
        sender.clear_corpus_signal(signal_type, inode, witness);
    }
}

// ============================================================================
// Signal Emission Helpers (Aggregate - Semantic Keys)
// ============================================================================
// These helpers use semantic string keys for aggregate signals.
// Used for LibraryStale, LibraryLeftover, and other semantic-keyed signals.

/// Ensure an aggregate signal exists, but only queue the write if it doesn't already exist.
///
/// Uses the read-only DB to check freshness before queueing to the write thread.
pub(crate) fn ensure_aggregate_signal_if_missing(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: AggregateSignalType,
    key: &str,
    metadata_json: Option<&str>,
    witness: &impl SignalWitness,
) {
    if !read_only_db.aggregate_signal_exists(signal_type, key) {
        sender.ensure_aggregate_signal(signal_type, key, metadata_json, witness);
    }
}

/// Drop a stale aggregate signal that this computation determined should not exist.
///
/// Use when a computation definitively determines "signal X should NOT exist for this key".
pub(crate) fn drop_stale_aggregate_signal(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: AggregateSignalType,
    key: &str,
    witness: &impl SignalWitness,
) {
    if read_only_db.aggregate_signal_exists(signal_type, key) {
        sender.clear_aggregate_signal(signal_type, key, witness);
    }
}

// ============================================================================
// Aggregate Signal Set Logic
// ============================================================================

/// A computed aggregate signal ready for reconciliation.
///
/// Contains the key, inodes, and optional extra metadata for comparison.
pub(super) struct ComputedAggregateSignal {
    pub key: String,
    pub inodes: Vec<i64>,
    pub metadata_json: String,
}

/// Extract inodes from metadata JSON string.
///
/// Returns empty vec if parsing fails or inodes not present.
fn extract_inodes_from_metadata(metadata_json: Option<&str>) -> Vec<i64> {
    metadata_json
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("inodes").cloned())
        .and_then(|arr| serde_json::from_value::<Vec<i64>>(arr).ok())
        .unwrap_or_default()
}

/// Reconcile computed signals against existing DB signals.
///
/// Computes set differences and queues appropriate operations:
/// - Stale signals (exist in DB but not computed): cleared
/// - New signals (computed but not in DB): ensured
/// - Changed signals (exist in both but inodes differ): replaced with new timestamp
/// - Unchanged signals (exist in both, same inodes): no-op
///
/// Returns (cleared_count, new_count, updated_count, unchanged_count).
pub(super) fn reconcile_aggregate_signals(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &db_thread::SignalWriteSender,
    signal_type: AggregateSignalType,
    computed: Vec<ComputedAggregateSignal>,
    witness: &ComputationWitness,
) -> (usize, usize, usize, usize) {
    // Get existing signals from DB
    let existing = read_only_db
        .get_aggregate_signal_keys_with_metadata(signal_type)
        .unwrap_or_default();

    // Build lookup maps
    let computed_map: HashMap<&str, &ComputedAggregateSignal> =
        computed.iter().map(|s| (s.key.as_str(), s)).collect();

    let existing_map: HashMap<&str, Option<&str>> = existing
        .iter()
        .map(|(k, m)| (k.as_str(), m.as_deref()))
        .collect();

    let computed_keys: HashSet<&str> = computed_map.keys().copied().collect();
    let existing_keys: HashSet<&str> = existing_map.keys().copied().collect();

    let mut cleared = 0;
    let mut new_count = 0;
    let mut updated = 0;
    let mut unchanged = 0;

    // Stale signals: exist in DB but not computed -> clear
    for key in existing_keys.difference(&computed_keys) {
        sender.clear_aggregate_signal(signal_type, key, witness);
        cleared += 1;
    }

    // New signals: computed but not in DB -> ensure
    for key in computed_keys.difference(&existing_keys) {
        let signal = computed_map[key];
        sender.ensure_aggregate_signal(
            signal_type,
            &signal.key,
            Some(&signal.metadata_json),
            witness,
        );
        new_count += 1;
    }

    // Existing signals: check if inodes changed
    for key in computed_keys.intersection(&existing_keys) {
        let computed_signal = computed_map[key];
        let existing_metadata = existing_map[key];

        let existing_inodes = extract_inodes_from_metadata(existing_metadata);

        // Compare inodes (sorted for stable comparison)
        let mut computed_ids = computed_signal.inodes.clone();
        let mut existing_ids = existing_inodes;
        computed_ids.sort();
        existing_ids.sort();

        if computed_ids != existing_ids {
            // Inodes changed -> replace (updates timestamp)
            let signal = AggregateSignal {
                id: None,
                signal_type,
                key: computed_signal.key.clone(),
                discovered_at: None, // Will use CURRENT_TIMESTAMP
                metadata_json: Some(computed_signal.metadata_json.clone()),
            };
            sender.replace_aggregate_signal(signal, witness);
            updated += 1;
        } else {
            // Unchanged
            unchanged += 1;
        }
    }

    (cleared, new_count, updated, unchanged)
}
