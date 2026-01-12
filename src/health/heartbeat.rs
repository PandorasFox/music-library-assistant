//! Startup Heartbeat Check
//!
//! Quick validation of corpus against index at app launch.
//! Detects files missing from disk or new files not yet indexed.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::config::{self, Config};
use crate::db::Database;

/// Result of the startup heartbeat check
#[derive(Debug, Clone)]
pub struct HeartbeatResult {
    /// Number of files in the index
    #[allow(dead_code)]
    pub indexed_count: usize,
    /// Number of audio files found on disk
    #[allow(dead_code)]
    pub disk_count: usize,
    /// Number of indexed files missing from disk
    pub missing_from_disk: usize,
    /// Number of files on disk not in index
    pub new_on_disk: usize,
    /// How long the check took
    pub duration: Duration,
}

impl HeartbeatResult {
    /// Returns true if the corpus is in sync with the index
    pub fn is_healthy(&self) -> bool {
        self.missing_from_disk == 0 && self.new_on_disk == 0
    }
}

/// Audio file extensions to check
const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "ogg", "m4a", "opus", "wav", "aiff", "aif"];

/// Spawn heartbeat check in a background thread
pub fn spawn_heartbeat(config: &Config) -> mpsc::Receiver<HeartbeatResult> {
    let (tx, rx) = mpsc::channel();
    let corpus_root = config.corpus_root.clone();

    std::thread::spawn(move || {
        let result = run_heartbeat(&corpus_root);
        let _ = tx.send(result);
    });

    rx
}

/// Run the heartbeat check synchronously
fn run_heartbeat(corpus_root: &Path) -> HeartbeatResult {
    let start = Instant::now();

    // Get database path and open connection
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => {
            return HeartbeatResult {
                indexed_count: 0,
                disk_count: 0,
                missing_from_disk: 0,
                new_on_disk: 0,
                duration: start.elapsed(),
            };
        }
    };

    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(_) => {
            return HeartbeatResult {
                indexed_count: 0,
                disk_count: 0,
                missing_from_disk: 0,
                new_on_disk: 0,
                duration: start.elapsed(),
            };
        }
    };

    // Get indexed inodes from scan_state for corpus
    let indexed_inodes = get_indexed_inodes(&db);

    // Walk corpus directory and collect audio file inodes
    let disk_inodes = walk_corpus_inodes(corpus_root);

    // Calculate differences
    let missing_from_disk = indexed_inodes.difference(&disk_inodes).count();
    let new_on_disk = disk_inodes.difference(&indexed_inodes).count();

    HeartbeatResult {
        indexed_count: indexed_inodes.len(),
        disk_count: disk_inodes.len(),
        missing_from_disk,
        new_on_disk,
        duration: start.elapsed(),
    }
}

/// Get all indexed inodes for corpus from scan_state table
fn get_indexed_inodes(db: &Database) -> HashSet<i64> {
    // Query scan_state for all corpus inodes
    // This is faster than querying tracks table
    db.get_all_scan_state_inodes("corpus").unwrap_or_default()
}

/// Walk corpus directory and collect inodes of audio files
fn walk_corpus_inodes(root: &Path) -> HashSet<i64> {
    let mut inodes = HashSet::new();

    if !root.exists() {
        return inodes;
    }

    walk_dir_recursive(root, &mut inodes);
    inodes
}

/// Recursively walk directory and collect audio file inodes
fn walk_dir_recursive(dir: &Path, inodes: &mut HashSet<i64>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            walk_dir_recursive(&path, inodes);
        } else if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                inodes.insert(metadata.ino() as i64);
            }
        }
    }
}

/// Check if path has audio file extension
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}
