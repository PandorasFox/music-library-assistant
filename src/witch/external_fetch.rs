//! Coordinator + worker architecture for external metadata fetching.
//!
//! The Witch spawns three threads:
//! - **Coordinator**: manages scheduling, rate limiting, queues, dedup, and
//!   chain-emit. Does NO HTTP calls. Ticks on its own schedule.
//! - **AcoustID worker**: receives individual tasks, makes blocking HTTP calls,
//!   returns results. No rate limiting or state.
//! - **MusicBrainz worker**: same pattern, different backend.
//!
//! True parallelism: AcoustID and MB HTTP calls happen concurrently on
//! different threads while the coordinator manages scheduling centrally.
//!
//! ## Channel Topology
//!
//! ```text
//!                   FetchRequest                FetchResult
//!   Witch ───────────────────► Coordinator ──────────────────► Witch
//!                                  │   ▲           │   ▲
//!                   AcoustIdTask   │   │  MbTask   │   │
//!                                  ▼   │           ▼   │
//!                           AcoustID  (result)  MB Worker
//!                            Worker              (result)
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
// Public Types (Coordinator ↔ Witch)
// ============================================================================

/// Handle held by the Witch for communicating with the fetch subsystem.
pub struct ExternalFetchHandle {
    /// Send requests to the coordinator thread.
    request_tx: Sender<FetchRequest>,
    /// Receive results from the coordinator thread.
    result_rx: Receiver<FetchResult>,
    /// Join handles for all three threads.
    handles: Option<FetchThreadHandles>,
    /// Whether the coordinator is currently active.
    batch_active: bool,
}

struct FetchThreadHandles {
    coordinator: JoinHandle<()>,
    acoustid_worker: JoinHandle<()>,
    mb_worker: JoinHandle<()>,
}

/// A match row to write to external_matches (recording MBID + confidence only).
pub struct MatchRow {
    pub recording_id: String,
    pub confidence: f64,
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
}

/// Result from coordinator back to the Witch.
pub enum FetchResult {
    // --- AcoustID results ---
    /// AcoustID match(es) found for an inode (recording MBIDs + confidence only).
    AcoustIdMatch {
        inode: i64,
        fingerprint: Vec<u8>,
        recordings: Vec<MatchRow>,
    },
    /// No AcoustID match for this fingerprint.
    AcoustIdNoMatch {
        inode: i64,
        fingerprint: Vec<u8>,
    },
    /// AcoustID lookup failed — needs retry.
    AcoustIdRetry {
        inode: i64,
        fingerprint: Vec<u8>,
        error: String,
    },

    // --- MusicBrainz results ---
    /// MB entity data fetched and ready to cache (recording, artist, or release).
    MbEntityCached {
        kind: MbEntityKind,
        mbid: String,
        raw_json: Vec<u8>,
    },
    /// MB entity not found (404).
    MbEntityNotFound {
        kind: MbEntityKind,
        mbid: String,
    },
    /// MB entity fetch failed (transient).
    MbEntityRetry {
        kind: MbEntityKind,
        mbid: String,
        error: String,
    },
    /// Newly discovered MB entity IDs (artist/release from recording parsing).
    /// Witch persists these to mb_known_entities for resumable fetching.
    MbEntitiesDiscovered {
        /// (kind, mbid, discovered_from recording_id)
        entities: Vec<(MbEntityKind, String, Option<String>)>,
    },

    // --- Scheduler-level ---
    /// Intermediate progress snapshot.
    Progress(FetchProgress),
    /// One source's queue has drained.
    SourceDone {
        source: ExternalSource,
        stats: SourceProgress,
    },
    /// Both sources done — coordinator going back to sleep.
    AllDone,
}

// ============================================================================
// Request Types (Witch → Coordinator)
// ============================================================================

/// Request from Witch to coordinator thread.
enum FetchRequest {
    /// Scan eligible dirs for inodes needing AcoustID fingerprint lookup.
    /// Also populates MB queue for existing matches needing enrichment.
    AcoustIdScan { eligible_dirs: Vec<PathBuf> },
    /// Shut down the fetch subsystem.
    Shutdown,
}

// ============================================================================
// Worker Task/Result Types (Coordinator ↔ Workers)
// ============================================================================

/// Task sent to the AcoustID worker. One HTTP call per task.
struct AcoustIdTask {
    inode: i64,
    fingerprint_raw: Vec<u32>,
    fingerprint_blob: Vec<u8>,
    duration_secs: u32,
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

/// Task sent to the MB worker. One HTTP call per task.
struct MbTask {
    kind: MbEntityKind,
    mbid: String,
}

/// Result from the AcoustID worker for one lookup.
enum AcoustIdWorkerResult {
    /// Matches found.
    Match {
        inode: i64,
        fingerprint_blob: Vec<u8>,
        recordings: Vec<MatchRow>,
    },
    /// No match for this fingerprint.
    NoMatch {
        inode: i64,
        fingerprint_blob: Vec<u8>,
    },
    /// Rate limited (429). Returns task for coordinator to re-queue.
    RateLimited {
        task: AcoustIdTask,
    },
    /// Hard error (network failure, bad response). Don't re-queue.
    Error {
        inode: i64,
        fingerprint_blob: Vec<u8>,
        error: String,
    },
}

/// Result from the MB worker for one fetch.
enum MbWorkerResult {
    /// Entity found, raw JSON for cache.
    Found {
        kind: MbEntityKind,
        mbid: String,
        raw_json: Vec<u8>,
    },
    /// Entity does not exist (404).
    NotFound {
        kind: MbEntityKind,
        mbid: String,
    },
    /// Rate limited (429) or service unavailable (503). Returns task for re-queue.
    RateLimited {
        task: MbTask,
    },
    /// Hard error. Don't re-queue.
    Error {
        kind: MbEntityKind,
        mbid: String,
        error: String,
    },
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

    fn new_mb() -> Self {
        Self {
            base_interval: Duration::from_secs(1), // MB enforces 1 req/sec
            backoff_multiplier: 1,
            last_request_at: None,
            max_backoff: 32, // Exponential: 1→2→4→8→16→32s
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
}

// ============================================================================
// ExternalFetchHandle — Witch-side API
// ============================================================================

impl ExternalFetchHandle {
    /// Spawn the coordinator and worker threads.
    ///
    /// All three threads sleep until they receive work. The coordinator
    /// blocks on `recv()` when idle; workers block on their task channels.
    pub fn spawn(shared_config: SharedConfig) -> Self {
        // Witch ↔ Coordinator channels
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        // Coordinator ↔ AcoustID Worker channels
        let (acoustid_task_tx, acoustid_task_rx) = mpsc::channel::<Option<AcoustIdTask>>();
        let (acoustid_result_tx, acoustid_result_rx) = mpsc::channel::<AcoustIdWorkerResult>();

        // Coordinator ↔ MB Worker channels
        let (mb_task_tx, mb_task_rx) = mpsc::channel::<Option<MbTask>>();
        let (mb_result_tx, mb_result_rx) = mpsc::channel::<MbWorkerResult>();

        // Read API key for AcoustID worker
        let api_key = {
            let config = shared_config.read().expect("SharedConfig lock poisoned");
            config.opinions.external_matching.acoustid_api_key.clone()
        };

        // Spawn AcoustID worker
        let acoustid_worker = thread::spawn(move || {
            run_acoustid_worker(acoustid_task_rx, acoustid_result_tx, api_key);
        });

        // Spawn MB worker
        let mb_worker = thread::spawn(move || {
            run_mb_worker(mb_task_rx, mb_result_tx);
        });

        // Spawn coordinator
        let coord_config = shared_config;
        let coordinator = thread::spawn(move || {
            run_coordinator(
                request_rx, result_tx,
                acoustid_task_tx, acoustid_result_rx,
                mb_task_tx, mb_result_rx,
                coord_config,
            );
        });

        Self {
            request_tx,
            result_rx,
            handles: Some(FetchThreadHandles {
                coordinator,
                acoustid_worker,
                mb_worker,
            }),
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
        let _ = self.request_tx.send(FetchRequest::AcoustIdScan { eligible_dirs });
    }

    /// Drain available results (non-blocking).
    ///
    /// Returns results received since last drain. The Witch calls this
    /// each tick() to process fetch results.
    pub fn drain_results(&mut self) -> Vec<FetchResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            if matches!(result, FetchResult::AllDone) {
                self.batch_active = false;
            }
            results.push(result);
        }
        results
    }

    /// Whether the coordinator is currently active.
    pub fn is_batch_active(&self) -> bool {
        self.batch_active
    }

    /// Shut down all threads (coordinator + workers).
    pub fn shutdown(&mut self) {
        let _ = self.request_tx.send(FetchRequest::Shutdown);
        if let Some(handles) = self.handles.take() {
            let _ = handles.coordinator.join();
            let _ = handles.acoustid_worker.join();
            let _ = handles.mb_worker.join();
        }
    }
}

impl Drop for ExternalFetchHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// AcoustID Worker Thread
// ============================================================================

/// Simple worker loop: receive task → HTTP call → send result. No state.
fn run_acoustid_worker(
    task_rx: Receiver<Option<AcoustIdTask>>,
    result_tx: Sender<AcoustIdWorkerResult>,
    api_key: String,
) {
    use crate::external::acoustid::{AcoustIDClient, LookupOutcome};

    crate::logging::log_general("[FETCH] AcoustID worker started");
    let client = AcoustIDClient::new(api_key);

    loop {
        let task = match task_rx.recv() {
            Ok(Some(task)) => task,
            Ok(None) => break, // Shutdown signal
            Err(_) => break,   // Channel closed
        };

        let result = match client.lookup_with_raw(&task.fingerprint_raw, task.duration_secs) {
            Ok((LookupOutcome::Matches(recordings), _raw)) => {
                // _raw intentionally discarded — MB data supersedes AcoustID metadata
                let rows: Vec<MatchRow> = recordings
                    .into_iter()
                    .map(|r| MatchRow {
                        recording_id: r.recording_id,
                        confidence: r.confidence,
                    })
                    .collect();
                AcoustIdWorkerResult::Match {
                    inode: task.inode,
                    fingerprint_blob: task.fingerprint_blob,
                    recordings: rows,
                }
            }
            Ok((LookupOutcome::NoMatch, _)) => {
                AcoustIdWorkerResult::NoMatch {
                    inode: task.inode,
                    fingerprint_blob: task.fingerprint_blob,
                }
            }
            Ok((LookupOutcome::RateLimited, _)) => {
                AcoustIdWorkerResult::RateLimited { task }
            }
            Err(e) => {
                AcoustIdWorkerResult::Error {
                    inode: task.inode,
                    fingerprint_blob: task.fingerprint_blob,
                    error: format!("{:#}", e),
                }
            }
        };

        let _ = result_tx.send(result);
    }

    crate::logging::log_general("[FETCH] AcoustID worker exiting");
}

// ============================================================================
// MusicBrainz Worker Thread
// ============================================================================

/// Simple worker loop: receive task → HTTP call → send result. No state.
fn run_mb_worker(
    task_rx: Receiver<Option<MbTask>>,
    result_tx: Sender<MbWorkerResult>,
) {
    use crate::external::musicbrainz::{MusicBrainzClient, MbLookupOutcome};

    crate::logging::log_general("[FETCH] MusicBrainz worker started");
    let client = MusicBrainzClient::new();

    loop {
        let task = match task_rx.recv() {
            Ok(Some(task)) => task,
            Ok(None) => break,
            Err(_) => break,
        };

        let fetch_result = match task.kind {
            MbEntityKind::Recording => client.fetch_recording(&task.mbid),
            MbEntityKind::Artist => client.fetch_artist(&task.mbid),
            MbEntityKind::Release => client.fetch_release(&task.mbid),
        };

        let kind = task.kind;
        let result = match fetch_result {
            Ok(MbLookupOutcome::Found(json)) => {
                MbWorkerResult::Found {
                    kind,
                    mbid: task.mbid,
                    raw_json: json,
                }
            }
            Ok(MbLookupOutcome::NotFound) => {
                MbWorkerResult::NotFound {
                    kind,
                    mbid: task.mbid,
                }
            }
            Ok(MbLookupOutcome::RateLimited) | Ok(MbLookupOutcome::ServiceUnavailable) => {
                MbWorkerResult::RateLimited { task }
            }
            Err(e) => {
                MbWorkerResult::Error {
                    kind,
                    mbid: task.mbid,
                    error: format!("{:#}", e),
                }
            }
        };

        let _ = result_tx.send(result);
    }

    crate::logging::log_general("[FETCH] MusicBrainz worker exiting");
}

// ============================================================================
// Coordinator Thread
// ============================================================================

/// Coordinator main loop. Manages scheduling, rate limiting, queues,
/// dedup, and chain-emit. Dispatches individual tasks to workers and
/// reads back results each tick. Never does HTTP.
fn run_coordinator(
    request_rx: Receiver<FetchRequest>,
    result_tx: Sender<FetchResult>,
    acoustid_task_tx: Sender<Option<AcoustIdTask>>,
    acoustid_result_rx: Receiver<AcoustIdWorkerResult>,
    mb_task_tx: Sender<Option<MbTask>>,
    mb_result_rx: Receiver<MbWorkerResult>,
    shared_config: SharedConfig,
) {
    crate::logging::log_general("[FETCH] Coordinator started");

    // Open thread-local read-only DB connection
    let db = match crate::config::get_db_path().and_then(|p| Database::open_read_only(&p)) {
        Ok(db) => db,
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] Failed to open database: {}", e));
            shutdown_workers(&acoustid_task_tx, &mb_task_tx);
            return;
        }
    };

    loop {
        // Block until we get a request (idle state)
        let request = match request_rx.recv() {
            Ok(req) => req,
            Err(_) => break, // Channel closed
        };

        match request {
            FetchRequest::Shutdown => break,
            work_request => {
                let shutdown = run_scheduling_loop(
                    &db, &request_rx, &result_tx,
                    &acoustid_task_tx, &acoustid_result_rx,
                    &mb_task_tx, &mb_result_rx,
                    &shared_config, work_request,
                );
                if shutdown {
                    break;
                }
            }
        }
    }

    shutdown_workers(&acoustid_task_tx, &mb_task_tx);
    crate::logging::log_general("[FETCH] Coordinator exiting");
}

fn shutdown_workers(
    acoustid_tx: &Sender<Option<AcoustIdTask>>,
    mb_tx: &Sender<Option<MbTask>>,
) {
    let _ = acoustid_tx.send(None);
    let _ = mb_tx.send(None);
}

/// Active scheduling loop. Dispatches tasks to workers, drains results,
/// manages rate limiting and chain-emit. Returns `true` if shutdown requested.
#[allow(clippy::too_many_arguments)]
fn run_scheduling_loop(
    db: &Database,
    request_rx: &Receiver<FetchRequest>,
    result_tx: &Sender<FetchResult>,
    acoustid_task_tx: &Sender<Option<AcoustIdTask>>,
    acoustid_result_rx: &Receiver<AcoustIdWorkerResult>,
    mb_task_tx: &Sender<Option<MbTask>>,
    mb_result_rx: &Receiver<MbWorkerResult>,
    shared_config: &SharedConfig,
    initial_request: FetchRequest,
) -> bool {
    // Read config
    let (api_key, rps, auto_enrich, ttl_secs, max_candidates) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        let em = &config.opinions.external_matching;
        (
            em.acoustid_api_key.clone(),
            em.requests_per_second,
            em.auto_enrich_on_match,
            (em.mb_cache_ttl_days as i64) * 86400,
            em.mb_max_candidates,
        )
    };

    // Work queues
    let mut acoustid_queue: VecDeque<AcoustIdTask> = VecDeque::new();
    let mut mb_queue: VecDeque<MbTask> = VecDeque::new();

    // Rate limiters (coordinator-owned, workers have none)
    let mut acoustid_limiter = RateLimiter::new_acoustid(rps);
    let mut mb_limiter = RateLimiter::new_mb();

    // Per-source stats
    let mut acoustid_stats = SourceProgress::default();
    let mut mb_stats = SourceProgress::default();

    // In-flight tracking: at most 1 task per worker
    let mut acoustid_in_flight = false;
    let mut mb_in_flight = false;

    // Tracking flags
    let mut acoustid_done = false;
    let mut mb_done = false;

    // All MB recording IDs already queued (for chain-emit dedup)
    let mut mb_queued_ids: HashSet<String> = HashSet::new();

    // Populate queues from the triggering request
    match initial_request {
        FetchRequest::AcoustIdScan { ref eligible_dirs } => {
            if api_key.is_empty() {
                crate::logging::log_general("[FETCH] No AcoustID API key configured, skipping");
                acoustid_done = true;
            } else {
                populate_acoustid_queue(db, eligible_dirs, &mut acoustid_queue);
                acoustid_stats.total = acoustid_queue.len();
            }
            // Populate MB queue upfront from existing matches needing enrichment
            if auto_enrich {
                populate_mb_queue(db, ttl_secs, max_candidates, &mut mb_queue);
                mb_stats.total = mb_queue.len();
                // Seed dedup set from pre-populated MB items
                for item in &mb_queue {
                    mb_queued_ids.insert(item.mbid.clone());
                }
            } else {
                mb_done = true;
            }
        }
        FetchRequest::Shutdown => return true,
    };

    // Nothing to do at all
    if acoustid_queue.is_empty() && mb_queue.is_empty() {
        crate::logging::log_general("[FETCH] No work needed for any source");
        let _ = result_tx.send(FetchResult::AllDone);
        return false;
    }

    crate::logging::log_general(format!(
        "[FETCH] Scheduling: {} AcoustID items, {} MB items, auto_enrich={}",
        acoustid_queue.len(), mb_queue.len(), auto_enrich,
    ));

    // Send initial progress
    send_progress(result_tx, &acoustid_stats, &mb_stats);

    loop {
        // ---- 1. Check for new requests / shutdown (non-blocking) ----
        match request_rx.try_recv() {
            Ok(FetchRequest::Shutdown) => {
                crate::logging::log_general("[FETCH] Shutdown received mid-scheduling");
                return true;
            }
            Ok(FetchRequest::AcoustIdScan { eligible_dirs }) => {
                populate_acoustid_queue(db, &eligible_dirs, &mut acoustid_queue);
                acoustid_stats.total += acoustid_queue.len();
                acoustid_done = false;
            }
            Err(_) => {} // Empty or Disconnected — both fine
        }

        // ---- 2. Drain AcoustID worker results (non-blocking) ----
        while let Ok(result) = acoustid_result_rx.try_recv() {
            acoustid_in_flight = false;
            match result {
                AcoustIdWorkerResult::Match { inode, fingerprint_blob, recordings } => {
                    // Chain-emit: push matched recording IDs into MB queue
                    if auto_enrich {
                        for row in &recordings {
                            if mb_queued_ids.insert(row.recording_id.clone()) {
                                mb_queue.push_back(MbTask {
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
                    let _ = result_tx.send(FetchResult::AcoustIdMatch {
                        inode,
                        fingerprint: fingerprint_blob,
                        recordings,
                    });
                    acoustid_limiter.reset_backoff();
                }
                AcoustIdWorkerResult::NoMatch { inode, fingerprint_blob } => {
                    acoustid_stats.no_match += 1;
                    acoustid_stats.processed += 1;
                    let _ = result_tx.send(FetchResult::AcoustIdNoMatch {
                        inode,
                        fingerprint: fingerprint_blob,
                    });
                    acoustid_limiter.reset_backoff();
                }
                AcoustIdWorkerResult::RateLimited { task } => {
                    crate::logging::log_general("[FETCH] AcoustID rate limited, backing off");
                    acoustid_stats.retries += 1;
                    // Re-queue at front for retry after backoff
                    acoustid_queue.push_front(task);
                    acoustid_limiter.apply_backoff();
                    // Mark the limiter as having just fired so backoff interval applies
                    acoustid_limiter.mark_request();
                }
                AcoustIdWorkerResult::Error { inode, fingerprint_blob, error } => {
                    crate::logging::log_error(format!(
                        "[FETCH] AcoustID lookup failed for inode {}: {}", inode, error
                    ));
                    acoustid_stats.retries += 1;
                    acoustid_stats.processed += 1;
                    let _ = result_tx.send(FetchResult::AcoustIdRetry {
                        inode,
                        fingerprint: fingerprint_blob,
                        error,
                    });
                }
            }
            send_progress(result_tx, &acoustid_stats, &mb_stats);
        }

        // ---- 3. Drain MB worker results (non-blocking) ----
        while let Ok(result) = mb_result_rx.try_recv() {
            mb_in_flight = false;
            match result {
                MbWorkerResult::Found { kind, mbid, raw_json } => {
                    mb_stats.matched += 1;
                    mb_stats.processed += 1;

                    // Chain-emit: when a Recording is fetched, parse it to discover
                    // artist and release IDs, queue them for fetching too.
                    if kind == MbEntityKind::Recording {
                        if let Some(discovered) = extract_entities_from_recording(&raw_json, &mbid) {
                            // Queue newly discovered entities
                            for &(ek, ref eid) in &discovered {
                                if mb_queued_ids.insert(eid.clone()) {
                                    mb_queue.push_back(MbTask { kind: ek, mbid: eid.clone() });
                                    mb_stats.total += 1;
                                    mb_done = false;
                                }
                            }
                            // Tell Witch about discovered entities for DB persistence
                            let entities: Vec<_> = discovered.into_iter()
                                .map(|(ek, eid)| (ek, eid, Some(mbid.clone())))
                                .collect();
                            if !entities.is_empty() {
                                let _ = result_tx.send(FetchResult::MbEntitiesDiscovered { entities });
                            }
                        }
                    }

                    let _ = result_tx.send(FetchResult::MbEntityCached {
                        kind,
                        mbid,
                        raw_json,
                    });
                    mb_limiter.reset_backoff();
                }
                MbWorkerResult::NotFound { kind, mbid } => {
                    mb_stats.no_match += 1;
                    mb_stats.processed += 1;
                    let _ = result_tx.send(FetchResult::MbEntityNotFound {
                        kind,
                        mbid,
                    });
                    mb_limiter.reset_backoff();
                }
                MbWorkerResult::RateLimited { task } => {
                    let backoff_secs = mb_limiter.base_interval.as_secs()
                        * mb_limiter.backoff_multiplier as u64 * 2;
                    crate::logging::log_general(format!(
                        "[FETCH] MB rate limited for {} {}, backoff {}s",
                        task.kind.as_str(), task.mbid, backoff_secs
                    ));
                    mb_stats.retries += 1;
                    let _ = result_tx.send(FetchResult::MbEntityRetry {
                        kind: task.kind,
                        mbid: task.mbid.clone(),
                        error: format!("Rate limited (backoff {}s)", backoff_secs),
                    });
                    mb_queue.push_front(task);
                    mb_limiter.apply_backoff();
                    mb_limiter.mark_request();
                }
                MbWorkerResult::Error { kind, mbid, error } => {
                    crate::logging::log_error(format!(
                        "[FETCH] MB fetch failed for {} {}: {}", kind.as_str(), mbid, error
                    ));
                    mb_stats.retries += 1;
                    mb_stats.processed += 1;
                    let _ = result_tx.send(FetchResult::MbEntityRetry {
                        kind,
                        mbid,
                        error,
                    });
                    // Don't re-queue on hard errors — next scan picks it up
                }
            }
            send_progress(result_tx, &acoustid_stats, &mb_stats);
        }

        // ---- 4. Dispatch to workers if rate limiter ready AND worker idle ----
        if !acoustid_in_flight
            && !acoustid_queue.is_empty()
            && acoustid_limiter.time_until_ready() == Duration::ZERO
        {
            let task = acoustid_queue.pop_front().unwrap();
            acoustid_limiter.mark_request();
            acoustid_in_flight = true;
            let _ = acoustid_task_tx.send(Some(task));
        }

        if !mb_in_flight
            && !mb_queue.is_empty()
            && mb_limiter.time_until_ready() == Duration::ZERO
        {
            let task = mb_queue.pop_front().unwrap();

            // Check DB cache first — skip if already fresh
            let cache_result = match task.kind {
                MbEntityKind::Recording => db.get_mb_recording_cache(&task.mbid),
                MbEntityKind::Artist => db.get_mb_artist_cache(&task.mbid),
                MbEntityKind::Release => db.get_mb_release_cache(&task.mbid),
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
                // Don't consume rate limiter slot
                mb_limiter.last_request_at = None;
                // Don't set mb_in_flight — immediately eligible for next item
            } else {
                mb_limiter.mark_request();
                mb_in_flight = true;
                let _ = mb_task_tx.send(Some(task));
            }
        }

        // ---- 5. Check source completion ----
        if !acoustid_done && !acoustid_in_flight && acoustid_queue.is_empty() {
            acoustid_done = true;
            crate::logging::log_general(format!(
                "[FETCH] AcoustID done: {} processed, {} matched, {} no-match, {} retries",
                acoustid_stats.processed, acoustid_stats.matched,
                acoustid_stats.no_match, acoustid_stats.retries
            ));
            let _ = result_tx.send(FetchResult::SourceDone {
                source: ExternalSource::AcoustID,
                stats: acoustid_stats.clone(),
            });
        }

        if !mb_done && !mb_in_flight && mb_queue.is_empty() {
            mb_done = true;
            crate::logging::log_general(format!(
                "[FETCH] MusicBrainz done: {} processed, {} cached, {} not-found, {} retries",
                mb_stats.processed, mb_stats.matched,
                mb_stats.no_match, mb_stats.retries
            ));
            let _ = result_tx.send(FetchResult::SourceDone {
                source: ExternalSource::MusicBrainz,
                stats: mb_stats.clone(),
            });
        }

        // ---- 6. Both done? Signal AllDone and return to outer idle loop ----
        if acoustid_done && mb_done {
            let _ = result_tx.send(FetchResult::AllDone);
            return false;
        }

        // ---- 7. Sleep for the shortest relevant interval ----
        let acoustid_wait = if acoustid_queue.is_empty() || acoustid_in_flight {
            Duration::from_secs(60) // effectively infinite — nothing to dispatch
        } else {
            acoustid_limiter.time_until_ready()
        };
        let mb_wait = if mb_queue.is_empty() || mb_in_flight {
            Duration::from_secs(60)
        } else {
            mb_limiter.time_until_ready()
        };
        // Cap at 100ms for shutdown responsiveness and result draining
        let sleep_time = acoustid_wait.min(mb_wait).min(Duration::from_millis(100));
        if sleep_time > Duration::ZERO {
            thread::sleep(sleep_time);
        }
    }
}

// ============================================================================
// Queue Population Helpers
// ============================================================================

/// Populate the AcoustID work queue from DB (inodes needing lookup + retries).
fn populate_acoustid_queue(
    db: &Database,
    eligible_dirs: &[PathBuf],
    queue: &mut VecDeque<AcoustIdTask>,
) {
    let source_key = ExternalSource::AcoustID.to_key();
    let dir_refs: Vec<&std::path::Path> = eligible_dirs.iter().map(|p| p.as_path()).collect();

    // Fresh lookup candidates
    match db.get_inodes_needing_lookup(source_key, &dir_refs, i64::MAX as usize) {
        Ok(candidates) => {
            for c in candidates {
                let blob = fingerprint_to_blob(&c.fingerprint);
                queue.push_back(AcoustIdTask {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query AcoustID lookup candidates: {}", e
            ));
        }
    }

    // Retry candidates
    match db.get_retry_candidates(source_key, i64::MAX as usize) {
        Ok(retries) => {
            for c in retries {
                let blob = fingerprint_to_blob(&c.fingerprint);
                queue.push_back(AcoustIdTask {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query AcoustID retry candidates: {}", e
            ));
        }
    }
}

/// Populate the MB work queue from DB.
///
/// Queries `mb_known_entities` for all entity types that need fetching,
/// plus the legacy `get_recording_ids_needing_mb_fetch` for recordings
/// discovered via external_matches (before mb_known_entities existed).
fn populate_mb_queue(
    db: &Database,
    ttl_secs: i64,
    max_candidates: u32,
    queue: &mut VecDeque<MbTask>,
) {
    // Legacy path: recording IDs from external_matches needing cache
    match db.get_recording_ids_needing_mb_fetch(ttl_secs, max_candidates) {
        Ok(ids) => {
            for id in ids {
                queue.push_back(MbTask { kind: MbEntityKind::Recording, mbid: id });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] Failed to query MB recording candidates: {}", e
            ));
        }
    }

    // Known entities path: recordings, artists, releases from mb_known_entities
    for entity_kind in &[MbEntityKind::Recording, MbEntityKind::Artist, MbEntityKind::Release] {
        match db.get_known_entities_needing_fetch(entity_kind.as_str(), ttl_secs) {
            Ok(ids) => {
                for id in ids {
                    queue.push_back(MbTask { kind: *entity_kind, mbid: id });
                }
            }
            Err(e) => {
                crate::logging::log_error(format!(
                    "[FETCH] Failed to query known {} entities: {}", entity_kind.as_str(), e
                ));
            }
        }
    }
}

/// Send a combined progress snapshot for both sources.
fn send_progress(
    result_tx: &Sender<FetchResult>,
    acoustid: &SourceProgress,
    mb: &SourceProgress,
) {
    let _ = result_tx.send(FetchResult::Progress(FetchProgress {
        acoustid: acoustid.clone(),
        mb: mb.clone(),
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
fn extract_entities_from_recording(
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
