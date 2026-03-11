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

mod handle;
mod rate_limiter;
mod types;

// Re-export everything that was previously pub or pub(super)
pub use handle::ExternalFetchHandle;
pub use types::{
    AcoustIdFetchTask, ExternalFetchTask, FetchOutcome, FetchProgress, MatchRow, MbEntityKind,
    MbFetchTask, SourceProgress,
};
pub(super) use types::SchedulerMessage;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::config::SharedConfig;
use crate::db::Database;
use crate::meta::external::ExternalSource;

use rate_limiter::{AdaptiveRateLimiter, RateLimiter};
use types::FetchCommand;

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

    // Work queues (internal to scheduler -- items here haven't been dispatched yet)
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
            Err(_) => {} // Empty or Disconnected -- both fine
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
                    // Don't re-queue on hard errors -- next scan picks it up
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
                task: ExternalFetchTask::AcoustId(types::AcoustIdFetchTask {
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

            // Check DB cache first -- skip if already fresh
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
                // Already fresh in cache -- skip without HTTP call
                mb_stats.total = mb_stats.total.saturating_sub(1);
                // Don't consume rate limiter slot -- clear last_request so
                // the next real dispatch isn't delayed by the cache check.
                mb_limiter.clear_last_request();
            } else {
                mb_limiter.mark_request();
                mb_in_flight += 1;
                let label = format!("MB {} fetch", item.kind.as_str());
                let _ = message_tx.send(SchedulerMessage::TaskRequest {
                    task: ExternalFetchTask::MusicBrainz(types::MbFetchTask {
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
            Duration::from_secs(60) // effectively infinite -- nothing to dispatch
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
pub(super) fn fingerprint_to_blob(fp: &[u32]) -> Vec<u8> {
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
