//! Computation System - Phase-stratified background processing
//!
//! See [`docs/COMPUTATION_REFERENCE.md`](../../../docs/COMPUTATION_REFERENCE.md) for the
//! canonical reference of computation phases, signal matrices, and spawn relationships.
//!
//! **Any changes to computation behavior must be reflected in that document.**
//!
//! ## Overview
//!
//! Computations are derived facts computed from corpus/index state. Unlike Mutations,
//! they do not alter state - they only emit signals. This distinction is important
//! because:
//!
//! - **Mutations** require a `DecisionWitness` to queue (user-led decision context)
//! - **Computations** can be queued without a witness (config-driven, automatic)
//!
//! ## Phase Stratification
//!
//! Computations are organized into three phases with compile-time enforced boundaries:
//!
//! - **Asleep** (`asleep/`) - Corpus observation (WalkCorpus, ScanCorpusDirectory, etc.)
//! - **Awakening** (`awakening/`) - First-level derivations (DeriveDirectorySignals, etc.)
//! - **Awake** (`awake/`) - Full-corpus analysis (DetectFingerprintDuplicates, etc.)
//!
//! Each phase has its own `Computation` enum and `Result` type. The `Result::spawn`
//! field can ONLY contain computations from the same phase - this is enforced at
//! compile time, preventing cross-phase spawning errors.
//!
//! ## Module Organization
//!
//! - `types.rs` - ComputationWitness (shared across phases)
//! - `stats.rs` - Thread-local performance tracking
//! - `helpers.rs` - Shared utility functions
//! - `asleep/` - Asleep phase computations
//! - `awakening/` - Awakening phase computations
//! - `awake/` - Awake phase computations

// Module declarations
mod types;
mod stats;
mod helpers;
pub mod asleep;
pub mod awakening;
pub mod awake;

// Public re-exports
pub use types::ComputationWitness;
pub use stats::{close_thread_local_connection, get_thread_stats, with_read_only_db, ThreadStats};

// Internal imports for execute functions
use crate::config;
use stats::{ensure_thread_id, record_task_stats};

// ============================================================================
// Unified Computation Enum (for daemon's queue)
// ============================================================================

/// Unified computation enum that wraps phase-specific computations.
///
/// This is used by the daemon for queuing - the daemon doesn't care about
/// phase boundaries, it just executes computations. The phase boundaries
/// are enforced at the point where computations SPAWN other computations.
#[derive(Debug, Clone)]
pub enum Computation {
    Asleep(asleep::Computation),
    Awakening(awakening::Computation),
    Awake(awake::Computation),
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::Asleep(c) => c.label(),
            Computation::Awakening(c) => c.label(),
            Computation::Awake(c) => c.label(),
        }
    }
}

// ============================================================================
// Unified Computation Result (for daemon's result handling)
// ============================================================================

/// Unified result that wraps phase-specific results.
///
/// The `spawn_*` fields contain phase-specific spawned computations.
/// The daemon converts these to the unified `Computation` type when queuing.
#[derive(Debug)]
pub struct ComputationResult {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Asleep computations to spawn
    pub spawn_asleep: Vec<asleep::Computation>,
    /// Awakening computations to spawn
    pub spawn_awakening: Vec<awakening::Computation>,
    /// Awake computations to spawn
    pub spawn_awake: Vec<awake::Computation>,
}

impl ComputationResult {
    fn from_asleep(result: asleep::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_asleep: result.spawn,
            spawn_awakening: Vec::new(),
            spawn_awake: Vec::new(),
        }
    }

    fn from_awakening(result: awakening::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_asleep: Vec::new(),
            spawn_awakening: result.spawn,
            spawn_awake: Vec::new(),
        }
    }

    fn from_awake(result: awake::Result) -> Self {
        Self {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            spawn_asleep: Vec::new(),
            spawn_awakening: Vec::new(),
            spawn_awake: result.spawn,
        }
    }

    /// Get all spawned computations as unified Computation enums.
    pub fn all_spawned(&self) -> Vec<Computation> {
        let mut result = Vec::new();
        for c in &self.spawn_asleep {
            result.push(Computation::Asleep(c.clone()));
        }
        for c in &self.spawn_awakening {
            result.push(Computation::Awakening(c.clone()));
        }
        for c in &self.spawn_awake {
            result.push(Computation::Awake(c.clone()));
        }
        result
    }
}

// ============================================================================
// Execution
// ============================================================================

/// Execute a single computation.
///
/// Dispatches to the appropriate phase-specific executor based on the
/// computation variant. Uses thread-local cached read-only connection.
pub fn execute_single(computation: &Computation) -> ComputationResult {
    // Ensure this thread has an ID assigned for stats tracking
    let _thread_id = ensure_thread_id();

    let start = std::time::Instant::now();

    // Create witness for signal operations
    let witness = ComputationWitness::new();

    // Use thread-local cached READ-ONLY connection
    // IMPORTANT: All writes must go through db_thread::signal_sender()
    let db_access_start = std::time::Instant::now();

    let result = with_read_only_db(|read_only_db| {
        let db_access_ms = db_access_start.elapsed().as_millis();

        let compute_result = match computation {
            // Asleep phase computations
            Computation::Asleep(c) => {
                let result = match c {
                    asleep::Computation::ClearExistingObservationState => {
                        asleep::execute_clear_existing_observation_state(read_only_db, &witness, start)
                    }
                    asleep::Computation::WalkCorpus { root, source, force_check } => {
                        asleep::execute_walk_corpus(read_only_db, root, source, *force_check, start)
                    }
                    asleep::Computation::ScanCorpusDirectory { directory, source, force_check } => {
                        asleep::execute_scan_corpus_directory(read_only_db, directory, source, *force_check, &witness, start)
                    }
                    asleep::Computation::VerifyMtime { inode, path, expected_mtime_secs, expected_mtime_nanos } => {
                        asleep::execute_verify_mtime(read_only_db, *inode, path, *expected_mtime_secs, *expected_mtime_nanos, start)
                    }
                    asleep::Computation::VerifyTags { inode, path } => {
                        asleep::execute_verify_tags(read_only_db, *inode, path, &witness, start)
                    }
                    asleep::Computation::VerifyAudio { inode, path } => {
                        asleep::execute_verify_audio(read_only_db, *inode, path, &witness, start)
                    }
                };
                ComputationResult::from_asleep(result)
            }

            // Awakening phase computations
            Computation::Awakening(c) => {
                let result = match c {
                    awakening::Computation::ScheduleSecondLevelDerivations => {
                        awakening::execute_schedule_second_level_derivations(read_only_db, &witness, start)
                    }
                    awakening::Computation::DeriveCorpusSignals => {
                        awakening::execute_derive_corpus_signals(read_only_db, &witness, start)
                    }
                    awakening::Computation::UpdateCorpusFileSignals { path } => {
                        awakening::execute_update_corpus_file_signals(read_only_db, path, &witness, start)
                    }
                    awakening::Computation::UpdateLibraryFileSignals { path } => {
                        awakening::execute_update_library_file_signals(read_only_db, path, &witness, start)
                    }
                    awakening::Computation::WalkLibrary { library_root, library_name, corpus_path_prefixes } => {
                        awakening::execute_walk_library(read_only_db, library_root, library_name, corpus_path_prefixes, &witness, start)
                    }
                    awakening::Computation::ScanLibraryDirectory { directory, library_name, library_root, corpus_path_prefixes } => {
                        awakening::execute_scan_library_directory(read_only_db, directory, library_name, library_root, corpus_path_prefixes, &witness, start)
                    }
                    awakening::Computation::UpdateDeploySignals { corpus_path, library_path } => {
                        awakening::execute_update_deploy_signals(read_only_db, corpus_path, library_path, &witness, start)
                    }
                };
                ComputationResult::from_awakening(result)
            }

            // Awake phase computations
            Computation::Awake(c) => {
                let result = match c {
                    awake::Computation::ScheduleContentAnalysis => {
                        awake::execute_schedule_content_analysis(read_only_db, start)
                    }
                    awake::Computation::DetectFingerprintOverlaps => {
                        awake::execute_detect_fingerprint_overlaps(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectDuplicateInodes => {
                        awake::execute_detect_duplicate_inodes(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectMissingTags => {
                        awake::execute_detect_missing_tags(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectMetadataDuplicates => {
                        awake::execute_detect_metadata_duplicates(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectTagCanonicalizations => {
                        awake::execute_detect_tag_canonicalizations(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectInconsistentAlbumArtist => {
                        awake::execute_detect_inconsistent_album_artist(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectCompoundTagValues => {
                        awake::execute_detect_compound_tag_values(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectCompoundTagsForInode { inode } => {
                        awake::execute_detect_compound_tags_for_inode(read_only_db, *inode, &witness, start)
                    }
                    awake::Computation::DetectShitFormats => {
                        awake::execute_detect_shit_formats(read_only_db, &witness, start)
                    }
                    awake::Computation::AnalyzeFingerprintOverlaps => {
                        awake::execute_analyze_fingerprint_overlaps(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectCrossSourceOverlaps => {
                        awake::execute_detect_cross_source_overlaps(read_only_db, &witness, start)
                    }
                    awake::Computation::DetectDeployConflicts => {
                        awake::execute_detect_deploy_conflicts(read_only_db, &witness, start)
                    }
                    awake::Computation::DeriveDeployHealthSignals { library_name, library_root, corpus_path_prefixes } => {
                        awake::execute_derive_deploy_health_signals(read_only_db, library_name, library_root, corpus_path_prefixes, &witness, start)
                    }
                    awake::Computation::DeriveCorpusDeployStatus => {
                        awake::execute_derive_corpus_deploy_status(read_only_db, &witness, start)
                    }
                };
                ComputationResult::from_awake(result)
            }
        };

        // Log timing (only on first access when connection is opened, and only if timing instrumentation enabled)
        if db_access_ms > 1 && config::is_timing_enabled() {
            crate::logging::log_perf(format!(
                "[PERF] {} db_access={}ms (thread-local init)",
                computation.label(),
                db_access_ms
            ));
        }

        compute_result
    });

    // Handle db access failure
    let final_result = match result {
        Ok(r) => r,
        Err(e) => ComputationResult {
            success: false,
            error: Some(format!("DB access failed: {}", e)),
            duration_ms: start.elapsed().as_millis() as u64,
            spawn_asleep: Vec::new(),
            spawn_awakening: Vec::new(),
            spawn_awake: Vec::new(),
        },
    };

    // Record task completion stats
    record_task_stats(computation.label(), final_result.duration_ms);

    final_result
}
