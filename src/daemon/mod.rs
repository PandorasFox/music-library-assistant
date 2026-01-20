//! Task Daemon - Background task execution system
//!
//! ## Module Organization
//! - `types.rs` - Core types, enums, witness system
//! - `worker_stats.rs` - Thread-safe performance statistics
//! - `transaction.rs` - Transaction lifecycle management
//! - `execution.rs` - Task execution (mutations, computations, migrations)
//!
//! ## Extending the Daemon
//! - New task types: Add variants to `Task` enum in `types.rs`
//! - New execution logic: Add to `execution.rs`
//! - Transaction features: Modify `transaction.rs`
//! - Performance tracking: Modify `worker_stats.rs`

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use std::sync::mpsc::{self, Receiver, Sender};

use crate::config;
use crate::corpus::computations::Computation;
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;
use crate::db_thread::{self, DbThreadHandle, DbThreadStats};

// Module declarations
mod execution;
mod transaction;
mod types;
mod worker_stats;

// Re-export public types
#[allow(unused_imports)] // Re-exports for public API
pub use types::{
    confirm_decision, confirm_startup_migration, CommitSummary, CompletedSession,
    CorpusObservationState, DaemonState, DaemonStateSnapshot, DaemonStatus, DecisionWitness,
    DiscardSummary, EyeState, Migration, MigrationWitness, MutationExecutionWitness,
    PendingTransaction, Task, TaskLabel, TransactionError, TransactionInfo, WitnessedDecision,
    WorkerStats,
};

// Internal imports
use execution::execute_task;
use types::TaskResult;
use worker_stats::SharedWorkerStats;

// ============================================================================
// Task Daemon
// ============================================================================

/// Simple parallel task queue with state machine.
pub struct TaskDaemon {
    // Rayon-based task execution with channel for results
    result_tx: Sender<TaskResult>,
    result_rx: Receiver<TaskResult>,

    /// When this daemon instance was created.
    /// Used for signal freshness tracking (`discovered_at > launch_time` = new signal).
    launch_time: DateTime<Utc>,

    // State machine
    state: DaemonState,

    // Eye and corpus observation state
    eye_state: EyeState,
    observation_state: CorpusObservationState,

    /// Whether mutations are accepted. Only becomes true when eyeballing completes,
    /// and only if read_only_mode opinion is false. Never reverts to false.
    accepting_mutations: bool,

    /// When true, mutations are permanently disabled (read-only debug mode).
    read_only_mode: bool,

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
    current_label: Option<String>,

    // Completed session for lingering display
    completed_session: Option<CompletedSession>,
    completed_at: Option<Instant>,

    // Transaction state
    pending_transaction: Option<PendingTransaction>,

    /// Cached read-only database connection for UI queries.
    /// Only accessed from the main thread via `read_only_db()`.
    /// UI code should use this instead of creating direct connections.
    read_only_conn: Option<Database>,

    /// Handle to the dedicated DB write thread.
    /// Provides stats access and shutdown coordination.
    db_thread_handle: DbThreadHandle,

    // -------------------------------------------------------------------------
    // Worker Performance Stats (Thread-Safe, Isolated)
    // -------------------------------------------------------------------------

    /// Thread-safe worker stats in separate heap allocation.
    /// None when timing instrumentation is disabled.
    worker_stats_shared: Option<Arc<SharedWorkerStats>>,
}

impl TaskDaemon {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        // Use rayon's global thread pool with work-stealing for better performance.
        // Thread count from config (default: 2x logical cores for I/O-bound workloads).
        let num_threads = config::get_worker_thread_count();

        // Configure rayon's global thread pool (only first call takes effect)
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global();

        let _ = config::log_message(&format!(
            "[WORKER] Using rayon thread pool with {} threads",
            num_threads
        ));

        // Channel for receiving task results
        let (result_tx, result_rx) = mpsc::channel();

        // Spawn the dedicated DB write thread
        let db_thread_handle = db_thread::spawn();

        // Create isolated worker stats only when timing instrumentation is enabled
        let worker_stats_shared = if config::is_timing_enabled() {
            Some(Arc::new(SharedWorkerStats::new()))
        } else {
            None
        };

        let daemon = Self {
            result_tx,
            result_rx,
            launch_time: Utc::now(),
            state: DaemonState::Idle,
            eye_state: EyeState::Closed,
            observation_state: CorpusObservationState::Unseen,
            accepting_mutations: false,
            read_only_mode: false,
            session_start: None,
            session_queued: 0,
            total_processed: 0,
            total_failed: 0,
            in_flight: 0,
            recent_errors: VecDeque::with_capacity(5),
            task_counts: HashMap::new(),
            current_label: None,
            completed_session: None,
            completed_at: None,
            pending_transaction: None,
            read_only_conn: None,
            db_thread_handle,
            worker_stats_shared,
        };

        // DEBUG: Verify initialization (only when timing enabled)
        if let Some(ref stats) = daemon.worker_stats_shared {
            let (_tasks, total_qw, max_qw) = stats.debug_values();
            let _ = config::log_message(&format!(
                "[PERF INIT] TaskDaemon::new() - total_queue_wait_ms={}, max_queue_wait_ms={}, total_processed={}",
                total_qw, max_qw, daemon.total_processed
            ));
        }

        daemon
    }

    /// Create a new daemon with opinions applied.
    pub fn with_opinions(read_only_mode: bool) -> Self {
        let mut daemon = Self::new();
        daemon.read_only_mode = read_only_mode;
        daemon
    }

    // -------------------------------------------------------------------------
    // State Machine API
    // -------------------------------------------------------------------------

    /// O(1) state check - returns current daemon state.
    pub fn state(&self) -> DaemonState {
        self.state
    }

    /// Get the timestamp when this daemon was created.
    ///
    /// Used for signal freshness tracking: signals with `discovered_at > launch_time`
    /// are new this session.
    pub fn launch_time(&self) -> DateTime<Utc> {
        self.launch_time
    }

    // -------------------------------------------------------------------------
    // Eye and Observation State
    // -------------------------------------------------------------------------

    /// Get current eye state for UI rendering decisions.
    pub fn eye_state(&self) -> EyeState {
        self.eye_state
    }

    /// Get current corpus observation state.
    pub fn observation_state(&self) -> CorpusObservationState {
        self.observation_state
    }

    /// Check if daemon is accepting mutations.
    ///
    /// Only becomes true after first eyeballing completes, and only if
    /// read_only_mode is false. Never reverts to false.
    pub fn is_accepting_mutations(&self) -> bool {
        self.accepting_mutations
    }

    /// Check if read-only mode is enabled.
    pub fn is_read_only(&self) -> bool {
        self.read_only_mode
    }

    /// Check if eyeballing is currently in progress (Lazy or Paranoid).
    pub fn is_eyeballing(&self) -> bool {
        matches!(
            self.observation_state,
            CorpusObservationState::Lazy | CorpusObservationState::Paranoid
        )
    }

    /// Start lazy eyeballing. Returns false if eyeballing already in progress.
    ///
    /// Queues WalkCorpus computations for corpus and optional legacy library.
    pub fn start_lazy_eyeball(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
    ) -> bool {
        if self.is_eyeballing() {
            return false;
        }

        self.observation_state = CorpusObservationState::Lazy;
        self.queue_eyeballing_computations(corpus_root, legacy, false);
        true
    }

    /// Start paranoid eyeballing. Returns false if eyeballing already in progress.
    ///
    /// Queues WalkCorpus computations with paranoid=true flag.
    pub fn start_paranoid_eyeball(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
    ) -> bool {
        if self.is_eyeballing() {
            return false;
        }

        self.observation_state = CorpusObservationState::Paranoid;
        self.queue_eyeballing_computations(corpus_root, legacy, true);
        true
    }

    /// Queue eyeballing computations (internal helper).
    fn queue_eyeballing_computations(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
        paranoid: bool,
    ) {
        // Queue corpus walk
        self.queue_computation_with_label(
            Computation::WalkCorpus {
                root: corpus_root.to_path_buf(),
                source: "corpus".to_string(),
                paranoid,
            },
            Some("Eyeballing corpus".to_string()),
        );

        // Queue legacy library walk if configured
        if let Some(legacy_path) = legacy {
            self.queue_computation_with_label(
                Computation::WalkCorpus {
                    root: legacy_path.to_path_buf(),
                    source: "legacy".to_string(),
                    paranoid,
                },
                Some("Eyeballing legacy".to_string()),
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
        // DEBUG: Log first tick state (only when timing enabled)
        if let Some(ref stats) = self.worker_stats_shared {
            let (_, total_qw_before, _) = stats.debug_values();
            if self.total_processed == 0 && total_qw_before != 0 {
                let _ = config::log_message(&format!(
                    "[PERF BUG] tick() called with total_processed=0 but total_queue_wait_ms={}!",
                    total_qw_before
                ));
            }
        }

        let mut completed = 0;
        let mut failed = 0;

        // Drain completed results and collect spawned computations
        let mut spawned: Vec<Computation> = Vec::new();

        while let Ok(result) = self.result_rx.try_recv() {
            // Task has completed - no longer in flight
            self.in_flight = self.in_flight.saturating_sub(1);
            self.total_processed += 1;

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;

            // Record stats via thread-safe interface (only when timing enabled)
            if let Some(ref stats) = self.worker_stats_shared {
                stats.record_result(&result);

                // DEBUG: Log queue wait values
                let (_, total_qw_now, max_qw_now) = stats.debug_values();
                if self.total_processed == 1 {
                    let _ = config::log_message(&format!(
                        "[PERF FIRST] FIRST TASK: queue_wait_ms={}, total_queue_wait_ms={} (should equal queue_wait_ms!), max_queue_wait_ms={}",
                        result.queue_wait_ms, total_qw_now, max_qw_now
                    ));
                }
                if self.total_processed <= 50 || self.total_processed % 500 == 0 || result.queue_wait_ms > 50000 {
                    let _ = config::log_message(&format!(
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

            // Collect spawned follow-up computations
            spawned.extend(result.spawn);
        }

        // Queue spawned follow-up computations (chaining)
        // IMPORTANT: This happens BEFORE we check in_flight for state transitions,
        // ensuring spawned tasks are counted before we decide to transition.
        for comp in spawned {
            self.queue_computation_internal(comp, None);
        }

        // Capture current state for status
        let current_in_flight = self.in_flight;
        let current_total_processed = self.total_processed;
        let current_session_queued = self.session_queued;
        let current_task_counts = self.task_counts.clone();
        let current_errors = self.recent_errors.iter().cloned().collect();
        let elapsed = self.session_start.map(|start| start.elapsed());

        // State machine transitions (uses self.in_flight internally)
        self.update_state();

        DaemonStatus {
            state: self.state.into(),
            pending: current_in_flight,
            completed,
            failed,
            total_processed: current_total_processed,
            session_queued: current_session_queued,
            recent_errors: current_errors,
            task_counts: current_task_counts,
            elapsed,
            completed_session: self.completed_session.clone(),
        }
    }

    /// Update state machine based on in-flight tasks and timing.
    fn update_state(&mut self) {
        match self.state {
            DaemonState::Idle => {
                // Idle → Working: handled in queue methods
            }
            DaemonState::Working => {
                // Working → Completed: when all work finishes (no in-flight tasks AND
                // db_thread queue empty). Uses centralized has_pending() for consistency
                // with exit handlers and UI state display.
                if !self.has_pending() && self.total_processed > 0 {
                    self.transition_to_completed();
                }
            }
            DaemonState::Completed => {
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
        let duration = self.session_start.map(|s| s.elapsed()).unwrap_or_default();

        // Flag to queue awakening computations after session reset
        let mut queue_awakening_after_reset = false;

        self.completed_session = Some(CompletedSession {
            completed_at: Instant::now(),
            duration,
            total_processed: self.total_processed,
            failed: self.total_failed,
            task_counts: self.task_counts.clone(),
        });

        self.completed_at = Some(Instant::now());
        self.state = DaemonState::Completed;

        // Handle eyeballing completion (first-level computations)
        if self.is_eyeballing() {
            self.observation_state = CorpusObservationState::Complete;

            match self.eye_state {
                EyeState::Closed => {
                    // Eyeballing complete - enter Awakening for directory derivations
                    let _ = config::log_message(&format!(
                        "[STATE] Eyeballing complete. Transitioning Closed → Awakening. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    self.eye_state = EyeState::Awakening;
                    // Flag to queue awakening computations AFTER session reset
                    // (avoids off-by-one: task queued before reset, completed after)
                    queue_awakening_after_reset = true;
                }
                EyeState::Awake => {
                    let _ = config::log_message(
                        "[STATE] Re-eyeballing complete while Awake. NOP."
                    );
                }
                EyeState::Awakening => {
                    // Invalid: can't complete eyeballing while already awakening
                    panic!("Invalid state: eyeballing completed while eye is Awakening");
                }
            }
        }
        // Handle awakening completion - transition directly to Awake
        // ContentAnalysis is triggered separately via queue_content_analysis() after intake
        else if self.eye_state == EyeState::Awakening {
            let _ = config::log_message(&format!(
                "[STATE] Awakening complete. Transitioning Awakening → Awake. \
                 Processed {} tasks.",
                self.total_processed
            ));
            self.eye_state = EyeState::Awake;

            // Enable mutations - ONE-TIME transition from read-only init to read-write operation
            if !self.read_only_mode {
                self.accepting_mutations = true;
                let _ = config::log_message(
                    "[STATE] Mutations now enabled (read-write mode)."
                );
            }
        }

        // Reset session state
        self.session_start = None;
        self.total_processed = 0;
        self.total_failed = 0;
        self.session_queued = 0;
        self.task_counts.clear();
        self.recent_errors.clear();
        self.current_label = None;

        // Queue awakening computations AFTER reset to fix off-by-one counting
        // (if queued before reset, the task's queue count gets wiped but it still completes)
        if queue_awakening_after_reset {
            self.queue_awakening_computations();
        }
    }

    /// Queue second-level signal computations during Awakening.
    ///
    /// This queues `ScheduleSecondLevelDerivations` which will spawn per-directory
    /// computations to derive signals like UnindexedFile, MissingFile, etc.
    fn queue_awakening_computations(&mut self) {
        let _ = config::log_message(
            "[STATE] Queueing ScheduleSecondLevelDerivations for Awakening"
        );

        // Queue the orchestrator computation that will spawn per-directory derivations
        self.queue_computation_with_label(
            Computation::ScheduleSecondLevelDerivations,
            Some("Computing directory signals".to_string()),
        );
    }

    /// Queue content analysis computations.
    ///
    /// This queues `ScheduleContentAnalysis` which will spawn bulk detection
    /// computations for FingerprintDuplicate, MissingTag, etc.
    ///
    /// Called from UI after intake flow completes (Eye is already Awake).
    pub fn queue_content_analysis(&mut self) {
        let _ = config::log_message(
            "[STATE] Queueing ScheduleContentAnalysis for content analysis"
        );

        // Queue the orchestrator computation that will spawn all detection computations
        self.queue_computation_with_label(
            Computation::ScheduleContentAnalysis,
            Some("Analyzing metadata".to_string()),
        );
    }

    fn transition_to_idle(&mut self) {
        self.state = DaemonState::Idle;
        self.completed_session = None;
        self.completed_at = None;
    }

    fn transition_to_working(&mut self) {
        if self.session_start.is_none() {
            self.session_start = Some(Instant::now());
        }
        self.state = DaemonState::Working;
    }

    // -------------------------------------------------------------------------
    // Task Spawning Helper
    // -------------------------------------------------------------------------

    /// Spawn a task on the rayon thread pool with panic catching.
    ///
    /// If the task panics, we still send a failure result so the daemon's
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
                        duration_ms: queue_time.elapsed().as_millis() as u64,
                        queue_wait_ms: 0,
                        thread_stats: None,
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

    pub(crate) fn queue_mutation_internal(&mut self, mutation: Mutation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Mutation(mutation);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    pub(crate) fn queue_mutations_internal(&mut self, mutations: impl IntoIterator<Item = Mutation>, label: Option<String>) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let mutations: Vec<_> = mutations.into_iter().collect();

        let _ = config::log_message(&format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(), label
        ));

        self.session_queued += mutations.len();
        self.in_flight += mutations.len();

        for mutation in mutations {
            let task = Task::Mutation(mutation);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Computation Queueing (no witness required)
    // -------------------------------------------------------------------------

    /// Queue a single computation (no witness required).
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    pub fn queue_computation(&mut self, computation: Computation) {
        self.queue_computation_with_label(computation, None);
    }

    /// Queue a single computation with an explicit label (no witness required).
    pub fn queue_computation_with_label(&mut self, computation: Computation, label: Option<String>) {
        self.queue_computation_internal(computation, label);
    }

    fn queue_computation_internal(&mut self, computation: Computation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Computation(computation);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    /// Queue multiple computations (no witness required).
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    pub fn queue_computations(&mut self, computations: impl IntoIterator<Item = Computation>) {
        self.queue_computations_with_label(computations, None);
    }

    /// Queue multiple computations with an explicit label (no witness required).
    pub fn queue_computations_with_label(&mut self, computations: impl IntoIterator<Item = Computation>, label: Option<String>) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let computations: Vec<_> = computations.into_iter().collect();

        self.session_queued += computations.len();
        self.in_flight += computations.len();

        for computation in computations {
            let task = Task::Computation(computation);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Migration Queueing (requires witness, bypasses accepting_mutations)
    // -------------------------------------------------------------------------

    /// Queue a single migration for execution.
    ///
    /// Migrations require a [`DecisionWitness`] (user approval) but bypass the
    /// `accepting_mutations` gate. They can run before eyeballing completes.
    pub fn queue_migration(&mut self, migration: Migration, _witness: &DecisionWitness) {
        self.queue_migration_internal(migration, None);
    }

    /// Queue a single migration with an explicit label.
    pub fn queue_migration_with_label(
        &mut self,
        migration: Migration,
        label: Option<String>,
        _witness: &DecisionWitness,
    ) {
        self.queue_migration_internal(migration, label);
    }

    /// Queue multiple migrations for execution.
    pub fn queue_migrations(
        &mut self,
        migrations: impl IntoIterator<Item = Migration>,
        _witness: &DecisionWitness,
    ) {
        self.queue_migrations_internal(migrations, None);
    }

    fn queue_migration_internal(&mut self, migration: Migration, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Migration(migration);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    fn queue_migrations_internal(
        &mut self,
        migrations: impl IntoIterator<Item = Migration>,
        label: Option<String>,
    ) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let migrations: Vec<_> = migrations.into_iter().collect();

        self.session_queued += migrations.len();
        self.in_flight += migrations.len();

        for migration in migrations {
            let task = Task::Migration(migration);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Read-Only Database Access (UI Queries)
    // -------------------------------------------------------------------------

    /// Get a read-only database connection for UI queries.
    ///
    /// This connection is cached for the daemon's lifetime. All UI code
    /// should use this instead of creating direct `Database::open()` connections.
    ///
    /// The connection uses `PRAGMA query_only = ON` to prevent any writes,
    /// ensuring UI code cannot accidentally mutate the database.
    ///
    /// # Panics
    ///
    /// Panics if database path is not configured or database cannot be opened.
    pub fn read_only_db(&mut self) -> &Database {
        if self.read_only_conn.is_none() {
            let db_path = config::get_db_path().expect("Database path not configured");
            let db = Database::open_read_only(&db_path)
                .expect("Failed to open read-only database connection");
            self.read_only_conn = Some(db);
        }
        self.read_only_conn.as_ref().unwrap()
    }

    /// Invalidate the cached read-only connection.
    ///
    /// Call this after schema migrations to ensure the UI sees the updated schema.
    /// The next call to `read_only_db()` will open a fresh connection.
    pub fn invalidate_read_only_conn(&mut self) {
        self.read_only_conn = None;
    }

    // -------------------------------------------------------------------------
    // Utility Methods
    // -------------------------------------------------------------------------

    /// Get current daemon status snapshot (readonly, does not advance state).
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
            recent_errors: self.recent_errors.iter().cloned().collect(),
            task_counts: self.task_counts.clone(),
            elapsed: self.session_start.map(|start| start.elapsed()),
            completed_session: self.completed_session.clone(),
        }
    }

    /// Check if there's pending work (tasks queued or in-flight).
    pub fn has_pending(&self) -> bool {
        self.in_flight > 0 || !self.db_thread_handle.queue_empty()
    }

    /// Get current DB thread stats for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn db_stats(&self) -> Option<DbThreadStats> {
        self.db_thread_handle.stats()
    }

    /// Get pending DB write queue depth (always available, no timing guard).
    pub fn db_queue_depth(&self) -> u64 {
        self.db_thread_handle.queue_depth()
    }

    /// Get current worker performance stats for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn worker_stats(&self) -> Option<WorkerStats> {
        let stats = self.worker_stats_shared.as_ref()?.snapshot();

        // DEBUG: Log if avg > max (should never happen now with isolated stats)
        if stats.tasks_completed > 0 && stats.queue_wait_avg_ms > stats.queue_wait_max_ms {
            let _ = config::log_message(&format!(
                "[PERF BUG] avg > max! tasks={}, avg={}, max={}",
                stats.tasks_completed, stats.queue_wait_avg_ms, stats.queue_wait_max_ms
            ));
        }

        Some(stats)
    }

    /// Check if there's a lingering completed session to display.
    pub fn has_completed_session(&self) -> bool {
        self.state == DaemonState::Completed
    }

    /// Cancel all pending tasks.
    ///
    /// Note: With rayon, spawned tasks will still complete but their results
    /// are drained and discarded. The in_flight counter is reset to 0.
    pub fn cancel(&mut self) {
        // Drain any pending results (discard them)
        while self.result_rx.try_recv().is_ok() {}
        self.in_flight = 0;
    }
}

impl Default for TaskDaemon {
    fn default() -> Self {
        Self::new()
    }
}
