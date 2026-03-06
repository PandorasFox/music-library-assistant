//! Release bin-packing staged pipeline.
//!
//! Five-stage pipeline for assigning corpus files to MusicBrainz releases:
//!
//! ```text
//! Stage 1: PackReleases (orchestrator)
//!     │  Loads external matches, identifies releases, writes manifest
//!     │  Spawns N ScoreReleaseCandidates
//!     │  Defers [Stage 3: Resolve, Stage 4: Eliminate, Stage 5: Analyze]
//!     ▼
//! Stage 2: ScoreReleaseCandidates { release_id } × N  (parallel)
//!     │  Each scores its release's tracklist against corpus inodes
//!     │  Optimal per-release assignment via Hungarian algorithm
//!     │  Writes results to intermediate table
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3: ResolveReleaseConflicts
//!     │  Greedy global resolution (cross-release conflict handling)
//!     │  Emits ReleasePackingSignal (match_method: AcoustId) per assigned inode
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 4: EliminateByDirectory
//!     │  For cohesive directories (all assigned to one release),
//!     │  assigns remaining files to unfilled slots by elimination
//!     │  Emits ReleasePackingSignal (match_method: Elimination)
//!     │  Records pending AcoustID submissions
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 5: AnalyzeReleaseGaps
//!        Unmatched corpus tracks, unfilled slots, near-miss detection
//!        Emits gap analysis signals
//! ```
//!
//! Manual trigger only — not part of ScheduleContentAnalysis.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

/// A cohesive directory mapping: (directory_path, release_id, filled_slots).
/// Used by EliminateByDirectory to represent directories where all assigned
/// files point to exactly one release.
type CohesiveDir = (String, String, HashSet<(u32, u32)>);

use crate::db::queries::external::ExternalMatchRow;
use crate::db::types::Zone;
use crate::db::write_thread::{self, PackingScoreRow};
use crate::db::ReadOnlyDb;
use crate::external::musicbrainz::{self, MbArtistCredit, MbRelease};
use crate::logging::log_general;
use crate::meta::computations::helpers::{
    reconcile_aggregate_signals, reconcile_corpus_signals, ComputedAggregateSignal,
    ComputedCorpusSignal,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::computations::{Computation, PipelineStage};
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::{
    MatchMethod, NearMissReleaseData, NearMissReleaseSignal, PackingScoreBreakdown,
    ReleasePackingData, ReleasePackingSignal, TypedSignalWrite, UnfilledReleaseSlotData,
    UnfilledReleaseSlotSignal, UnmatchedCorpusTrackData, UnmatchedCorpusTrackSignal,
};

use super::{Computation as AnalysisComputation, Result};

// ============================================================================
// Shared types
// ============================================================================

/// Corpus file metadata needed for scoring.
struct CorpusFileInfo {
    parent_dir: String,
    tags: HashMap<String, Vec<String>>,
    duration_ms: Option<i64>,
}

/// A recording match for an inode, filtered for quality.
struct RecordingMatch {
    recording_id: String,
    confidence: f64,
}

/// A candidate assignment of an inode to a (release, medium, track) slot.
struct CandidateAssignment {
    inode: i64,
    recording_id: String,
    release_id: String,
    medium_pos: u32,
    track_pos: u32,
    medium_format: Option<String>,
    track_number: String,
    track_title: String,
    score: f64,
    breakdown: PackingScoreBreakdown,
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Group external match rows by inode, keeping all recordings per inode.
fn group_all_per_inode(rows: Vec<ExternalMatchRow>) -> HashMap<i64, Vec<ExternalMatchRow>> {
    let mut grouped: HashMap<i64, Vec<ExternalMatchRow>> = HashMap::new();
    for row in rows {
        grouped.entry(row.inode).or_default().push(row);
    }
    grouped
}

/// Compute the scoring components for a candidate assignment.
fn compute_score(
    rec_match: &RecordingMatch,
    track: &musicbrainz::MbTrack,
    release_artist: &str,
    release_title: &str,
    corpus: Option<&CorpusFileInfo>,
    duration_tolerance_pct: f64,
) -> (f64, PackingScoreBreakdown) {
    let acoustid_confidence = rec_match.confidence;

    // Duration match
    let mb_duration_ms = track.length.or(track.recording.length);
    let duration_match = match (corpus.and_then(|c| c.duration_ms), mb_duration_ms) {
        (Some(corpus_dur), Some(mb_dur)) if mb_dur > 0 => {
            let ratio = (corpus_dur as f64 - mb_dur as f64).abs() / mb_dur as f64;
            if ratio > duration_tolerance_pct {
                0.0
            } else {
                1.0 - (ratio / duration_tolerance_pct)
            }
        }
        _ => 0.5,
    };

    // Tag similarity
    let title_sim = corpus
        .and_then(|c| c.tags.get("TITLE"))
        .and_then(|v| v.first())
        .map(|corpus_title| {
            let track_sim = strsim::normalized_levenshtein(corpus_title, &track.title);
            let rec_sim =
                strsim::normalized_levenshtein(corpus_title, &track.recording.title);
            track_sim.max(rec_sim)
        })
        .unwrap_or(0.0);

    let artist_sim = corpus
        .and_then(|c| c.tags.get("ARTIST"))
        .and_then(|v| v.first())
        .map(|corpus_artist| strsim::normalized_levenshtein(corpus_artist, release_artist))
        .unwrap_or(0.0);

    let album_sim = corpus
        .and_then(|c| c.tags.get("ALBUM"))
        .and_then(|v| v.first())
        .map(|corpus_album| strsim::normalized_levenshtein(corpus_album, release_title))
        .unwrap_or(0.0);

    let tag_similarity = 0.4 * title_sim + 0.35 * artist_sim + 0.25 * album_sim;

    // Track number match
    let track_number_match = corpus
        .and_then(|c| c.tags.get("TRACKNUMBER"))
        .and_then(|v| v.first())
        .and_then(|tn| tn.parse::<u32>().ok())
        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
        .unwrap_or(0.0);

    let breakdown = PackingScoreBreakdown {
        acoustid_confidence,
        duration_match,
        tag_similarity,
        track_number_match,
        directory_cohesion: 0.0, // Set later by caller
    };

    let score = weighted_composite(&breakdown);
    (score, breakdown)
}

/// Compute weighted composite score from breakdown components.
fn weighted_composite(b: &PackingScoreBreakdown) -> f64 {
    0.25 * b.acoustid_confidence
        + 0.20 * b.duration_match
        + 0.20 * b.tag_similarity
        + 0.10 * b.track_number_match
        + 0.25 * b.directory_cohesion
}

/// Optimal bipartite assignment via Hungarian algorithm.
///
/// Returns set of (inode, (medium_pos, track_pos)) pairs that maximize total score.
/// Uses the Kuhn-Munkres algorithm on a cost matrix built from candidate scores.
/// Matrix sizes are small (99.6% of releases have <100 score entries), so O(n³)
/// is negligible.
#[allow(clippy::needless_range_loop)] // Hungarian algorithm uses 1-based index arithmetic
fn hungarian_assignment(candidates: &[CandidateAssignment]) -> HashSet<(i64, (u32, u32))> {
    // Collect unique inodes and slots
    let mut inode_set: Vec<i64> = candidates.iter().map(|c| c.inode).collect();
    inode_set.sort();
    inode_set.dedup();
    let inode_idx: HashMap<i64, usize> = inode_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let mut slot_set: Vec<(u32, u32)> = candidates.iter().map(|c| (c.medium_pos, c.track_pos)).collect();
    slot_set.sort();
    slot_set.dedup();
    let slot_idx: HashMap<(u32, u32), usize> = slot_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let n_rows = inode_set.len();
    let n_cols = slot_set.len();

    if n_rows == 0 || n_cols == 0 {
        return HashSet::new();
    }

    // Pad to square matrix
    let n = n_rows.max(n_cols);

    // Build cost matrix (negate scores for minimization; 0.0 for impossible/dummy)
    let mut cost = vec![vec![0.0f64; n]; n];
    for candidate in candidates {
        let r = inode_idx[&candidate.inode];
        let c = slot_idx[&(candidate.medium_pos, candidate.track_pos)];
        // Take the max score if multiple candidates map same (inode, slot)
        if candidate.score > -cost[r][c] {
            cost[r][c] = -candidate.score;
        }
    }

    // Hungarian algorithm (Kuhn-Munkres) for minimum cost assignment.
    // Uses 1-indexed potentials and predecessor-tracked augmenting paths.
    let inf = f64::MAX / 2.0;
    let mut u = vec![0.0f64; n + 1];
    let mut v = vec![0.0f64; n + 1];
    let mut col_to_row = vec![0usize; n + 1]; // col_to_row[j] = row assigned to col j (1-indexed)

    for i in 1..=n {
        // Assign row i
        let mut links = vec![0usize; n + 1]; // links[j] = previous column in augmenting path
        let mut mins = vec![inf; n + 1];
        let mut visited = vec![false; n + 1];

        col_to_row[0] = i;
        let mut j0 = 0usize;

        loop {
            visited[j0] = true;
            let row = col_to_row[j0];
            let mut delta = inf;
            let mut j1 = 0usize;

            for j in 1..=n {
                if visited[j] {
                    continue;
                }
                let val = cost[row - 1][j - 1] - u[row] - v[j];
                if val < mins[j] {
                    mins[j] = val;
                    links[j] = j0;
                }
                if mins[j] < delta {
                    delta = mins[j];
                    j1 = j;
                }
            }

            for j in 0..=n {
                if visited[j] {
                    u[col_to_row[j]] += delta;
                    v[j] -= delta;
                } else {
                    mins[j] -= delta;
                }
            }

            j0 = j1;
            if col_to_row[j0] == 0 {
                break;
            }
        }

        // Augment path
        loop {
            let prev = links[j0];
            col_to_row[j0] = col_to_row[prev];
            j0 = prev;
            if j0 == 0 {
                break;
            }
        }
    }

    // Extract result: filter out dummy assignments
    let mut result = HashSet::new();
    for j in 1..=n {
        let row = col_to_row[j];
        if row == 0 {
            continue;
        }
        let row_idx = row - 1;
        let col_idx = j - 1;
        if row_idx < n_rows && col_idx < n_cols {
            // Only include if this was a real (non-zero) score
            if cost[row_idx][col_idx] < 0.0 {
                result.insert((inode_set[row_idx], slot_set[col_idx]));
            }
        }
    }

    result
}

/// Resolve a release's artist credit string using locale preferences.
fn localized_release_artist(
    credits: &[MbArtistCredit],
    release_artist_data: &HashMap<String, Vec<(String, Option<musicbrainz::MbArtist>)>>,
    release_id: &str,
    preferred_locales: &[String],
) -> String {
    if let Some(artists) = release_artist_data.get(release_id) {
        musicbrainz::join_artist_credits_localized(credits, artists, preferred_locales)
    } else {
        musicbrainz::join_artist_credits_localized(credits, &[], preferred_locales)
    }
}

// ============================================================================
// Stage 1: PackReleases (orchestrator)
// ============================================================================

/// Execute PackReleases — Stage 1 orchestrator.
///
/// Loads external matches, identifies candidate releases, writes the session
/// manifest and per-inode recording maps, then spawns N ScoreReleaseCandidates
/// and defers Resolve + Analyze as barrier-separated phases.
pub fn execute_pack_releases(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::PackReleases;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };
    let min_confidence = config.opinions.release_packing.min_confidence;
    let duration_tolerance_pct = config.opinions.release_packing.duration_tolerance_pct;
    let preferred_locales = config.opinions.external_matching.preferred_locales.clone();

    let source_key = ExternalSource::AcoustID.to_key();

    // === Load all external matches (corpus only) ===
    let all_rows = match read_only_db.get_external_matches_slim(source_key) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query external matches: {}", e),
            );
        }
    };

    let all_per_inode = group_all_per_inode(all_rows);

    if all_per_inode.is_empty() {
        // No matches — clear stale signals and exit pipeline
        let (cleared, _, _, _) = reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] PackReleases: cleared {} stale signals (no external matches)",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    log_general(format!(
        "[COMPUTE] PackReleases: {} inodes with external matches",
        all_per_inode.len()
    ));

    // === Load corpus metadata ===
    let files_with_tags = match read_only_db.get_all_audio_files_with_tags(Zone::Corpus, false) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query corpus files: {}", e),
            );
        }
    };

    let corpus_info: HashMap<i64, CorpusFileInfo> = files_with_tags
        .into_iter()
        .map(|(af, tags)| {
            let path = af.path().to_string();
            let parent_dir = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            (
                af.inode(),
                CorpusFileInfo {
                    parent_dir,
                    tags,
                    duration_ms: af.audio.duration_ms,
                },
            )
        })
        .collect();

    // === Parse recordings, filter, collect release IDs ===
    let mut inode_recordings: HashMap<i64, Vec<RecordingMatch>> = HashMap::new();
    let mut all_release_ids: HashSet<String> = HashSet::new();
    let mut recording_releases: HashMap<String, Vec<String>> = HashMap::new();
    let mut filtered_duration = 0usize;
    let mut filtered_confidence = 0usize;
    let mut recording_parse_failures = 0usize;

    for (inode, rows) in &all_per_inode {
        for row in rows {
            if row.confidence < min_confidence {
                filtered_confidence += 1;
                continue;
            }

            // Skip recordings we've already parsed (same recording from different source rows)
            if recording_releases.contains_key(&row.recording_id) {
                // Still add to inode_recordings with this inode's confidence
                inode_recordings
                    .entry(*inode)
                    .or_default()
                    .push(RecordingMatch {
                        recording_id: row.recording_id.clone(),
                        confidence: row.confidence,
                    });
                continue;
            }

            let recording_json = match read_only_db.get_mb_recording_cache(&row.recording_id) {
                Ok(Some((json, _))) => json,
                _ => {
                    recording_parse_failures += 1;
                    continue;
                }
            };

            let recording = match musicbrainz::parse_recording(&recording_json) {
                Ok(r) => r,
                Err(_) => {
                    recording_parse_failures += 1;
                    continue;
                }
            };

            if let (Some(corpus_dur), Some(mb_dur)) = (
                corpus_info.get(inode).and_then(|c| c.duration_ms),
                recording.length,
            ) {
                if mb_dur > 0 {
                    let ratio = (corpus_dur as f64 - mb_dur as f64).abs() / mb_dur as f64;
                    if ratio > duration_tolerance_pct {
                        filtered_duration += 1;
                        continue;
                    }
                }
            }

            for release_ref in &recording.releases {
                all_release_ids.insert(release_ref.id.clone());
            }
            recording_releases.insert(
                recording.id.clone(),
                recording.releases.iter().map(|r| r.id.clone()).collect(),
            );

            inode_recordings
                .entry(*inode)
                .or_default()
                .push(RecordingMatch {
                    recording_id: recording.id.clone(),
                    confidence: row.confidence,
                });
        }
    }

    log_general(format!(
        "[COMPUTE] PackReleases: {} inodes after filtering (duration={}, confidence={}, parse_fail={}), {} release IDs",
        inode_recordings.len(), filtered_duration, filtered_confidence, recording_parse_failures,
        all_release_ids.len()
    ));

    if inode_recordings.is_empty() {
        let (cleared, _, _, _) = reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] PackReleases: cleared {} stale signals (all filtered)",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // === Load release tracklists ===
    let release_id_refs: Vec<&str> = all_release_ids.iter().map(|s| s.as_str()).collect();
    let release_cache_entries = match read_only_db.get_mb_release_cache_bulk(&release_id_refs) {
        Ok(entries) => entries,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to bulk-load release cache: {}", e),
            );
        }
    };

    let mut release_tracklists: HashMap<String, MbRelease> = HashMap::new();
    for (release_id, raw_json) in release_cache_entries {
        if let Ok(release) = musicbrainz::parse_release(&raw_json) {
            if !release.media.is_empty() {
                release_tracklists.insert(release_id, release);
            }
        }
    }

    let missing_tracklists = all_release_ids.len() - release_tracklists.len();
    log_general(format!(
        "[COMPUTE] PackReleases: {} releases with tracklists, {} missing/empty",
        release_tracklists.len(),
        missing_tracklists
    ));

    if release_tracklists.is_empty() {
        log_general("[COMPUTE] PackReleases: no releases with tracklist data, skipping");
        reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // === Load artist data for locale-aware name resolution ===
    let release_artist_data: HashMap<String, Vec<(String, Option<musicbrainz::MbArtist>)>> =
        if preferred_locales.is_empty() {
            HashMap::new()
        } else {
            let mut artist_ids_seen: HashSet<String> = HashSet::new();
            for release in release_tracklists.values() {
                for credit in &release.artist_credit {
                    artist_ids_seen.insert(credit.artist.id.clone());
                }
            }
            let mut artist_cache: HashMap<String, musicbrainz::MbArtist> = HashMap::new();
            for artist_id in &artist_ids_seen {
                if let Ok(Some((json, _))) = read_only_db.get_mb_artist_cache(artist_id) {
                    if let Ok(artist) = musicbrainz::parse_artist(&json) {
                        artist_cache.insert(artist_id.clone(), artist);
                    }
                }
            }
            release_tracklists
                .iter()
                .map(|(release_id, release)| {
                    let artists: Vec<(String, Option<musicbrainz::MbArtist>)> = release
                        .artist_credit
                        .iter()
                        .map(|c| {
                            let cached = artist_cache.get(&c.artist.id).cloned();
                            (c.artist.id.clone(), cached)
                        })
                        .collect();
                    (release_id.clone(), artists)
                })
                .collect()
        };

    // === Truncate intermediate tables for fresh pipeline ===
    sender.truncate_packing_tables(witness);

    // === Write manifest ===
    let manifest_rows: Vec<(String, i32, String, String)> = release_tracklists
        .iter()
        .map(|(release_id, release)| {
            let total: i32 = release.media.iter().map(|m| m.tracks.len() as i32).sum();
            let artist = localized_release_artist(
                &release.artist_credit,
                &release_artist_data,
                release_id,
                &preferred_locales,
            );
            (
                release_id.clone(),
                total,
                release.title.clone(),
                artist,
            )
        })
        .collect();
    sender.write_packing_manifest(manifest_rows, witness);

    // === Compute directory file counts for cohesion scoring ===
    let mut dir_total_files: HashMap<String, i32> = HashMap::new();
    for info in corpus_info.values() {
        *dir_total_files.entry(info.parent_dir.clone()).or_default() += 1;
    }

    // === Build inode→path lookup from all_per_inode ===
    let inode_paths: HashMap<i64, String> = all_per_inode
        .iter()
        .map(|(inode, rows)| (*inode, rows[0].path.clone()))
        .collect();

    // === Write candidate rows, deduplicating per (release_id, inode) ===
    // Keeps the highest-confidence recording for each pair.
    let mut deduped: HashMap<(String, i64), write_thread::PackingCandidateRow> = HashMap::new();

    for (inode, rec_matches) in &inode_recordings {
        let corpus = corpus_info.get(inode);
        let inode_path = inode_paths.get(inode).cloned().unwrap_or_default();
        let parent_dir = corpus
            .map(|c| c.parent_dir.clone())
            .unwrap_or_default();
        let dir_file_count = dir_total_files.get(&parent_dir).copied().unwrap_or(1);

        for rec_match in rec_matches {
            if let Some(release_ids) = recording_releases.get(&rec_match.recording_id) {
                for release_id in release_ids {
                    if !release_tracklists.contains_key(release_id) {
                        continue;
                    }

                    let key = (release_id.clone(), *inode);
                    let entry = deduped.entry(key);
                    use std::collections::hash_map::Entry;
                    match entry {
                        Entry::Occupied(mut e) => {
                            if rec_match.confidence > e.get().confidence {
                                e.get_mut().recording_id = rec_match.recording_id.clone();
                                e.get_mut().confidence = rec_match.confidence;
                            }
                        }
                        Entry::Vacant(e) => {
                            let duration_ms = corpus.and_then(|c| c.duration_ms);
                            let tag_title = corpus
                                .and_then(|c| c.tags.get("TITLE"))
                                .and_then(|v| v.first())
                                .cloned();
                            let tag_artist = corpus
                                .and_then(|c| c.tags.get("ARTIST"))
                                .and_then(|v| v.first())
                                .cloned();
                            let tag_album = corpus
                                .and_then(|c| c.tags.get("ALBUM"))
                                .and_then(|v| v.first())
                                .cloned();
                            let tag_tracknumber = corpus
                                .and_then(|c| c.tags.get("TRACKNUMBER"))
                                .and_then(|v| v.first())
                                .cloned();

                            e.insert(write_thread::PackingCandidateRow {
                                release_id: release_id.clone(),
                                inode: *inode,
                                recording_id: rec_match.recording_id.clone(),
                                confidence: rec_match.confidence,
                                path: inode_path.clone(),
                                parent_dir: parent_dir.clone(),
                                duration_ms,
                                tag_title,
                                tag_artist,
                                tag_album,
                                tag_tracknumber,
                                dir_file_count,
                            });
                        }
                    }
                }
            }
        }
    }

    let mut releases_with_candidates: HashSet<String> = HashSet::new();
    for (release_id, _) in deduped.keys() {
        releases_with_candidates.insert(release_id.clone());
    }
    let candidate_rows: Vec<write_thread::PackingCandidateRow> =
        deduped.into_values().collect();

    // Write candidates to intermediate table for Stage 2 consumption
    if !candidate_rows.is_empty() {
        log_general(format!(
            "[COMPUTE] PackReleases: writing {} candidate rows to intermediate table",
            candidate_rows.len()
        ));
        sender.write_packing_candidates(candidate_rows, witness);
        // Drain write queue so candidates are visible to Stage 2 threads
        write_thread::wait_for_queue_drain();
    }

    // === Spawn per-release scorers (Stage 2) ===
    let spawn: Vec<AnalysisComputation> = releases_with_candidates
        .iter()
        .map(|release_id| AnalysisComputation::ScoreReleaseCandidates {
            release_id: release_id.clone(),
        })
        .collect();

    log_general(format!(
        "[COMPUTE] PackReleases: spawning {} ScoreReleaseCandidates, deferring Resolve + Analyze",
        spawn.len()
    ));

    // === Defer Stage 3, 4, 5 as barrier-separated phases ===
    let deferred_phases = vec![
        (
            PipelineStage::Resolve,
            vec![Computation::Analysis(
                AnalysisComputation::ResolveReleaseConflicts,
            )],
        ),
        (
            PipelineStage::Elimination,
            vec![Computation::Analysis(
                AnalysisComputation::EliminateByDirectory,
            )],
        ),
        (
            PipelineStage::Analyze,
            vec![Computation::Analysis(
                AnalysisComputation::AnalyzeReleaseGaps,
            )],
        ),
    ];

    Result::pipeline(
        computation,
        start.elapsed().as_millis() as u64,
        spawn,
        deferred_phases,
    )
}

// ============================================================================
// Stage 2: ScoreReleaseCandidates
// ============================================================================

/// Execute ScoreReleaseCandidates — score all candidate inodes for one release.
///
/// Reads pre-filtered candidates from `release_packing_candidates` (written by Stage 1),
/// loads the release tracklist, scores each (inode, track_slot) pairing, solves optimal
/// per-release assignment via greedy, and writes results to `release_packing_scores`.
pub fn execute_score_release_candidates(
    read_only_db: &ReadOnlyDb<'_>,
    release_id: &str,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::ScoreReleaseCandidates {
        release_id: release_id.to_string(),
    };

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };
    let duration_tolerance_pct = config.opinions.release_packing.duration_tolerance_pct;
    let preferred_locales = config.opinions.external_matching.preferred_locales.clone();

    // Load the release tracklist
    let release = match read_only_db.get_mb_release_cache(release_id) {
        Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
            Ok(r) if !r.media.is_empty() => r,
            _ => {
                return Result::failure(
                    computation,
                    start.elapsed().as_millis() as u64,
                    format!("Release {} has no parseable tracklist", release_id),
                );
            }
        },
        _ => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Release {} not in cache", release_id),
            );
        }
    };

    // Load artist data for locale-aware name resolution
    let release_artist_data: HashMap<String, Vec<(String, Option<musicbrainz::MbArtist>)>> =
        if preferred_locales.is_empty() {
            HashMap::new()
        } else {
            let mut artist_cache: HashMap<String, musicbrainz::MbArtist> = HashMap::new();
            for credit in &release.artist_credit {
                if let Ok(Some((json, _))) = read_only_db.get_mb_artist_cache(&credit.artist.id) {
                    if let Ok(artist) = musicbrainz::parse_artist(&json) {
                        artist_cache.insert(credit.artist.id.clone(), artist);
                    }
                }
            }
            let artists: Vec<(String, Option<musicbrainz::MbArtist>)> = release
                .artist_credit
                .iter()
                .map(|c| {
                    let cached = artist_cache.get(&c.artist.id).cloned();
                    (c.artist.id.clone(), cached)
                })
                .collect();
            let mut map = HashMap::new();
            map.insert(release_id.to_string(), artists);
            map
        };

    let resolved_artist = localized_release_artist(
        &release.artist_credit,
        &release_artist_data,
        release_id,
        &preferred_locales,
    );

    // Load pre-filtered candidates from intermediate table (written by Stage 1)
    let candidate_rows = match read_only_db.get_packing_candidates_for_release(release_id) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query packing candidates: {}", e),
            );
        }
    };

    if candidate_rows.is_empty() {
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Build candidate_inodes and corpus_info from the pre-computed candidate rows
    let mut candidate_inodes: HashMap<i64, RecordingMatch> = HashMap::new();
    let mut corpus_info: HashMap<i64, CorpusFileInfo> = HashMap::new();

    for row in &candidate_rows {
        candidate_inodes.entry(row.inode).or_insert_with(|| RecordingMatch {
            recording_id: row.recording_id.clone(),
            confidence: row.confidence,
        });

        corpus_info.entry(row.inode).or_insert_with(|| {
            let mut tags = HashMap::new();
            if let Some(ref v) = row.tag_title {
                tags.insert("TITLE".to_string(), vec![v.clone()]);
            }
            if let Some(ref v) = row.tag_artist {
                tags.insert("ARTIST".to_string(), vec![v.clone()]);
            }
            if let Some(ref v) = row.tag_album {
                tags.insert("ALBUM".to_string(), vec![v.clone()]);
            }
            if let Some(ref v) = row.tag_tracknumber {
                tags.insert("TRACKNUMBER".to_string(), vec![v.clone()]);
            }
            CorpusFileInfo {
                parent_dir: row.parent_dir.clone(),
                tags,
                duration_ms: row.duration_ms,
            }
        });
    }

    // Build candidates by matching against the release tracklist
    let mut candidates: Vec<CandidateAssignment> = Vec::new();

    for (inode, rec_match) in &candidate_inodes {
        let corpus = corpus_info.get(inode);

        for medium in &release.media {
            for track in &medium.tracks {
                if track.recording.id == rec_match.recording_id {
                    let (score, breakdown) = compute_score(
                        rec_match,
                        track,
                        &resolved_artist,
                        &release.title,
                        corpus,
                        duration_tolerance_pct,
                    );

                    candidates.push(CandidateAssignment {
                        inode: *inode,
                        recording_id: rec_match.recording_id.clone(),
                        release_id: release_id.to_string(),
                        medium_pos: medium.position,
                        track_pos: track.position,
                        medium_format: medium.format.clone(),
                        track_number: track.number.clone(),
                        track_title: track.title.clone(),
                        score,
                        breakdown,
                    });
                }
            }
        }
    }

    // Compute directory cohesion for this release's candidates.
    // cohesion = (unique candidate inodes from this dir for this release) / (total audio files in dir)
    // dir_file_count comes from the candidates table (computed in Stage 1).
    let mut dir_candidate_inodes: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut dir_total: HashMap<String, i32> = HashMap::new();

    for candidate in &candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            dir_candidate_inodes
                .entry(corpus.parent_dir.clone())
                .or_default()
                .insert(candidate.inode);
        }
    }
    for row in &candidate_rows {
        dir_total.entry(row.parent_dir.clone()).or_insert(row.dir_file_count);
    }

    for candidate in &mut candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            let unique_candidates = dir_candidate_inodes
                .get(&corpus.parent_dir)
                .map(|s| s.len())
                .unwrap_or(0);
            let total_files = dir_total
                .get(&corpus.parent_dir)
                .copied()
                .unwrap_or(1)
                .max(1);

            candidate.breakdown.directory_cohesion =
                unique_candidates as f64 / total_files as f64;
            candidate.score = weighted_composite(&candidate.breakdown);
        }
    }

    // Optimal per-release assignment via Hungarian algorithm
    let optimal_pairs = hungarian_assignment(&candidates);

    // Write all candidates to scoring table, marking optimal ones
    let mut score_rows: Vec<PackingScoreRow> = Vec::new();
    let mut assigned_inodes: HashSet<i64> = HashSet::new();

    for candidate in &candidates {
        let slot = (candidate.medium_pos, candidate.track_pos);
        let is_optimal = optimal_pairs.contains(&(candidate.inode, slot));

        if is_optimal {
            assigned_inodes.insert(candidate.inode);
        }

        let breakdown_bytes = bincode::serialize(&candidate.breakdown).unwrap_or_default();

        score_rows.push(PackingScoreRow {
            release_id: candidate.release_id.clone(),
            inode: candidate.inode,
            recording_id: candidate.recording_id.clone(),
            medium_pos: candidate.medium_pos as i32,
            track_pos: candidate.track_pos as i32,
            track_title: candidate.track_title.clone(),
            medium_format: candidate.medium_format.clone(),
            track_number: candidate.track_number.clone(),
            score: candidate.score,
            score_breakdown: breakdown_bytes,
            is_optimal,
        });
    }

    if !score_rows.is_empty() {
        sender.write_packing_scores(score_rows, witness);
    }

    log_general(format!(
        "[COMPUTE] ScoreReleaseCandidates {}: {} candidates, {} optimal picks",
        release_id,
        candidates.len(),
        assigned_inodes.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Stage 3: ResolveReleaseConflicts
// ============================================================================

/// Execute ResolveReleaseConflicts — global conflict resolution.
///
/// Reads optimal picks from all releases, resolves cross-release inode conflicts
/// via greedy global assignment, emits ReleasePackingSignal per assigned inode.
pub fn execute_resolve_release_conflicts(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::ResolveReleaseConflicts;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Read all optimal picks from scoring table
    let optimal_scores = match read_only_db.get_optimal_packing_scores() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read scoring table: {}", e),
            );
        }
    };

    // Read manifest for total track counts and metadata
    let manifest = match read_only_db.get_packing_manifest() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read manifest: {}", e),
            );
        }
    };

    let manifest_map: HashMap<&str, (&str, &str, i32)> = manifest
        .iter()
        .map(|r| {
            (
                r.release_id.as_str(),
                (
                    r.release_title.as_str(),
                    r.release_artist.as_str(),
                    r.total_tracks,
                ),
            )
        })
        .collect();

    if optimal_scores.is_empty() {
        reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Count how many releases each inode appears in (alternatives)
    let mut inode_release_set: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in &optimal_scores {
        inode_release_set
            .entry(row.inode)
            .or_default()
            .insert(row.release_id.clone());
    }

    // Sort by score descending for greedy assignment
    let mut sorted_scores = optimal_scores;
    sorted_scores.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Greedy global assignment
    let mut assigned_inodes: HashSet<i64> = HashSet::new();
    let mut occupied_slots: HashSet<(String, i32, i32)> = HashSet::new();
    let mut assignments: Vec<_> = Vec::new();

    for row in &sorted_scores {
        if assigned_inodes.contains(&row.inode) {
            continue;
        }
        let slot = (
            row.release_id.clone(),
            row.medium_pos,
            row.track_pos,
        );
        if occupied_slots.contains(&slot) {
            continue;
        }
        assigned_inodes.insert(row.inode);
        occupied_slots.insert(slot);
        assignments.push(row);
    }

    // Compute per-release coverage
    let mut release_filled: HashMap<&str, u32> = HashMap::new();
    for row in &assignments {
        *release_filled.entry(&row.release_id).or_default() += 1;
    }

    // Load corpus paths from candidates table (no full corpus scan needed)
    let corpus_paths: HashMap<i64, String> = match read_only_db.get_candidate_paths() {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query candidate paths: {}", e),
            );
        }
    };

    // Emit signals
    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();

    for row in &assignments {
        let path = match corpus_paths.get(&row.inode) {
            Some(p) => p.clone(),
            None => continue,
        };

        let (release_title, release_artist, total_tracks): (String, String, i32) =
            match manifest_map.get(row.release_id.as_str()) {
                Some(&(title, artist, total)) => {
                    (title.to_string(), artist.to_string(), total)
                }
                None => continue,
            };

        let filled = release_filled
            .get(row.release_id.as_str())
            .copied()
            .unwrap_or(0);

        let alternatives_count = inode_release_set
            .get(&row.inode)
            .map(|s| s.len() as u16)
            .unwrap_or(1);

        let breakdown: PackingScoreBreakdown =
            bincode::deserialize(&row.score_breakdown).unwrap_or(PackingScoreBreakdown {
                acoustid_confidence: 0.0,
                duration_match: 0.0,
                tag_similarity: 0.0,
                track_number_match: 0.0,
                directory_cohesion: 0.0,
            });

        let signal = ReleasePackingSignal {
            inode: row.inode,
            path,
            data: ReleasePackingData {
                release_id: row.release_id.clone(),
                release_title,
                release_artist,
                track_position: row.track_pos as u32,
                medium_position: row.medium_pos as u32,
                medium_format: row.medium_format.clone(),
                track_number: row.track_number.clone(),
                recording_id: row.recording_id.clone(),
                track_title: row.track_title.clone(),
                score: row.score,
                score_breakdown: breakdown,
                alternatives_count,
                release_coverage: filled as f32 / total_tracks.max(1) as f32,
                match_method: MatchMethod::AcoustId,
            },
        };

        computed.push(ComputedCorpusSignal::new(
            signal.inode,
            TypedSignalWrite::ReleasePacking(signal),
        ));
    }

    let unique_releases = release_filled.len();
    let (cleared, new, updated, unchanged) =
        reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, computed, witness);

    log_general(format!(
        "[COMPUTE] ResolveReleaseConflicts: {} assigned to {} releases | \
         signals: cleared={}, new={}, updated={}, unchanged={}",
        assignments.len(),
        unique_releases,
        cleared,
        new,
        updated,
        unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Stage 4: EliminateByDirectory
// ============================================================================

/// Execute EliminateByDirectory — assign unmatched files by elimination in cohesive directories.
///
/// For directories where all AcoustID-assigned inodes map to a single release,
/// finds unassigned audio files and maps them to unfilled track slots.
/// Records (fingerprint, recording_id) pairs for future AcoustID submission.
#[allow(clippy::needless_range_loop)] // Hungarian algorithm uses 1-based index arithmetic
pub fn execute_eliminate_by_directory(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::EliminateByDirectory;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };
    let duration_tolerance_pct = config.opinions.release_packing.duration_tolerance_pct;

    // Read all current assignments from Stage 3
    let assignments = match read_only_db.get_release_packing_assignments() {
        Ok(a) => a,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read packing assignments: {}", e),
            );
        }
    };

    if assignments.is_empty() {
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Build per-directory state: which inodes assigned, to which release(s), which slots filled
    let assigned_inodes: HashSet<i64> = assignments.iter().map(|(inode, _, _, _, _)| *inode).collect();

    struct DirState {
        release_ids: HashSet<String>,
        assigned_inodes: HashSet<i64>,
        filled_slots: HashSet<(u32, u32)>,
    }

    let mut dir_states: HashMap<String, DirState> = HashMap::new();

    for (inode, path, release_id, medium_pos, track_pos) in &assignments {
        let parent_dir = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let state = dir_states.entry(parent_dir).or_insert_with(|| DirState {
            release_ids: HashSet::new(),
            assigned_inodes: HashSet::new(),
            filled_slots: HashSet::new(),
        });
        state.release_ids.insert(release_id.clone());
        state.assigned_inodes.insert(*inode);
        state.filled_slots.insert((*medium_pos, *track_pos));
    }

    // Filter to directories mapping to exactly one release
    let cohesive_dirs: Vec<CohesiveDir> = dir_states
        .into_iter()
        .filter(|(_, state)| state.release_ids.len() == 1)
        .map(|(dir, state)| {
            let release_id = state.release_ids.into_iter().next().unwrap();
            (dir, release_id, state.filled_slots)
        })
        .collect();

    log_general(format!(
        "[COMPUTE] EliminateByDirectory: {} cohesive directories to check",
        cohesive_dirs.len()
    ));

    // Read manifest for release metadata
    let manifest = match read_only_db.get_packing_manifest() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read manifest: {}", e),
            );
        }
    };
    let manifest_map: HashMap<&str, (&str, &str, i32)> = manifest
        .iter()
        .map(|r| (r.release_id.as_str(), (r.release_title.as_str(), r.release_artist.as_str(), r.total_tracks)))
        .collect();

    let mut total_eliminations = 0u32;
    let mut total_submissions = 0u32;
    let mut pending_submissions: Vec<write_thread::PendingAcoustIdSubmission> = Vec::new();

    for (dir, release_id, filled_slots) in &cohesive_dirs {
        // Load release tracklist to find unfilled slots
        let release = match read_only_db.get_mb_release_cache(release_id) {
            Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
                Ok(r) => r,
                Err(_) => continue,
            },
            _ => continue,
        };

        // Enumerate unfilled slots
        let mut unfilled: Vec<(u32, u32, &musicbrainz::MbTrack)> = Vec::new();
        for medium in &release.media {
            for track in &medium.tracks {
                let slot = (medium.position, track.position);
                if !filled_slots.contains(&slot) {
                    unfilled.push((medium.position, track.position, track));
                }
            }
        }

        if unfilled.is_empty() {
            continue;
        }

        // Find unassigned audio files in this directory
        let unassigned = match read_only_db.get_unassigned_audio_in_directory(dir, &assigned_inodes) {
            Ok(files) => files,
            Err(_) => continue,
        };

        if unassigned.is_empty() {
            continue;
        }

        // Determine matching strategy
        let n_unassigned = unassigned.len();
        let n_unfilled = unfilled.len();

        // Get release metadata for scoring and signal construction
        let (release_title, release_artist, total_tracks) = match manifest_map.get(release_id.as_str()) {
            Some(&(title, artist, total)) => (title.to_string(), artist.to_string(), total),
            None => continue,
        };

        let filled_count = filled_slots.len() as u32;

        // Build assignment pairs
        let mut elimination_assignments: Vec<(usize, usize)> = Vec::new(); // (unassigned_idx, unfilled_idx)

        if n_unassigned == n_unfilled {
            // Exact match: use Hungarian on duration scores if N > 1
            if n_unassigned == 1 {
                elimination_assignments.push((0, 0));
            } else {
                // Build cost matrix using duration match scores
                let n = n_unassigned;
                let inf = f64::MAX / 2.0;
                let mut u = vec![0.0f64; n + 1];
                let mut v = vec![0.0f64; n + 1];
                let mut col_to_row = vec![0usize; n + 1];

                // Build cost matrix (negate duration_match for minimization)
                let mut cost = vec![vec![0.0f64; n]; n];
                for (ui, (_, _, _, dur_ms)) in unassigned.iter().enumerate() {
                    for (fi, (med_pos, trk_pos, track)) in unfilled.iter().enumerate() {
                        let mb_dur = track.length.or(track.recording.length);
                        let duration_match = match (*dur_ms, mb_dur) {
                            (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                                let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                                if ratio > duration_tolerance_pct { 0.0 } else { 1.0 - (ratio / duration_tolerance_pct) }
                            }
                            _ => 0.5,
                        };
                        cost[ui][fi] = -duration_match;
                        let _ = (med_pos, trk_pos); // used in assignment below
                    }
                }

                // Run Hungarian
                for i in 1..=n {
                    let mut links = vec![0usize; n + 1];
                    let mut mins = vec![inf; n + 1];
                    let mut visited = vec![false; n + 1];
                    col_to_row[0] = i;
                    let mut j0 = 0usize;

                    loop {
                        visited[j0] = true;
                        let row = col_to_row[j0];
                        let mut delta = inf;
                        let mut j1 = 0usize;
                        for j in 1..=n {
                            if visited[j] { continue; }
                            let val = cost[row - 1][j - 1] - u[row] - v[j];
                            if val < mins[j] { mins[j] = val; links[j] = j0; }
                            if mins[j] < delta { delta = mins[j]; j1 = j; }
                        }
                        for j in 0..=n {
                            if visited[j] { u[col_to_row[j]] += delta; v[j] -= delta; }
                            else { mins[j] -= delta; }
                        }
                        j0 = j1;
                        if col_to_row[j0] == 0 { break; }
                    }
                    loop {
                        let prev = links[j0];
                        col_to_row[j0] = col_to_row[prev];
                        j0 = prev;
                        if j0 == 0 { break; }
                    }
                }

                for j in 1..=n {
                    let row = col_to_row[j];
                    if row > 0 {
                        elimination_assignments.push((row - 1, j - 1));
                    }
                }
            }
        } else if n_unassigned > n_unfilled {
            // More files than slots: pick best duration matches, only if > 0.5
            // Build all duration scores and use Hungarian on rectangular (pad to square)
            let n = n_unassigned.max(n_unfilled);
            let inf = f64::MAX / 2.0;
            let mut u = vec![0.0f64; n + 1];
            let mut v = vec![0.0f64; n + 1];
            let mut col_to_row = vec![0usize; n + 1];

            let mut cost = vec![vec![0.0f64; n]; n];
            for (ui, (_, _, _, dur_ms)) in unassigned.iter().enumerate() {
                for (fi, (_, _, track)) in unfilled.iter().enumerate() {
                    let mb_dur = track.length.or(track.recording.length);
                    let duration_match = match (*dur_ms, mb_dur) {
                        (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                            let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                            if ratio > duration_tolerance_pct { 0.0 } else { 1.0 - (ratio / duration_tolerance_pct) }
                        }
                        _ => 0.5,
                    };
                    cost[ui][fi] = -duration_match;
                }
                // Dummy columns get 0.0 cost (no penalty for unassigned files)
            }

            for i in 1..=n {
                let mut links = vec![0usize; n + 1];
                let mut mins = vec![inf; n + 1];
                let mut visited = vec![false; n + 1];
                col_to_row[0] = i;
                let mut j0 = 0usize;

                loop {
                    visited[j0] = true;
                    let row = col_to_row[j0];
                    let mut delta = inf;
                    let mut j1 = 0usize;
                    for j in 1..=n {
                        if visited[j] { continue; }
                        let val = cost[row - 1][j - 1] - u[row] - v[j];
                        if val < mins[j] { mins[j] = val; links[j] = j0; }
                        if mins[j] < delta { delta = mins[j]; j1 = j; }
                    }
                    for j in 0..=n {
                        if visited[j] { u[col_to_row[j]] += delta; v[j] -= delta; }
                        else { mins[j] -= delta; }
                    }
                    j0 = j1;
                    if col_to_row[j0] == 0 { break; }
                }
                loop {
                    let prev = links[j0];
                    col_to_row[j0] = col_to_row[prev];
                    j0 = prev;
                    if j0 == 0 { break; }
                }
            }

            for j in 1..=n {
                let row = col_to_row[j];
                if row == 0 { continue; }
                let ui = row - 1;
                let fi = j - 1;
                if ui < n_unassigned && fi < n_unfilled {
                    // Only accept if duration_match > 0.5
                    if cost[ui][fi] < -0.5 {
                        elimination_assignments.push((ui, fi));
                    }
                }
            }
        } else {
            // More slots than files — skip, not enough evidence
            continue;
        }

        // Emit signals and record submissions for each elimination assignment
        let preferred_locales = &config.opinions.external_matching.preferred_locales;
        let _ = preferred_locales; // used for artist resolution already done via manifest

        for (ui, fi) in &elimination_assignments {
            let (inode, path, fingerprint_hex, dur_ms) = &unassigned[*ui];
            let (medium_pos, track_pos, track) = &unfilled[*fi];

            // Compute score breakdown for elimination match
            let mb_dur = track.length.or(track.recording.length);
            let duration_match = match (*dur_ms, mb_dur) {
                (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                    let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                    if ratio > duration_tolerance_pct { 0.0 } else { 1.0 - (ratio / duration_tolerance_pct) }
                }
                _ => 0.5,
            };

            // Load tags for this inode for tag_similarity scoring
            let raw_tags = read_only_db.get_corpus_tags(*inode).unwrap_or_default();
            let mut corpus_tags: HashMap<String, Vec<String>> = HashMap::new();
            for tag in raw_tags {
                corpus_tags.entry(tag.tag_name).or_default().push(tag.tag_value);
            }

            let title_sim = corpus_tags
                .get("TITLE")
                .and_then(|v| v.first())
                .map(|t| {
                    let track_sim = strsim::normalized_levenshtein(t, &track.title);
                    let rec_sim = strsim::normalized_levenshtein(t, &track.recording.title);
                    track_sim.max(rec_sim)
                })
                .unwrap_or(0.0);

            let artist_sim = corpus_tags
                .get("ARTIST")
                .and_then(|v| v.first())
                .map(|a| strsim::normalized_levenshtein(a, &release_artist))
                .unwrap_or(0.0);

            let album_sim = corpus_tags
                .get("ALBUM")
                .and_then(|v| v.first())
                .map(|a| strsim::normalized_levenshtein(a, &release_title))
                .unwrap_or(0.0);

            let tag_similarity = 0.4 * title_sim + 0.35 * artist_sim + 0.25 * album_sim;

            let track_number_match = corpus_tags
                .get("TRACKNUMBER")
                .and_then(|v| v.first())
                .and_then(|tn| tn.parse::<u32>().ok())
                .map(|tn| if tn == *track_pos { 1.0 } else { 0.0 })
                .unwrap_or(0.0);

            let breakdown = PackingScoreBreakdown {
                acoustid_confidence: 0.0,
                duration_match,
                tag_similarity,
                track_number_match,
                directory_cohesion: 1.0,
            };
            let score = weighted_composite(&breakdown);

            let new_filled = filled_count + elimination_assignments.len() as u32;

            let signal = ReleasePackingSignal {
                inode: *inode,
                path: path.clone(),
                data: ReleasePackingData {
                    release_id: release_id.clone(),
                    release_title: release_title.clone(),
                    release_artist: release_artist.clone(),
                    track_position: *track_pos,
                    medium_position: *medium_pos,
                    medium_format: release.media.iter()
                        .find(|m| m.position == *medium_pos)
                        .and_then(|m| m.format.clone()),
                    track_number: track.number.clone(),
                    recording_id: track.recording.id.clone(),
                    track_title: track.title.clone(),
                    score,
                    score_breakdown: breakdown,
                    alternatives_count: 1,
                    release_coverage: new_filled as f32 / total_tracks.max(1) as f32,
                    match_method: MatchMethod::Elimination,
                },
            };

            // Write directly (INSERT OR REPLACE), not reconcile
            sender.write_typed_signal(TypedSignalWrite::ReleasePacking(signal), witness);
            total_eliminations += 1;

            // Record pending submission if fingerprint available
            if let Some(fp_hex) = fingerprint_hex {
                if let Some(duration) = dur_ms {
                    pending_submissions.push(write_thread::PendingAcoustIdSubmission {
                        fingerprint: fp_hex.clone(),
                        recording_id: track.recording.id.clone(),
                        duration_ms: *duration,
                        source: "elimination".to_string(),
                    });
                    total_submissions += 1;
                }
            }
        }
    }

    // Write pending submissions in bulk
    if !pending_submissions.is_empty() {
        sender.write_pending_acoustid_submissions(pending_submissions, witness);
    }

    log_general(format!(
        "[COMPUTE] EliminateByDirectory: {} eliminations, {} pending AcoustID submissions",
        total_eliminations, total_submissions
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Stage 5: AnalyzeReleaseGaps
// ============================================================================

/// Execute AnalyzeReleaseGaps — identify unmatched tracks and near-miss patterns.
pub fn execute_analyze_release_gaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::AnalyzeReleaseGaps;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Read the manifest and optimal scores
    let manifest = match read_only_db.get_packing_manifest() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read manifest: {}", e),
            );
        }
    };

    let optimal_scores = match read_only_db.get_optimal_packing_scores() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to read scoring table: {}", e),
            );
        }
    };

    // Build set of assigned inodes (from ReleasePackingSignal, written by Stage 3)
    let assigned_inodes: HashSet<i64> = read_only_db
        .corpus_signal_all_inodes::<ReleasePackingSignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // === Unmatched corpus tracks ===
    // Inodes with external matches but no release assignment

    // Build inode → recording IDs map from candidates table (no full external_matches scan)
    let mut inode_recordings: HashMap<i64, Vec<String>> = HashMap::new();
    match read_only_db.get_candidate_inode_recordings() {
        Ok(rows) => {
            for (inode, recording_id) in rows {
                inode_recordings.entry(inode).or_default().push(recording_id);
            }
        }
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query candidate inode recordings: {}", e),
            );
        }
    }

    // Build inode → considered releases from scoring table
    let mut inode_considered_releases: HashMap<i64, Vec<String>> = HashMap::new();
    for row in &optimal_scores {
        inode_considered_releases
            .entry(row.inode)
            .or_default()
            .push(row.release_id.clone());
    }

    // Load corpus paths from candidates table (no full corpus scan)
    let corpus_paths: HashMap<i64, String> = match read_only_db.get_candidate_paths() {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query candidate paths: {}", e),
            );
        }
    };

    let mut unmatched_signals: Vec<ComputedCorpusSignal> = Vec::new();
    for (inode, recording_ids) in &inode_recordings {
        if assigned_inodes.contains(inode) {
            continue;
        }
        let path = match corpus_paths.get(inode) {
            Some(p) => p.clone(),
            None => continue,
        };
        let considered = inode_considered_releases
            .get(inode)
            .cloned()
            .unwrap_or_default();

        // Only emit if this inode was actually scored (had candidates)
        if considered.is_empty() {
            continue;
        }

        let mut recording_ids_dedup = recording_ids.clone();
        recording_ids_dedup.sort();
        recording_ids_dedup.dedup();

        let mut considered_dedup = considered;
        considered_dedup.sort();
        considered_dedup.dedup();

        let signal = UnmatchedCorpusTrackSignal {
            inode: *inode,
            path,
            data: UnmatchedCorpusTrackData {
                recording_ids: recording_ids_dedup,
                considered_release_ids: considered_dedup,
            },
        };
        unmatched_signals.push(ComputedCorpusSignal::new(
            *inode,
            TypedSignalWrite::UnmatchedCorpusTrack(signal),
        ));
    }

    let (uc_cleared, uc_new, uc_updated, uc_unchanged) =
        reconcile_corpus_signals::<UnmatchedCorpusTrackSignal>(
            read_only_db,
            &sender,
            unmatched_signals,
            witness,
        );

    // === Unfilled release slots ===
    // For each release in manifest, find track positions not filled by any assignment
    let mut filled_slots: HashMap<String, HashSet<(i32, i32)>> = HashMap::new();
    for row in &optimal_scores {
        if assigned_inodes.contains(&row.inode) {
            filled_slots
                .entry(row.release_id.clone())
                .or_default()
                .insert((row.medium_pos, row.track_pos));
        }
    }

    // We need release tracklist info to identify unfilled slots
    let mut unfilled_signals: Vec<ComputedAggregateSignal> = Vec::new();

    for manifest_row in &manifest {
        // Load release tracklist for slot enumeration
        let release = match read_only_db.get_mb_release_cache(&manifest_row.release_id) {
            Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
                Ok(r) => r,
                Err(_) => continue,
            },
            _ => continue,
        };

        let filled_for_release = filled_slots
            .get(&manifest_row.release_id)
            .cloned()
            .unwrap_or_default();

        let filled_count = filled_for_release.len() as u32;

        // Only emit unfilled slots for releases that have at least one filled slot
        if filled_count == 0 {
            continue;
        }

        for medium in &release.media {
            for track in &medium.tracks {
                let slot = (medium.position as i32, track.position as i32);
                if filled_for_release.contains(&slot) {
                    continue;
                }

                let key = format!(
                    "{}:{}:{}",
                    manifest_row.release_id, medium.position, track.position
                );

                let signal = UnfilledReleaseSlotSignal {
                    key: key.clone(),
                    data: UnfilledReleaseSlotData {
                        release_id: manifest_row.release_id.clone(),
                        release_title: manifest_row.release_title.clone(),
                        release_artist: manifest_row.release_artist.clone(),
                        medium_pos: medium.position,
                        track_pos: track.position,
                        track_title: track.title.clone(),
                        recording_id: track.recording.id.clone(),
                        filled_count,
                        total_tracks: manifest_row.total_tracks as u32,
                    },
                };

                unfilled_signals.push(ComputedAggregateSignal::new(
                    key,
                    TypedSignalWrite::UnfilledReleaseSlot(signal),
                ));
            }
        }
    }

    let (us_cleared, us_new, us_updated, us_unchanged) =
        reconcile_aggregate_signals::<UnfilledReleaseSlotSignal>(
            read_only_db,
            &sender,
            unfilled_signals,
            witness,
        );

    // === Near-miss detection ===
    // A release with (n-1)/n tracks matched, all from the same directory containing
    // exactly n audio files. The unmatched file is the likely missing track.
    let mut near_miss_signals: Vec<ComputedAggregateSignal> = Vec::new();

    for manifest_row in &manifest {
        let total = manifest_row.total_tracks as u32;
        if total < 2 {
            continue;
        }

        let filled_for_release = match filled_slots.get(&manifest_row.release_id) {
            Some(s) => s,
            None => continue,
        };
        let filled_count = filled_for_release.len() as u32;

        // Near-miss: exactly (n-1)/n filled
        if filled_count + 1 != total {
            continue;
        }

        // Get the assigned inodes for this release
        let mut release_inodes: Vec<i64> = Vec::new();
        for row in &optimal_scores {
            if row.release_id == manifest_row.release_id
                && assigned_inodes.contains(&row.inode)
            {
                release_inodes.push(row.inode);
            }
        }

        // Check directory cohesion: all assigned inodes from same directory
        let mut dirs: HashSet<String> = HashSet::new();
        for inode in &release_inodes {
            if let Some(path) = corpus_paths.get(inode) {
                if let Some(parent) = Path::new(path).parent() {
                    dirs.insert(parent.to_string_lossy().to_string());
                }
            }
        }

        if dirs.len() != 1 {
            continue;
        }
        let directory = dirs.into_iter().next().unwrap();

        // Count audio files in that directory (from corpus)
        let dir_audio_count = corpus_paths
            .values()
            .filter(|p| {
                Path::new(p.as_str())
                    .parent()
                    .map(|parent| parent.to_string_lossy() == directory)
                    .unwrap_or(false)
            })
            .count() as u32;

        // The directory should have exactly total tracks (n audio files for an n-track release)
        if dir_audio_count != total {
            continue;
        }

        // Find the unmatched file in this directory
        let assigned_set: HashSet<i64> = release_inodes.iter().copied().collect();
        let mut candidate_inode: Option<i64> = None;
        let mut candidate_path: Option<String> = None;

        for (inode, path) in &corpus_paths {
            if assigned_set.contains(inode) {
                continue;
            }
            if let Some(parent) = Path::new(path.as_str()).parent() {
                if parent.to_string_lossy() == directory {
                    candidate_inode = Some(*inode);
                    candidate_path = Some(path.clone());
                    break;
                }
            }
        }

        let (candidate_inode, candidate_path) = match (candidate_inode, candidate_path) {
            (Some(i), Some(p)) => (i, p),
            _ => continue,
        };

        // Find the missing slot
        let release = match read_only_db.get_mb_release_cache(&manifest_row.release_id) {
            Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
                Ok(r) => r,
                Err(_) => continue,
            },
            _ => continue,
        };

        let mut missing_slot = None;
        for medium in &release.media {
            for track in &medium.tracks {
                let slot = (medium.position as i32, track.position as i32);
                if !filled_for_release.contains(&slot) {
                    missing_slot = Some((medium.position, track.position, track.title.clone(), track.recording.id.clone()));
                    break;
                }
            }
            if missing_slot.is_some() {
                break;
            }
        }

        let (missing_medium, missing_track, missing_title, missing_recording) =
            match missing_slot {
                Some(s) => s,
                None => continue,
            };

        let key = format!("{}:{}", manifest_row.release_id, directory);
        let signal = NearMissReleaseSignal {
            key: key.clone(),
            data: NearMissReleaseData {
                release_id: manifest_row.release_id.clone(),
                release_title: manifest_row.release_title.clone(),
                release_artist: manifest_row.release_artist.clone(),
                directory,
                candidate_inode,
                candidate_path,
                missing_medium_pos: missing_medium,
                missing_track_pos: missing_track,
                missing_track_title: missing_title,
                missing_recording_id: missing_recording,
                filled_count,
                total_tracks: total,
            },
        };

        near_miss_signals.push(ComputedAggregateSignal::new(
            key,
            TypedSignalWrite::NearMissRelease(signal),
        ));
    }

    let (nm_cleared, nm_new, nm_updated, nm_unchanged) =
        reconcile_aggregate_signals::<NearMissReleaseSignal>(
            read_only_db,
            &sender,
            near_miss_signals,
            witness,
        );

    log_general(format!(
        "[COMPUTE] AnalyzeReleaseGaps: \
         unmatched_corpus: cleared={}, new={}, updated={}, unchanged={} | \
         unfilled_slots: cleared={}, new={}, updated={}, unchanged={} | \
         near_miss: cleared={}, new={}, updated={}, unchanged={}",
        uc_cleared, uc_new, uc_updated, uc_unchanged, us_cleared, us_new, us_updated,
        us_unchanged, nm_cleared, nm_new, nm_updated, nm_unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
