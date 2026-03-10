//! Scheduler + rayon architecture for external metadata fetching.
//!
//! The Witch spawns one scheduler thread that manages queues, rate limiting,
//! dedup, and chain-emit. Actual HTTP calls are dispatched to the Witch's
//! rayon pool as `ExternalFetch` tasks, keeping all background work under
//! the Witch's orchestration.
//!
//! ## Channel Topology
//!
//! ```text
//!                    SchedulerMessage              TaskResult (existing)
//!   Scheduler ────────────────────► Witch ──────────────────► (processes)
//!       ▲                              │                          │
//!       │          FetchOutcome        │  spawns on rayon         │
//!       └──────────────────────────────┘                          │
//!                                      │                          │
//!                                 rayon pool ◄────────────────────┘
//!                                 (HTTP calls)
//! ```
//!
//! Does NOT affect the Witch's work_state — She stays Idle while fetches run.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::SharedConfig;
use crate::db::Database;
use crate::meta::external::ExternalSource;

// ============================================================================
// Public Types
// ============================================================================

/// A single external API call to execute on rayon.
#[derive(Debug, Clone)]
pub enum ExternalFetchTask {
    AcoustId(AcoustIdFetchTask),
    MusicBrainz(MbFetchTask),
}

/// AcoustID fingerprint lookup task.
#[derive(Debug, Clone)]
pub struct AcoustIdFetchTask {
    pub inode: i64,
    pub fingerprint_raw: Vec<u32>,
    pub fingerprint_blob: Vec<u8>,
    pub duration_secs: u32,
    pub api_key: String,
}

/// MusicBrainz entity fetch task.
#[derive(Debug, Clone)]
pub struct MbFetchTask {
    pub kind: MbEntityKind,
    pub mbid: String,
    pub base_url: String,
}

impl ExternalFetchTask {
    /// Human-readable label for status display.
    pub fn label(&self) -> &str {
        match self {
            Self::AcoustId(_) => "AcoustID lookup",
            Self::MusicBrainz(t) => match t.kind {
                MbEntityKind::Recording => "MB recording fetch",
                MbEntityKind::Artist => "MB artist fetch",
                MbEntityKind::Release => "MB release fetch",
            },
        }
    }
}

/// A match row to write to external_matches (recording MBID + confidence only).
pub struct MatchRow {
    pub recording_id: String,
    pub confidence: f64,
}

impl std::fmt::Debug for MatchRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchRow")
            .field("recording_id", &self.recording_id)
            .field("confidence", &self.confidence)
            .finish()
    }
}

/// Per-source progress snapshot.
#[derive(Debug, Clone, Default)]
pub struct SourceProgress {
    pub total: usize,
    pub processed: usize,
    pub matched: usize,
    pub no_match: usize,
    pub retries: usize,
}

/// Combined progress for both sources.
#[derive(Debug, Clone, Default)]
pub struct FetchProgress {
    pub acoustid: SourceProgress,
    pub mb: SourceProgress,
    /// Current effective AcoustID requests/sec (from rate limiter).
    pub acoustid_rps: f32,
    /// Current effective MB requests/sec (from adaptive rate limiter).
    pub mb_rps: f32,
}

/// What kind of MusicBrainz entity to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbEntityKind {
    Recording,
    Artist,
    Release,
}

impl MbEntityKind {
    /// The entity_type string used in mb_known_entities table.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Artist => "artist",
            Self::Release => "release",
        }
    }
}

// ============================================================================
// Internal Types (Scheduler ↔ Witch)
// ============================================================================

/// Message from scheduler to Witch (tasks + status updates).
pub(super) enum SchedulerMessage {
    /// A task for the Witch to execute on rayon.
    TaskRequest {
        task: ExternalFetchTask,
        label: String,
    },
    /// Intermediate progress snapshot.
    Progress(FetchProgress),
    /// One source's queue has drained.
    SourceDone {
        source: ExternalSource,
        stats: SourceProgress,
    },
    /// Both sources done — scheduler going back to sleep.
    AllDone,
}

/// Result of an external fetch task execution.
///
/// Used both as the rayon task result (carried in `TaskResult::fetch_result`)
/// and as the outcome sent from Witch back to scheduler for chain-emit and
/// progress tracking. DB writes already happened on the rayon thread.
#[derive(Debug)]
pub enum FetchOutcome {
    AcoustIdMatch {
        recordings: Vec<MatchRow>,
    },
    AcoustIdNoMatch,
    AcoustIdRateLimited {
        task: ExternalFetchTask,
    },
    AcoustIdError,
    /// MB entity fetched successfully. `discovered_entities` carries newly
    /// discovered artist/release IDs (from recording parsing) for the
    /// scheduler to queue as follow-up fetches.
    MbFound {
        discovered_entities: Vec<(MbEntityKind, String)>,
    },
    MbNotFound,
    MbRateLimited {
        task: ExternalFetchTask,
    },
    MbError,
}

/// Command from Witch to scheduler.
enum FetchCommand {
    /// Scan eligible dirs for inodes needing AcoustID fingerprint lookup.
    /// Also populates MB queue for existing matches needing enrichment.
    Start { eligible_dirs: Vec<PathBuf> },
    /// Shut down the scheduler thread.
    Shutdown,
}

// ============================================================================
// Rate Limiter
// ============================================================================

/// Per-source rate limiter with exponential backoff support.
struct RateLimiter {
    /// Minimum interval between requests.
    base_interval: Duration,
    /// Current backoff multiplier (1 = no backoff).
    backoff_multiplier: u32,
    /// When the last request was issued.
    last_request_at: Option<Instant>,
    /// Maximum backoff multiplier.
    max_backoff: u32,
}

impl RateLimiter {
    fn new_acoustid(requests_per_second: u32) -> Self {
        Self {
            base_interval: Duration::from_millis(1000 / requests_per_second.max(1) as u64),
            backoff_multiplier: 1,
            last_request_at: None,
            max_backoff: 1, // AcoustID uses fixed 2s penalty, not exponential
        }
    }

    /// Duration until this limiter is ready for another request.
    /// Returns `Duration::ZERO` if ready now.
    fn time_until_ready(&self) -> Duration {
        match self.last_request_at {
            None => Duration::ZERO,
            Some(last) => {
                let effective = self.base_interval * self.backoff_multiplier;
                effective.saturating_sub(last.elapsed())
            }
        }
    }

    /// Mark that a request was just dispatched.
    fn mark_request(&mut self) {
        self.last_request_at = Some(Instant::now());
    }

    /// Double the backoff multiplier (capped at max_backoff).
    fn apply_backoff(&mut self) {
        self.backoff_multiplier = (self.backoff_multiplier * 2).min(self.max_backoff);
    }

    /// Reset backoff to normal rate.
    fn reset_backoff(&mut self) {
        self.backoff_multiplier = 1;
    }

    /// Current effective requests per second (accounting for backoff).
    fn effective_rps(&self) -> f64 {
        let effective_micros =
            self.base_interval.as_micros() as f64 * self.backoff_multiplier.max(1) as f64;
        1_000_000.0 / effective_micros
    }
}

/// Initial MB requests per second (conservative start).
const MB_INITIAL_RPS: f64 = 4.0;
/// How many consecutive successes before ramping up by 1 RPS.
const MB_RAMP_SUCCESS_WINDOW: u32 = 20;

/// Adaptive rate limiter for MusicBrainz.
///
/// Starts at a conservative rate (~4 RPS), ramps up toward the configured
/// ceiling after sustained success, and halves on rate-limit responses.
/// Logs every rate adjustment with the RPS at which failure occurred.
struct AdaptiveRateLimiter {
    /// Current interval between requests (1/current_rps).
    current_interval: Duration,
    /// Minimum RPS floor (won't drop below this on backoff).
    min_rps: f64,
    /// Maximum RPS ceiling from config.
    max_rps: f64,
    /// Current effective RPS (tracked as f64 for smooth ramping).
    current_rps: f64,
    /// When the last request was issued.
    last_request_at: Option<Instant>,
    /// Consecutive successes since last failure (for ramp-up gating).
    consecutive_successes: u32,
    /// RPS values at which rate-limit failures occurred (for heuristics).
    failure_rps_history: Vec<f64>,
}

impl AdaptiveRateLimiter {
    fn new(max_rps: u32) -> Self {
        let max = (max_rps.max(1) as f64).max(MB_INITIAL_RPS);
        let initial = MB_INITIAL_RPS.min(max);
        Self {
            current_interval: Self::interval_for_rps(initial),
            min_rps: 1.0,
            max_rps: max,
            current_rps: initial,
            last_request_at: None,
            consecutive_successes: 0,
            failure_rps_history: Vec::new(),
        }
    }

    /// For local mirrors: start at the ceiling immediately, no ramp-up needed.
    fn new_unthrottled(max_rps: u32) -> Self {
        let max = max_rps.max(1) as f64;
        Self {
            current_interval: Self::interval_for_rps(max),
            min_rps: max,
            max_rps: max,
            current_rps: max,
            last_request_at: None,
            consecutive_successes: 0,
            failure_rps_history: Vec::new(),
        }
    }

    fn interval_for_rps(rps: f64) -> Duration {
        Duration::from_micros((1_000_000.0 / rps) as u64)
    }

    /// Duration until this limiter is ready for another request.
    fn time_until_ready(&self) -> Duration {
        match self.last_request_at {
            None => Duration::ZERO,
            Some(last) => self.current_interval.saturating_sub(last.elapsed()),
        }
    }

    /// Mark that a request was just dispatched.
    fn mark_request(&mut self) {
        self.last_request_at = Some(Instant::now());
    }

    /// Record a successful response. After enough consecutive successes,
    /// ramp up rate by ~1 RPS toward the ceiling.
    fn record_success(&mut self) {
        self.consecutive_successes += 1;
        if self.consecutive_successes >= MB_RAMP_SUCCESS_WINDOW && self.current_rps < self.max_rps {
            let old_rps = self.current_rps;
            self.current_rps = (self.current_rps + 1.0).min(self.max_rps);
            self.current_interval = Self::interval_for_rps(self.current_rps);
            self.consecutive_successes = 0;
            crate::logging::log_general(format!(
                "[FETCH] MB rate ramp-up: {:.1} -> {:.1} RPS (after {} clean results)",
                old_rps, self.current_rps, MB_RAMP_SUCCESS_WINDOW,
            ));
        }
    }

    /// Rate-limit hit: halve the current rate, log the failure RPS.
    fn apply_backoff(&mut self) {
        let failed_at = self.current_rps;
        self.failure_rps_history.push(failed_at);
        self.consecutive_successes = 0;

        let new_rps = (self.current_rps / 2.0).max(self.min_rps);
        crate::logging::log_general(format!(
            "[FETCH] MB rate backoff: {:.1} -> {:.1} RPS (rate-limited at {:.1}, \
             failure history: {:?})",
            self.current_rps, new_rps, failed_at, self.failure_rps_history,
        ));
        self.current_rps = new_rps;
        self.current_interval = Self::interval_for_rps(self.current_rps);
    }

    /// Clear last-request timestamp (used when a cache hit skips HTTP).
    fn clear_last_request(&mut self) {
        self.last_request_at = None;
    }

    /// Current effective RPS (for logging).
    fn current_rps(&self) -> f64 {
        self.current_rps
    }
}

// ============================================================================
// ExternalFetchHandle — Witch-side API
// ============================================================================

/// Handle held by the Witch for communicating with the scheduler thread.
pub struct ExternalFetchHandle {
    /// Send commands to scheduler (start, shutdown).
    command_tx: Sender<FetchCommand>,
    /// Receive messages from scheduler (task requests + status).
    message_rx: Receiver<SchedulerMessage>,
    /// Send outcomes back to scheduler (for chain-emit).
    outcome_tx: Sender<FetchOutcome>,
    /// Join handle for the scheduler thread.
    handle: Option<JoinHandle<()>>,
    /// Whether the scheduler is currently active.
    batch_active: bool,
}

impl ExternalFetchHandle {
    /// Spawn the scheduler thread.
    ///
    /// The scheduler sleeps until it receives a Start command, then
    /// dispatches tasks to the Witch via the message channel.
    pub fn spawn(shared_config: SharedConfig) -> Self {
        // Witch → Scheduler: commands
        let (command_tx, command_rx) = mpsc::channel();
        // Scheduler → Witch: task requests + status
        let (message_tx, message_rx) = mpsc::channel();
        // Witch → Scheduler: outcomes for chain-emit
        let (outcome_tx, outcome_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_scheduler(command_rx, message_tx, outcome_rx, shared_config);
        });

        Self {
            command_tx,
            message_rx,
            outcome_tx,
            handle: Some(handle),
            batch_active: false,
        }
    }

    /// Request an external metadata fetch for the given directories.
    ///
    /// Populates both the AcoustID queue (fingerprint lookups) and
    /// the MB queue (recording/artist/release enrichment) upfront.
    pub fn request_fetch(&mut self, eligible_dirs: Vec<PathBuf>) {
        if self.batch_active {
            return; // Don't stack requests
        }
        self.batch_active = true;
        let _ = self.command_tx.send(FetchCommand::Start { eligible_dirs });
    }

    /// Drain available messages from the scheduler (non-blocking).
    ///
    /// Returns messages received since last drain. Also clears batch_active
    /// when AllDone is received.
    pub(super) fn drain_messages(&mut self) -> Vec<SchedulerMessage> {
        let mut msgs = Vec::new();
        while let Ok(msg) = self.message_rx.try_recv() {
            if matches!(msg, SchedulerMessage::AllDone) {
                self.batch_active = false;
            }
            msgs.push(msg);
        }
        msgs
    }

    /// Send an outcome back to the scheduler for chain-emit decisions.
    pub(super) fn send_outcome(&self, outcome: FetchOutcome) {
        let _ = self.outcome_tx.send(outcome);
    }

    /// Whether the scheduler is currently active.
    pub fn is_batch_active(&self) -> bool {
        self.batch_active
    }

}

impl super::types::ManagedThread for ExternalFetchHandle {
    fn send_shutdown(&self) {
        let _ = self.command_tx.send(FetchCommand::Shutdown);
    }

    fn take_handle(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.handle.take()
    }
}

impl Drop for ExternalFetchHandle {
    fn drop(&mut self) {
        use super::types::ManagedThread;
        self.shutdown();
    }
}

// ============================================================================
// Scheduler Thread
// ============================================================================

/// Scheduler main loop. Manages queues, rate limiting, dedup, and chain-emit.
/// Dispatches individual tasks to the Witch via message channel. Never does HTTP.
fn run_scheduler(
    command_rx: Receiver<FetchCommand>,
    message_tx: Sender<SchedulerMessage>,
    outcome_rx: Receiver<FetchOutcome>,
    shared_config: SharedConfig,
) {
    crate::logging::log_general("[FETCH] Scheduler started");

    // Open thread-local read-only DB connection
    let db = match crate::config::get_db_path().and_then(|p| Database::open_read_only(&p)) {
        Ok(db) => db,
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] Failed to open database: {}", e));
            return;
        }
    };

    while let Ok(command) = command_rx.recv() {
        match command {
            FetchCommand::Shutdown => break,
            FetchCommand::Start { eligible_dirs } => {
                let shutdown = run_scheduling_loop(
                    &db,
                    &command_rx,
                    &message_tx,
                    &outcome_rx,
                    &shared_config,
                    eligible_dirs,
                );
                if shutdown {
                    break;
                }
            }
        }
    }

    crate::logging::log_general("[FETCH] Scheduler exiting");
}

/// Active scheduling loop. Dispatches tasks to Witch via message channel,
/// receives outcomes back for chain-emit. Returns `true` if shutdown requested.
fn run_scheduling_loop(
    db: &Database,
    command_rx: &Receiver<FetchCommand>,
    message_tx: &Sender<SchedulerMessage>,
    outcome_rx: &Receiver<FetchOutcome>,
    shared_config: &SharedConfig,
    eligible_dirs: Vec<PathBuf>,
) -> bool {
    // Read config
    let (api_key, rps, mb_rps, mb_base_url, auto_enrich, ttl_secs) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        let em = &config.opinions.external_matching;
        (
            em.acoustid_api_key.clone(),
            em.requests_per_second,
            em.mb_requests_per_second,
            em.mb_base_url.clone(),
            em.auto_enrich_on_match,
            (em.mb_cache_ttl_days as i64) * 86400,
        )
    };

    // Work queues (internal to scheduler — items here haven't been dispatched yet)
    let mut acoustid_queue: VecDeque<AcoustIdQueueItem> = VecDeque::new();
    let mut mb_queue: VecDeque<MbQueueItem> = VecDeque::new();

    // Rate limiters
    let mut acoustid_limiter = RateLimiter::new_acoustid(rps);
    let is_local_mirror = mb_base_url != crate::config::ExternalMatchingConfig::DEFAULT_MB_BASE_URL;
    let mut mb_limiter = if is_local_mirror {
        AdaptiveRateLimiter::new_unthrottled(mb_rps)
    } else {
        AdaptiveRateLimiter::new(mb_rps)
    };

    // Per-source stats
    let mut acoustid_stats = SourceProgress::default();
    let mut mb_stats = SourceProgress::default();

    // In-flight tracking (multiple tasks can be in-flight on rayon concurrently)
    let mut acoustid_in_flight: usize = 0;
    let mut mb_in_flight: usize = 0;

    // Tracking flags
    let mut acoustid_done = false;
    let mut mb_done = false;

    // All MB recording IDs already queued (for chain-emit dedup)
    let mut mb_queued_ids: HashSet<String> = HashSet::new();

    // Populate queues
    if api_key.is_empty() {
        crate::logging::log_general("[FETCH] No AcoustID API key configured, skipping");
        acoustid_done = true;
    } else {
        populate_acoustid_queue(db, &eligible_dirs, &api_key, &mut acoustid_queue);
        acoustid_stats.total = acoustid_queue.len();
    }

    if auto_enrich {
        populate_mb_queue(db, ttl_secs, &mut mb_queue);
        mb_stats.total = mb_queue.len();
        for item in &mb_queue {
            mb_queued_ids.insert(item.mbid.clone());
        }
    } else {
        mb_done = true;
    }

    // Nothing to do at all
    if acoustid_queue.is_empty()
        && mb_queue.is_empty()
        && acoustid_in_flight == 0
        && mb_in_flight == 0
    {
        crate::logging::log_general("[FETCH] No work needed for any source");
        let _ = message_tx.send(SchedulerMessage::AllDone);
        return false;
    }

    crate::logging::log_general(format!(
        "[FETCH] Scheduling: {} AcoustID items, {} MB items, auto_enrich={}, \
         MB rate={:.1}rps (ceiling {}rps)",
        acoustid_queue.len(),
        mb_queue.len(),
        auto_enrich,
        mb_limiter.current_rps(),
        mb_rps,
    ));

    // Send initial progress
    send_progress(
        message_tx,
        &acoustid_stats,
        &mb_stats,
        &acoustid_limiter,
        &mb_limiter,
    );

    loop {
        // ---- 1. Check for new commands / shutdown (non-blocking) ----
        match command_rx.try_recv() {
            Ok(FetchCommand::Shutdown) => {
                crate::logging::log_general("[FETCH] Shutdown received mid-scheduling");
                return true;
            }
            Ok(FetchCommand::Start { eligible_dirs }) => {
                populate_acoustid_queue(db, &eligible_dirs, &api_key, &mut acoustid_queue);
                acoustid_stats.total += acoustid_queue.len();
                acoustid_done = false;
            }
            Err(_) => {} // Empty or Disconnected — both fine
        }

        // ---- 2. Drain outcomes from Witch (non-blocking) ----
        while let Ok(outcome) = outcome_rx.try_recv() {
            match outcome {
                FetchOutcome::AcoustIdMatch { recordings } => {
                    acoustid_in_flight = acoustid_in_flight.saturating_sub(1);
                    // Chain-emit: push matched recording IDs into MB queue
                    if auto_enrich {
                        for row in &recordings {
                            if mb_queued_ids.insert(row.recording_id.clone()) {
                                mb_queue.push_back(MbQueueItem {
                                    kind: MbEntityKind::Recording,
                                    mbid: row.recording_id.clone(),
                                });
                                mb_stats.total += 1;
                                mb_done = false;
                            }
                        }
                    }
                    acoustid_stats.matched += 1;
                    acoustid_stats.processed += 1;
                    acoustid_limiter.reset_backoff();
                }
                FetchOutcome::AcoustIdNoMatch => {
                    acoustid_in_flight = acoustid_in_flight.saturating_sub(1);
                    acoustid_stats.no_match += 1;
                    acoustid_stats.processed += 1;
                    acoustid_limiter.reset_backoff();
                }
                FetchOutcome::AcoustIdRateLimited { task } => {
                    acoustid_in_flight = acoustid_in_flight.saturating_sub(1);
                    crate::logging::log_general("[FETCH] AcoustID rate limited, backing off");
                    acoustid_stats.retries += 1;
                    // Re-queue the original item at front for retry
                    if let ExternalFetchTask::AcoustId(t) = task {
                        acoustid_queue.push_front(AcoustIdQueueItem {
                            inode: t.inode,
                            fingerprint_raw: t.fingerprint_raw,
                            fingerprint_blob: t.fingerprint_blob,
                            duration_secs: t.duration_secs,
                            api_key: t.api_key,
                        });
                    }
                    acoustid_limiter.apply_backoff();
                    acoustid_limiter.mark_request();
                }
                FetchOutcome::AcoustIdError => {
                    acoustid_in_flight = acoustid_in_flight.saturating_sub(1);
                    acoustid_stats.retries += 1;
                    acoustid_stats.processed += 1;
                }
                FetchOutcome::MbFound {
                    discovered_entities,
                } => {
                    mb_in_flight = mb_in_flight.saturating_sub(1);
                    mb_stats.matched += 1;
                    mb_stats.processed += 1;

                    // Chain-emit: queue discovered artist/release entities for fetching.
                    // Entity persistence (mb_known_entities) already happened on the rayon
                    // thread via signal_sender, so even if we shut down now, nothing is lost.
                    for (ek, eid) in discovered_entities {
                        if mb_queued_ids.insert(eid.clone()) {
                            mb_queue.push_back(MbQueueItem {
                                kind: ek,
                                mbid: eid,
                            });
                            mb_stats.total += 1;
                            mb_done = false;
                        }
                    }

                    mb_limiter.record_success();
                }
                FetchOutcome::MbNotFound => {
                    mb_in_flight = mb_in_flight.saturating_sub(1);
                    mb_stats.no_match += 1;
                    mb_stats.processed += 1;
                    mb_limiter.record_success();
                }
                FetchOutcome::MbRateLimited { task } => {
                    mb_in_flight = mb_in_flight.saturating_sub(1);
                    if let ExternalFetchTask::MusicBrainz(ref t) = task {
                        crate::logging::log_general(format!(
                            "[FETCH] MB rate limited for {} {}",
                            t.kind.as_str(),
                            t.mbid,
                        ));
                    }
                    mb_stats.retries += 1;
                    // Re-queue the item at front for retry
                    if let ExternalFetchTask::MusicBrainz(t) = task {
                        mb_queue.push_front(MbQueueItem {
                            kind: t.kind,
                            mbid: t.mbid,
                        });
                    }
                    mb_limiter.apply_backoff();
                    mb_limiter.mark_request();
                }
                FetchOutcome::MbError => {
                    mb_in_flight = mb_in_flight.saturating_sub(1);
                    mb_stats.retries += 1;
                    mb_stats.processed += 1;
                    // Don't re-queue on hard errors — next scan picks it up
                }
            }
            send_progress(
                message_tx,
                &acoustid_stats,
                &mb_stats,
                &acoustid_limiter,
                &mb_limiter,
            );
        }

        // ---- 3. Dispatch AcoustID tasks if rate limiter ready ----
        if !acoustid_queue.is_empty() && acoustid_limiter.time_until_ready() == Duration::ZERO {
            let item = acoustid_queue.pop_front().unwrap();
            acoustid_limiter.mark_request();
            acoustid_in_flight += 1;
            let _ = message_tx.send(SchedulerMessage::TaskRequest {
                task: ExternalFetchTask::AcoustId(AcoustIdFetchTask {
                    inode: item.inode,
                    fingerprint_raw: item.fingerprint_raw,
                    fingerprint_blob: item.fingerprint_blob,
                    duration_secs: item.duration_secs,
                    api_key: item.api_key,
                }),
                label: "AcoustID lookup".to_string(),
            });
        }

        // ---- 4. Dispatch MB tasks if rate limiter ready ----
        if !mb_queue.is_empty() && mb_limiter.time_until_ready() == Duration::ZERO {
            let item = mb_queue.pop_front().unwrap();

            // Check DB cache first — skip if already fresh
            let cache_result = match item.kind {
                MbEntityKind::Recording => db.get_mb_recording_cache(&item.mbid),
                MbEntityKind::Artist => db.get_mb_artist_cache(&item.mbid),
                MbEntityKind::Release => db.get_mb_release_cache(&item.mbid),
            };
            let already_cached = cache_result
                .ok()
                .flatten()
                .map(|(_json, fetched_at)| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    now - fetched_at < ttl_secs
                })
                .unwrap_or(false);

            if already_cached {
                // Already fresh in cache — skip without HTTP call
                mb_stats.total = mb_stats.total.saturating_sub(1);
                // Don't consume rate limiter slot — clear last_request so
                // the next real dispatch isn't delayed by the cache check.
                mb_limiter.clear_last_request();
            } else {
                mb_limiter.mark_request();
                mb_in_flight += 1;
                let label = format!("MB {} fetch", item.kind.as_str());
                let _ = message_tx.send(SchedulerMessage::TaskRequest {
                    task: ExternalFetchTask::MusicBrainz(MbFetchTask {
                        kind: item.kind,
                        mbid: item.mbid,
                        base_url: mb_base_url.clone(),
                    }),
                    label,
                });
            }
        }

        // ---- 5. Check source completion ----
        if !acoustid_done && acoustid_in_flight == 0 && acoustid_queue.is_empty() {
            acoustid_done = true;
            crate::logging::log_general(format!(
                "[FETCH] AcoustID done: {} processed, {} matched, {} no-match, {} retries",
                acoustid_stats.processed,
                acoustid_stats.matched,
                acoustid_stats.no_match,
                acoustid_stats.retries
            ));
            let _ = message_tx.send(SchedulerMessage::SourceDone {
                source: ExternalSource::AcoustID,
                stats: acoustid_stats.clone(),
            });
        }

        if !mb_done && mb_in_flight == 0 && mb_queue.is_empty() {
            mb_done = true;
            crate::logging::log_general(format!(
                "[FETCH] MusicBrainz done: {} processed, {} cached, {} not-found, {} retries, \
                 final rate={:.1}rps, failure-at-rps={:?}",
                mb_stats.processed,
                mb_stats.matched,
                mb_stats.no_match,
                mb_stats.retries,
                mb_limiter.current_rps(),
                mb_limiter.failure_rps_history,
            ));
            let _ = message_tx.send(SchedulerMessage::SourceDone {
                source: ExternalSource::MusicBrainz,
                stats: mb_stats.clone(),
            });
        }

        // ---- 6. Both done? Signal AllDone and return to outer idle loop ----
        if acoustid_done && mb_done {
            let _ = message_tx.send(SchedulerMessage::AllDone);
            return false;
        }

        // ---- 7. Sleep for the shortest relevant interval ----
        let acoustid_wait = if acoustid_queue.is_empty() {
            Duration::from_secs(60) // effectively infinite — nothing to dispatch
        } else {
            acoustid_limiter.time_until_ready()
        };
        let mb_wait = if mb_queue.is_empty() {
            Duration::from_secs(60)
        } else {
            mb_limiter.time_until_ready()
        };
        // Cap at 100ms for shutdown responsiveness and outcome draining
        let sleep_time = acoustid_wait.min(mb_wait).min(Duration::from_millis(100));
        if sleep_time > Duration::ZERO {
            thread::sleep(sleep_time);
        }
    }
}

// ============================================================================
// Internal Queue Item Types (scheduler-internal, not sent over channels)
// ============================================================================

/// AcoustID queue item (scheduler-internal).
struct AcoustIdQueueItem {
    inode: i64,
    fingerprint_raw: Vec<u32>,
    fingerprint_blob: Vec<u8>,
    duration_secs: u32,
    api_key: String,
}

/// MB queue item (scheduler-internal).
struct MbQueueItem {
    kind: MbEntityKind,
    mbid: String,
}

// ============================================================================
// Queue Population Helpers
// ============================================================================

/// Populate the AcoustID work queue from DB (inodes needing lookup + retries).
fn populate_acoustid_queue(
    db: &Database,
    eligible_dirs: &[PathBuf],
    api_key: &str,
    queue: &mut VecDeque<AcoustIdQueueItem>,
) {
    let source_key = ExternalSource::AcoustID.to_key();
    let dir_refs: Vec<&std::path::Path> = eligible_dirs.iter().map(|p| p.as_path()).collect();

    // Fresh lookup candidates
    match db.get_inodes_needing_lookup(source_key, &dir_refs, i64::MAX as usize) {
        Ok(candidates) => {
            for c in candidates {
                let blob = fingerprint_to_blob(&c.fingerprint);
                queue.push_back(AcoustIdQueueItem {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                    api_key: api_key.to_string(),
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query AcoustID lookup candidates: {}",
                e
            ));
        }
    }

    // Retry candidates
    match db.get_retry_candidates(source_key, i64::MAX as usize) {
        Ok(retries) => {
            for c in retries {
                let blob = fingerprint_to_blob(&c.fingerprint);
                queue.push_back(AcoustIdQueueItem {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                    api_key: api_key.to_string(),
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query AcoustID retry candidates: {}",
                e
            ));
        }
    }
}

/// Populate the MB work queue from DB.
///
/// Queries `mb_known_entities` for all entity types that need fetching,
/// plus the legacy `get_recording_ids_needing_mb_fetch` for recordings
/// discovered via external_matches (before mb_known_entities existed).
fn populate_mb_queue(db: &Database, ttl_secs: i64, queue: &mut VecDeque<MbQueueItem>) {
    // Legacy path: recording IDs from external_matches needing cache
    match db.get_recording_ids_needing_mb_fetch(ttl_secs) {
        Ok(ids) => {
            for id in ids {
                queue.push_back(MbQueueItem {
                    kind: MbEntityKind::Recording,
                    mbid: id,
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query MB recording candidates: {}",
                e
            ));
        }
    }

    // Known entities path: recordings, artists, releases from mb_known_entities
    for entity_kind in &[
        MbEntityKind::Recording,
        MbEntityKind::Artist,
        MbEntityKind::Release,
    ] {
        match db.get_known_entities_needing_fetch(entity_kind.as_str(), ttl_secs) {
            Ok(ids) => {
                for id in ids {
                    queue.push_back(MbQueueItem {
                        kind: *entity_kind,
                        mbid: id,
                    });
                }
            }
            Err(e) => {
                crate::logging::log_error(format!(
                    "[FETCH] Failed to query known {} entities: {}",
                    entity_kind.as_str(),
                    e
                ));
            }
        }
    }
}

/// Send a combined progress snapshot for both sources.
fn send_progress(
    message_tx: &Sender<SchedulerMessage>,
    acoustid: &SourceProgress,
    mb: &SourceProgress,
    acoustid_limiter: &RateLimiter,
    mb_limiter: &AdaptiveRateLimiter,
) {
    let _ = message_tx.send(SchedulerMessage::Progress(FetchProgress {
        acoustid: acoustid.clone(),
        mb: mb.clone(),
        acoustid_rps: acoustid_limiter.effective_rps() as f32,
        mb_rps: mb_limiter.current_rps() as f32,
    }));
}

/// Convert fingerprint Vec<u32> to BLOB bytes (little-endian).
fn fingerprint_to_blob(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|n| n.to_le_bytes()).collect()
}

/// Extract artist and release MBIDs from a cached recording JSON response.
///
/// Parses the recording, collects artist IDs from credits and relations,
/// and release IDs from the releases list. Returns None on parse failure.
pub(super) fn extract_entities_from_recording(
    raw_json: &[u8],
    _recording_mbid: &str,
) -> Option<Vec<(MbEntityKind, String)>> {
    let recording = crate::external::musicbrainz::parse_recording(raw_json).ok()?;

    let mut entities = Vec::new();
    let mut seen = HashSet::new();

    // Artist IDs from credits
    for credit in &recording.artist_credit {
        if seen.insert(credit.artist.id.clone()) {
            entities.push((MbEntityKind::Artist, credit.artist.id.clone()));
        }
    }

    // Artist IDs from relations
    for relation in &recording.relations {
        if let Some(ref artist) = relation.artist {
            if seen.insert(artist.id.clone()) {
                entities.push((MbEntityKind::Artist, artist.id.clone()));
            }
        }
    }

    // Release IDs
    for release in &recording.releases {
        if seen.insert(release.id.clone()) {
            entities.push((MbEntityKind::Release, release.id.clone()));
        }
    }

    if entities.is_empty() {
        None
    } else {
        Some(entities)
    }
}
