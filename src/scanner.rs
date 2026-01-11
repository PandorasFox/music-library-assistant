//! Scan Module
//!
//! Part of MLA's toolkit: provides the "scan" capability for indexing audio files.

use anyhow::Result;
use rayon::prelude::*;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Instant, SystemTime};
use walkdir::WalkDir;

use crate::config;
use crate::db::Database;
use crate::metadata;
use crate::progress::{ScanMessage, ScanProgress, ScanResult};

#[derive(Debug, Clone)]
struct FileInfo {
    path: PathBuf,
    size: u64,
    inode: u64,
    mtime: SystemTime,
}

struct PreScanResult {
    total_bytes: u64,
    files_to_scan: Vec<FileInfo>,
    files_unchanged: Vec<FileInfo>,
    files_to_scan_bytes: u64,
}

/// Check if a file path has an audio file extension
fn is_audio_file_by_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        if let Some(ext_str) = ext.to_str() {
            let ext_lower = ext_str.to_lowercase();
            return matches!(
                ext_lower.as_str(),
                "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "wma" | "ape" | "wv"
            );
        }
    }
    false
}

/// Pre-scan directory to count total bytes and collect file list
/// Now with incremental scanning support: queries scan_state to skip unchanged files
fn pre_scan_directory(path: &Path, source_name: &str, db: &Database) -> Result<PreScanResult> {
    let mut total_bytes = 0u64;
    let mut all_files = Vec::new();

    // Step 1: Collect all audio files with metadata
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();
        if !is_audio_file_by_extension(file_path) {
            continue;
        }

        let metadata = entry.metadata()?;
        let file_size = metadata.len();
        let inode = metadata.ino();
        let mtime = metadata.modified()?;

        all_files.push(FileInfo {
            path: file_path.to_path_buf(),
            size: file_size,
            inode,
            mtime,
        });

        total_bytes += file_size;
    }

    // Step 2: Batch query scan_state for all inodes
    let inodes: Vec<i64> = all_files.iter().map(|f| f.inode as i64).collect();
    let scan_state_map = db.get_scan_state_batch(source_name, &inodes)?;

    // Step 3: Categorize files into unchanged vs. to_scan
    let mut files_to_scan = Vec::new();
    let mut files_unchanged = Vec::new();
    let mut files_to_scan_bytes = 0u64;

    for file_info in all_files {
        let inode_i64 = file_info.inode as i64;
        let mut needs_scan = true;

        if let Some(cached) = scan_state_map.get(&inode_i64) {
            // Check if mtime matches
            if let Ok(duration) = file_info.mtime.duration_since(SystemTime::UNIX_EPOCH) {
                let mtime_secs = duration.as_secs() as i64;
                let mtime_nanos = duration.subsec_nanos();

                if mtime_secs == cached.mtime_secs && mtime_nanos == cached.mtime_nanos {
                    // File unchanged, skip scanning
                    needs_scan = false;
                }
            }
        }

        if needs_scan {
            files_to_scan_bytes += file_info.size;
            files_to_scan.push(file_info);
        } else {
            files_unchanged.push(file_info);
        }
    }

    Ok(PreScanResult {
        total_bytes,
        files_to_scan,
        files_unchanged,
        files_to_scan_bytes,
    })
}

/// Scan directory with progress reporting and cancellation support
/// Now with incremental scanning: skips unchanged files based on inode + mtime
pub fn scan_directory_with_progress(
    path: &Path,
    source_name: &str,
    progress_tx: Option<mpsc::Sender<ScanMessage>>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<ScanResult> {
    use crate::db::ScanStateEntry;
    use std::collections::HashSet;

    let start_time = Instant::now();

    // Step 1: Open database first
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    // Step 2: Pre-scan with incremental scan support
    let pre_scan = pre_scan_directory(path, source_name, &db)?;
    let _total_bytes = pre_scan.total_bytes;
    let files_to_scan = pre_scan.files_to_scan;
    let files_unchanged = pre_scan.files_unchanged;
    let files_to_scan_bytes = pre_scan.files_to_scan_bytes;

    // Send initial progress with skip statistics
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(ScanMessage::Progress(ScanProgress {
            total_bytes: files_to_scan_bytes,
            bytes_processed: 0,
            files_processed: 0,
            total_files: files_to_scan.len(),
            current_file: Some(format!(
                "Indexing {} new/modified files (skipped {} unchanged)",
                files_to_scan.len(),
                files_unchanged.len()
            )),
            errors: 0,
            start_time,
        }));
    }

    // Step 3: Parallel processing with rayon (only for files_to_scan)
    let bytes_processed = AtomicU64::new(0);
    let files_processed = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    let total_files = files_to_scan.len();

    let tracks: Vec<_> = files_to_scan
        .par_iter()
        .filter_map(|file_info| {
            // Check cancellation
            if cancel_flag.load(Ordering::Relaxed) {
                return None;
            }

            // Extract metadata
            let result = metadata::extract_metadata(&file_info.path, source_name);

            // Update progress
            bytes_processed.fetch_add(file_info.size, Ordering::Relaxed);
            let files = files_processed.fetch_add(1, Ordering::Relaxed) + 1;

            // Send progress update every 10 files
            if files % 10 == 0 {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(ScanMessage::Progress(ScanProgress {
                        total_bytes: files_to_scan_bytes,
                        bytes_processed: bytes_processed.load(Ordering::Relaxed),
                        files_processed: files,
                        total_files,
                        current_file: Some(
                            file_info
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("...")
                                .to_string(),
                        ),
                        errors: errors.load(Ordering::Relaxed),
                        start_time,
                    }));
                }
            }

            match result {
                Ok(track) => Some(track),
                Err(e) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    // Log error to file
                    let error_msg = format!(
                        "Failed to extract metadata from {}: {}",
                        file_info.path.display(),
                        e
                    );
                    let _ = config::log_scan_error(&error_msg);
                    None
                }
            }
        })
        .collect();

    // Check if cancelled
    if cancel_flag.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("Scan cancelled by user"));
    }

    // Step 4: Batch insert to database (INSERT OR REPLACE keeps existing data)
    for track in tracks {
        db.insert_track(&track)?;
    }

    // Step 5: Update scan_state for all processed files
    for file_info in &files_to_scan {
        if let Ok(duration) = file_info.mtime.duration_since(SystemTime::UNIX_EPOCH) {
            let entry = ScanStateEntry {
                source: source_name.to_string(),
                inode: file_info.inode as i64,
                path: file_info.path.to_string_lossy().to_string(),
                mtime_secs: duration.as_secs() as i64,
                mtime_nanos: duration.subsec_nanos(),
                file_size: file_info.size as i64,
            };
            db.upsert_scan_state(&entry)?;
        }
    }

    // Step 6: Cleanup stale scan_state entries (files that no longer exist)
    let all_inodes: HashSet<i64> = files_to_scan
        .iter()
        .chain(files_unchanged.iter())
        .map(|f| f.inode as i64)
        .collect();
    let _deleted = db.cleanup_stale_scan_state(source_name, &all_inodes)?;

    let duration = start_time.elapsed();
    let result = ScanResult {
        files_scanned: files_processed.load(Ordering::Relaxed),
        files_skipped: files_unchanged.len(),
        bytes_scanned: bytes_processed.load(Ordering::Relaxed),
        errors: errors.load(Ordering::Relaxed),
        duration,
    };

    // Log scan
    let scan_time = SystemTime::now();
    let time_str = chrono::DateTime::<chrono::Utc>::from(scan_time)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    db.log_scan(source_name, result.files_scanned, &time_str, &time_str)?;

    // Send completion
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(ScanMessage::Complete(result.clone()));
    }

    Ok(result)
}
