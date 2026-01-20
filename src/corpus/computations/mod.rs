//! Corpus Computations Module
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `DecisionWitness` to queue (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Eyeballing Computation Chain
//!
//! The eyeballing system uses computation chaining:
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFromDisk     (spawn next)   OutOfBandTagChange
//!             MissingFromIndex
//! ```
//!
//! Each computation can spawn follow-up computations via `ComputationResult::spawn`.
//!
//! ## Architecture
//!
//! ```text
//! Eyeballing (automatic)
//!     │
//!     └──► TaskDaemon.queue_computation() ───► Computation Executor
//!                                                    │
//!                                                    ├──► Signals (health_issues table)
//!                                                    └──► Spawned Computations (chained)
//! ```
//!
//! Computations never bypass the witness system because they don't alter state.
//! They only observe and record findings.
//!
//! ## ComputationWitness
//!
//! Signal-altering operations (insert, delete, upsert) require a `ComputationWitness`
//! which can only be created inside computation execution. This ensures signals are
//! only modified through the computation system, not ad-hoc from UI or other code.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::log_message;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, FileSignalType};
use crate::db_thread;

// ============================================================================
// ComputationWitness - Proof of Computation Execution Context
// ============================================================================

/// Sealed module to prevent external construction of ComputationWitness.
mod sealed {
    /// Zero-sized proof that code is executing within a computation context.
    ///
    /// This witness is required by signal-altering database operations to ensure
    /// health signals are only modified through the computation system.
    ///
    /// Cannot be constructed outside of `execute_single` or the DB write thread.
    #[derive(Debug, Clone, Copy)]
    pub struct ComputationWitness(());

    impl ComputationWitness {
        /// Create a new witness. Only callable from within this crate's computation execution.
        pub(crate) fn new() -> Self {
            Self(())
        }

        /// Create a witness for the DB write thread.
        ///
        /// The DB thread executes signal operations that were enqueued from legitimate
        /// computation contexts (which required a witness at send time). This constructor
        /// allows the DB thread to obtain a witness for the actual DB call.
        pub(crate) fn new_for_db_thread() -> Self {
            Self(())
        }
    }
}

pub use sealed::ComputationWitness;

/// A computation operation that derives facts without altering corpus state.
///
/// Computations can be queued without a `DecisionWitness` because they only
/// emit signals - they don't modify files or index data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Verify tags on disk match database, recording mismatches as signals.
    ///
    /// This compares the actual file tags to what's stored in the index.
    /// Any mismatches are recorded as health signals for operator review.
    VerifyTags {
        track_id: i64,
        path: PathBuf,
    },

    // -------------------------------------------------------------------------
    // Eyeballing Computations
    // -------------------------------------------------------------------------

    /// Phase 1: Walk corpus directory tree to collect file state.
    ///
    /// Enumerates top-level directories under root and spawns per-directory scans.
    /// This provides granular progress feedback during startup.
    WalkCorpus {
        root: PathBuf,
        source: String,  // "corpus" or "legacy"
        /// If true, verify tags for ALL files (paranoid mode)
        paranoid: bool,
    },

    /// Walk and scan a single directory subtree.
    ///
    /// Walks the directory recursively, collects disk state, and compares
    /// against scan_state. Creates FileInCorpus signals and spawns
    /// mtime/tag verification as needed.
    ScanCorpusDirectory {
        directory: PathBuf,
        source: String,
        paranoid: bool,
    },

    /// Phase 2: Compare disk state to database index.
    ///
    /// Creates signals for:
    /// - MissingFromDisk: indexed files not on disk
    /// - MissingFromIndex: disk files not indexed
    ///
    /// Spawns: VerifyMtime for files with mtime mismatches (non-paranoid)
    ///         or VerifyTags for all files (paranoid mode)
    CompareInodes {
        source: String,
        /// Disk state: (inode, path, mtime_secs, mtime_nanos)
        disk_state: Vec<(i64, PathBuf, i64, i64)>,
        /// If true, verify tags for ALL files (paranoid mode)
        paranoid: bool,
    },

    /// Phase 3: Verify single file mtime.
    ///
    /// Checks if current mtime differs from expected (from scan_state).
    /// Spawns: VerifyTags if mtime mismatched.
    VerifyMtime {
        track_id: i64,
        path: PathBuf,
        expected_mtime_secs: i64,
        expected_mtime_nanos: i64,
    },

    // -------------------------------------------------------------------------
    // Second-Level Signal Computations
    // -------------------------------------------------------------------------

    /// Schedule second-level signal derivations.
    ///
    /// This is an orchestrator computation that spawns per-directory computations.
    /// Called during daemon Awakening phase after first-level eyeballing completes.
    ///
    /// Spawns: `DeriveDirectorySignals` for each directory that has either:
    /// - FileInCorpus signals (corpus directories with audio files)
    /// - Indexed tracks (may be missing from corpus now)
    ScheduleSecondLevelDerivations,

    /// Derive second-level signals for a single directory.
    ///
    /// Compares FileInCorpus signals against indexed tracks for this directory:
    /// - FileInCorpus without matching track → `UnindexedFile`
    /// - Track without matching FileInCorpus → `MissingFile`
    /// - Track with matching FileInCorpus (same inode/mtime) → `HealthyFile`
    /// - Track with different mtime → `CorpusFileModifiedOutOfBand`
    ///
    /// For files that become `HealthyFile`, spawns `CheckDeployConflicts`.
    DeriveDirectorySignals { directory: PathBuf },

    /// Check for deployment conflicts on a healthy track.
    ///
    /// Third-level computation triggered when a file is marked as healthy.
    /// Checks if this track has deployment conflicts with other tracks.
    CheckDeployConflicts { track_id: i64 },

    /// Update signals for a single file after mutation.
    ///
    /// Lightweight per-file computation that updates:
    /// - FileInCorpus: if file exists on disk
    /// - HealthyFile: if file exists in corpus AND is indexed
    /// - UnindexedFile: if file exists in corpus but NOT indexed
    /// - MissingFile: if file is indexed but NOT in corpus
    ///
    /// Does NOT spawn CheckDeployConflicts (handled in bulk by DetectDeployConflicts).
    UpdateFileSignals { path: PathBuf },

    // -------------------------------------------------------------------------
    // Library Health Computations
    // -------------------------------------------------------------------------

    /// Walk a library directory tree to collect file inodes.
    ///
    /// Spawns `ScanLibraryDirectory` for each subdirectory found.
    WalkLibrary {
        library_root: PathBuf,
        library_name: String,
        /// Corpus path prefixes that deploy to this library (from config.deploy_mappings)
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Scan a single library directory and collect (path, inode) pairs.
    ///
    /// Results are accumulated in memory, then passed to DeriveLibraryHealthSignals.
    ScanLibraryDirectory {
        directory: PathBuf,
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
    },

    /// Derive library health signals for files in a single directory.
    ///
    /// Compares library inodes against corpus index:
    /// - Match by inode → check if path is correct (LibraryStale if wrong)
    /// - Library file without corpus backing → LibraryOrphan
    DeriveLibraryHealthSignals {
        library_name: String,
        library_root: PathBuf,
        corpus_path_prefixes: Vec<PathBuf>,
        /// Library files: (path, inode)
        library_files: Vec<(PathBuf, i64)>,
    },

    // -------------------------------------------------------------------------
    // Content Analysis Computations (Awakening Stage 2)
    // -------------------------------------------------------------------------

    /// Schedule all content analysis computations.
    ///
    /// This is an orchestrator that spawns all detection computations in parallel:
    /// - DetectFingerprintDuplicates
    /// - DetectDuplicateInodes
    /// - DetectMissingTags
    /// - DetectMetadataDuplicates
    /// - DetectTagCanonicalizations
    /// - VerifyOutOfBandChanges
    ScheduleContentAnalysis,

    /// Detect fingerprint duplicates across all tracks.
    ///
    /// Bulk SQL query: GROUP BY fingerprint HAVING COUNT > 1
    /// Creates/updates FingerprintDuplicate signals with fingerprint as issue_key.
    DetectFingerprintDuplicates,

    /// Detect duplicate inodes across all tracks.
    ///
    /// Bulk SQL query: GROUP BY inode HAVING COUNT > 1
    /// Creates DuplicateInode signals for each duplicate group.
    DetectDuplicateInodes,

    /// Detect tracks missing required tags.
    ///
    /// Checks each required tag (from config) and creates MissingTag signals.
    /// Issue key: "missing_tag:{tag_name}"
    DetectMissingTags,

    /// Detect metadata duplicates (exact match on artist/album/title).
    ///
    /// Case-insensitive tag names, CASE-SENSITIVE tag values.
    /// Creates MetadataDuplicate signals with metadata as issue_key.
    DetectMetadataDuplicates,

    /// Detect tag canonicalization opportunities.
    ///
    /// Calls existing detect_and_store_canonicalizations() which uses TagCloud.
    /// Stores to tag_canonicalization table (tag-level, not track-level).
    DetectTagCanonicalizations,

    /// Verify out-of-band changes for files with modified mtime.
    ///
    /// For each CorpusFileModifiedOutOfBand signal:
    /// - Read file tags and compare to database
    /// - If tags differ: create OutOfBandTagChange signal
    /// - If tags match: clear CorpusFileModifiedOutOfBand (file was touched but unchanged)
    VerifyOutOfBandChanges,

    /// Detect deployment conflicts across all healthy tracks (bulk).
    ///
    /// Groups healthy tracks by their computed deployment path. Tracks that would
    /// deploy to the same path are marked as DeployConflict signals.
    ///
    /// This is more efficient than per-track CheckDeployConflicts during bulk operations.
    DetectDeployConflicts,
}

impl Computation {
    /// Get the primary file path affected by this computation, if any.
    pub fn primary_path(&self) -> Option<&std::path::Path> {
        match self {
            Computation::VerifyTags { path, .. } => Some(path),
            Computation::WalkCorpus { root, .. } => Some(root),
            Computation::ScanCorpusDirectory { directory, .. } => Some(directory),
            Computation::CompareInodes { .. } => None,
            Computation::VerifyMtime { path, .. } => Some(path),
            Computation::ScheduleSecondLevelDerivations => None,
            Computation::DeriveDirectorySignals { directory } => Some(directory),
            Computation::CheckDeployConflicts { .. } => None,
            Computation::UpdateFileSignals { path } => Some(path),
            Computation::WalkLibrary { library_root, .. } => Some(library_root),
            Computation::ScanLibraryDirectory { directory, .. } => Some(directory),
            Computation::DeriveLibraryHealthSignals { .. } => None,
            // Content analysis computations
            Computation::ScheduleContentAnalysis => None,
            Computation::DetectFingerprintDuplicates => None,
            Computation::DetectDuplicateInodes => None,
            Computation::DetectMissingTags => None,
            Computation::DetectMetadataDuplicates => None,
            Computation::DetectTagCanonicalizations => None,
            Computation::VerifyOutOfBandChanges => None,
            Computation::DetectDeployConflicts => None,
        }
    }

    /// Get a human-readable label for this computation type.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::WalkCorpus { paranoid: true, .. } => "Eyeballing (paranoid)",
            Computation::WalkCorpus { paranoid: false, .. } => "Eyeballing",
            Computation::ScanCorpusDirectory { paranoid: true, .. } => "Scanning directory (paranoid)",
            Computation::ScanCorpusDirectory { paranoid: false, .. } => "Scanning directory",
            Computation::CompareInodes { .. } => "Comparing inodes",
            Computation::VerifyMtime { .. } => "Verifying mtime",
            Computation::ScheduleSecondLevelDerivations => "Scheduling signal derivations",
            Computation::DeriveDirectorySignals { .. } => "Deriving signals",
            Computation::CheckDeployConflicts { .. } => "Checking deploy conflicts",
            Computation::UpdateFileSignals { .. } => "Updating file signals",
            Computation::WalkLibrary { .. } => "Walking library",
            Computation::ScanLibraryDirectory { .. } => "Scanning library directory",
            Computation::DeriveLibraryHealthSignals { .. } => "Deriving library health",
            // Content analysis computations
            Computation::ScheduleContentAnalysis => "Scheduling content analysis",
            Computation::DetectFingerprintDuplicates => "Detecting fingerprint duplicates",
            Computation::DetectDuplicateInodes => "Detecting duplicate inodes",
            Computation::DetectMissingTags => "Detecting missing tags",
            Computation::DetectMetadataDuplicates => "Detecting metadata duplicates",
            Computation::DetectTagCanonicalizations => "Detecting tag canonicalizations",
            Computation::VerifyOutOfBandChanges => "Verifying out-of-band changes",
            Computation::DetectDeployConflicts => "Detecting deploy conflicts",
        }
    }
}

/// Result of executing a single computation.
#[derive(Debug)]
pub struct ComputationResult {
    pub computation: Computation,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Follow-up computations to queue (chaining mechanism).
    pub spawn: Vec<Computation>,
}

impl ComputationResult {
    fn success(computation: Computation, duration_ms: u64, spawn: Vec<Computation>) -> Self {
        Self {
            computation,
            success: true,
            error: None,
            duration_ms,
            spawn,
        }
    }

    fn failure(computation: Computation, duration_ms: u64, error: String) -> Self {
        Self {
            computation,
            success: false,
            error: Some(error),
            duration_ms,
            spawn: Vec::new(),
        }
    }
}

// Thread-local cached read-only database connection for computation workers.
// Each worker thread opens once and reuses, eliminating connection overhead.
thread_local! {
    static THREAD_DB: std::cell::RefCell<Option<crate::corpus::db::Database>> = const { std::cell::RefCell::new(None) };
    static THREAD_STATS: std::cell::RefCell<ThreadStats> = const { std::cell::RefCell::new(ThreadStats::new()) };
}

// ============================================================================
// Thread-Local Performance Stats
// ============================================================================

/// Number of read time samples to keep per thread for median calculation
const READ_SAMPLE_SIZE: usize = 64;

/// Performance statistics accumulated per worker thread.
///
/// Each thread maintains its own stats via thread-local storage.
/// These are periodically snapshotted and aggregated by the daemon.
#[derive(Debug, Clone)]
pub struct ThreadStats {
    /// Unique identifier for this thread (assigned on first task)
    pub thread_id: u64,
    /// Total tasks completed by this thread
    pub tasks_completed: u64,
    /// Cumulative task execution time in milliseconds
    pub total_task_ms: u64,
    /// Slowest single task execution time
    pub max_task_ms: u64,
    /// Label of the slowest task
    pub max_task_label: String,
    /// Number of DB connection opens (should be 1 per thread)
    pub db_opens: u64,
    /// Cumulative time spent in DB reads (microseconds)
    pub total_db_read_us: u64,
    /// Number of DB read operations
    pub db_read_count: u64,
    /// Slowest single DB read (microseconds)
    pub max_db_read_us: u64,
    /// Circular buffer of recent read times for median calculation
    pub read_samples: [u64; READ_SAMPLE_SIZE],
    /// Write index into read_samples (wraps around)
    pub read_sample_idx: usize,
    /// How many samples have been written (caps at READ_SAMPLE_SIZE)
    pub read_sample_count: usize,
}

impl Default for ThreadStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadStats {
    pub const fn new() -> Self {
        Self {
            thread_id: 0,
            tasks_completed: 0,
            total_task_ms: 0,
            max_task_ms: 0,
            max_task_label: String::new(),
            db_opens: 0,
            total_db_read_us: 0,
            db_read_count: 0,
            max_db_read_us: 0,
            read_samples: [0; READ_SAMPLE_SIZE],
            read_sample_idx: 0,
            read_sample_count: 0,
        }
    }

    /// Average DB read time in microseconds
    pub fn avg_db_read_us(&self) -> u64 {
        if self.db_read_count > 0 {
            self.total_db_read_us / self.db_read_count
        } else {
            0
        }
    }

    /// Average task time in milliseconds
    pub fn avg_task_ms(&self) -> u64 {
        if self.tasks_completed > 0 {
            self.total_task_ms / self.tasks_completed
        } else {
            0
        }
    }
}

/// Get a snapshot of the current thread's stats.
pub fn get_thread_stats() -> ThreadStats {
    THREAD_STATS.with(|cell| cell.borrow().clone())
}

/// Assign a thread ID if not already set. Returns the thread ID.
/// Only tracks when timing instrumentation is enabled.
fn ensure_thread_id() -> u64 {
    use crate::config;
    if !config::is_timing_enabled() {
        return 0;
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        if stats.thread_id == 0 {
            stats.thread_id = THREAD_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        }
        stats.thread_id
    })
}

/// Record a completed task's stats.
/// No-op when timing instrumentation is disabled.
fn record_task_stats(label: &str, duration_ms: u64) {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.tasks_completed += 1;
        stats.total_task_ms += duration_ms;
        if duration_ms > stats.max_task_ms {
            stats.max_task_ms = duration_ms;
            stats.max_task_label = label.to_string();
        }
    });
}

/// Record a DB read operation's timing.
/// No-op when timing instrumentation is disabled.
fn record_db_read(duration_us: u64) {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.total_db_read_us += duration_us;
        stats.db_read_count += 1;
        if duration_us > stats.max_db_read_us {
            stats.max_db_read_us = duration_us;
        }
        // Add to circular sample buffer
        let idx = stats.read_sample_idx;
        stats.read_samples[idx] = duration_us;
        stats.read_sample_idx = (idx + 1) % READ_SAMPLE_SIZE;
        if stats.read_sample_count < READ_SAMPLE_SIZE {
            stats.read_sample_count += 1;
        }
    });
}

/// Record a DB connection open.
/// No-op when timing instrumentation is disabled.
fn record_db_open() {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.db_opens += 1;
    });
}

/// Execute a function with the thread-local read-only database connection.
/// Opens and caches the connection on first use per thread.
/// Tracks DB read timing for performance instrumentation (when enabled).
fn with_thread_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&crate::corpus::db::Database) -> T,
{
    use crate::config;
    use crate::corpus::db::Database;

    THREAD_DB.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let db_path = config::get_db_path().map_err(|e| e.to_string())?;
            let db = Database::open_read_only(&db_path).map_err(|e| e.to_string())?;
            *opt = Some(db);
            record_db_open();
        }

        // Only time DB access when instrumentation is enabled
        if config::is_timing_enabled() {
            let db_start = std::time::Instant::now();
            let result = f(opt.as_ref().unwrap());
            record_db_read(db_start.elapsed().as_micros() as u64);
            Ok(result)
        } else {
            Ok(f(opt.as_ref().unwrap()))
        }
    })
}

// ============================================================================
// Signal Emission Helpers (with freshness checks)
// ============================================================================

/// Ensure a file signal exists, but only queue the write if it doesn't already exist.
///
/// Uses the read-only DB to check freshness before queueing to the write thread.
/// This dramatically reduces redundant writes during re-computation.
fn ensure_file_signal_if_missing(
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
fn clear_file_signal_if_present(
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

/// Execute a single computation.
///
/// Uses thread-local cached read-only connection for queries.
/// Writes go through the DB thread via SignalWriteSender.
/// Creates a `ComputationWitness` for signal-altering operations.
pub fn execute_single(computation: &Computation) -> ComputationResult {
    use crate::config;
    use crate::corpus::mutations::indexing;

    // Ensure this thread has an ID assigned for stats tracking
    let _thread_id = ensure_thread_id();

    let start = std::time::Instant::now();

    // Create witness for signal operations - only valid within this execution context
    let witness = ComputationWitness::new();

    // Use thread-local cached connection - first access per thread opens, subsequent reuses
    let db_access_start = std::time::Instant::now();

    let result = with_thread_db(|db| {
        let db_access_ms = db_access_start.elapsed().as_millis();

        let compute_result = match computation {
            Computation::VerifyTags { track_id, path } => {
                // Delegate to the existing execute_verify_tags implementation
                match indexing::execute_verify_tags(db, *track_id, path) {
                    Ok(()) => ComputationResult::success(
                        computation.clone(),
                        start.elapsed().as_millis() as u64,
                        Vec::new(),
                    ),
                    Err(e) => ComputationResult::failure(
                        computation.clone(),
                        start.elapsed().as_millis() as u64,
                        e.to_string(),
                    ),
                }
            }

            Computation::WalkCorpus { root, source, paranoid } => {
                execute_walk_corpus(db, root, source, *paranoid, start)
            }

            Computation::ScanCorpusDirectory { directory, source, paranoid } => {
                execute_scan_corpus_directory(db, directory, source, *paranoid, &witness, start)
            }

            Computation::CompareInodes { source, disk_state, paranoid } => {
                execute_compare_inodes(db, source, disk_state, *paranoid, &witness, start)
            }

            Computation::VerifyMtime { track_id, path, expected_mtime_secs, expected_mtime_nanos } => {
                execute_verify_mtime(db, *track_id, path, *expected_mtime_secs, *expected_mtime_nanos, start)
            }

            Computation::ScheduleSecondLevelDerivations => {
                execute_schedule_second_level_derivations(db, start)
            }

            Computation::DeriveDirectorySignals { directory } => {
                execute_derive_directory_signals(db, directory, &witness, start)
            }

            Computation::CheckDeployConflicts { track_id } => {
                execute_check_deploy_conflicts(db, *track_id, start)
            }

            Computation::UpdateFileSignals { path } => {
                execute_update_file_signals(db, path, &witness, start)
            }

            Computation::WalkLibrary { library_root, library_name, corpus_path_prefixes } => {
                execute_walk_library(db, library_root, library_name, corpus_path_prefixes, start)
            }

            Computation::ScanLibraryDirectory { directory, library_name, library_root, corpus_path_prefixes } => {
                execute_scan_library_directory(db, directory, library_name, library_root, corpus_path_prefixes, start)
            }

            Computation::DeriveLibraryHealthSignals { library_name, library_root, corpus_path_prefixes, library_files } => {
                execute_derive_library_health_signals(db, library_name, library_root, corpus_path_prefixes, library_files, &witness, start)
            }

            // Content analysis computations
            Computation::ScheduleContentAnalysis => {
                execute_schedule_content_analysis(start)
            }

            Computation::DetectFingerprintDuplicates => {
                execute_detect_fingerprint_duplicates(db, &witness, start)
            }

            Computation::DetectDuplicateInodes => {
                execute_detect_duplicate_inodes(db, &witness, start)
            }

            Computation::DetectMissingTags => {
                execute_detect_missing_tags(db, &witness, start)
            }

            Computation::DetectMetadataDuplicates => {
                execute_detect_metadata_duplicates(db, &witness, start)
            }

            Computation::DetectTagCanonicalizations => {
                execute_detect_tag_canonicalizations(db, start)
            }

            Computation::VerifyOutOfBandChanges => {
                execute_verify_out_of_band_changes(db, &witness, start)
            }

            Computation::DetectDeployConflicts => {
                execute_detect_deploy_conflicts(db, &witness, start)
            }
        };

        // Log timing (only on first access when connection is opened)
        if db_access_ms > 1 {
            let _ = config::log_message(&format!(
                "[PERF] {} db_access={}ms (thread-local init)",
                computation.label(),
                db_access_ms
            ));
        }

        compute_result
    });

    // Handle db access failure and record stats
    let final_result = match result {
        Ok(r) => r,
        Err(e) => ComputationResult::failure(
            computation.clone(),
            start.elapsed().as_millis() as u64,
            format!("DB access failed: {}", e),
        ),
    };

    // Record task completion stats
    record_task_stats(computation.label(), final_result.duration_ms);

    final_result
}

// ============================================================================
// Eyeballing Computation Executors
// ============================================================================

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use crate::config::AUDIO_EXTENSIONS;
use crate::corpus::db::Database;

/// Check if path has audio file extension.
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Get signal sender or return early failure.
///
/// Helper to reduce duplication across computations that need signal sender access.
fn get_signal_sender_or_fail(
    computation: Computation,
    start: Instant,
) -> Result<db_thread::SignalWriteSender, ComputationResult> {
    match db_thread::signal_sender() {
        Some(s) => Ok(s.clone()),
        None => Err(ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            "DB thread not initialized".to_string(),
        )),
    }
}

/// Parse comma-separated track IDs into Vec<i64>.
///
/// Filters out invalid integers and whitespace.
fn parse_track_ids_csv(s: &str) -> Vec<i64> {
    s.split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// Extract modification time from metadata as (seconds, nanoseconds) tuple.
///
/// Returns (0, 0) if mtime extraction fails.
fn extract_mtime(metadata: &std::fs::Metadata) -> (i64, i64) {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0))
}

/// Enumerate all directories under root, including root itself.
///
/// Returns (directories, symlink_count). Symlinks are skipped.
fn enumerate_all_directories(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut symlink_count = 0;
    enumerate_directories_recursive(root, &mut directories, &mut symlink_count);

    // Include root itself (for files directly in root)
    directories.push(root.to_path_buf());

    (directories, symlink_count)
}

/// Phase 1: Enumerate ALL corpus directories and spawn per-directory scans.
///
/// Recursively walks the entire corpus tree to find all directories,
/// then spawns `ScanCorpusDirectory` for each. This provides granular
/// progress feedback during startup (one computation per directory).
fn execute_walk_corpus(
    _db: &Database,
    root: &Path,
    source: &str,
    paranoid: bool,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::WalkCorpus {
        root: root.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    if !root.exists() {
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Root directory does not exist: {:?}", root),
        );
    }

    // Recursively enumerate ALL directories
    let (directories, symlink_count) = enumerate_all_directories(root);

    if symlink_count > 0 {
        let _ = log_message(&format!(
            "[WARN] WalkCorpus: skipped {} directory symlinks in {:?}",
            symlink_count, root
        ));
    }

    let _ = log_message(&format!(
        "[COMPUTE] WalkCorpus: found {} directories in {:?}",
        directories.len(), root
    ));

    // Spawn ScanCorpusDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanCorpusDirectory {
            directory,
            source: source.to_string(),
            paranoid,
        })
        .collect();

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Recursively enumerate all directories under a root path.
///
/// Skips symlinks and logs a warning count.
fn enumerate_directories_recursive(dir: &Path, directories: &mut Vec<PathBuf>, symlink_count: &mut usize) {
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

/// Recursively walk directory and collect audio file state.
fn walk_dir_recursive(dir: &Path, disk_state: &mut Vec<(i64, PathBuf, i64, i64)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            walk_dir_recursive(&path, disk_state);
        } else if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let mtime = extract_mtime(&metadata);

                disk_state.push((inode, path, mtime.0, mtime.1));
            }
        }
    }
}

/// Scan a single corpus directory (non-recursive).
///
/// Processes only audio files directly in the given directory, compares against
/// scan_state, creates FileInCorpus signals (one per file), and spawns follow-up
/// computations.
fn execute_scan_corpus_directory(
    db: &Database,
    directory: &Path,
    source: &str,
    paranoid: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::ScanCorpusDirectory {
        directory: directory.to_path_buf(),
        source: source.to_string(),
        paranoid,
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    if !directory.exists() {
        return ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Directory does not exist: {:?}", directory),
        );
    }

    // Collect disk state for files DIRECTLY in this directory (not recursive)
    let disk_state = collect_directory_files(directory);

    // If no files in directory, nothing to do
    if disk_state.is_empty() {
        return ComputationResult::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    // Build lookup map for disk inodes
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();

    // Get indexed inodes from scan_state for comparison
    let inode_vec: Vec<i64> = disk_inodes.iter().copied().collect();
    let indexed_by_inode = db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

    let mut spawn: Vec<Computation> = Vec::new();

    // Process each file found on disk
    for (inode, path, disk_mtime_s, disk_mtime_ns) in &disk_state {
        let path_str = path.to_string_lossy().to_string();

        if let Some(entry) = indexed_by_inode.get(inode) {
            // File is indexed - check if we need to verify mtime or tags
            let track = match db.get_track_by_path(&path_str) {
                Ok(Some(t)) => t,
                _ => continue,
            };
            let Some(track_id) = track.id else { continue };

            if paranoid {
                // Paranoid mode: verify tags for ALL indexed files
                spawn.push(Computation::VerifyTags {
                    track_id,
                    path: path.clone(),
                });
            } else if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                // Non-paranoid: only verify if mtime mismatched
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
            }
        } else {
            // File not indexed - create a FileInCorpus signal for this file
            ensure_file_signal_if_missing(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
        }
    }

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Collect audio files directly in a directory (non-recursive).
fn collect_directory_files(dir: &Path) -> Vec<(i64, PathBuf, i64, i64)> {
    let mut disk_state = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return disk_state,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip directories and symlinks
        if path.is_dir() || path.is_symlink() {
            continue;
        }

        if is_audio_file(&path) {
            if let Ok(metadata) = std::fs::metadata(&path) {
                let inode = metadata.ino() as i64;
                let mtime = extract_mtime(&metadata);

                disk_state.push((inode, path, mtime.0, mtime.1));
            }
        }
    }

    disk_state
}

/// Phase 2: Compare disk state to database index.
fn execute_compare_inodes(
    db: &Database,
    source: &str,
    disk_state: &[(i64, PathBuf, i64, i64)],
    paranoid: bool,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: comparing {} disk files for source '{}'",
        disk_state.len(),
        source
    ));

    let computation = Computation::CompareInodes {
        source: source.to_string(),
        disk_state: disk_state.to_vec(),
        paranoid,
    };

    // Build disk inode set and lookup maps
    let disk_inodes: HashSet<i64> = disk_state.iter().map(|(inode, _, _, _)| *inode).collect();
    let disk_inode_to_state: HashMap<i64, (&PathBuf, i64, i64)> = disk_state
        .iter()
        .map(|(inode, path, mtime_s, mtime_ns)| (*inode, (path, *mtime_s, *mtime_ns)))
        .collect();

    // Get indexed inodes from scan_state
    let indexed_inodes = db.get_all_scan_state_inodes(source).unwrap_or_default();

    // Calculate differences
    let missing_from_disk: HashSet<i64> = indexed_inodes.difference(&disk_inodes).cloned().collect();
    let missing_from_index: HashSet<i64> = disk_inodes.difference(&indexed_inodes).cloned().collect();

    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: {} indexed, {} on disk, {} missing from disk, {} missing from index",
        indexed_inodes.len(),
        disk_inodes.len(),
        missing_from_disk.len(),
        missing_from_index.len()
    ));

    // Create MissingFromDisk signals
    if !missing_from_disk.is_empty() {
        if let Ok(missing_paths) = db.get_scan_state_paths_for_inodes(source, &missing_from_disk) {
            create_missing_from_disk_issues(db, source, &missing_paths, witness);
        }
    }

    // Create MissingFromIndex signals
    if !missing_from_index.is_empty() {
        let missing_paths: Vec<String> = missing_from_index
            .iter()
            .filter_map(|inode| disk_inode_to_state.get(inode))
            .map(|(path, _, _)| path.to_string_lossy().to_string())
            .collect();
        create_missing_from_index_issues(db, &missing_paths, witness);
    }

    // Spawn follow-up computations for mtime verification
    let mut spawn: Vec<Computation> = Vec::new();

    // Get scan_state entries for comparison (already returns HashMap<i64, ScanStateEntry>)
    let inode_vec: Vec<i64> = indexed_inodes.iter().copied().collect();
    let indexed_by_inode = db.get_scan_state_batch(source, &inode_vec).unwrap_or_default();

    for (inode, path, disk_mtime_s, disk_mtime_ns) in disk_state {
        // Skip files not in index (already reported as MissingFromIndex)
        let Some(entry) = indexed_by_inode.get(inode) else {
            continue;
        };

        // Get track_id for this path
        let track = match db.get_track_by_path(&path.to_string_lossy()) {
            Ok(Some(t)) => t,
            _ => continue,
        };
        let Some(track_id) = track.id else { continue };

        if paranoid {
            // Paranoid mode: verify tags for ALL indexed files
            spawn.push(Computation::VerifyTags {
                track_id,
                path: path.clone(),
            });
        } else {
            // Non-paranoid: only verify if mtime mismatched
            if entry.mtime_secs != *disk_mtime_s || entry.mtime_nanos != *disk_mtime_ns {
                spawn.push(Computation::VerifyMtime {
                    track_id,
                    path: path.clone(),
                    expected_mtime_secs: entry.mtime_secs,
                    expected_mtime_nanos: entry.mtime_nanos,
                });
            }
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] CompareInodes: spawning {} follow-up computations ({})",
        spawn.len(),
        if paranoid { "paranoid tag verify" } else { "mtime verify" }
    ));

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Create MissingFile signals for files in index but missing from disk.
///
/// Creates one signal per file with path as key (no metadata).
fn create_missing_from_disk_issues(
    db: &Database,
    _source: &str,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    // Create one signal per missing file (with freshness check)
    for path in missing_paths {
        ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, path, witness);
    }
}

/// Create UnindexedFile signals for files on disk but not in index.
///
/// Called from CompareInodes when it identifies files missing from the index.
fn create_missing_from_index_issues(
    db: &Database,
    missing_paths: &[String],
    witness: &ComputationWitness,
) {
    let Some(sender) = db_thread::signal_sender() else {
        return;
    };

    // File exists on disk but is not indexed = UnindexedFile (with freshness check)
    for path in missing_paths {
        ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, path, witness);
    }
}

/// Phase 3: Verify single file mtime.
fn execute_verify_mtime(
    _db: &Database,
    track_id: i64,
    path: &Path,
    expected_mtime_secs: i64,
    expected_mtime_nanos: i64,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::VerifyMtime {
        track_id,
        path: path.to_path_buf(),
        expected_mtime_secs,
        expected_mtime_nanos,
    };

    // Read current mtime from disk
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read file metadata: {}", e),
            );
        }
    };

    let (current_secs, current_nanos) = extract_mtime(&metadata);

    // If mtime differs from expected, spawn VerifyTags to check actual content
    let spawn = if current_secs != expected_mtime_secs || current_nanos != expected_mtime_nanos {
        vec![Computation::VerifyTags {
            track_id,
            path: path.to_path_buf(),
        }]
    } else {
        Vec::new()
    };

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get all configured library names from deploy mappings (inlined from removed library.rs)
fn get_configured_library_names(config: &crate::config::Config) -> Vec<String> {
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
// Second-Level Signal Computation Executors
// ============================================================================

/// Schedule second-level signal derivations by spawning per-directory computations.
fn execute_schedule_second_level_derivations(
    db: &Database,
    start: Instant,
) -> ComputationResult {
    let _ = log_message("[COMPUTE] ScheduleSecondLevelDerivations: starting");

    // Get all directories that have:
    // 1. FileInCorpus signals (corpus directories with audio files)
    // 2. Indexed tracks (may be missing from corpus now)
    let corpus_dirs = db.get_distinct_corpus_directories().unwrap_or_default();
    let _ = log_message(&format!(
        "[COMPUTE] Found {} directories with FileInCorpus signals",
        corpus_dirs.len()
    ));

    let index_dirs = db.get_distinct_track_directories().unwrap_or_default();
    let _ = log_message(&format!(
        "[COMPUTE] Found {} directories with indexed tracks",
        index_dirs.len()
    ));

    // Union of all directories that need second-level signal derivation
    let all_dirs: HashSet<PathBuf> = corpus_dirs.into_iter()
        .chain(index_dirs.into_iter())
        .collect();

    let _ = log_message(&format!(
        "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} directory computations",
        all_dirs.len()
    ));

    // Spawn a DeriveDirectorySignals computation for each directory
    let mut spawn: Vec<Computation> = all_dirs
        .into_iter()
        .map(|directory| Computation::DeriveDirectorySignals { directory })
        .collect();

    // Also spawn library health computations for each configured library
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        let _ = log_message(&format!(
            "[COMPUTE] ScheduleSecondLevelDerivations: spawning {} library walks",
            library_names.len()
        ));

        for library_name in library_names {
            let library_root = config.libraries_root.join(&library_name);
            let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
            spawn.push(Computation::WalkLibrary {
                library_root,
                library_name,
                corpus_path_prefixes,
            });
        }
    }

    ComputationResult::success(
        Computation::ScheduleSecondLevelDerivations,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Derive second-level signals for files in a single directory.
///
/// Computes the expected state for each file and ensures the correct signals exist
/// while clearing signals that no longer apply.
fn execute_derive_directory_signals(
    db: &Database,
    directory: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::db::types::HealthIssueType;

    let computation = Computation::DeriveDirectorySignals {
        directory: directory.to_path_buf(),
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get FileInCorpus signals for this directory
    let corpus_signals = match db.get_signals_in_directory(directory, HealthIssueType::FileInCorpus) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get FileInCorpus signals: {}", e),
            );
        }
    };

    // Get indexed tracks for this directory (regardless of fingerprint status)
    let dir_str = directory.to_string_lossy();
    let tracks = match db.get_tracks_by_corpus_path_prefix(&dir_str) {
        Ok(t) => t,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get tracks: {}", e),
            );
        }
    };

    // Build lookup sets
    let corpus_paths: HashSet<String> = corpus_signals
        .iter()
        .map(|s| s.issue_key.clone())
        .collect();

    let indexed_paths: HashMap<String, &crate::corpus::db::types::Track> = tracks
        .iter()
        .map(|t| (t.path.clone(), t))
        .collect();

    // Prune stale UnindexedFile signals: those without a matching FileInCorpus.
    // This handles files that were deleted/converted (e.g., WMA→FLAC) - the old
    // UnindexedFile signal should be removed since the file no longer exists.
    // NOTE: No freshness check needed here - we're iterating signals we know exist.
    if let Ok(existing_unindexed) =
        db.get_signals_in_directory(directory, HealthIssueType::UnindexedFile)
    {
        for signal in existing_unindexed {
            if !corpus_paths.contains(&signal.issue_key) {
                // No FileInCorpus for this path → file was deleted → prune stale signal
                // Direct call since we already know the signal exists from DB query
                sender.clear_file_signal(FileSignalType::UnindexedFile, &signal.issue_key, witness);
            }
        }
    }

    let spawn: Vec<Computation> = Vec::new();

    // Process files in corpus (FileInCorpus signals)
    for corpus_path in &corpus_paths {
        if indexed_paths.contains_key(corpus_path) {
            // File is in both corpus and index → clear UnindexedFile if it exists
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        } else {
            // File in corpus but not indexed → ensure UnindexedFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, corpus_path, witness);
        }
    }

    // Process indexed tracks
    for (path, _track) in &indexed_paths {
        if corpus_paths.contains(path) {
            // File exists in both corpus and index → healthy
            // Clear MissingFile signal if it existed
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, path, witness);

            // Ensure HealthyFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, path, witness);

            // NOTE: CheckDeployConflicts is now handled in bulk by DetectDeployConflicts
            // during ScheduleContentAnalysis, not per-file here.
        } else {
            // Track without FileInCorpus → file is missing from disk
            // Clear HealthyFile signal if it existed
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, path, witness);

            // Ensure MissingFile signal
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, path, witness);
        }
    }

    // Clean up stale signals for paths no longer tracked
    // UnindexedFile signals for paths that are now indexed (handled above)
    // HealthyFile signals for paths no longer in corpus or index need cleanup
    // This requires knowing all paths that WERE tracked - for now we handle the common cases above

    ComputationResult::success(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Check for deployment conflicts on a healthy track.
///
/// TODO: Integrate with existing deploy conflict detection logic in corpus/health/detection.rs.
/// For now, this is a no-op placeholder that allows the computation chain to complete.
fn execute_check_deploy_conflicts(
    _db: &Database,
    track_id: i64,
    start: Instant,
) -> ComputationResult {
    // Deploy conflict detection is handled by the existing library-level detection
    // in corpus/health/detection.rs. Per-track conflict checking would require
    // refactoring that logic to work at the track level.
    //
    // For now, this computation is a no-op - actual deploy conflicts are still
    // detected during the dedicated deploy conflict detection pass.
    let _ = log_message(&format!(
        "CheckDeployConflicts: skipping track {} (pending integration)",
        track_id
    ));

    ComputationResult::success(
        Computation::CheckDeployConflicts { track_id },
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
}

/// Update signals for a single file after a mutation.
///
/// This is a lightweight computation that:
/// 1. Checks if file exists on disk (FileInCorpus)
/// 2. Checks if file is indexed (track exists)
/// 3. Updates exactly the signals that apply to THIS file
///
/// Does NOT spawn CheckDeployConflicts - that is handled in bulk by
/// DetectDeployConflicts during ScheduleContentAnalysis.
fn execute_update_file_signals(
    db: &Database,
    path: &Path,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::UpdateFileSignals {
        path: path.to_path_buf(),
    };

    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    let path_str = path.to_string_lossy().to_string();
    let file_exists = path.exists() && is_audio_file(path);
    let is_indexed = db.get_track_by_path(&path_str).ok().flatten().is_some();

    if file_exists {
        // File exists on disk
        ensure_file_signal_if_missing(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);

        if is_indexed {
            // Healthy: exists + indexed
            clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
        } else {
            // Unindexed: exists but not indexed
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);
        }
    } else {
        // File does not exist on disk
        clear_file_signal_if_present(db, &sender, FileSignalType::FileInCorpus, &path_str, witness);
        clear_file_signal_if_present(db, &sender, FileSignalType::UnindexedFile, &path_str, witness);

        if is_indexed {
            // Missing: indexed but not on disk
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            ensure_file_signal_if_missing(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        } else {
            // Gone: not indexed, not on disk - clear all
            clear_file_signal_if_present(db, &sender, FileSignalType::HealthyFile, &path_str, witness);
            clear_file_signal_if_present(db, &sender, FileSignalType::MissingFile, &path_str, witness);
        }
    }

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Library Health Computation Executors
// ============================================================================

/// Walk a library directory tree and spawn per-directory scans.
fn execute_walk_library(
    _db: &Database,
    library_root: &Path,
    library_name: &str,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> ComputationResult {
    let computation = Computation::WalkLibrary {
        library_root: library_root.to_path_buf(),
        library_name: library_name.to_string(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    if !library_root.exists() {
        let _ = log_message(&format!(
            "[COMPUTE] WalkLibrary: library root does not exist: {:?}",
            library_root
        ));
        return ComputationResult::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(), // No directories to scan
        );
    }

    // Enumerate all directories recursively
    let (directories, _symlink_count) = enumerate_all_directories(library_root);

    let _ = log_message(&format!(
        "[COMPUTE] WalkLibrary '{}': found {} directories in {:?}",
        library_name,
        directories.len(),
        library_root
    ));

    // Spawn ScanLibraryDirectory for each directory
    let spawn: Vec<Computation> = directories
        .into_iter()
        .map(|directory| Computation::ScanLibraryDirectory {
            directory,
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            corpus_path_prefixes: corpus_path_prefixes.to_vec(),
        })
        .collect();

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Scan a single library directory and collect (path, inode) pairs.
///
/// Since we need to aggregate files before deriving signals, this stores
/// files in a temporary table or accumulates them in memory. For simplicity,
/// we'll collect files and spawn DeriveLibraryHealthSignals when the last
/// directory scan completes.
///
/// Note: This design accumulates files across multiple ScanLibraryDirectory
/// calls by tracking completion in a separate mechanism. For now, each
/// scan immediately spawns a DeriveLibraryHealthSignals with its files.
/// A more efficient implementation would batch directories.
fn execute_scan_library_directory(
    _db: &Database,
    directory: &Path,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    start: Instant,
) -> ComputationResult {
    let computation = Computation::ScanLibraryDirectory {
        directory: directory.to_path_buf(),
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    // Collect audio files in this directory (non-recursive, only immediate children)
    let mut library_files: Vec<(PathBuf, i64)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_audio_file(&path) {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    library_files.push((path, metadata.ino() as i64));
                }
            }
        }
    }

    let _file_count = library_files.len();

    // Spawn DeriveLibraryHealthSignals to process these files
    // Note: Each directory spawns its own derivation. A future optimization
    // could batch all directories and run one final derivation.
    let spawn = if library_files.is_empty() {
        Vec::new()
    } else {
        vec![Computation::DeriveLibraryHealthSignals {
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            corpus_path_prefixes: corpus_path_prefixes.to_vec(),
            library_files,
        }]
    };

    // Verbose logging disabled - too noisy for directory-level computations
    // let _ = log_message(&format!(
    //     "[COMPUTE] ScanLibraryDirectory '{}': found {} audio files in {:?}",
    //     library_name, file_count, directory
    // ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Derive library health signals by comparing library inodes against corpus.
fn execute_derive_library_health_signals(
    db: &Database,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[PathBuf],
    library_files: &[(PathBuf, i64)],
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::deploy::compute_deployment_path_with_tags;

    let computation = Computation::DeriveLibraryHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
        library_files: library_files.to_vec(),
    };

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get all corpus track inodes (inode -> corpus_path)
    let corpus_inodes = db.get_all_track_inodes().unwrap_or_default();

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut orphan_count: usize = 0;

    // Check each library file against corpus
    for (library_path, library_inode) in library_files {
        let orphan_key = format!(
            "library_orphan:{}:{}",
            library_name,
            library_path.display()
        );
        let stale_key = format!(
            "library_stale:{}:{}",
            library_name,
            library_path.display()
        );

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            // Inode match found - this file is deployed from corpus
            // Clear any orphan signal that might have existed
            clear_file_signal_if_present(db, &sender, FileSignalType::LibraryOrphan, &orphan_key, witness);

            // Check if deployed at correct path (stale detection)
            let is_stale = if let Ok(Some(track)) = db.get_track_by_path(corpus_path) {
                if let Some(track_id) = track.id {
                    // Get tags and compute expected deployment path
                    let tags = db.get_track_tags(track_id).unwrap_or_default();
                    let tag_map: std::collections::HashMap<String, String> = tags
                        .into_iter()
                        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                        .collect();

                    let expected_relative = compute_deployment_path_with_tags(&track, &tag_map);
                    let expected_path = library_root.join(&expected_relative);

                    // Compare paths (normalize for comparison)
                    library_path != &expected_path
                } else {
                    false
                }
            } else {
                false
            };

            if is_stale {
                stale_count += 1;
                ensure_file_signal_if_missing(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
            } else {
                healthy_count += 1;
                clear_file_signal_if_present(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
            }
        } else {
            // No corpus match - this is an orphan
            orphan_count += 1;
            ensure_file_signal_if_missing(db, &sender, FileSignalType::LibraryOrphan, &orphan_key, witness);
            // Clear any stale signal (orphans aren't stale, they're orphans)
            clear_file_signal_if_present(db, &sender, FileSignalType::LibraryStale, &stale_key, witness);
        }
    }

    // Library health is now just per-file signals (LibraryOrphan, LibraryStale).
    // Aggregate counts are computed at UI time via queries, not stored as signals.
    // This avoids the N×M explosion when running per-directory computations.

    let _ = log_message(&format!(
        "[COMPUTE] DeriveLibraryHealthSignals '{}': {} files, {} healthy, {} stale, {} orphan",
        library_name,
        library_files.len(),
        healthy_count,
        stale_count,
        orphan_count,
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Content Analysis Computations
// ============================================================================

/// Execute ScheduleContentAnalysis - spawns all content detection computations in parallel.
fn execute_schedule_content_analysis(
    start: std::time::Instant,
) -> ComputationResult {
    let _ = log_message("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    let spawn = vec![
        Computation::DetectFingerprintDuplicates,
        Computation::DetectDuplicateInodes,
        Computation::DetectMissingTags,
        Computation::DetectMetadataDuplicates,
        Computation::DetectTagCanonicalizations,
        Computation::VerifyOutOfBandChanges,
        Computation::DetectDeployConflicts,
    ];

    ComputationResult::success(
        Computation::ScheduleContentAnalysis,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

/// Execute DetectFingerprintDuplicates - bulk detection of fingerprint duplicates.
fn execute_detect_fingerprint_duplicates(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use rusqlite::params;

    let computation = Computation::DetectFingerprintDuplicates;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear stale FingerprintDuplicate signals
    // (fingerprints that no longer have duplicates)
    let clear_result = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'fingerprint_dup'
         AND issue_key NOT IN (
             SELECT fingerprint FROM tracks
             WHERE fingerprint IS NOT NULL
             GROUP BY fingerprint HAVING COUNT(*) > 1
         )",
        params![],
    );
    if let Err(e) = clear_result {
        let _ = log_message(&format!(
            "[COMPUTE] DetectFingerprintDuplicates: error clearing stale signals: {}",
            e
        ));
    }

    // Find all fingerprints with duplicates, including track IDs
    let query = "SELECT fingerprint, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 WHERE fingerprint IS NOT NULL
                 GROUP BY fingerprint
                 HAVING COUNT(*) > 1";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let rows = match stmt.query_map(params![], |row| {
        let fingerprint: String = row.get(0)?;
        let track_ids_str: String = row.get(1)?;
        Ok((fingerprint, track_ids_str))
    }) {
        Ok(r) => r,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    let mut total_groups = 0;
    let mut total_tracks = 0;

    for row_result in rows {
        let (fingerprint, track_ids_str) = match row_result {
            Ok(r) => r,
            Err(e) => {
                let _ = log_message(&format!(
                    "[COMPUTE] DetectFingerprintDuplicates: row error: {}",
                    e
                ));
                continue;
            }
        };

        // Parse track IDs from comma-separated string
        let track_ids = parse_track_ids_csv(&track_ids_str);

        total_groups += 1;
        total_tracks += track_ids.len();

        // Create/replace signal with embedded track IDs
        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::FingerprintDuplicate,
            key: fingerprint.clone(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "fingerprint": fingerprint,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectFingerprintDuplicates: {} groups, {} tracks total",
        total_groups, total_tracks
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute DetectDuplicateInodes - bulk detection of duplicate inodes.
fn execute_detect_duplicate_inodes(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use rusqlite::params;

    let computation = Computation::DetectDuplicateInodes;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear stale DuplicateInode signals
    let clear_result = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'duplicate_inode'
         AND issue_key NOT IN (
             SELECT CAST(inode AS TEXT) FROM tracks
             GROUP BY inode HAVING COUNT(*) > 1
         )",
        params![],
    );
    if let Err(e) = clear_result {
        let _ = log_message(&format!(
            "[COMPUTE] DetectDuplicateInodes: error clearing stale signals: {}",
            e
        ));
    }

    // Find all inodes with duplicates, including track IDs
    let query = "SELECT inode, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 GROUP BY inode
                 HAVING COUNT(*) > 1";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let rows = match stmt.query_map(params![], |row| {
        let inode: i64 = row.get(0)?;
        let track_ids_str: String = row.get(1)?;
        Ok((inode, track_ids_str))
    }) {
        Ok(r) => r,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    let mut total_groups = 0;

    for row_result in rows {
        let (inode, track_ids_str) = match row_result {
            Ok(r) => r,
            Err(e) => {
                let _ = log_message(&format!(
                    "[COMPUTE] DetectDuplicateInodes: row error: {}",
                    e
                ));
                continue;
            }
        };

        // Parse track IDs from comma-separated string
        let track_ids = parse_track_ids_csv(&track_ids_str);

        total_groups += 1;

        // Create/replace signal with embedded track IDs
        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::DuplicateInode,
            key: inode.to_string(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "inode": inode,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectDuplicateInodes: {} duplicate inode groups",
        total_groups
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute DetectMissingTags - detect tracks missing required tags.
///
/// Groups tracks by album (or directory if no album tag), listing which tags
/// are missing in the metadata. Key format: `album=AlbumName` or `dir=/path`.
fn execute_detect_missing_tags(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use rusqlite::params;
    use std::collections::{HashMap, HashSet};

    let computation = Computation::DetectMissingTags;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Load required tags from config
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    let required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    // Clear all existing MissingTag signals (we'll rebuild)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'missing_tag'",
        params![],
    );

    // Get all tracks with their tags and paths
    // We need: track_id, path, album (if present), all tag names present
    let query = "
        SELECT t.id, t.path,
               (SELECT tag_value FROM track_tags WHERE track_id = t.id AND LOWER(tag_name) = 'album' LIMIT 1) as album,
               GROUP_CONCAT(LOWER(tt.tag_name), ',') as present_tags
        FROM tracks t
        LEFT JOIN track_tags tt ON t.id = tt.track_id
        GROUP BY t.id
    ";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    // Group by album (or directory), accumulating which tags are missing
    // Key: album=value or dir=/path
    // Value: (missing_tags set, track_ids)
    let mut groups: HashMap<String, (HashSet<String>, Vec<i64>)> = HashMap::new();

    let rows = match stmt.query_map(params![], |row| {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let album: Option<String> = row.get(2)?;
        let present_tags: Option<String> = row.get(3)?;
        Ok((track_id, path, album, present_tags))
    }) {
        Ok(r) => r,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    for row_result in rows {
        let (track_id, path, album, present_tags_str) = match row_result {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Parse present tags
        let present_tags: HashSet<String> = present_tags_str
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        // Find missing required tags
        let missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

        if missing.is_empty() {
            continue;
        }

        // Determine grouping key: album=value or dir=/path
        let key = if let Some(album_name) = album {
            format!("album={}", album_name)
        } else {
            // Use parent directory as fallback
            let parent = std::path::Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            format!("dir={}", parent)
        };

        let entry = groups.entry(key).or_insert_with(|| (HashSet::new(), Vec::new()));
        entry.0.extend(missing);
        entry.1.push(track_id);
    }

    // Create signals for each group
    let mut total_groups = 0;

    for (key, (missing_tags, track_ids)) in groups {
        total_groups += 1;

        let mut missing_list: Vec<String> = missing_tags.into_iter().collect();
        missing_list.sort();

        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::MissingTag,
            key: key.clone(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "missing_tags": missing_list,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectMissingTags: {} groups with missing tags",
        total_groups
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute DetectMetadataDuplicates - detect exact metadata duplicates.
///
/// Groups tracks by their FULL tag signature (all tags, sorted by name).
/// Two tracks are duplicates if ALL their tags match exactly.
fn execute_detect_metadata_duplicates(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use rusqlite::params;
    use std::collections::HashMap;

    let computation = Computation::DetectMetadataDuplicates;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear all MetadataDuplicate signals (we'll rebuild)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'metadata_dup'",
        params![],
    );

    // Build tag signature for each track
    // GROUP_CONCAT with ORDER BY ensures consistent ordering
    let _query = "
        SELECT t.id,
               GROUP_CONCAT(LOWER(tt.tag_name) || '=' || tt.tag_value, '|')
               OVER (PARTITION BY t.id ORDER BY LOWER(tt.tag_name)) as tag_sig
        FROM tracks t
        JOIN track_tags tt ON t.id = tt.track_id
    ";

    // Actually, window functions with GROUP_CONCAT are tricky in SQLite.
    // Let's do this more simply: get all tracks with their tags, build signatures in code.
    let query = "
        SELECT t.id, tt.tag_name, tt.tag_value
        FROM tracks t
        JOIN track_tags tt ON t.id = tt.track_id
        ORDER BY t.id, LOWER(tt.tag_name)
    ";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    // Build track_id -> sorted tag signature map
    let mut track_tags: HashMap<i64, Vec<(String, String)>> = HashMap::new();

    let rows = match stmt.query_map(params![], |row| {
        let track_id: i64 = row.get(0)?;
        let tag_name: String = row.get(1)?;
        let tag_value: String = row.get(2)?;
        Ok((track_id, tag_name, tag_value))
    }) {
        Ok(r) => r,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    for row_result in rows {
        let (track_id, tag_name, tag_value) = match row_result {
            Ok(r) => r,
            Err(_) => continue,
        };
        track_tags
            .entry(track_id)
            .or_default()
            .push((tag_name.to_lowercase(), tag_value));
    }

    // Build signature -> track_ids map
    let mut sig_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for (track_id, mut tags) in track_tags {
        // Sort by tag name for consistent signature
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        let signature: String = tags
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("|");

        sig_to_tracks
            .entry(signature)
            .or_default()
            .push(track_id);
    }

    // Create signals for duplicates
    let mut total_groups = 0;

    for (signature, track_ids) in sig_to_tracks {
        if track_ids.len() < 2 {
            continue;
        }

        total_groups += 1;

        // Create/replace signal with embedded track IDs
        // The key is a hash of the signature to keep it manageable
        // (full signatures can be very long)
        let key_hash = format!("{:x}", md5_hash(&signature));

        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::MetadataDuplicate,
            key: key_hash,
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "tag_signature": signature,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectMetadataDuplicates: {} duplicate metadata groups",
        total_groups
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Simple hash function for signature strings.
fn md5_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Execute DetectTagCanonicalizations - detect tag canonicalization opportunities.
fn execute_detect_tag_canonicalizations(
    db: &Database,
    start: std::time::Instant,
) -> ComputationResult {
    use crate::corpus::health::canonicalization::detect_and_store_canonicalizations;

    let computation = Computation::DetectTagCanonicalizations;

    match detect_and_store_canonicalizations(db) {
        Ok(count) => {
            let _ = log_message(&format!(
                "[COMPUTE] DetectTagCanonicalizations: {} new canonicalization entries",
                count
            ));
            ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
        Err(e) => ComputationResult::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Failed to detect canonicalizations: {}", e),
        ),
    }
}

/// Execute VerifyOutOfBandChanges - verify files with modified mtime.
fn execute_verify_out_of_band_changes(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use crate::corpus::db::HealthIssueType;
    use crate::corpus::mutations::indexing::execute_verify_tags;
    use rusqlite::params;

    let computation = Computation::VerifyOutOfBandChanges;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get all CorpusFileModifiedOutOfBand signals
    let oob_signals = match db.get_health_signals(Some(HealthIssueType::CorpusFileModifiedOutOfBand)) {
        Ok(signals) => signals,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get OOB signals: {}", e),
            );
        }
    };

    let mut verified_count = 0;
    let mut tag_change_count = 0;
    let mut mtime_only_count = 0;

    for signal in oob_signals {
        // The issue_key is the file path
        let path = &signal.issue_key;

        // Find the track for this path
        let track = match db.get_track_by_path(path) {
            Ok(Some(t)) => t,
            Ok(None) => {
                // Track no longer exists - clear the signal
                sender.clear_file_signal(FileSignalType::CorpusFileModifiedOutOfBand, path, witness);
                continue;
            }
            Err(_) => continue,
        };

        let track_id = match track.id {
            Some(id) => id,
            None => continue,
        };

        // Run tag verification
        if let Err(e) = execute_verify_tags(db, track_id, std::path::Path::new(path)) {
            let _ = log_message(&format!(
                "[COMPUTE] VerifyOutOfBandChanges: error verifying {}: {}",
                path, e
            ));
            continue;
        }

        verified_count += 1;

        // Check if there are any tag mismatches for this track
        let mismatch_count: i64 = db.conn
            .query_row(
                "SELECT COUNT(*) FROM tag_mismatches WHERE track_id = ?1",
                params![track_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if mismatch_count > 0 {
            // Tags differ - create OutOfBandTagChange signal
            tag_change_count += 1;
            ensure_file_signal_if_missing(db, &sender, FileSignalType::OutOfBandTagChange, path, witness);
        } else {
            // Tags match but mtime changed - file was touched but unchanged
            mtime_only_count += 1;
            // Clear the CorpusFileModifiedOutOfBand signal
            sender.clear_file_signal(FileSignalType::CorpusFileModifiedOutOfBand, path, witness);

            // Update scan_state to current mtime to avoid future toil
            // (This is a minor mutation but acceptable in computation context for bookkeeping)
            let file_path = std::path::Path::new(path);
            if let Ok(metadata) = std::fs::metadata(file_path) {
                use std::os::unix::fs::MetadataExt;
                let mtime_secs = metadata.mtime();
                let mtime_nanos = metadata.mtime_nsec() as i64;

                let _ = db.conn.execute(
                    "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
                    params![mtime_secs, mtime_nanos, path],
                );
            }
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] VerifyOutOfBandChanges: verified {} files, {} tag changes, {} mtime-only",
        verified_count, tag_change_count, mtime_only_count
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
///
/// Groups healthy tracks by their computed deployment path. Tracks that would
/// deploy to the same path are marked as DeployConflict signals.
fn execute_detect_deploy_conflicts(
    db: &Database,
    witness: &ComputationWitness,
    start: std::time::Instant,
) -> ComputationResult {
    use crate::corpus::db::types::HealthIssueType;
    use crate::corpus::deploy::compute_deployment_path_with_tags;
    use std::collections::HashMap;

    let computation = Computation::DetectDeployConflicts;

    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Clear all existing DeployConflict signals (we rebuild from scratch)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'deploy_conflict'",
        rusqlite::params![],
    );

    // Get all HealthyFile signals
    let healthy_signals = db
        .get_health_signals(Some(HealthIssueType::HealthyFile))
        .unwrap_or_default();

    // Compute deployment paths and group by path
    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let path = &signal.issue_key;
        if let Ok(Some(track)) = db.get_track_by_path(path) {
            if let Some(track_id) = track.id {
                let tags = db.get_track_tags(track_id).unwrap_or_default();
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();

                let deploy_path = compute_deployment_path_with_tags(&track, &tag_map)
                    .to_string_lossy()
                    .to_string();

                deploy_path_to_tracks
                    .entry(deploy_path)
                    .or_default()
                    .push(track_id);
            }
        }
    }

    // Create signals for conflicts (paths with multiple tracks)
    let mut conflict_count = 0;
    for (deploy_path, track_ids) in deploy_path_to_tracks {
        if track_ids.len() > 1 {
            conflict_count += 1;
            let signal = AggregateSignal {
                id: None,
                signal_type: AggregateSignalType::DeployConflict,
                key: deploy_path.clone(),
                discovered_at: None,
                metadata_json: Some(
                    serde_json::json!({
                        "deploy_path": deploy_path,
                    })
                    .to_string(),
                ),
            }
            .with_track_ids(&track_ids);

            sender.replace_aggregate_signal(signal, witness);
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files",
        conflict_count,
        healthy_signals.len()
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
