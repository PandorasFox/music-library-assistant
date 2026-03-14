//! Scheduler thread for external metadata fetching.
//!
//! The Witch spawns one scheduler thread that runs a single-threaded tokio
//! runtime with an event-driven `select!` loop. Commands arrive via tokio
//! channel, a periodic tick evaluates rate limiters and dispatches ready work
//! as async HTTP calls into a `JoinSet`, and results flow back naturally.
//!
//! ## Channel Topology
//!
//! ```text
//!                    SchedulerMessage
//!   Scheduler ────────────────────► Witch
//!       ▲                              │
//!       │          FetchCommand        │
//!       └──────────────────────────────┘
//! ```
//!
//! Does NOT affect the Witch's work_state — She stays Idle while fetches run.

mod handle;
mod rate_limiter;
mod types;

// Re-export everything that was previously pub or pub(super)
pub use handle::ExternalFetchHandle;
pub use types::{
    FetchProgress, MbEntityKind, MatchRow, SourceProgress,
};
pub(super) use types::SchedulerMessage;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

use crate::config::SharedConfig;
use crate::db::write_thread;
use crate::db::Database;
use crate::external::acoustid::{AcoustIDClient, LookupOutcome};
use crate::external::musicbrainz::{MbLookupOutcome, MusicBrainzClient};
use crate::meta::external::ExternalSource;

use rate_limiter::{AdaptiveRateLimiter, RateLimiter};
use types::FetchCommand;

// ============================================================================
// Scheduler Thread
// ============================================================================

/// Scheduler entry point. Creates a single-threaded tokio runtime and
/// runs the async event loop on the existing std::thread.
fn run_scheduler(
    command_rx: UnboundedReceiver<FetchCommand>,
    message_tx: UnboundedSender<SchedulerMessage>,
    shared_config: SharedConfig,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build scheduler tokio runtime");
    rt.block_on(scheduler_loop(command_rx, message_tx, shared_config));
}

/// Async event loop: select! over commands, tick, and in-flight HTTP results.
async fn scheduler_loop(
    mut command_rx: UnboundedReceiver<FetchCommand>,
    message_tx: UnboundedSender<SchedulerMessage>,
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

    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut in_flight: JoinSet<FetchResult> = JoinSet::new();
    let mut batch: Option<BatchState> = None;

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Some(FetchCommand::Start { eligible_dirs }) => {
                        match batch.as_mut() {
                            Some(b) => {
                                // Extend existing batch with new AcoustID work
                                extend_acoustid_queue(&db, &shared_config, &eligible_dirs, b);
                            }
                            None => {
                                // Initialize new batch
                                batch = Some(init_batch(&db, &shared_config, eligible_dirs));
                                let b = batch.as_ref().unwrap();

                                if b.acoustid_queue.is_empty() && b.mb_queue.is_empty() {
                                    crate::logging::log_general("[FETCH] No work needed for any source");
                                    let _ = message_tx.send(SchedulerMessage::AllDone);
                                    batch = None;
                                    continue;
                                }

                                crate::logging::log_general(format!(
                                    "[FETCH] Scheduling: {} AcoustID items, {} MB items, auto_enrich={}, \
                                     MB rate={:.1}rps (ceiling {}rps)",
                                    b.acoustid_queue.len(),
                                    b.mb_queue.len(),
                                    b.auto_enrich,
                                    b.mb_limiter.current_rps(),
                                    b.mb_rps_ceiling,
                                ));

                                send_progress(&message_tx, b);
                            }
                        }
                    }
                    Some(FetchCommand::Shutdown) | None => break,
                }
            }

            _ = tick.tick(), if batch.is_some() => {
                let b = batch.as_mut().unwrap();

                // Dispatch AcoustID if limiter ready and queue non-empty
                if !b.acoustid_queue.is_empty()
                    && b.acoustid_limiter.time_until_ready() == Duration::ZERO
                {
                    let item = b.acoustid_queue.pop_front().unwrap();
                    b.acoustid_limiter.mark_request();
                    b.acoustid_in_flight += 1;
                    let client = b.acoustid_client.clone();
                    in_flight.spawn(async move {
                        let outcome = execute_acoustid_lookup(
                            &client,
                            &item.fingerprint_raw,
                            item.duration_secs,
                        ).await;
                        FetchResult::AcoustId { outcome, item }
                    });
                }

                // Dispatch MB if limiter ready and queue non-empty
                if !b.mb_queue.is_empty()
                    && b.mb_limiter.time_until_ready() == Duration::ZERO
                {
                    let item = b.mb_queue.pop_front().unwrap();

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
                            let now = now_unix();
                            now - fetched_at < b.ttl_secs
                        })
                        .unwrap_or(false);

                    if already_cached {
                        b.mb_stats.total = b.mb_stats.total.saturating_sub(1);
                        b.mb_limiter.clear_last_request();
                    } else {
                        b.mb_limiter.mark_request();
                        b.mb_in_flight += 1;
                        let client = b.mb_client.clone();
                        let kind = item.kind;
                        let mbid = item.mbid.clone();
                        in_flight.spawn(async move {
                            let outcome = execute_mb_fetch(&client, kind, &mbid).await;
                            FetchResult::Mb { outcome, item }
                        });
                    }
                }
            }

            Some(result) = in_flight.join_next(), if !in_flight.is_empty() => {
                let Ok(result) = result else { continue };
                let b = batch.as_mut().unwrap();

                match result {
                    FetchResult::AcoustId { outcome, item } => {
                        b.acoustid_in_flight -= 1;
                        match outcome {
                            AcoustIdOutcome::Match(recordings) => {
                                // Write matches to DB
                                write_acoustid_matches(item.inode, &item.fingerprint_blob, &recordings);

                                // Chain-emit: push matched recording IDs into MB queue
                                if b.auto_enrich {
                                    for row in &recordings {
                                        if b.mb_queued_ids.insert(row.recording_id.clone()) {
                                            b.mb_queue.push_back(MbQueueItem {
                                                kind: MbEntityKind::Recording,
                                                mbid: row.recording_id.clone(),
                                            });
                                            b.mb_stats.total += 1;
                                            b.mb_done = false;
                                        }
                                    }
                                }
                                b.acoustid_stats.matched += 1;
                                b.acoustid_stats.processed += 1;
                                b.acoustid_limiter.reset_backoff();
                            }
                            AcoustIdOutcome::NoMatch => {
                                write_acoustid_no_match(item.inode, &item.fingerprint_blob);
                                b.acoustid_stats.no_match += 1;
                                b.acoustid_stats.processed += 1;
                                b.acoustid_limiter.reset_backoff();
                            }
                            AcoustIdOutcome::RateLimited => {
                                crate::logging::log_general("[FETCH] AcoustID rate limited, backing off");
                                b.acoustid_stats.retries += 1;
                                b.acoustid_queue.push_front(item);
                                b.acoustid_limiter.apply_backoff();
                                b.acoustid_limiter.mark_request();
                            }
                            AcoustIdOutcome::Error => {
                                write_acoustid_error(item.inode, &item.fingerprint_blob);
                                b.acoustid_stats.retries += 1;
                                b.acoustid_stats.processed += 1;
                            }
                        }
                        send_progress(&message_tx, b);
                    }

                    FetchResult::Mb { outcome, item } => {
                        b.mb_in_flight -= 1;
                        match outcome {
                            MbOutcome::Found { discovered_entities } => {
                                b.mb_stats.matched += 1;
                                b.mb_stats.processed += 1;

                                // Chain-emit: queue discovered artist/release entities
                                for (ek, eid) in discovered_entities {
                                    if b.mb_queued_ids.insert(eid.clone()) {
                                        b.mb_queue.push_back(MbQueueItem {
                                            kind: ek,
                                            mbid: eid,
                                        });
                                        b.mb_stats.total += 1;
                                        b.mb_done = false;
                                    }
                                }

                                b.mb_limiter.record_success();
                            }
                            MbOutcome::NotFound => {
                                b.mb_stats.no_match += 1;
                                b.mb_stats.processed += 1;
                                b.mb_limiter.record_success();
                            }
                            MbOutcome::RateLimited => {
                                crate::logging::log_general(format!(
                                    "[FETCH] MB rate limited for {} {}",
                                    item.kind.as_str(),
                                    item.mbid,
                                ));
                                b.mb_stats.retries += 1;
                                b.mb_queue.push_front(item);
                                b.mb_limiter.apply_backoff();
                                b.mb_limiter.mark_request();
                            }
                            MbOutcome::Error => {
                                b.mb_stats.retries += 1;
                                b.mb_stats.processed += 1;
                            }
                        }
                        send_progress(&message_tx, b);
                    }
                }

                // Check source completion
                let b = batch.as_mut().unwrap();

                if !b.acoustid_done && b.acoustid_queue.is_empty() && b.acoustid_in_flight == 0 {
                    b.acoustid_done = true;
                    crate::logging::log_general(format!(
                        "[FETCH] AcoustID done: {} processed, {} matched, {} no-match, {} retries",
                        b.acoustid_stats.processed,
                        b.acoustid_stats.matched,
                        b.acoustid_stats.no_match,
                        b.acoustid_stats.retries
                    ));
                    let _ = message_tx.send(SchedulerMessage::SourceDone {
                        source: ExternalSource::AcoustID,
                        stats: b.acoustid_stats.clone(),
                    });
                }

                if !b.mb_done && b.mb_queue.is_empty() && b.mb_in_flight == 0 {
                    b.mb_done = true;
                    crate::logging::log_general(format!(
                        "[FETCH] MusicBrainz done: {} processed, {} cached, {} not-found, {} retries, \
                         final rate={:.1}rps, failure-at-rps={:?}",
                        b.mb_stats.processed,
                        b.mb_stats.matched,
                        b.mb_stats.no_match,
                        b.mb_stats.retries,
                        b.mb_limiter.current_rps(),
                        b.mb_limiter.failure_rps_history,
                    ));
                    let _ = message_tx.send(SchedulerMessage::SourceDone {
                        source: ExternalSource::MusicBrainz,
                        stats: b.mb_stats.clone(),
                    });
                }

                if b.acoustid_done && b.mb_done && in_flight.is_empty() {
                    let _ = message_tx.send(SchedulerMessage::AllDone);
                    batch = None;
                }
            }
        }
    }

    crate::logging::log_general("[FETCH] Scheduler exiting");
}

// ============================================================================
// Batch State
// ============================================================================

/// Groups all per-batch state: queues, rate limiters, stats, clients, config.
struct BatchState {
    acoustid_queue: VecDeque<AcoustIdQueueItem>,
    mb_queue: VecDeque<MbQueueItem>,
    acoustid_limiter: RateLimiter,
    mb_limiter: AdaptiveRateLimiter,
    acoustid_stats: SourceProgress,
    mb_stats: SourceProgress,
    acoustid_done: bool,
    mb_done: bool,
    mb_queued_ids: HashSet<String>,
    /// Number of AcoustID items currently in-flight (dispatched, not yet returned).
    acoustid_in_flight: usize,
    /// Number of MB items currently in-flight.
    mb_in_flight: usize,
    acoustid_client: AcoustIDClient,
    mb_client: MusicBrainzClient,
    auto_enrich: bool,
    ttl_secs: i64,
    mb_rps_ceiling: u32,
}

/// Initialize a new batch from config + DB state.
fn init_batch(
    db: &Database,
    shared_config: &SharedConfig,
    eligible_dirs: Vec<PathBuf>,
) -> BatchState {
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

    let mut acoustid_queue: VecDeque<AcoustIdQueueItem> = VecDeque::new();
    let mut mb_queue: VecDeque<MbQueueItem> = VecDeque::new();

    let acoustid_limiter = RateLimiter::new_acoustid(rps);
    let is_local_mirror = mb_base_url != crate::config::ExternalMatchingConfig::DEFAULT_MB_BASE_URL;
    let mb_limiter = if is_local_mirror {
        AdaptiveRateLimiter::new_unthrottled(mb_rps)
    } else {
        AdaptiveRateLimiter::new(mb_rps)
    };

    let mut acoustid_stats = SourceProgress::default();
    let mut mb_stats = SourceProgress::default();
    let mut acoustid_done = false;
    let mut mb_done = false;
    let mut mb_queued_ids: HashSet<String> = HashSet::new();

    // Populate AcoustID queue
    if api_key.is_empty() {
        crate::logging::log_general("[FETCH] No AcoustID API key configured, skipping");
        acoustid_done = true;
    } else {
        populate_acoustid_queue(db, &eligible_dirs, &mut acoustid_queue);
        acoustid_stats.total = acoustid_queue.len();
    }

    // Populate MB queue
    if auto_enrich {
        populate_mb_queue(db, ttl_secs, &mut mb_queue);
        mb_stats.total = mb_queue.len();
        for item in &mb_queue {
            mb_queued_ids.insert(item.mbid.clone());
        }
    } else {
        mb_done = true;
    }

    // Build async clients
    let acoustid_client = AcoustIDClient::new(api_key);
    let mb_client = MusicBrainzClient::new(&mb_base_url);

    BatchState {
        acoustid_queue,
        mb_queue,
        acoustid_limiter,
        mb_limiter,
        acoustid_stats,
        mb_stats,
        acoustid_done,
        mb_done,
        mb_queued_ids,
        acoustid_in_flight: 0,
        mb_in_flight: 0,
        acoustid_client,
        mb_client,
        auto_enrich,
        ttl_secs,
        mb_rps_ceiling: mb_rps,
    }
}

/// Extend an existing batch with new AcoustID work from additional directories.
fn extend_acoustid_queue(
    db: &Database,
    shared_config: &SharedConfig,
    eligible_dirs: &[PathBuf],
    batch: &mut BatchState,
) {
    let api_key = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        config.opinions.external_matching.acoustid_api_key.clone()
    };
    if !api_key.is_empty() {
        populate_acoustid_queue(db, eligible_dirs, &mut batch.acoustid_queue);
        batch.acoustid_stats.total += batch.acoustid_queue.len();
        batch.acoustid_done = false;
    }
}

// ============================================================================
// Fetch Result Types
// ============================================================================

/// Result returned from JoinSet tasks.
enum FetchResult {
    AcoustId {
        outcome: AcoustIdOutcome,
        item: AcoustIdQueueItem,
    },
    Mb {
        outcome: MbOutcome,
        item: MbQueueItem,
    },
}

/// Scheduler-internal outcome from an AcoustID lookup.
enum AcoustIdOutcome {
    Match(Vec<MatchRow>),
    NoMatch,
    RateLimited,
    Error,
}

/// Scheduler-internal outcome from a MB fetch.
enum MbOutcome {
    Found { discovered_entities: Vec<(MbEntityKind, String)> },
    NotFound,
    RateLimited,
    Error,
}

// ============================================================================
// Async HTTP Execution
// ============================================================================

/// Execute an AcoustID fingerprint lookup. Returns outcome only —
/// DB writes happen in the select! result handler (sync, on scheduler thread).
async fn execute_acoustid_lookup(
    client: &AcoustIDClient,
    fingerprint_raw: &[u32],
    duration_secs: u32,
) -> AcoustIdOutcome {
    match client.lookup_with_raw(fingerprint_raw, duration_secs).await {
        Ok((LookupOutcome::Matches(recordings), _raw)) => {
            let rows: Vec<MatchRow> = recordings
                .into_iter()
                .map(|r| MatchRow {
                    recording_id: r.recording_id,
                    confidence: r.confidence,
                })
                .collect();
            AcoustIdOutcome::Match(rows)
        }
        Ok((LookupOutcome::NoMatch, _)) => AcoustIdOutcome::NoMatch,
        Ok((LookupOutcome::RateLimited, _)) => AcoustIdOutcome::RateLimited,
        Err(e) => {
            crate::logging::log_error(format!("[FETCH] AcoustID lookup failed: {:#}", e));
            AcoustIdOutcome::Error
        }
    }
}

/// Execute a MusicBrainz entity fetch. Writes cache + discovered entities to DB.
async fn execute_mb_fetch(
    client: &MusicBrainzClient,
    kind: MbEntityKind,
    mbid: &str,
) -> MbOutcome {
    let fetch_result = match kind {
        MbEntityKind::Recording => client.fetch_recording(mbid).await,
        MbEntityKind::Artist => client.fetch_artist(mbid).await,
        MbEntityKind::Release => client.fetch_release(mbid).await,
    };

    match fetch_result {
        Ok(MbLookupOutcome::Found(raw_json)) => {
            // Write cache entry immediately
            if let Some(sender) = write_thread::signal_sender() {
                let now = now_unix();
                match kind {
                    MbEntityKind::Recording => {
                        sender.upsert_mb_recording_cache(mbid, raw_json.clone(), now);
                    }
                    MbEntityKind::Artist => {
                        sender.upsert_mb_artist_cache(mbid, raw_json.clone(), now);
                    }
                    MbEntityKind::Release => {
                        sender.upsert_mb_release_cache(mbid, raw_json.clone(), now);
                    }
                }
            }

            // Extract and persist discovered entities (recordings only)
            let discovered_entities = if kind == MbEntityKind::Recording {
                let entities =
                    extract_entities_from_recording(&raw_json, mbid)
                        .unwrap_or_default();

                if !entities.is_empty() {
                    if let Some(sender) = write_thread::signal_sender() {
                        let now = now_unix();
                        for (ek, ref eid) in &entities {
                            sender.insert_mb_known_entity(eid, ek.as_str(), Some(mbid), now);
                        }
                    }
                }

                entities
            } else {
                Vec::new()
            };

            MbOutcome::Found { discovered_entities }
        }
        Ok(MbLookupOutcome::NotFound) => {
            crate::logging::log_general(format!(
                "[FETCH] MB {} {} not found (404)",
                kind.as_str(),
                mbid
            ));
            MbOutcome::NotFound
        }
        Ok(MbLookupOutcome::RateLimited) | Ok(MbLookupOutcome::ServiceUnavailable) => {
            MbOutcome::RateLimited
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] MB fetch failed for {} {}: {:#}",
                kind.as_str(),
                mbid,
                e
            ));
            MbOutcome::Error
        }
    }
}

// ============================================================================
// DB Write Helpers (sync, called from select! result handler)
// ============================================================================

/// Write AcoustID match results to DB via signal_sender.
fn write_acoustid_matches(inode: i64, fingerprint_blob: &[u8], rows: &[MatchRow]) {
    let acoustid_source_key = ExternalSource::AcoustID.to_key();
    if let Some(sender) = write_thread::signal_sender() {
        let now = now_unix();
        for row in rows {
            sender.insert_external_match(
                inode,
                fingerprint_blob.to_vec(),
                acoustid_source_key,
                &row.recording_id,
                row.confidence,
                None,
                now,
            );
            sender.insert_mb_known_entity(&row.recording_id, "recording", None, now);
        }
        sender.delete_external_retry(inode, acoustid_source_key);
    }
}

/// Write AcoustID no-match result to DB.
fn write_acoustid_no_match(inode: i64, fingerprint_blob: &[u8]) {
    let acoustid_source_key = ExternalSource::AcoustID.to_key();
    if let Some(sender) = write_thread::signal_sender() {
        let now = now_unix();
        sender.insert_external_no_match(fingerprint_blob.to_vec(), acoustid_source_key, now);
        sender.delete_external_retry(inode, acoustid_source_key);
    }
}

/// Write AcoustID error/retry to DB.
fn write_acoustid_error(inode: i64, fingerprint_blob: &[u8]) {
    let acoustid_source_key = ExternalSource::AcoustID.to_key();
    if let Some(sender) = write_thread::signal_sender() {
        sender.upsert_external_retry(inode, fingerprint_blob.to_vec(), acoustid_source_key, "lookup failed");
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    message_tx: &UnboundedSender<SchedulerMessage>,
    batch: &BatchState,
) {
    let _ = message_tx.send(SchedulerMessage::Progress(FetchProgress {
        acoustid: batch.acoustid_stats.clone(),
        mb: batch.mb_stats.clone(),
        acoustid_rps: batch.acoustid_limiter.effective_rps() as f32,
        mb_rps: batch.mb_limiter.current_rps() as f32,
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
