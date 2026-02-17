//! The Witch - Background task execution and orderliness enforcement
//!
//! The Witch is named such because She enforces orderliness in her domain,
//! and provides all guarantees for data which flows properly through Her.
//!
//! ## Module Organization
//! - `types.rs` - Core types, enums, witness system
//! - `worker_stats.rs` - Thread-safe performance statistics
//! - `transaction.rs` - Transaction lifecycle management
//! - `execution.rs` - Task execution (mutations, computations, migrations)
//!
//! ## Extending the Witch
//! - New task types: Add variants to `Task` enum in `types.rs`
//! - New execution logic: Add to `execution.rs`
//! - Transaction features: Modify `transaction.rs`
//! - Performance tracking: Modify `worker_stats.rs`

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::mpsc::{self, Receiver, Sender};

use crate::config::{self, Config, SharedConfig};
use crate::meta::computations::{Computation, asleep, awakening, awake};
use crate::corpus::db::{Database, ReadOnlyDb};
use crate::meta::mutations::Mutation;
use crate::db_thread::{self, DbThreadHandle, DbThreadStats};

// Module declarations
mod execution;
pub mod messages;
mod transaction;
mod types;
mod ui_read_cache;
mod worker_stats;

// Re-export public types
pub use messages::InitialUiState;
pub use types::{
    CorpusObservationState, DaemonStatus, DecisionWitness,
    EyeState, Migration, MigrationWitness, MutationExecutionWitness,
    PendingTransaction, SpawnedMutation, Task, TaskExecutionState, TaskExecutionStateSnapshot,
    TaskLabel, WorkerStats,
};
// NOTE: confirm_startup_migration() has been removed - Witch now handles witness internally.
// NOTE: confirm_decision() is deliberately NOT exported.
// All decision authority flows through with_operator_decision() which should
// ONLY be called from ui/operator_decisions.rs. See types.rs for details.
pub use ui_read_cache::UiReadCache;

// Internal imports
use execution::execute_task;
use types::{ContentAnalysisWitness, TaskResult};
use worker_stats::SharedWorkerStats;

// ============================================================================
// Global Mount Violation Flag
// ============================================================================

use std::sync::OnceLock;

/// Global flag for mount boundary violations detected by worker threads.
///
/// When a computation detects that a path crosses a filesystem mount boundary
/// (different st_dev than expected), it sets this flag. The Witch checks this
/// on each tick() and latches into read-only mode if set.
///
/// This is a OnceLock because once a violation is detected, it's permanent
/// for this process lifetime.
static MOUNT_VIOLATION: OnceLock<String> = OnceLock::new();

/// Report a mount boundary violation from a worker thread.
///
/// Called by computations when they detect a path with a different st_dev
/// than the expected root filesystem. The Witch will pick this up on next
/// tick() and latch into read-only mode.
pub fn report_mount_violation(reason: String) {
    let _ = MOUNT_VIOLATION.set(reason);
}

/// Check if a mount violation has been reported.
///
/// Returns the violation reason if one has been reported.
fn check_mount_violation() -> Option<&'static str> {
    MOUNT_VIOLATION.get().map(|s| s.as_str())
}

// ============================================================================
// The Witch
// ============================================================================

/// The Witch - enforcer of orderliness, guarantor of data integrity.
///
/// She provides a parallel task queue with state machine semantics,
/// ensuring all mutations flow through proper witness channels.
pub struct Witch {
    // Rayon-based task execution with channel for results
    result_tx: Sender<TaskResult>,
    result_rx: Receiver<TaskResult>,

    // State machine
    state: TaskExecutionState,

    // Eye and corpus observation state
    eye_state: EyeState,
    observation_state: CorpusObservationState,

    /// Whether legacy library observation is enabled.
    /// Derived from Config at construction time.
    legacy_enabled: bool,

    /// When true, mutations are permanently disabled (read-only debug mode).
    /// Set from config at startup.
    read_only_mode: bool,

    /// Runtime safety latch: if Some, mutations are permanently disabled for this session.
    /// Contains the reason why the safety latch was triggered (e.g., mount boundary violation).
    /// Once set, cannot be unset - operator must fix the issue and restart MM.
    safety_latch_reason: Option<String>,

    /// Whether mutations have run this session.
    /// Used to auto-trigger content analysis after mutations + awakening drain.
    mutations_ran_this_session: bool,

    /// Force verification of all indexed files at startup, bypassing mtime optimization.
    /// Catches out-of-band tag changes and corrupt files.
    force_check_all_files_at_startup: bool,

    // Session tracking
    session_start: Option<Instant>,
    session_queued: usize,
    total_processed: usize,
    total_failed: usize,

    /// Tasks that have been sent to the worker but not yet drained from results.
    /// This includes both queued tasks AND tasks currently executing on worker threads.
    /// Incremented on add_task(), decremented when result is drained from get().
    in_flight: usize,

    // Status tracking
    recent_errors: VecDeque<String>,
    task_counts: HashMap<String, usize>,
    /// Pending task counts by label (queued but not yet completed).
    pending_by_label: HashMap<String, usize>,
    current_label: Option<String>,

    completed_at: Option<Instant>,

    // Transaction state
    pending_transaction: Option<PendingTransaction>,

    /// Cached read-only database connection for UI queries.
    /// Only accessed from the main thread via `read_db()`.
    /// UI code should use this instead of creating direct connections.
    read_only_conn: Option<Database>,

    /// Handle to the dedicated DB write thread.
    /// Provides stats access and shutdown coordination.
    /// None during migration phase (before db_thread is safe to spawn).
    db_thread_handle: Option<DbThreadHandle>,

    // -------------------------------------------------------------------------
    // Worker Performance Stats (Thread-Safe, Isolated)
    // -------------------------------------------------------------------------

    /// Thread-safe worker stats in separate heap allocation.
    /// None when timing instrumentation is disabled.
    worker_stats_shared: Option<Arc<SharedWorkerStats>>,

    /// Background cache for UI read queries.
    /// UI calls want_*() methods, the Witch spawns refresh tasks in tick().
    ui_read_cache: UiReadCache,

    /// Corpus inodes observed on disk during the current observation cycle.
    /// Accumulated from ScanCorpusDirectory results in tick().
    /// Consumed by queue_awakening_computations() via std::mem::take().
    observed_corpus_inodes: HashMap<i64, String>,
    /// Inbox inodes observed on disk during the current observation cycle.
    /// Accumulated from ScanCorpusDirectory results in tick().
    /// Consumed by queue_awakening_computations() via std::mem::take().
    observed_inbox_inodes: HashMap<i64, String>,

    /// Library files observed on disk during the current awakening cycle.
    /// Accumulated from ScanLibraryDirectory results in tick().
    /// Consumed by transition_to_completed() when awakening first drains,
    /// which queues ReconcileLibraryFiles with this data.
    observed_library_files: Vec<awakening::ObservedLibraryFile>,

    /// Whether library reconciliation has completed in the current awakening cycle.
    /// Used for two-stage awakening transition:
    ///   - First drain (false): queue ReconcileLibraryFiles, stay in Awakening
    ///   - Second drain (true): transition to Awake normally
    library_reconciliation_done: bool,

    /// Handle to the dedicated logging thread for shutdown coordination.
    log_thread_handle: Option<crate::logging::LogThreadHandle>,

    /// Shared config reference for runtime config updates.
    /// Set after construction via `set_shared_config()`.
    shared_config: Option<SharedConfig>,

    /// When the Witch entered Idle state while Awake. For idle rescan timer.
    idle_since: Option<Instant>,

    /// True while an idle rescan is in progress.
    idle_rescan_active: bool,

    /// UI-controlled gate: true only on safe browsing views (lateral ring).
    idle_rescan_eligible: bool,
}

impl Witch {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    pub fn new(cfg: &Config, log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>) -> Self {
        // Spawn the logging thread if we have the receiver
        let log_thread_handle = log_rx.map(crate::logging::spawn_log_thread);

        // Use rayon's global thread pool with work-stealing for better performance.
        // Thread count from config (default: 2x logical cores for I/O-bound workloads).
        let num_threads = config::get_worker_thread_count();

        // Configure rayon's global thread pool (only first call takes effect)
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global();

        crate::logging::log_general(format!(
            "[WORKER] Using rayon thread pool with {} threads",
            num_threads
        ));

        // Channel for receiving task results
        let (result_tx, result_rx) = mpsc::channel();

        // NOTE: db_thread is NOT spawned here. It is spawned later via spawn_db_thread()
        // after migrations complete. This ensures the schema is correct before
        // db_thread opens its write connection.

        // Create isolated worker stats only when timing instrumentation is enabled
        let worker_stats_shared = if config::is_timing_enabled() {
            Some(Arc::new(SharedWorkerStats::new()))
        } else {
            None
        };

        let she = Self {
            result_tx,
            result_rx,
            state: TaskExecutionState::Idle,
            eye_state: EyeState::Closed,
            observation_state: CorpusObservationState::Unseen,
            legacy_enabled: cfg.legacy_enabled,
            read_only_mode: false,
            safety_latch_reason: None,
            mutations_ran_this_session: false,
            force_check_all_files_at_startup: false, // Set via with_opinions()
            session_start: None,
            session_queued: 0,
            total_processed: 0,
            total_failed: 0,
            in_flight: 0,
            recent_errors: VecDeque::with_capacity(5),
            task_counts: HashMap::new(),
            pending_by_label: HashMap::new(),
            current_label: None,
            completed_at: None,
            pending_transaction: None,
            read_only_conn: None,
            db_thread_handle: None,  // Spawned later via spawn_db_thread()
            worker_stats_shared,
            ui_read_cache: UiReadCache::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: Vec::new(),
            library_reconciliation_done: false,
            log_thread_handle,
            shared_config: None,
            idle_since: None,
            idle_rescan_active: false,
            idle_rescan_eligible: false,
        };

        // DEBUG: Verify initialization (only when timing enabled)
        if let Some(ref stats) = she.worker_stats_shared {
            let (_tasks, total_qw, max_qw) = stats.debug_values();
            crate::logging::log_perf(format!(
                "[PERF INIT] Witch::new() - total_queue_wait_ms={}, max_queue_wait_ms={}, total_processed={}",
                total_qw, max_qw, she.total_processed
            ));
        }

        she
    }

    /// Create a new Witch with opinions applied.
    pub fn with_opinions(cfg: &Config, read_only_mode: bool, force_check_all_files_at_startup: bool, log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>) -> Self {
        let mut she = Self::new(cfg, log_rx);
        she.read_only_mode = read_only_mode;
        she.force_check_all_files_at_startup = force_check_all_files_at_startup;
        if force_check_all_files_at_startup {
            crate::logging::log_general(
                "[WITCH] force_check_all_files_at_startup=true: will verify all indexed files at startup"
            );
        }
        she
    }

    // -------------------------------------------------------------------------
    // Shared Config
    // -------------------------------------------------------------------------

    /// Store the shared config reference after construction.
    ///
    /// Called from `run_menu()` after both App and Witch are created.
    pub fn set_shared_config(&mut self, shared: SharedConfig) {
        self.shared_config = Some(shared);
    }

    /// Replace the in-memory config with a new version (after config edit mutation).
    ///
    /// Write-locks briefly; safe because tick() and render() are sequential on main thread.
    pub fn update_shared_config(&self, new_config: Config) {
        if let Some(ref shared) = self.shared_config {
            let mut guard = shared.write().expect("SharedConfig lock poisoned");
            *guard = new_config;
        }
    }

    // -------------------------------------------------------------------------
    // DB Thread Lifecycle
    // -------------------------------------------------------------------------

    /// Spawn the db_thread. Called after migrations complete.
    ///
    /// # Panics
    ///
    /// Panics if db_thread is already spawned.
    pub fn spawn_db_thread(&mut self) {
        assert!(
            self.db_thread_handle.is_none(),
            "db_thread already spawned"
        );
        crate::logging::log_general("[WITCH] Spawning db_thread");
        self.db_thread_handle = Some(db_thread::spawn());
    }

    // -------------------------------------------------------------------------
    // Eye and Observation State
    // -------------------------------------------------------------------------

    /// Get current eye state for UI rendering decisions.
    pub fn eye_state(&self) -> EyeState {
        self.eye_state
    }

    /// Check if observing is currently in progress.
    pub fn is_observing(&self) -> bool {
        matches!(self.observation_state, CorpusObservationState::Observing)
    }

    /// Set the UI-controlled idle rescan eligibility flag.
    ///
    /// Should be true only on lateral browsing views (Insights, CorpusBrowser,
    /// TagSearch, Inbox). False during modals, resolution flows, or transactions.
    pub fn set_idle_rescan_eligible(&mut self, eligible: bool) {
        self.idle_rescan_eligible = eligible;
    }

    /// Check if an idle rescan is currently in progress.
    pub fn idle_rescan_active(&self) -> bool {
        self.idle_rescan_active
    }

    /// Check if mutations are currently accepted.
    ///
    /// Mutations are only accepted when:
    /// - Eye state is Awake (observing and awakening complete)
    /// - Not in read-only mode (config setting)
    /// - Safety latch not triggered (runtime invariant violation)
    /// - No idle rescan in progress
    ///
    /// This is derived state - mutations are automatically blocked during
    /// re-awakening cycles after mutations drain, and during idle rescans.
    fn accepting_mutations(&self) -> bool {
        self.eye_state == EyeState::Awake
            && !self.read_only_mode
            && self.safety_latch_reason.is_none()
            && !self.idle_rescan_active
    }

    /// Trigger the safety latch, permanently disabling mutations for this session.
    ///
    /// This is a one-way operation - once latched, cannot be unlatched.
    /// The operator must fix the underlying issue and restart MM.
    ///
    /// Called when a runtime invariant is violated (e.g., mount boundary crossed).
    pub fn latch_read_only_for_safety(&mut self, reason: String) {
        if self.safety_latch_reason.is_none() {
            crate::logging::log_error(format!(
                "[WITCH] SAFETY LATCH TRIGGERED: {}",
                reason
            ));
            self.safety_latch_reason = Some(reason);
        }
    }

    /// Start observing. Returns false if observing already in progress.
    ///
    /// Derives paths from the global resolver and stored config.
    /// Queues WalkCorpus computations for corpus and optional legacy library.
    pub fn start_observing(&mut self) -> bool {
        if self.is_observing() {
            return false;
        }

        self.observation_state = CorpusObservationState::Observing;
        self.queue_observing_computations();
        true
    }

    /// Queue observing computations (internal helper).
    fn queue_observing_computations(&mut self) {
        let resolver = crate::corpus::paths::get_resolver();
        let force_check = self.force_check_all_files_at_startup;

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();
        self.observed_library_files.clear();
        self.library_reconciliation_done = false;

        // Queue corpus walk
        self.queue_computation_with_label(
            Computation::Asleep(asleep::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check,
            }),
            Some("Observing corpus".to_string()),
        );

        // Queue inbox walk if inbox directory exists
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Asleep(asleep::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check,
                }),
                Some("Observing inbox".to_string()),
            );
        }

        // Queue legacy library walk if enabled
        if self.legacy_enabled {
            self.queue_computation_with_label(
                Computation::Asleep(asleep::Computation::WalkCorpus {
                    root: resolver.libraries_dir().join("legacy"),
                    zone: "legacy".to_string(),
                    force_check,
                }),
                Some("Observing legacy".to_string()),
            );
        }
    }

    /// Advance internal processing. MUST be called exactly once per frame from the event loop.
    ///
    /// **IMPORTANT**: Only call from `run_app()` event loop. Use `status()` for readonly access elsewhere.
    /// Calling tick() multiple times per frame will corrupt task counting.
    ///
    /// - Drains completed task results from worker
    /// - Auto-queues any spawned follow-up computations
    /// - Updates state machine transitions
    /// - Aggregates worker performance stats (when timing enabled)
    pub fn tick(&mut self) -> DaemonStatus {
        // Check for mount boundary violations reported by worker threads
        if let Some(reason) = check_mount_violation() {
            self.latch_read_only_for_safety(reason.to_string());
        }

        // DEBUG: Log first tick state (only when timing enabled)
        if let Some(ref stats) = self.worker_stats_shared {
            let (_, total_qw_before, _) = stats.debug_values();
            if self.total_processed == 0 && total_qw_before != 0 {
                crate::logging::log_perf(format!(
                    "[PERF BUG] tick() called with total_processed=0 but total_queue_wait_ms={}!",
                    total_qw_before
                ));
            }
        }

        let mut completed = 0;
        let mut failed = 0;

        // Drain completed results and collect spawned computations and mutations
        let mut spawned_computations: Vec<Computation> = Vec::new();
        let mut spawned_mutations: Vec<types::SpawnedMutation> = Vec::new();

        while let Ok(result) = self.result_rx.try_recv() {
            // Task has completed - no longer in flight
            self.in_flight = self.in_flight.saturating_sub(1);
            self.total_processed += 1;

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;

            // Decrement pending count for this label
            if let Some(count) = self.pending_by_label.get_mut(&result.label) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.pending_by_label.remove(&result.label);
                }
            }

            // Record stats via thread-safe interface (only when timing enabled)
            if let Some(ref stats) = self.worker_stats_shared {
                stats.record_result(&result);

                // DEBUG: Log queue wait values
                let (_, total_qw_now, max_qw_now) = stats.debug_values();
                if self.total_processed == 1 {
                    crate::logging::log_perf(format!(
                        "[PERF FIRST] FIRST TASK: queue_wait_ms={}, total_queue_wait_ms={} (should equal queue_wait_ms!), max_queue_wait_ms={}",
                        result.queue_wait_ms, total_qw_now, max_qw_now
                    ));
                }
                if self.total_processed <= 50 || self.total_processed.is_multiple_of(500) || result.queue_wait_ms > 50000 {
                    crate::logging::log_perf(format!(
                        "[PERF DEBUG] queue_wait_ms={} for task={}, total_processed={}, total_queue_wait_ms={}, max_queue_wait_ms={}",
                        result.queue_wait_ms, result.label, self.total_processed, total_qw_now, max_qw_now
                    ));
                }
            }

            if result.success {
                completed += 1;
            } else {
                failed += 1;
                self.total_failed += 1;
                if let Some(err) = result.error {
                    if self.recent_errors.len() >= 5 {
                        self.recent_errors.pop_front();
                    }
                    self.recent_errors.push_back(err);
                }
            }

            // Apply config update if present (from ApplyConfigEdits mutation)
            if let Some(new_config) = result.config_update {
                self.update_shared_config(new_config);
            }

            // Accumulate observed inodes from ScanCorpusDirectory results
            self.observed_corpus_inodes.extend(result.observed_corpus_inodes);
            self.observed_inbox_inodes.extend(result.observed_inbox_inodes);

            // Accumulate observed library files from ScanLibraryDirectory results
            self.observed_library_files.extend(result.observed_library_files);

            // Collect spawned follow-up computations and mutations
            spawned_computations.extend(result.spawn);
            spawned_mutations.extend(result.spawn_mutations);
        }

        // Queue spawned follow-up computations (chaining)
        // IMPORTANT: This happens BEFORE we check in_flight for state transitions,
        // ensuring spawned tasks are counted before we decide to transition.
        for comp in spawned_computations {
            self.queue_computation_with_label(comp, None);
        }

        // Queue spawned follow-up mutations (chaining from mutations like ApplyTagOps)
        // These are pre-authorized by the parent mutation's witness chain.
        for mutation in spawned_mutations {
            self.queue_spawned_mutation(mutation);
        }

        // Spawn background cache refresh tasks (demand-driven, throttled)
        self.ui_read_cache.spawn_refreshes();

        // Capture current state for status
        let current_in_flight = self.in_flight;
        let current_total_processed = self.total_processed;
        let current_session_queued = self.session_queued;
        let current_pending_by_label = self.pending_by_label.clone();

        // State machine transitions (uses self.in_flight internally)
        self.update_state();

        // Check if idle rescan should trigger
        self.maybe_start_idle_rescan();

        DaemonStatus {
            state: self.state.into(),
            pending: current_in_flight,
            completed,
            failed,
            total_processed: current_total_processed,
            session_queued: current_session_queued,
            pending_by_label: current_pending_by_label,
            idle_rescan_active: self.idle_rescan_active,
        }
    }

    /// Update state machine based on in-flight tasks and timing.
    fn update_state(&mut self) {
        match self.state {
            TaskExecutionState::Idle => {
                // Idle → Working: handled in queue methods
            }
            TaskExecutionState::Working => {
                // Working → Completed: when all work finishes (no in-flight tasks AND
                // db_thread queue empty). Uses centralized has_pending() for consistency
                // with exit handlers and UI state display.
                if !self.has_pending() && self.total_processed > 0 {
                    self.transition_to_completed();
                }
            }
            TaskExecutionState::Completed => {
                // Completed → Idle: after linger timeout
                if let Some(completed_at) = self.completed_at {
                    if completed_at.elapsed() >= Self::LINGER_DURATION {
                        self.transition_to_idle();
                    }
                }
                // Completed → Working: handled in queue methods
            }
        }
    }

    fn transition_to_completed(&mut self) {
        // Capture mutation flag before session reset
        let had_mutations = self.mutations_ran_this_session;

        // Flag to queue awakening computations after session reset
        let mut queue_awakening_after_reset = false;
        // Flag to queue idle-rescan-only awakening (corpus+inbox signals, no second-level)
        let mut queue_idle_rescan_awakening_after_reset = false;
        // Flag to auto-queue content analysis after mutations drain
        let mut queue_content_analysis_after_reset = false;
        // Flag to queue re-observation (WalkCorpus) after mutations complete
        let mut queue_reobservation_after_reset = false;
        // Flag to queue ReconcileLibraryFiles after awakening stage 1
        let mut queue_reconcile_library_after_reset = false;
        let mut reconcile_library_observed: Option<Vec<awakening::ObservedLibraryFile>> = None;

        self.completed_at = Some(Instant::now());
        self.state = TaskExecutionState::Completed;

        // State transition based on (observing, eye_state) tuple
        // All combinations explicitly handled; invalid states panic
        match (self.is_observing(), self.eye_state) {
            // Observing completed while Closed: begin Awakening
            (true, EyeState::Closed) => {
                self.observation_state = CorpusObservationState::Complete;
                crate::logging::log_general(format!(
                    "[STATE] Observing complete. Transitioning Closed -> Awakening. \
                     Processed {} tasks.",
                    self.total_processed
                ));
                self.eye_state = EyeState::Awakening;
                queue_awakening_after_reset = true;
            }

            // Re-observing completed while Awake: sync signals via awakening
            (true, EyeState::Awake) => {
                self.observation_state = CorpusObservationState::Complete;
                if self.idle_rescan_active {
                    crate::logging::log_general(format!(
                        "[STATE] Idle rescan observation complete. Queueing lightweight signal derivation. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    queue_idle_rescan_awakening_after_reset = true;
                } else {
                    crate::logging::log_general(format!(
                        "[STATE] Re-observing complete while Awake. Queueing awakening to sync signals. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    queue_awakening_after_reset = true;
                }
            }

            // Re-walk completed during re-awakening: proceed to derivations
            (true, EyeState::Awakening) => {
                self.observation_state = CorpusObservationState::Complete;
                crate::logging::log_general(format!(
                    "[STATE] Re-observation complete during Awakening. Queueing derivations. \
                     Processed {} tasks.",
                    self.total_processed
                ));
                queue_awakening_after_reset = true;
            }

            // Awakening completed: two-stage transition
            // Stage 1: Queue ReconcileLibraryFiles, stay in Awakening
            // Stage 2: Transition to Awake and queue content analysis
            (false, EyeState::Awakening) => {
                if !self.library_reconciliation_done {
                    // Stage 1: Library reconciliation not yet done
                    self.library_reconciliation_done = true;
                    let observed = std::mem::take(&mut self.observed_library_files);
                    crate::logging::log_general(format!(
                        "[STATE] Awakening stage 1 complete. Queueing ReconcileLibraryFiles ({} observed files). \
                         Staying in Awakening. Processed {} tasks.",
                        observed.len(), self.total_processed
                    ));
                    queue_reconcile_library_after_reset = true;
                    reconcile_library_observed = Some(observed);
                } else {
                    // Stage 2: Library reconciliation done, NOW transition to Awake
                    self.library_reconciliation_done = false;
                    crate::logging::log_general(format!(
                        "[STATE] Awakening stage 2 complete. Transitioning Awakening -> Awake. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    self.eye_state = EyeState::Awake;

                    if !self.read_only_mode {
                        crate::logging::log_general("[STATE] Mutations now enabled (read-write mode).");
                    }

                    // Queue content analysis after full awakening
                    queue_content_analysis_after_reset = true;
                }
            }

            // Normal operation: work completed while Awake
            (false, EyeState::Awake) => {
                if self.idle_rescan_active {
                    // Idle rescan signal derivation complete
                    crate::logging::log_general(format!(
                        "[STATE] Idle rescan complete. Processed {} tasks.",
                        self.total_processed
                    ));
                    self.idle_rescan_active = false;
                } else if had_mutations {
                    // Mutations ran - re-validate everything via re-awakening
                    crate::logging::log_general(format!(
                        "[STATE] Mutations complete. Transitioning Awake -> Awakening for re-validation. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    self.eye_state = EyeState::Awakening;
                    self.observation_state = CorpusObservationState::Observing;
                    queue_reobservation_after_reset = true;
                }
                // If no mutations and not idle rescan, stay Awake (normal work completion)
            }

            // Migrations can complete while Closed - this is valid, just NOP
            (false, EyeState::Closed) => {
                let had_migrations = self.task_counts.keys().any(|k| k.starts_with("Migration"));
                if had_migrations {
                    crate::logging::log_general(format!(
                        "[STATE] Migrations complete while Closed. Staying Closed. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    // NOP: stay Closed, let session reset happen normally
                } else {
                    panic!(
                        "Invalid state: non-observing, non-migration work completed while eye is Closed. \
                         The only work while Closed should be observing or migrations."
                    );
                }
            }
        }

        // Reset session state
        self.session_start = None;
        self.total_processed = 0;
        self.total_failed = 0;
        self.session_queued = 0;
        self.task_counts.clear();
        self.pending_by_label.clear();
        self.recent_errors.clear();
        self.current_label = None;
        self.mutations_ran_this_session = false;

        // Flush all pending db_thread writes before queueing the next phase.
        // Computation tasks fire writes asynchronously via db_thread (fire-and-forget).
        // The task completes when the worker returns, NOT when db_thread commits the
        // writes. Without this barrier, the next phase's computations could read stale
        // data (e.g., DeriveDeployHealthSignals reading library files written by
        // ScanLibraryDirectory, or Awake-phase computations reading Awakening signals).
        if queue_awakening_after_reset || queue_idle_rescan_awakening_after_reset || queue_content_analysis_after_reset || queue_reobservation_after_reset || queue_reconcile_library_after_reset {
            db_thread::wait_for_queue_drain();
        }

        // Queue follow-up computations AFTER reset to fix off-by-one counting
        // (if queued before reset, the task's queue count gets wiped but it still completes)
        if queue_reobservation_after_reset {
            self.queue_reobservation_computations();
        }
        if queue_awakening_after_reset {
            self.queue_awakening_computations();
        }
        if queue_idle_rescan_awakening_after_reset {
            self.queue_idle_rescan_awakening();
        }
        if queue_reconcile_library_after_reset {
            if let Some(observed) = reconcile_library_observed {
                self.queue_computation_with_label(
                    Computation::Awakening(awakening::Computation::ReconcileLibraryFiles {
                        observed_files: observed,
                    }),
                    Some("Reconciling library files".to_string()),
                );
            }
        }
        if queue_content_analysis_after_reset {
            // Create witness here - this is the ONLY valid call site
            let witness = ContentAnalysisWitness::new();
            self.queue_content_analysis(witness);
        }
    }

    /// Queue Awakening-phase computations.
    ///
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals
    /// and DeriveInboxSignals directly with the data. Also queues
    /// ScheduleSecondLevelDerivations for directory checks and library walks.
    fn queue_awakening_computations(&mut self) {
        let observed_corpus = std::mem::take(&mut self.observed_corpus_inodes);
        let observed_inbox = std::mem::take(&mut self.observed_inbox_inodes);

        crate::logging::log_general(format!(
            "[STATE] Queueing Awakening: DeriveCorpusSignals ({} inodes), DeriveInboxSignals ({} inodes), ScheduleSecondLevelDerivations",
            observed_corpus.len(), observed_inbox.len()
        ));

        self.queue_computation_with_label(
            Computation::Awakening(awakening::Computation::DeriveCorpusSignals {
                observed_inodes: observed_corpus,
            }),
            Some("Deriving corpus signals".to_string()),
        );

        self.queue_computation_with_label(
            Computation::Awakening(awakening::Computation::DeriveInboxSignals {
                observed_inodes: observed_inbox,
            }),
            Some("Deriving inbox signals".to_string()),
        );

        // Directory checks + library walks
        self.queue_computation_with_label(
            Computation::Awakening(awakening::Computation::ScheduleSecondLevelDerivations),
            Some("Computing directory signals".to_string()),
        );
    }

    /// Check if conditions are met to start an idle rescan.
    ///
    /// Called from `tick()` after `update_state()`. All gates:
    /// - State is Idle, eye is Awake
    /// - `idle_rescan_eligible` (UI on lateral view)
    /// - No active transaction
    /// - Config interval > 0 and timer expired
    fn maybe_start_idle_rescan(&mut self) {
        if self.state != TaskExecutionState::Idle || self.eye_state != EyeState::Awake {
            return;
        }
        if !self.idle_rescan_eligible {
            return;
        }
        // In open-txn mode, idle rescans are always allowed (they're read-only observation).
        // In closed-txn mode, block if a transaction is open (user is actively reviewing).
        let open_txn_mode = self.shared_config.as_ref()
            .map(|sc| sc.read().expect("SharedConfig lock poisoned").opinions.leave_transactions_open)
            .unwrap_or(false);
        if !open_txn_mode && self.pending_transaction.is_some() {
            return;
        }

        let interval_secs = self.shared_config.as_ref()
            .map(|sc| sc.read().expect("SharedConfig lock poisoned").opinions.idle_rescan_interval_secs)
            .unwrap_or(0);
        if interval_secs == 0 {
            return;
        }

        let idle_since = match self.idle_since {
            Some(t) => t,
            None => return,
        };
        if idle_since.elapsed() < Duration::from_secs(interval_secs) {
            return;
        }

        // All gates passed — start idle rescan
        crate::logging::log_general(format!(
            "[STATE] Starting idle rescan (idle for {}s, interval={}s)",
            idle_since.elapsed().as_secs(), interval_secs
        ));

        self.idle_rescan_active = true;
        self.idle_since = None;
        self.observation_state = CorpusObservationState::Observing;

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();

        let resolver = crate::corpus::paths::get_resolver();

        // Queue corpus walk (mtime-optimized)
        self.queue_computation_with_label(
            Computation::Asleep(asleep::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check: false,
            }),
            Some("Rescanning corpus".to_string()),
        );

        // Queue inbox walk if directory exists (mtime-optimized)
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Asleep(asleep::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check: false,
                }),
                Some("Rescanning inbox".to_string()),
            );
        }
    }

    /// Queue lightweight awakening computations for idle rescan.
    ///
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals
    /// and DeriveInboxSignals. Does NOT queue ScheduleSecondLevelDerivations
    /// (no library walks, no directory-level checks).
    fn queue_idle_rescan_awakening(&mut self) {
        let observed_corpus = std::mem::take(&mut self.observed_corpus_inodes);
        let observed_inbox = std::mem::take(&mut self.observed_inbox_inodes);

        crate::logging::log_general(format!(
            "[STATE] Queueing idle rescan awakening: DeriveCorpusSignals ({} inodes), DeriveInboxSignals ({} inodes)",
            observed_corpus.len(), observed_inbox.len()
        ));

        self.queue_computation_with_label(
            Computation::Awakening(awakening::Computation::DeriveCorpusSignals {
                observed_inodes: observed_corpus,
            }),
            Some("Deriving corpus signals".to_string()),
        );

        self.queue_computation_with_label(
            Computation::Awakening(awakening::Computation::DeriveInboxSignals {
                observed_inodes: observed_inbox,
            }),
            Some("Deriving inbox signals".to_string()),
        );
    }

    /// Queue re-observation computations (WalkCorpus) for re-awakening after mutations.
    ///
    /// Similar to `queue_observing_computations` but for re-awakening cycles.
    /// Uses mtime optimization (`force_check: false`) since mutations that changed
    /// files will naturally trigger verification via mtime changes.
    fn queue_reobservation_computations(&mut self) {
        let resolver = crate::corpus::paths::get_resolver();

        crate::logging::log_general(
            "[STATE] Queueing re-observation computations for post-mutation re-awakening"
        );

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();
        self.observed_library_files.clear();
        self.library_reconciliation_done = false;

        // Re-walk corpus (mtime-optimized)
        self.queue_computation_with_label(
            Computation::Asleep(asleep::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check: false,
            }),
            Some("Re-observing corpus".to_string()),
        );

        // Re-walk inbox if directory exists (mtime-optimized)
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Asleep(asleep::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check: false,
                }),
                Some("Re-observing inbox".to_string()),
            );
        }

        // Re-walk legacy library if enabled (mtime-optimized)
        if self.legacy_enabled {
            self.queue_computation_with_label(
                Computation::Asleep(asleep::Computation::WalkCorpus {
                    root: resolver.libraries_dir().join("legacy"),
                    zone: "legacy".to_string(),
                    force_check: false,
                }),
                Some("Re-observing legacy".to_string()),
            );
        }
    }

    /// Queue content analysis computations (internal only).
    ///
    /// This queues `ScheduleContentAnalysis` which will spawn bulk detection
    /// computations for FingerprintOverlap, CrossSourceOverlap, MissingTag, etc.
    ///
    /// Only callable from `transition_to_completed` when mutations drain while Awake.
    /// Sealed by requiring `ContentAnalysisWitness` which can only be created in that context.
    fn queue_content_analysis(&mut self, _witness: ContentAnalysisWitness) {
        crate::logging::log_general(
            "[STATE] Queueing ScheduleContentAnalysis for content analysis"
        );

        // Queue the orchestrator computation that will spawn all detection computations
        self.queue_computation_with_label(
            Computation::Awake(awake::Computation::ScheduleContentAnalysis),
            Some("Analyzing metadata".to_string()),
        );
    }

    fn transition_to_idle(&mut self) {
        self.state = TaskExecutionState::Idle;
        self.completed_at = None;
        if self.eye_state == EyeState::Awake {
            self.idle_since = Some(Instant::now());
        }
    }

    fn transition_to_working(&mut self) {
        if self.session_start.is_none() {
            self.session_start = Some(Instant::now());
        }
        self.state = TaskExecutionState::Working;
    }

    // -------------------------------------------------------------------------
    // Task Spawning Helper
    // -------------------------------------------------------------------------

    /// Spawn a task on the rayon thread pool with panic catching.
    ///
    /// If the task panics, we still send a failure result so the Witch's
    /// in_flight counter stays accurate and we don't lose tasks silently.
    fn spawn_task(&self, task: Task, label: String, queue_time: Instant) {
        let tx = self.result_tx.clone();
        let label_for_panic = label.clone();
        rayon::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_task(task, label, queue_time)
            }));
            let result = match result {
                Ok(r) => r,
                Err(e) => {
                    let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };
                    TaskResult {
                        success: false,
                        error: Some(format!("Task panicked: {}", panic_msg)),
                        label: label_for_panic,
                        spawn: Vec::new(),
                        spawn_mutations: Vec::new(),
                        duration_ms: queue_time.elapsed().as_millis() as u64,
                        queue_wait_ms: 0,
                        thread_stats: None,
                        config_update: None,
                        observed_corpus_inodes: HashMap::new(),
                        observed_inbox_inodes: HashMap::new(),
                        observed_library_files: Vec::new(),
                    }
                }
            };
            let _ = tx.send(result);
        });
    }

    // -------------------------------------------------------------------------
    // Label Resolution Helper
    // -------------------------------------------------------------------------

    /// Resolve task label from explicit label, current_label, or task fallback.
    fn resolve_label(&self, explicit_label: Option<String>, task: &Task) -> String {
        explicit_label
            .or_else(|| self.current_label.clone())
            .unwrap_or_else(|| TaskLabel::from_task(task).0)
    }

    // -------------------------------------------------------------------------
    // Internal Mutation Queueing (used by transaction API)
    // -------------------------------------------------------------------------

    // NOTE: Direct mutation queueing methods (queue, queue_all, etc.) were removed.
    // All mutations must go through the transaction API:
    //   1. start_transaction(label)
    //   2. add_decision(idx, witness, label, mutations) for each decision
    //   3. confirm_transaction(witness) to commit, or discard_transaction(witness) to abort
    //
    // This ensures proper decision witness semantics where each user action is
    // explicitly witnessed, and batch review/commit is possible.

    pub(super) fn queue_mutations_internal(&mut self, mutations: impl IntoIterator<Item = Mutation>, label: Option<String>) {
        self.transition_to_working();
        self.mutations_ran_this_session = true;

        let queue_time = Instant::now();
        let mutations: Vec<_> = mutations.into_iter().collect();

        crate::logging::log_general(format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(), label
        ));

        self.session_queued += mutations.len();
        self.in_flight += mutations.len();

        for mutation in mutations {
            let task = Task::Mutation(mutation);
            let task_label = self.resolve_label(label.clone(), &task);
            *self.pending_by_label.entry(task_label.clone()).or_insert(0) += 1;
            self.spawn_task(task, task_label, queue_time);
        }
    }

    /// Queue a mutation spawned by another mutation (spawn chaining).
    ///
    /// This is called internally from tick() when processing spawn_mutations
    /// from completed tasks. The spawn chain is already authorized by the
    /// parent mutation's witness - no additional operator decision required.
    ///
    /// Example: ApplyTagOps spawns ApplyDbTagsToDisk after DB write succeeds.
    fn queue_spawned_mutation(&mut self, spawned: types::SpawnedMutation) {
        // Extract the inner mutation - SpawnedMutation's existence proves authorization
        let mutation = spawned.into_inner();

        // Spawned mutations inherit the working state from their parent
        // (transition_to_working already happened when parent was queued)
        self.mutations_ran_this_session = true;

        let task = Task::Mutation(mutation);
        let task_label = TaskLabel::from_task(&task).0;

        self.session_queued += 1;
        self.in_flight += 1;
        *self.pending_by_label.entry(task_label.clone()).or_insert(0) += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Operator Decision Scope (Sealed Access)
    // -------------------------------------------------------------------------

    /// Enter operator decision context.
    ///
    /// # Sealed Access Pattern
    ///
    /// ⚠️ **ONLY CALL FROM `ui/operator_decisions.rs`** ⚠️
    ///
    /// This method creates a `DecisionScope` that provides access to transaction
    /// operations with an internal `DecisionWitness`. The witness exists only
    /// within the callback scope and cannot be stored, returned, or passed elsewhere.
    ///
    /// All UI code that needs to make decisions should call functions in
    /// `ui/operator_decisions.rs`, which is the only sanctioned call site for
    /// this method.
    ///
    /// # Example (from operator_decisions.rs only)
    ///
    /// ```rust,ignore
    /// pub fn commit_transaction(witch: &mut Witch) -> Result<(), TransactionError> {
    ///     witch.with_operator_decision(|scope| {
    ///         scope.confirm_transaction()
    ///     })
    /// }
    /// ```
    pub(crate) fn with_operator_decision<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut types::DecisionScope<'_>) -> R,
    {
        let mut scope = types::DecisionScope::new(self);
        f(&mut scope)
    }

    // -------------------------------------------------------------------------
    // Computation Queueing (internal only, no witness required)
    // -------------------------------------------------------------------------

    /// Queue a single computation with an optional label.
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    fn queue_computation_with_label(&mut self, computation: Computation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Computation(computation);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        *self.pending_by_label.entry(task_label.clone()).or_insert(0) += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Migration Queueing (requires witness, bypasses accepting_mutations)
    // -------------------------------------------------------------------------

    /// Queue a single migration for execution (internal, called from DecisionScope).
    ///
    /// Migrations require a [`DecisionWitness`] (user approval) but bypass the
    /// `accepting_mutations` gate. They can run before observing completes.
    pub(super) fn queue_migration(&mut self, migration: Migration, _witness: &DecisionWitness) {
        self.transition_to_working();

        let task = Task::Migration(migration);
        let task_label = self.resolve_label(None, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        *self.pending_by_label.entry(task_label.clone()).or_insert(0) += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Migration-Aware Startup Methods
    // -------------------------------------------------------------------------

    /// Check if migrations are needed.
    ///
    /// Returns true if the database exists and has pending schema migrations.
    pub fn needs_migrations(&self) -> bool {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return false,
        };
        matches!(
            InitialUiState::determine(&db_path),
            InitialUiState::MigrationRequired
        )
    }

    /// Get pending migration descriptions for UI display.
    ///
    /// Returns a list of human-readable descriptions of pending migrations.
    pub fn pending_migration_descriptions(&self) -> Vec<String> {
        use crate::meta::mutations::MigrationRegistry;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        MigrationRegistry::new().pending_descriptions(&db)
    }

    /// Queue all pending migrations for execution.
    ///
    /// Creates a DecisionWitness internally via with_operator_decision.
    /// Call this only after user approval of migrations.
    pub fn queue_pending_migrations(&mut self) {
        use crate::meta::mutations::MigrationRegistry;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                crate::logging::log_error(format!(
                    "[WITCH] queue_pending_migrations: no db path: {}",
                    e
                ));
                return;
            }
        };
        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(e) => {
                crate::logging::log_error(format!(
                    "[WITCH] queue_pending_migrations: failed to open db: {}",
                    e
                ));
                return;
            }
        };

        let registry = MigrationRegistry::new();
        let current_version = db.get_schema_version().unwrap_or(1);

        // Collect migrations to queue
        let migrations: Vec<_> = registry
            .pending_migrations(current_version)
            .iter()
            .map(|m| Migration {
                from_version: m.from_version,
                to_version: m.to_version,
            })
            .collect();

        if migrations.is_empty() {
            crate::logging::log_general("[WITCH] No migrations to queue");
            return;
        }

        crate::logging::log_general(format!(
            "[WITCH] Queueing {} migrations via operator decision",
            migrations.len()
        ));

        // Queue via with_operator_decision to get proper witness
        self.with_operator_decision(|scope| {
            for migration in migrations {
                scope.queue_migration(migration);
            }
        });
    }

    // -------------------------------------------------------------------------
    // Database Maintenance (Pre-db_thread, Witness-Guarded)
    // -------------------------------------------------------------------------

    /// Execute VACUUM on the database, guarded by DecisionWitness.
    ///
    /// Runs synchronously before db_thread is spawned. Call only after
    /// operator approval (Enter in the vacuum prompt).
    pub fn execute_vacuum(&mut self, db_path: &std::path::Path) -> anyhow::Result<()> {
        self.with_operator_decision(|_scope| {
            // Witness exists within this scope — operator authorized the action.
            let conn = rusqlite::Connection::open(db_path)?;
            conn.execute_batch("VACUUM")?;
            drop(conn);
            Ok(())
        })
    }

    // -------------------------------------------------------------------------
    // Read-Only Database Access (UI Queries)
    // -------------------------------------------------------------------------

    /// Get a read-only database view for UI queries.
    ///
    /// Returns a `ReadOnlyDb` wrapper that only exposes read methods, providing
    /// compile-time safety that UI code cannot accidentally attempt writes.
    /// The underlying connection also uses `PRAGMA query_only = ON` for runtime
    /// protection.
    ///
    /// The connection is cached for the Witch's lifetime. All UI code should use
    /// this instead of creating direct `Database::open()` connections.
    ///
    /// # Naming Convention
    ///
    /// Variables holding this should be named `read_db` to make the read-only
    /// nature clear in code:
    ///
    /// ```ignore
    /// let read_db = witch.read_db();
    /// let audio_files = read_db.get_all_audio_files(Zone::Corpus)?;
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if database path is not configured or database cannot be opened.
    pub fn read_db(&mut self) -> ReadOnlyDb<'_> {
        if self.read_only_conn.is_none() {
            let db_path = config::get_db_path().expect("Database path not configured");
            let db = Database::open_read_only(&db_path)
                .expect("Failed to open read-only database connection");
            self.read_only_conn = Some(db);
        }
        ReadOnlyDb::new(self.read_only_conn.as_ref().unwrap())
    }

    /// Invalidate the cached read-only connection.
    ///
    /// Call this after schema migrations to ensure the UI sees the updated schema.
    /// The next call to `read_db()` will open a fresh connection.
    pub fn invalidate_read_only_conn(&mut self) {
        self.read_only_conn = None;
    }

    // -------------------------------------------------------------------------
    // Utility Methods
    // -------------------------------------------------------------------------

    /// Get current Witch status snapshot (readonly, does not advance state).
    ///
    /// Safe to call from render code, utility functions, etc.
    /// Use `tick()` only from the main event loop to advance state.
    pub fn status(&self) -> DaemonStatus {
        DaemonStatus {
            state: self.state.into(),
            pending: self.in_flight,
            completed: 0, // Only meaningful from tick() result
            failed: 0,    // Only meaningful from tick() result
            total_processed: self.total_processed,
            session_queued: self.session_queued,
            pending_by_label: self.pending_by_label.clone(),
            idle_rescan_active: self.idle_rescan_active,
        }
    }

    /// Check if there's pending work (tasks queued or in-flight).
    pub fn has_pending(&self) -> bool {
        // Check rayon in-flight tasks
        if self.in_flight > 0 {
            return true;
        }
        // Check db_thread queue (if spawned)
        if let Some(ref handle) = self.db_thread_handle {
            if !handle.queue_empty() {
                return true;
            }
        }
        false
    }

    /// Get current DB thread stats for UI display.
    /// Returns None if db_thread not spawned or timing instrumentation is disabled.
    pub fn db_stats(&self) -> Option<DbThreadStats> {
        self.db_thread_handle.as_ref()?.stats()
    }

    /// Get pending DB write queue depth (always available, no timing guard).
    /// Returns 0 if db_thread not spawned.
    pub fn db_queue_depth(&self) -> u64 {
        self.db_thread_handle
            .as_ref()
            .map(|h| h.queue_depth())
            .unwrap_or(0)
    }

    /// Get current worker performance stats for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn worker_stats(&self) -> Option<WorkerStats> {
        let stats = self.worker_stats_shared.as_ref()?.snapshot();

        // DEBUG: Log if avg > max (should never happen now with isolated stats)
        if stats.tasks_completed > 0 && stats.queue_wait_avg_ms > stats.queue_wait_max_ms {
            crate::logging::log_perf(format!(
                "[PERF BUG] avg > max! tasks={}, avg={}, max={}",
                stats.tasks_completed, stats.queue_wait_avg_ms, stats.queue_wait_max_ms
            ));
        }

        Some(stats)
    }

    /// Get the UI read cache for demand-driven background queries.
    ///
    /// UI components call `want_*()` methods to flag demand, then read
    /// cached values via the corresponding getter methods.
    pub fn ui_read_cache(&self) -> &UiReadCache {
        &self.ui_read_cache
    }
}

impl Drop for Witch {
    fn drop(&mut self) {
        // Shutdown order is critical for SQLite WAL cleanup:
        // 1. Stop UI cache refreshes and wait for in-flight tasks
        // 2. Close thread-local read-only connections on rayon workers
        // 3. Close the Witch's cached read-only connection
        // 4. DB thread checkpoints WAL and closes write connection
        // 5. With all connections closed, SQLite cleans up -wal and -shm files

        // Step 1: Stop UI cache refreshes and wait for in-flight tasks to complete
        // These tasks open ephemeral connections that must close before WAL checkpoint
        ui_read_cache::shutdown_ui_cache();

        // Step 2: Close thread-local read-only connections on all rayon worker threads
        // These are cached per-thread and must be explicitly closed
        rayon::broadcast(|_| {
            crate::meta::computations::close_thread_local_connection();
        });
        crate::logging::log_general("[WITCH] Closed all rayon thread-local DB connections");

        // Step 3: Close the Witch's cached read-only connection
        if self.read_only_conn.take().is_some() {
            crate::logging::log_general("[WITCH] Closed read-only database connection");
        }

        // Step 4: Shut down the DB thread (it will checkpoint and close write connection)
        // Only if db_thread was spawned (may not be if shutdown during migrations)
        if self.db_thread_handle.is_some() {
            crate::db_thread::request_shutdown();
            if let Some(ref mut handle) = self.db_thread_handle {
                handle.join();
            }
        }

        // Step 5: Shut down the logging thread last so all shutdown messages get logged
        crate::logging::request_shutdown();
        if let Some(ref mut handle) = self.log_thread_handle {
            handle.join();
        }
    }
}

// Note: Witch no longer implements Default because it requires Config for PathResolver
