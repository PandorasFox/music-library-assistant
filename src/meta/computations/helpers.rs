//! Shared utility functions for computations.
//!
//! These helpers are used across multiple computation modules for common tasks
//! like file type detection, path parsing, signal emission, and configuration access.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::AUDIO_EXTENSIONS;
use crate::corpus::paths;
use crate::db::write_thread::{self, SignalWitness};
use crate::db::ReadOnlyDb;
use crate::meta::signals::registry::TypedSignalWrite;
use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};

use super::types::ComputationWitness;

// ============================================================================
// File Type Detection
// ============================================================================

/// Image file extensions we recognise.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

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

/// Check if path has an image file extension.
///
/// Filters out macOS resource fork files (`._*`).
pub(crate) fn is_image_file(path: &Path) -> bool {
    if is_macos_resource_fork(path) {
        return false;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
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
pub(super) fn enumerate_directories_recursive(
    dir: &Path,
    directories: &mut Vec<PathBuf>,
    symlink_count: &mut usize,
) {
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
                             Please unmount the nested filesystem and restart MM.",
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
// Observation Generation Counter (vestigial — kept for UpdateCorpusFileSignals)
// ============================================================================

static OBSERVATION_GENERATION: AtomicU8 = AtomicU8::new(0);

/// Get the current observation generation (vestigial — used by UpdateCorpusFileSignals).
pub(crate) fn current_observation_generation() -> u8 {
    OBSERVATION_GENERATION.load(Ordering::SeqCst) % 13
}

// ============================================================================
// Signal Emission Helpers (Native Inode Column)
// ============================================================================
// These helpers use the native `inode` column in the signals table for
// inode-keyed corpus signals. This is the preferred pattern for all
// corpus file signals.

/// Ensure a typed signal exists in the database.
///
/// Uses `ReadOnlyDb::signal_exists()` for efficient freshness check,
/// then queues a typed write if the signal doesn't already exist.
pub(crate) fn ensure_typed_signal(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    signal: TypedSignalWrite,
    witness: &impl SignalWitness,
) {
    if !read_only_db.signal_exists(&signal) {
        sender.write_typed_signal(signal, witness);
    }
}

/// Drop a stale corpus signal by inode.
///
/// Use when a computation determines the signal should not exist for this inode.
pub(crate) fn drop_stale_corpus_signal<S: CorpusSignalStore>(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    inode: i64,
    witness: &impl SignalWitness,
) {
    if read_only_db.corpus_signal_exists::<S>(inode) {
        sender.clear_corpus_signal::<S>(inode, witness);
    }
}

// ============================================================================
// Signal Emission Helpers (Aggregate - Semantic Keys)
// ============================================================================

// ============================================================================
// Aggregate Signal Set Logic
// ============================================================================

/// A computed aggregate signal ready for reconciliation.
///
/// Contains the key, typed data, and content hash for change detection.
pub(super) struct ComputedAggregateSignal {
    pub key: String,
    pub typed_data: TypedSignalWrite,
    pub content_hash: i64,
}

impl ComputedAggregateSignal {
    /// Create a new computed signal, auto-computing the content hash.
    pub fn new(key: String, typed_data: TypedSignalWrite) -> Self {
        let content_hash = typed_data.content_hash() as i64;
        Self {
            key,
            typed_data,
            content_hash,
        }
    }
}

/// A computed corpus signal ready for reconciliation.
///
/// Contains the inode, typed data, and content hash for change detection.
pub(super) struct ComputedCorpusSignal {
    pub inode: i64,
    pub typed_data: TypedSignalWrite,
    pub content_hash: i64,
}

impl ComputedCorpusSignal {
    /// Create a new computed corpus signal, auto-computing the content hash.
    pub fn new(inode: i64, typed_data: TypedSignalWrite) -> Self {
        let content_hash = typed_data.content_hash() as i64;
        Self {
            inode,
            typed_data,
            content_hash,
        }
    }
}

/// Reconcile computed aggregate signals against existing DB signals.
///
/// Uses hash-based change detection to skip unchanged signals:
/// - Stale signals (exist in DB but not computed): cleared
/// - New signals (computed but not in DB): written
/// - Changed signals (key exists but hash differs): written
/// - Unchanged signals (key exists and hash matches): skipped
///
/// Returns (cleared, new, updated, unchanged).
pub(super) fn reconcile_aggregate_signals<S: AggregateSignalStore>(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    computed: Vec<ComputedAggregateSignal>,
    witness: &ComputationWitness,
) -> (usize, usize, usize, usize) {
    let existing_hashes: HashMap<String, i64> = read_only_db
        .aggregate_signal_key_hashes::<S>()
        .unwrap_or_default();

    let computed_keys: HashSet<&str> = computed.iter().map(|s| s.key.as_str()).collect();

    let mut cleared = 0;
    let mut new = 0;
    let mut updated = 0;
    let mut unchanged = 0;

    // Stale: exist in DB but not computed -> clear
    for key in existing_hashes.keys() {
        if !computed_keys.contains(key.as_str()) {
            sender.clear_aggregate_signal::<S>(key, witness);
            cleared += 1;
        }
    }

    // For each computed signal: check hash to decide write vs skip
    let mut batch: Vec<TypedSignalWrite> = Vec::new();
    for signal in &computed {
        match existing_hashes.get(&signal.key) {
            Some(&existing_hash) if existing_hash == signal.content_hash => {
                // Hash matches — skip write
                unchanged += 1;
            }
            Some(_) => {
                // Key exists but hash differs — update
                batch.push(signal.typed_data.clone());
                updated += 1;
            }
            None => {
                // New signal
                batch.push(signal.typed_data.clone());
                new += 1;
            }
        }
    }
    sender.write_typed_signal_batch(batch, witness);

    (cleared, new, updated, unchanged)
}

/// Reconcile computed corpus signals against existing DB signals.
///
/// Uses hash-based change detection for BLOB signal types.
/// For scalar-only signal types (which return empty hash maps), falls back
/// to inode existence checks.
///
/// Returns (cleared, new, updated, unchanged).
pub(super) fn reconcile_corpus_signals<S: CorpusSignalStore>(
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    computed: Vec<ComputedCorpusSignal>,
    witness: &ComputationWitness,
) -> (usize, usize, usize, usize) {
    let existing_hashes: HashMap<i64, i64> = read_only_db
        .corpus_signal_inode_hashes::<S>()
        .unwrap_or_default();

    // Scalar-only types return an empty hash map from query_inode_hashes.
    // Detect this by also fetching the inode list. If there are inodes but
    // no hashes, we're dealing with a scalar type.
    let existing_inodes_vec: Vec<i64> = if existing_hashes.is_empty() {
        read_only_db
            .corpus_signal_all_inodes::<S>()
            .unwrap_or_default()
    } else {
        Vec::new() // not needed when we have hashes
    };

    let use_hashes = !existing_hashes.is_empty() || existing_inodes_vec.is_empty();

    let existing_inodes: HashSet<i64> = if use_hashes {
        existing_hashes.keys().copied().collect()
    } else {
        existing_inodes_vec.into_iter().collect()
    };

    let computed_inodes: HashSet<i64> = computed.iter().map(|s| s.inode).collect();

    let mut cleared = 0;
    let mut new = 0;
    let mut updated = 0;
    let mut unchanged = 0;

    // Stale: exist in DB but not computed -> clear
    for &inode in &existing_inodes {
        if !computed_inodes.contains(&inode) {
            sender.clear_corpus_signal::<S>(inode, witness);
            cleared += 1;
        }
    }

    // For each computed signal: check hash or existence to decide write vs skip
    let mut batch: Vec<TypedSignalWrite> = Vec::new();
    for signal in &computed {
        if use_hashes {
            match existing_hashes.get(&signal.inode) {
                Some(&existing_hash) if existing_hash == signal.content_hash => {
                    unchanged += 1;
                }
                Some(_) => {
                    batch.push(signal.typed_data.clone());
                    updated += 1;
                }
                None => {
                    batch.push(signal.typed_data.clone());
                    new += 1;
                }
            }
        } else {
            // Scalar-only: existence check
            if existing_inodes.contains(&signal.inode) {
                unchanged += 1;
            } else {
                batch.push(signal.typed_data.clone());
                new += 1;
            }
        }
    }
    sender.write_typed_signal_batch(batch, witness);

    (cleared, new, updated, unchanged)
}
