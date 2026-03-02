//! Autonomous external fetch thread for AcoustID lookups.
//!
//! Managed by the Witch but operates independently: the Witch sends high-level
//! "refresh" requests; the fetch thread owns the full lifecycle of computing
//! what needs querying, rate-limiting, HTTP calls, and emitting results back.
//!
//! Does NOT affect the Witch's work_state — She stays Idle while fetches run.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::SharedConfig;
use crate::db::Database;
use crate::external::acoustid::{AcoustIDClient, LookupOutcome};
use crate::meta::external::ExternalSource;

/// Handle held by the Witch for communicating with the fetch thread.
pub struct ExternalFetchHandle {
    /// Send refresh requests (dirs to scan).
    request_tx: Sender<FetchRequest>,
    /// Receive individual fetch results.
    result_rx: Receiver<FetchResult>,
    /// Thread join handle.
    handle: Option<JoinHandle<()>>,
    /// Whether a batch is currently in progress.
    batch_active: bool,
}

/// Request from Witch to fetch thread.
enum FetchRequest {
    /// Refresh external data for these source dirs.
    Refresh {
        source: ExternalSource,
        eligible_dirs: Vec<PathBuf>,
    },
    /// Shut down the fetch thread.
    Shutdown,
}

/// Snapshot of batch progress, sent after each item to the Witch.
#[derive(Debug, Clone, Default)]
pub struct FetchProgress {
    pub total: usize,
    pub processed: usize,
    pub matched: usize,
    pub no_match: usize,
    pub retries: usize,
}

/// Result from fetch thread back to the Witch.
pub enum FetchResult {
    /// Match(es) found for an inode.
    Matches {
        inode: i64,
        fingerprint: Vec<u8>,
        source: ExternalSource,
        recordings: Vec<MatchRow>,
        raw_response: Option<Vec<u8>>,
    },
    /// No match found for an inode's fingerprint.
    NoMatch {
        inode: i64,
        fingerprint: Vec<u8>,
        source: ExternalSource,
    },
    /// Lookup failed — needs retry.
    NeedsRetry {
        inode: i64,
        fingerprint: Vec<u8>,
        source: ExternalSource,
        error: String,
    },
    /// Intermediate progress snapshot — sent after each item during batch.
    Progress(FetchProgress),
    /// Batch complete — thread going back to sleep.
    BatchDone {
        source: ExternalSource,
        processed: usize,
        matched: usize,
        no_match: usize,
        retries: usize,
    },
}

/// A match row to write to external_matches.
pub struct MatchRow {
    pub recording_id: String,
    pub confidence: f64,
}

/// Work item for the fetch thread's internal queue.
struct WorkItem {
    inode: i64,
    fingerprint_raw: Vec<u32>,
    fingerprint_blob: Vec<u8>,
    duration_secs: u32,
}

impl ExternalFetchHandle {
    /// Spawn the external fetch thread.
    ///
    /// The thread sleeps until it receives a refresh request.
    pub fn spawn(shared_config: SharedConfig) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_fetch_thread(request_rx, result_tx, shared_config);
        });

        Self {
            request_tx,
            result_rx,
            handle: Some(handle),
            batch_active: false,
        }
    }

    /// Send a refresh request to the fetch thread.
    ///
    /// The fetch thread will query the DB for inodes needing lookup in the
    /// given source dirs, then process them at the configured rate limit.
    pub fn request_refresh(&mut self, source: ExternalSource, eligible_dirs: Vec<PathBuf>) {
        if self.batch_active {
            return; // Don't stack requests
        }
        self.batch_active = true;
        let _ = self.request_tx.send(FetchRequest::Refresh {
            source,
            eligible_dirs,
        });
    }

    /// Drain available results (non-blocking).
    ///
    /// Returns results received since last drain. The Witch calls this
    /// each tick() to process fetch results.
    pub fn drain_results(&mut self) -> Vec<FetchResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            if matches!(result, FetchResult::BatchDone { .. }) {
                self.batch_active = false;
            }
            results.push(result);
        }
        results
    }

    /// Whether a batch is currently in progress.
    pub fn is_batch_active(&self) -> bool {
        self.batch_active
    }

    /// Shut down the fetch thread.
    pub fn shutdown(&mut self) {
        let _ = self.request_tx.send(FetchRequest::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ExternalFetchHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Main loop for the fetch thread.
fn run_fetch_thread(
    request_rx: Receiver<FetchRequest>,
    result_tx: Sender<FetchResult>,
    shared_config: SharedConfig,
) {
    crate::logging::log_general("[FETCH] External fetch thread started");

    // Open thread-local read-only DB connection
    let db = match crate::config::get_db_path().and_then(|p| Database::open_read_only(&p)) {
        Ok(db) => db,
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] Failed to open database: {}", e));
            return;
        }
    };

    loop {
        // Sleep until we get a request
        let request = match request_rx.recv() {
            Ok(req) => req,
            Err(_) => break, // Channel closed
        };

        match request {
            FetchRequest::Shutdown => {
                crate::logging::log_general("[FETCH] Shutdown requested");
                break;
            }
            FetchRequest::Refresh { source, eligible_dirs } => {
                let shutdown = process_refresh(&db, &request_rx, &result_tx, &shared_config, source, &eligible_dirs);
                if shutdown {
                    crate::logging::log_general("[FETCH] Shutdown during batch, exiting");
                    break;
                }
            }
        }
    }

    crate::logging::log_general("[FETCH] External fetch thread exiting");
}

/// Process a single refresh request: query DB, build work queue, HTTP calls.
///
/// Checks `request_rx` between items so Shutdown is respected mid-batch.
/// Returns `true` if a Shutdown was received (caller should exit the thread).
fn process_refresh(
    db: &Database,
    request_rx: &Receiver<FetchRequest>,
    result_tx: &Sender<FetchResult>,
    shared_config: &SharedConfig,
    source: ExternalSource,
    eligible_dirs: &[PathBuf],
) -> bool {
    // Read config for API key and rate limit
    let (api_key, requests_per_second) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        let em = &config.opinions.external_matching;
        (em.acoustid_api_key.clone(), em.requests_per_second)
    };

    if api_key.is_empty() {
        crate::logging::log_general("[FETCH] No API key configured, skipping refresh");
        let _ = result_tx.send(FetchResult::BatchDone {
            source,
            processed: 0,
            matched: 0,
            no_match: 0,
            retries: 0,
        });
        return false;
    }

    // Build work queue from DB
    let dir_refs: Vec<&std::path::Path> = eligible_dirs.iter().map(|p| p.as_path()).collect();

    let mut work_queue: VecDeque<WorkItem> = VecDeque::new();

    // Get all candidates — rate limiter is the real throttle, not batch size
    match db.get_inodes_needing_lookup(source.to_key(), &dir_refs, i64::MAX as usize) {
        Ok(candidates) => {
            for c in candidates {
                let blob = fingerprint_to_blob(&c.fingerprint);
                work_queue.push_back(WorkItem {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] Failed to query lookup candidates: {}", e));
        }
    }

    // Get retry candidates
    match db.get_retry_candidates(source.to_key(), i64::MAX as usize) {
        Ok(retries) => {
            for c in retries {
                let blob = fingerprint_to_blob(&c.fingerprint);
                work_queue.push_back(WorkItem {
                    inode: c.inode,
                    fingerprint_raw: c.fingerprint,
                    fingerprint_blob: blob,
                    duration_secs: c.duration_secs,
                });
            }
        }
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] Failed to query retry candidates: {}", e));
        }
    }

    if work_queue.is_empty() {
        crate::logging::log_general("[FETCH] No inodes need external lookup");
        let _ = result_tx.send(FetchResult::BatchDone {
            source,
            processed: 0,
            matched: 0,
            no_match: 0,
            retries: 0,
        });
        return false;
    }

    crate::logging::log_general(format!(
        "[FETCH] Processing {} items for {} at {}/sec",
        work_queue.len(), source.name(), requests_per_second
    ));

    let client = AcoustIDClient::new(api_key);
    let rate_interval = Duration::from_millis(1000 / requests_per_second.max(1) as u64);
    let total_items = work_queue.len();

    let mut processed = 0usize;
    let mut matched = 0usize;
    let mut no_match_count = 0usize;
    let mut retry_count = 0usize;

    // Send initial progress so the UI immediately shows queue size
    let _ = result_tx.send(FetchResult::Progress(FetchProgress {
        total: total_items,
        processed: 0,
        matched: 0,
        no_match: 0,
        retries: 0,
    }));

    let mut shutdown_requested = false;

    while let Some(item) = work_queue.pop_front() {
        // Check for shutdown between items so exit is prompt
        if let Ok(FetchRequest::Shutdown) = request_rx.try_recv() {
            crate::logging::log_general(format!(
                "[FETCH] Shutdown received mid-batch at {}/{}, stopping gracefully",
                processed, total_items
            ));
            shutdown_requested = true;
            break;
        }

        let start = Instant::now();

        match client.lookup_with_raw(&item.fingerprint_raw, item.duration_secs) {
            Ok((LookupOutcome::Matches(recordings), raw)) => {
                let rows: Vec<MatchRow> = recordings
                    .into_iter()
                    .map(|r| MatchRow {
                        recording_id: r.recording_id,
                        confidence: r.confidence,
                    })
                    .collect();
                matched += 1;
                let _ = result_tx.send(FetchResult::Matches {
                    inode: item.inode,
                    fingerprint: item.fingerprint_blob,
                    source,
                    recordings: rows,
                    raw_response: raw,
                });
            }
            Ok((LookupOutcome::NoMatch, _)) => {
                no_match_count += 1;
                let _ = result_tx.send(FetchResult::NoMatch {
                    inode: item.inode,
                    fingerprint: item.fingerprint_blob,
                    source,
                });
            }
            Ok((LookupOutcome::RateLimited, _)) => {
                crate::logging::log_general("[FETCH] Rate limited by AcoustID, backing off 2s");
                retry_count += 1;
                let _ = result_tx.send(FetchResult::NeedsRetry {
                    inode: item.inode,
                    fingerprint: item.fingerprint_blob,
                    source,
                    error: "Rate limited".to_string(),
                });
                // Back off extra on rate limit
                thread::sleep(Duration::from_secs(2));
            }
            Err(e) => {
                let error_msg = format!("{:#}", e);
                crate::logging::log_error(format!(
                    "[FETCH] Lookup failed for inode {}: {}",
                    item.inode, error_msg
                ));
                retry_count += 1;
                let _ = result_tx.send(FetchResult::NeedsRetry {
                    inode: item.inode,
                    fingerprint: item.fingerprint_blob,
                    source,
                    error: error_msg,
                });
            }
        }

        processed += 1;

        let _ = result_tx.send(FetchResult::Progress(FetchProgress {
            total: total_items,
            processed,
            matched,
            no_match: no_match_count,
            retries: retry_count,
        }));

        // Rate limit: sleep for remainder of interval
        let elapsed = start.elapsed();
        if elapsed < rate_interval {
            thread::sleep(rate_interval - elapsed);
        }
    }

    crate::logging::log_general(format!(
        "[FETCH] Batch done: {} processed, {} matched, {} no-match, {} retries",
        processed, matched, no_match_count, retry_count
    ));

    let _ = result_tx.send(FetchResult::BatchDone {
        source,
        processed,
        matched,
        no_match: no_match_count,
        retries: retry_count,
    });

    shutdown_requested
}

/// Convert fingerprint Vec<u32> to BLOB bytes (little-endian).
fn fingerprint_to_blob(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|n| n.to_le_bytes()).collect()
}
