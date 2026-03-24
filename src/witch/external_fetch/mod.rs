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
    CoverArtProgress, FetchProgress, MbEntityKind, MatchRow, SourceProgress,
};
pub(super) use types::SchedulerMessage;

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

use crate::config::SharedConfig;
use crate::db::write_thread;
use crate::db::Database;
use crate::external::acoustid::{AcoustIDClient, LookupOutcome};
use crate::external::coverart::CoverArtClient;
use crate::external::musicbrainz::{MbLookupOutcome, MusicBrainzClient};
use crate::meta::external::ExternalSource;
use mm_meta::config::CoverArtSanctity;
use mm_meta::paths::PathResolver;

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
    let mut caa_batch: Option<CoverArtBatch> = None;
    let mut caa_in_flight: JoinSet<(CoverArtResult, String)> = JoinSet::new();

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Some(FetchCommand::Start) => {
                        match batch.as_mut() {
                            Some(b) => {
                                // Extend existing batch with new AcoustID work
                                extend_acoustid_queue(&db, &shared_config, b);
                            }
                            None => {
                                // Initialize new batch
                                batch = Some(init_batch(&db, &shared_config));
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
                    Some(FetchCommand::StartCoverArt) => {
                        if caa_batch.is_some() {
                            continue; // Don't stack
                        }
                        caa_batch = Some(init_cover_art_batch(&db, &shared_config));
                        let cab = caa_batch.as_ref().unwrap();
                        if cab.queue.is_empty() {
                            crate::logging::log_general("[FETCH] No cover art work needed");
                            let _ = message_tx.send(SchedulerMessage::CoverArtDone(cab.progress.clone()));
                            caa_batch = None;
                            continue;
                        }
                        crate::logging::log_general(format!(
                            "[FETCH] Cover art: {} releases to process",
                            cab.queue.len(),
                        ));
                        let _ = message_tx.send(SchedulerMessage::CoverArtProgress(cab.progress.clone()));
                    }
                    Some(FetchCommand::Shutdown) | None => break,
                }
            }

            _ = tick.tick(), if batch.is_some() || caa_batch.is_some() => {
                // Dispatch cover art work (no rate limiter — CAA has no limits)
                if caa_in_flight.is_empty() {
                    if let Some(ref mut cab) = caa_batch {
                        if let Some(item) = cab.queue.pop_front() {
                            let client = cab.client.clone();
                            let corpus_root = cab.corpus_root.clone();
                            let stash_root = cab.stash_root.clone();
                            let sanctity = item.sanctity;
                            let wanted_types = cab.wanted_types.clone();
                            let release_id = item.release_id.clone();

                            let target_dir = item.target_dir.clone();
                            let existing_front = item.existing_front;
                            let existing_back = item.existing_back;

                            caa_in_flight.spawn(async move {
                                let result = execute_cover_art_fetch(
                                    &client, &release_id, &target_dir, &corpus_root, &stash_root,
                                    sanctity, &wanted_types, existing_front, existing_back,
                                ).await;
                                (result, release_id)
                            });
                        }
                    }
                }

                let Some(b) = batch.as_mut() else { continue };

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
                                                error_retries: 0,
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

                    FetchResult::Mb { outcome, mut item } => {
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
                                            error_retries: 0,
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
                                if item.error_retries < MB_MAX_ERROR_RETRIES {
                                    item.error_retries += 1;
                                    b.mb_queue.push_back(item);
                                } else {
                                    crate::logging::log_error(format!(
                                        "[FETCH] MB {} {} permanently failed after {} retries",
                                        item.kind.as_str(), item.mbid, item.error_retries
                                    ));
                                    b.mb_stats.processed += 1;
                                }
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

            Some(result) = caa_in_flight.join_next(), if !caa_in_flight.is_empty() => {
                let Ok((result, release_id)) = result else { continue };

                // Write CAA cache entry
                if let Some(sender) = write_thread::signal_sender() {
                    sender.upsert_caa_release_cache(
                        &release_id, result.cache_status,
                        result.cache_json.as_deref(), result.image_count, now_unix(),
                    );
                }

                if let Some(ref mut cab) = caa_batch {
                    cab.progress.processed += 1;
                    cab.progress.images_written += result.images_written;
                    cab.progress.images_skipped += result.images_skipped;
                    cab.progress.images_upgraded += result.images_upgraded;
                    let _ = message_tx.send(SchedulerMessage::CoverArtProgress(cab.progress.clone()));

                    // Check completion
                    if cab.queue.is_empty() && caa_in_flight.is_empty() {
                        let _ = message_tx.send(SchedulerMessage::CoverArtDone(cab.progress.clone()));
                        caa_batch = None;
                    }
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

/// State for a cover art fetch batch.
struct CoverArtBatch {
    queue: VecDeque<CaaQueueItem>,
    client: CoverArtClient,
    progress: CoverArtProgress,
    corpus_root: std::path::PathBuf,
    stash_root: std::path::PathBuf,
    wanted_types: Vec<String>,
}

/// Cover art queue item: one release to fetch art for.
struct CaaQueueItem {
    release_id: String,
    /// Corpus-relative directory containing the release's audio files.
    target_dir: String,
    /// Resolved sanctity for this directory.
    sanctity: CoverArtSanctity,
    /// Existing front cover dimensions, if a sidecar exists.
    existing_front: Option<(u32, u32)>,
    /// Existing back cover dimensions, if a sidecar exists.
    existing_back: Option<(u32, u32)>,
}

/// Result from processing a single cover art release.
struct CoverArtResult {
    images_written: usize,
    images_skipped: usize,
    images_upgraded: usize,
    cache_status: &'static str,
    cache_json: Option<String>,
    image_count: i64,
}

/// Initialize a new batch from config + DB state.
fn init_batch(
    db: &Database,
    shared_config: &SharedConfig,
) -> BatchState {
    let (api_key, rps, mb_rps, mb_base_url, auto_enrich, ttl_secs, excluded) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        let em = &config.opinions.external_matching;
        (
            em.acoustid_api_key.clone(),
            em.requests_per_second,
            em.mb_requests_per_second,
            em.mb_base_url.clone(),
            em.auto_enrich_on_match,
            (em.mb_cache_ttl_days as i64) * 86400,
            config.acoustid_excluded_db_prefixes(),
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
        populate_acoustid_queue(db, &excluded, &mut acoustid_queue);
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

/// Extend an existing batch with new AcoustID work.
fn extend_acoustid_queue(
    db: &Database,
    shared_config: &SharedConfig,
    batch: &mut BatchState,
) {
    let (api_key, excluded) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        (
            config.opinions.external_matching.acoustid_api_key.clone(),
            config.acoustid_excluded_db_prefixes(),
        )
    };
    if !api_key.is_empty() {
        populate_acoustid_queue(db, &excluded, &mut batch.acoustid_queue);
        batch.acoustid_stats.total += batch.acoustid_queue.len();
        batch.acoustid_done = false;
    }
}

/// Initialize a cover art fetch batch from config + DB state.
fn init_cover_art_batch(db: &Database, shared_config: &SharedConfig) -> CoverArtBatch {
    let (wanted_types, corpus_root, stash_root) = {
        let config = shared_config.read().expect("SharedConfig lock poisoned");
        let em = &config.opinions.external_matching;
        let resolver = PathResolver::from_config(&config);
        (
            em.cover_art_types.clone(),
            resolver.corpus_dir(),
            resolver.stash_dir(),
        )
    };

    let mut queue = VecDeque::new();
    populate_cover_art_queue(db, shared_config, &wanted_types, &corpus_root, &mut queue);

    let progress = CoverArtProgress {
        total_releases: queue.len(),
        ..Default::default()
    };

    CoverArtBatch {
        queue,
        client: CoverArtClient::new(),
        progress,
        corpus_root,
        stash_root,
        wanted_types,
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
    /// How many times this item has been re-queued after a fetch error.
    /// Incremented only on error re-insertion, not on initial queue or discovery.
    error_retries: u8,
}

/// Max error re-queues before permanently dropping an MB fetch item.
const MB_MAX_ERROR_RETRIES: u8 = 3;

// ============================================================================
// Queue Population Helpers
// ============================================================================

/// Populate the AcoustID work queue from DB (inodes needing lookup + retries).
fn populate_acoustid_queue(
    db: &Database,
    excluded_prefixes: &[String],
    queue: &mut VecDeque<AcoustIdQueueItem>,
) {
    let source_key = ExternalSource::AcoustID.to_key();

    // Fresh lookup candidates
    match db.get_inodes_needing_lookup(source_key, excluded_prefixes, i64::MAX as usize) {
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
    match db.get_retry_candidates(source_key, excluded_prefixes, i64::MAX as usize) {
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
                    error_retries: 0,
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
                        error_retries: 0,
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

// ============================================================================
// Cover Art Queue Population
// ============================================================================

/// Populate the cover art work queue from DB.
///
/// Queries `release_packing_scores` for releases with optimal matches, determines
/// their corpus directories, and checks for existing art to decide what to fetch.
fn populate_cover_art_queue(
    db: &Database,
    shared_config: &SharedConfig,
    wanted_types: &[String],
    _corpus_root: &std::path::Path,
    queue: &mut VecDeque<CaaQueueItem>,
) {
    use std::collections::HashMap;

    // Get all optimal packing scores with their corpus paths
    let mut release_dirs: HashMap<String, String> = HashMap::new();
    let sql = r#"
        SELECT DISTINCT rps.release_id, f.path
        FROM release_packing_scores rps
        JOIN files f ON rps.inode = f.inode
        WHERE rps.is_optimal = 1 AND f.zone = 'corpus'
    "#;
    if let Ok(mut stmt) = db.conn().prepare(sql) {
        let _ = stmt.query_map([], |row| {
            let release_id: String = row.get(0)?;
            let path: String = row.get(1)?;
            Ok((release_id, path))
        }).and_then(|rows| {
            for row in rows {
                if let Ok((release_id, path)) = row {
                    // Extract directory from path
                    if let Some(dir) = std::path::Path::new(&path).parent() {
                        let dir_str = dir.to_string_lossy().to_string();
                        release_dirs.entry(release_id).or_insert(dir_str);
                    }
                }
            }
            Ok(())
        });
    }

    let wants_front = wanted_types.iter().any(|t| t == "Front");
    let wants_back = wanted_types.iter().any(|t| t == "Back");

    let config = shared_config.read().expect("SharedConfig lock poisoned");

    for (release_id, dir) in release_dirs {
        // Resolve sanctity for this directory
        let sanctity = config
            .resolve_source_config(std::path::Path::new(&dir))
            .map(|rsc| rsc.cover_art_sanctity)
            .unwrap_or_default();

        // Check existing art in this directory
        let existing_front = if wants_front {
            get_existing_art_dims(db, &dir, "cover_front")
        } else {
            None
        };
        let existing_back = if wants_back {
            get_existing_art_dims(db, &dir, "cover_back")
        } else {
            None
        };

        // In DontTouch mode, skip releases that already have all wanted art
        if sanctity == CoverArtSanctity::DontTouch {
            let front_ok = !wants_front || existing_front.is_some();
            let back_ok = !wants_back || existing_back.is_some();
            if front_ok && back_ok {
                continue;
            }
        }

        queue.push_back(CaaQueueItem {
            release_id,
            target_dir: dir,
            sanctity,
            existing_front,
            existing_back,
        });
    }
}

/// Check if a directory has existing cover art of the given role, return its dimensions.
fn get_existing_art_dims(db: &Database, dir: &str, role: &str) -> Option<(u32, u32)> {
    use rusqlite::params;
    let sql = r#"
        SELECT ii.width, ii.height
        FROM image_info ii
        JOIN files f ON ii.inode = f.inode
        WHERE f.zone = 'corpus'
          AND ii.role = ?1
          AND f.path LIKE ?2
          AND f.path NOT LIKE ?3
        LIMIT 1
    "#;
    // Match files in this directory but not in subdirectories
    let dir_prefix = if dir.is_empty() { "%".to_string() } else { format!("{}/%", dir) };
    let subdir_prefix = if dir.is_empty() { "%/%".to_string() } else { format!("{}/%/%", dir) };

    db.conn().query_row(sql, params![role, dir_prefix, subdir_prefix], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?))
    }).ok()
}

// ============================================================================
// Cover Art Async Execution
// ============================================================================

/// Execute a cover art fetch for a single release.
///
/// Fetches the CAA listing, downloads wanted image types, and writes sidecar
/// files to the corpus directory. Sanctity controls existing-art behavior:
/// DontTouch skips, ReplaceIfBetter uses perceptual hashing, ReplaceAlways overwrites.
async fn execute_cover_art_fetch(
    client: &CoverArtClient,
    release_id: &str,
    target_dir: &str,
    corpus_root: &std::path::Path,
    stash_root: &std::path::Path,
    sanctity: CoverArtSanctity,
    wanted_types: &[String],
    existing_front: Option<(u32, u32)>,
    existing_back: Option<(u32, u32)>,
) -> CoverArtResult {
    // Fetch listing
    let listing = match client.fetch_listing(release_id).await {
        Ok(Some(listing)) => listing,
        Ok(None) => {
            return CoverArtResult {
                images_written: 0, images_skipped: 0, images_upgraded: 0,
                cache_status: "not_found", cache_json: None, image_count: 0,
            };
        }
        Err(e) => {
            crate::logging::log_error(format!(
                "[FETCH] CAA listing failed for {}: {:#}", release_id, e
            ));
            return CoverArtResult {
                images_written: 0, images_skipped: 0, images_upgraded: 0,
                cache_status: "error", cache_json: None, image_count: 0,
            };
        }
    };

    let cache_json = serde_json::to_string(&listing).ok();
    let image_count = listing.images.len() as i64;

    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut upgraded = 0usize;

    for image in &listing.images {
        for wanted_type in wanted_types {
            if !image.types.iter().any(|t| t == wanted_type) {
                continue;
            }

            let (sidecar_name, existing_dims) = match wanted_type.as_str() {
                "Front" => ("cover", existing_front),
                "Back" => ("back", existing_back),
                _ => continue,
            };

            // Existing art: behavior depends on sanctity setting
            if existing_dims.is_some() {
                match sanctity {
                    CoverArtSanctity::DontTouch => {
                        skipped += 1;
                        continue;
                    }
                    CoverArtSanctity::ReplaceIfBetter | CoverArtSanctity::ReplaceAlways => {
                        // Need to download to compare or replace
                    }
                }
            }

            // Download the original image
            let downloaded = match client.download_image(&image.image).await {
                Ok(img) => img,
                Err(e) => {
                    crate::logging::log_error(format!(
                        "[FETCH] CAA download failed for {} ({}): {:#}",
                        release_id, wanted_type, e
                    ));
                    skipped += 1;
                    continue;
                }
            };

            let ext = downloaded.format.extension();
            let filename = format!("{}.{}", sidecar_name, ext);
            let abs_dir = corpus_root.join(target_dir);
            let abs_path = abs_dir.join(&filename);

            // Handle existing art replacement
            if existing_dims.is_some() {
                let existing_path = find_existing_sidecar(&abs_dir, sidecar_name);
                if let Some(ref ep) = existing_path {
                    if sanctity == CoverArtSanctity::ReplaceIfBetter {
                        // Perceptual comparison: only replace if visually identical
                        match std::fs::read(ep) {
                            Ok(existing_bytes) => {
                                match crate::corpus::image_hash::is_visual_match(&existing_bytes, &downloaded.bytes) {
                                    Ok(true) => {} // proceed to stash + write below
                                    Ok(false) => {
                                        crate::logging::log_general(format!(
                                            "[FETCH] CAA art for {} differs visually from existing, skipping",
                                            release_id
                                        ));
                                        skipped += 1;
                                        continue;
                                    }
                                    Err(e) => {
                                        crate::logging::log_error(format!(
                                            "[FETCH] Visual comparison failed for {}: {:#}",
                                            release_id, e
                                        ));
                                        skipped += 1;
                                        continue;
                                    }
                                }
                            }
                            Err(e) => {
                                crate::logging::log_error(format!(
                                    "[FETCH] Failed to read existing sidecar {}: {:#}",
                                    ep.display(), e
                                ));
                                skipped += 1;
                                continue;
                            }
                        }
                    }
                    // ReplaceAlways: skip comparison, just stash and replace
                    // ReplaceIfBetter: comparison passed, stash and replace
                    if let Err(e) = stash_file(ep, stash_root, corpus_root) {
                        crate::logging::log_error(format!(
                            "[FETCH] Failed to stash {}: {:#}", ep.display(), e
                        ));
                        skipped += 1;
                        continue;
                    }
                }
                if let Err(e) = write_sidecar(&abs_path, &downloaded.bytes) {
                    crate::logging::log_error(format!(
                        "[FETCH] Failed to write upgraded cover art: {:#}", e
                    ));
                    skipped += 1;
                    continue;
                }
                upgraded += 1;
                continue;
            }

            // Write new sidecar (no existing art)
            if let Err(e) = write_sidecar(&abs_path, &downloaded.bytes) {
                crate::logging::log_error(format!(
                    "[FETCH] Failed to write cover art to {}: {:#}", abs_path.display(), e
                ));
                skipped += 1;
                continue;
            }

            crate::logging::log_general(format!(
                "[FETCH] Wrote cover art: {}/{}", target_dir, filename
            ));
            written += 1;
        }
    }

    CoverArtResult {
        images_written: written,
        images_skipped: skipped,
        images_upgraded: upgraded,
        cache_status: "found",
        cache_json,
        image_count,
    }
}

/// Write bytes to a sidecar file path, creating parent directories if needed.
fn write_sidecar(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Move an existing file to the stash, preserving corpus-relative path structure.
fn stash_file(
    file_path: &std::path::Path,
    stash_root: &std::path::Path,
    corpus_root: &std::path::Path,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let rel = file_path.strip_prefix(corpus_root)
        .context("file not under corpus root")?;
    let stash_dest = stash_root.join("cover-art").join(rel);
    if let Some(parent) = stash_dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(file_path, &stash_dest)?;
    crate::logging::log_general(format!(
        "[FETCH] Stashed old cover art: {} -> {}", file_path.display(), stash_dest.display()
    ));
    Ok(())
}

/// Find an existing sidecar file by stem name (e.g. "cover") in a directory,
/// trying all known image extensions.
fn find_existing_sidecar(dir: &std::path::Path, stem: &str) -> Option<std::path::PathBuf> {
    for ext in mm_utils::IMAGE_EXTENSIONS {
        let path = dir.join(format!("{}.{}", stem, ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}
