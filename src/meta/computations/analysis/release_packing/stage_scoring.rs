//! Stage 1 (PackReleases) and Stage 2 (ScoreReleaseCandidates).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::corpus::tags::TagSet;
use crate::db::types::Zone;
use crate::db::write_thread::{self, PackingScoreRow};
use crate::db::ReadOnlyDb;
use crate::external::musicbrainz::{self, MbRelease};
use crate::logging::log_general;
use crate::meta::computations::helpers::reconcile_corpus_signals;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::computations::{Computation, PipelineStage};
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::ReleasePackingSignal;

use super::hungarian::{
    kuhn_munkres, localized_release_artist, score_all_directories,
    TargetDirs,
};
use super::scoring::{
    compute_elimination_breakdown, compute_score, compute_title_similarity, weighted_composite,
};
use super::types::{
    group_all_per_inode, CandidateAssignment, CorpusFileInfo, RecordingMatch,
};
use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};
use crate::meta::signals::data::PackingScoreBreakdown;

/// Compute title similarity matrix between unassigned files and unfilled slots.
///
/// Returns a `n_files × n_slots` matrix where `[i][j]` is the best title
/// similarity between file `i`'s tags and slot `j`'s track.
fn compute_title_similarity_matrix(
    unassigned_tags: &[TagSet],
    unfilled: &[(u32, u32, &musicbrainz::MbTrack)],
) -> Vec<Vec<f64>> {
    unassigned_tags
        .iter()
        .map(|tags| {
            unfilled
                .iter()
                .map(|(_, _, track)| compute_title_similarity(tags, track))
                .collect()
        })
        .collect()
}

/// Find unambiguous 1:1 title pre-assignments above a similarity threshold.
///
/// A pre-assignment is made when a file has exactly one slot above threshold
/// and that slot has exactly one file above threshold. Per-medium affinity is
/// respected: if a file has a known medium, only slots from that medium qualify.
///
/// Returns `(file_idx, slot_idx)` pairs into the unassigned/unfilled arrays.
fn find_unambiguous_preassignments(
    title_sims: &[Vec<f64>],
    threshold: f64,
    file_medium: &[Option<u32>],
    unfilled: &[(u32, u32, &musicbrainz::MbTrack)],
) -> Vec<(usize, usize)> {
    let n_files = title_sims.len();
    let n_slots = if n_files > 0 { title_sims[0].len() } else { 0 };

    let medium_ok = |ui: usize, fi: usize| -> bool {
        file_medium[ui].is_none_or(|fm| unfilled[fi].0 == fm)
    };

    // file_candidates[ui] = slot indices above threshold (respecting medium affinity)
    let file_candidates: Vec<Vec<usize>> = (0..n_files)
        .map(|ui| {
            (0..n_slots)
                .filter(|&fi| title_sims[ui][fi] > threshold && medium_ok(ui, fi))
                .collect()
        })
        .collect();

    // slot_candidates[fi] = file indices above threshold
    let slot_candidates: Vec<Vec<usize>> = (0..n_slots)
        .map(|fi| {
            (0..n_files)
                .filter(|&ui| title_sims[ui][fi] > threshold && medium_ok(ui, fi))
                .collect()
        })
        .collect();

    let mut result = Vec::new();
    for (ui, fc) in file_candidates.iter().enumerate() {
        if fc.len() != 1 {
            continue;
        }
        let fi = fc[0];
        if slot_candidates[fi].len() != 1 {
            continue;
        }
        result.push((ui, fi));
    }
    result
}

/// Run Hungarian assignment on remaining (non-preassigned) files and slots.
///
/// Builds a cost matrix from elimination breakdowns, solves via Kuhn-Munkres,
/// and returns `PackingScoreRow`s for each assignment.
#[allow(clippy::too_many_arguments)]
fn run_elimination_hungarian(
    remaining_unassigned: &[usize],
    remaining_unfilled: &[usize],
    all_unassigned: &[(i64, String, Option<String>, Option<i64>)],
    unfilled: &[(u32, u32, &musicbrainz::MbTrack)],
    unassigned_tags: &[TagSet],
    file_medium: &[Option<u32>],
    release_id: &str,
    release: &MbRelease,
    resolved_artist: &str,
    duration_tolerance_pct: f64,
    elimination_weights: &crate::config::PackingWeights,
) -> Vec<PackingScoreRow> {
    let n_unassigned = remaining_unassigned.len();
    let n_unfilled = remaining_unfilled.len();
    let n = n_unassigned.max(n_unfilled);
    if n == 0 {
        return Vec::new();
    }

    let mut cost = vec![vec![0.0f64; n]; n];
    for (ri, &ui) in remaining_unassigned.iter().enumerate() {
        let tags = &unassigned_tags[ui];
        let dur_ms = &all_unassigned[ui].3;
        for (rj, &fi) in remaining_unfilled.iter().enumerate() {
            if let Some(fm) = file_medium[ui] {
                if unfilled[fi].0 != fm {
                    cost[ri][rj] = 1e9;
                    continue;
                }
            }
            let (_, _, track) = &unfilled[fi];
            let elim_breakdown = compute_elimination_breakdown(
                tags, track, resolved_artist, &release.title,
                *dur_ms, duration_tolerance_pct,
            );
            cost[ri][rj] = -weighted_composite(&elim_breakdown, elimination_weights);
        }
    }

    let col_to_row = kuhn_munkres(&cost, n);
    let mut rows = Vec::new();

    for (j, &row) in col_to_row.iter().enumerate().skip(1) {
        if row == 0 {
            continue;
        }
        let ri = row - 1;
        let rj = j - 1;
        if ri >= n_unassigned || rj >= n_unfilled {
            continue;
        }

        let ui = remaining_unassigned[ri];
        let fi = remaining_unfilled[rj];

        let (inode, _path, fingerprint_hex, dur_ms) = &all_unassigned[ui];
        let (medium_pos, track_pos, track) = &unfilled[fi];
        let corpus_tags = &unassigned_tags[ui];

        let breakdown = compute_elimination_breakdown(
            corpus_tags, track, resolved_artist, &release.title,
            *dur_ms, duration_tolerance_pct,
        );
        rows.push(build_elimination_score_row(
            release_id, *inode, track, *medium_pos, *track_pos,
            &release.media, &breakdown, elimination_weights,
            fingerprint_hex.clone(), *dur_ms,
        ));
    }

    rows
}

/// Build a `PackingScoreRow` for an elimination match.
///
/// Computes the weighted score and serializes the breakdown, looking up the
/// medium format from the release media list.
#[allow(clippy::too_many_arguments)]
fn build_elimination_score_row(
    release_id: &str,
    inode: i64,
    track: &musicbrainz::MbTrack,
    medium_pos: u32,
    track_pos: u32,
    media: &[musicbrainz::MbMedium],
    breakdown: &PackingScoreBreakdown,
    weights: &crate::config::PackingWeights,
    fingerprint_hex: Option<String>,
    raw_duration_ms: Option<i64>,
) -> PackingScoreRow {
    let score = weighted_composite(breakdown, weights);
    let breakdown_bytes = bincode::serialize(breakdown).unwrap_or_default();

    PackingScoreRow {
        release_id: release_id.to_string(),
        inode,
        recording_id: track.recording.id.clone(),
        medium_pos: medium_pos as i32,
        track_pos: track_pos as i32,
        track_title: track.title.clone(),
        medium_format: media
            .iter()
            .find(|m| m.position == medium_pos)
            .and_then(|m| m.format.clone()),
        track_number: track.number.clone(),
        score,
        score_breakdown: breakdown_bytes,
        is_optimal: true,
        match_method: 1,
        fingerprint_hex,
        raw_duration_ms,
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
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = AnalysisComputation::PackReleases;

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);
    let min_confidence = config.opinions.release_packing.min_confidence;
    let preferred_locales = &config.opinions.external_matching.preferred_locales;

    let source_key = ExternalSource::AcoustID.to_key();

    // === Load all external matches (corpus only) ===
    let all_rows = match read_only_db.get_external_matches_slim(source_key) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query external matches: {}", e),
            );
        }
    };

    let all_per_inode = group_all_per_inode(all_rows);

    if all_per_inode.is_empty() {
        // No matches — clear stale signals and exit pipeline
        let stats = reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if stats.cleared > 0 {
            log_general(format!(
                "[COMPUTE] PackReleases: cleared {} stale signals (no external matches)",
                stats.cleared
            ));
        }
        return Result::success(computation, Vec::new());
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
                format!("Failed to query corpus files: {}", e),
            );
        }
    };

    let mut corpus_info: HashMap<i64, CorpusFileInfo> = HashMap::new();
    let mut corpus_inode_paths: HashMap<i64, String> = HashMap::new();
    for (af, tags) in files_with_tags {
        let path = af.path().to_string();
        let parent_dir = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let inode = af.inode();
        corpus_inode_paths.insert(inode, path);
        let tag_set = TagSet::new(
            tags.into_iter()
                .flat_map(|(k, vs)| vs.into_iter().map(move |v| (k.clone(), v))),
        );
        corpus_info.insert(
            inode,
            CorpusFileInfo {
                parent_dir,
                tags: tag_set,
                duration_ms: af.audio.duration_ms,
            },
        );
    }

    // === Parse recordings, filter by confidence, collect release IDs ===
    // Duration is NOT filtered here — it's a scoring dimension in Stage 2.
    // Fingerprint matches already imply structural similarity; duration mismatches
    // are penalized by the duration_match scoring weight, not hard-gated.
    let mut inode_recordings: HashMap<i64, Vec<RecordingMatch>> = HashMap::new();
    let mut all_release_ids: HashSet<String> = HashSet::new();
    let mut recording_releases: HashMap<String, Vec<String>> = HashMap::new();
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
                Ok(None) => {
                    recording_parse_failures += 1;
                    if recording_parse_failures <= 3 {
                        log_general(format!(
                            "[COMPUTE] PackReleases: cache miss for recording_id={} (inode={})",
                            row.recording_id, inode
                        ));
                    }
                    continue;
                }
                Err(e) => {
                    recording_parse_failures += 1;
                    if recording_parse_failures <= 3 {
                        log_general(format!(
                            "[COMPUTE] PackReleases: DB error for recording_id={}: {}",
                            row.recording_id, e
                        ));
                    }
                    continue;
                }
            };

            let recording = match musicbrainz::parse_recording(&recording_json) {
                Ok(r) => r,
                Err(e) => {
                    recording_parse_failures += 1;
                    if recording_parse_failures <= 3 {
                        log_general(format!(
                            "[COMPUTE] PackReleases: JSON parse error for recording_id={}: {}",
                            row.recording_id, e
                        ));
                    }
                    continue;
                }
            };

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

    // === Inject pinned releases from config ===
    // Build pinned_dir_releases: dir_path (with "corpus/" prefix) → release_id
    let mut pinned_dir_releases: HashMap<String, String> = HashMap::new();
    for sd in &config.source_dirs {
        if let Some(ref release_id) = sd.pinned_release {
            let dir_path = sd.path.display().to_string();
            pinned_dir_releases.insert(dir_path, release_id.clone());
            all_release_ids.insert(release_id.clone());
        }
    }
    if !pinned_dir_releases.is_empty() {
        log_general(format!(
            "[COMPUTE] PackReleases: {} pinned release(s) from config",
            pinned_dir_releases.len()
        ));
    }

    log_general(format!(
        "[COMPUTE] PackReleases: {} inodes after filtering (confidence={}, parse_fail={}), {} release IDs",
        inode_recordings.len(), filtered_confidence, recording_parse_failures,
        all_release_ids.len()
    ));

    if inode_recordings.is_empty() {
        let stats = reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if stats.cleared > 0 {
            log_general(format!(
                "[COMPUTE] PackReleases: cleared {} stale signals (all filtered)",
                stats.cleared
            ));
        }
        return Result::success(computation, Vec::new());
    }

    // === Load release tracklists ===
    let release_id_refs: Vec<&str> = all_release_ids.iter().map(|s| s.as_str()).collect();
    let release_cache_entries = match read_only_db.get_mb_release_cache_bulk(&release_id_refs) {
        Ok(entries) => entries,
        Err(e) => {
            return Result::failure(
                computation,
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
        return Result::success(computation, Vec::new());
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
    let manifest_rows: Vec<(String, i32, String, String, i32)> = release_tracklists
        .iter()
        .map(|(release_id, release)| {
            let total: i32 = release.media.iter().map(|m| m.tracks.len() as i32).sum();
            let media_count = release.media.len() as i32;
            let artist = localized_release_artist(
                &release.artist_credit,
                &release_artist_data,
                release_id,
                preferred_locales,
            );
            (release_id.clone(), total, release.title.clone(), artist, media_count)
        })
        .collect();
    sender.write_packing_manifest(manifest_rows, witness);

    // === Compute directory file counts for cohesion scoring ===
    let mut dir_total_files: HashMap<&str, i32> = HashMap::new();
    for info in corpus_info.values() {
        *dir_total_files.entry(&info.parent_dir).or_default() += 1;
    }

    // === Build inode→path lookup from all_per_inode ===
    let inode_paths: HashMap<i64, &str> = all_per_inode
        .iter()
        .map(|(inode, rows)| (*inode, rows[0].path.as_str()))
        .collect();

    // === Write candidate rows, deduplicating per (release_id, inode) ===
    // Keeps the highest-confidence recording for each pair.
    let mut deduped: HashMap<(&str, i64), write_thread::PackingCandidateRow> = HashMap::new();

    for (inode, rec_matches) in &inode_recordings {
        let corpus = corpus_info.get(inode);
        let inode_path = inode_paths.get(inode).copied().unwrap_or_default();
        let parent_dir = corpus.map(|c| c.parent_dir.as_str()).unwrap_or_default();
        let dir_file_count = dir_total_files.get(parent_dir).copied().unwrap_or(1);

        for rec_match in rec_matches {
            if let Some(release_ids) = recording_releases.get(&rec_match.recording_id) {
                for release_id in release_ids {
                    if !release_tracklists.contains_key(release_id) {
                        continue;
                    }

                    let key = (release_id.as_str(), *inode);
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
                                .map(|s| s.to_string());
                            let tag_artist = corpus
                                .and_then(|c| c.tags.get("ARTIST"))
                                .map(|s| s.to_string());
                            let tag_album = corpus
                                .and_then(|c| c.tags.get("ALBUM"))
                                .map(|s| s.to_string());
                            let tag_tracknumber = corpus
                                .and_then(|c| c.tags.get("TRACKNUMBER"))
                                .map(|s| s.to_string());

                            e.insert(write_thread::PackingCandidateRow {
                                release_id: release_id.to_string(),
                                inode: *inode,
                                recording_id: rec_match.recording_id.clone(),
                                confidence: rec_match.confidence,
                                path: inode_path.to_string(),
                                parent_dir: parent_dir.to_string(),
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

    // === Inject synthetic candidates for pinned directories ===
    // For each pinned dir, ensure all its corpus inodes have at least one candidate
    // row for the pinned release. This guarantees Stage 2 considers the pinned dir.
    let mut pinned_injected = 0usize;
    for (dir_path, release_id) in &pinned_dir_releases {
        // Find all corpus inodes in this directory
        for (inode, info) in &corpus_info {
            if info.parent_dir == *dir_path {
                let key = (release_id.as_str(), *inode);
                if let std::collections::hash_map::Entry::Vacant(entry) = deduped.entry(key) {
                    let inode_path = corpus_inode_paths.get(inode).map(|s| s.as_str()).unwrap_or_default();
                    let dir_file_count = dir_total_files.get(dir_path.as_str()).copied().unwrap_or(1);
                    entry.insert(
                        write_thread::PackingCandidateRow {
                            release_id: release_id.clone(),
                            inode: *inode,
                            recording_id: String::new(), // synthetic — no AcoustID match
                            confidence: 1.0,
                            path: inode_path.to_string(),
                            parent_dir: dir_path.to_string(),
                            duration_ms: info.duration_ms,
                            tag_title: info
                                .tags
                                .get("TITLE")
                                .map(|s| s.to_string()),
                            tag_artist: info
                                .tags
                                .get("ARTIST")
                                .map(|s| s.to_string()),
                            tag_album: info
                                .tags
                                .get("ALBUM")
                                .map(|s| s.to_string()),
                            tag_tracknumber: info
                                .tags
                                .get("TRACKNUMBER")
                                .map(|s| s.to_string()),
                            dir_file_count,
                        },
                    );
                    pinned_injected += 1;
                }
            }
        }
    }
    if pinned_injected > 0 {
        log_general(format!(
            "[COMPUTE] PackReleases: injected {} synthetic candidate rows for pinned releases",
            pinned_injected
        ));
    }

    let mut releases_with_candidates: HashSet<&str> = HashSet::new();
    for (release_id, _) in deduped.keys() {
        releases_with_candidates.insert(release_id);
    }
    let candidate_rows: Vec<write_thread::PackingCandidateRow> = deduped.into_values().collect();

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
            release_id: release_id.to_string(),
        })
        .collect();

    log_general(format!(
        "[COMPUTE] PackReleases: spawning {} ScoreReleaseCandidates, deferring mapping pipeline",
        spawn.len()
    ));

    // === Defer Stage 3 (ComputeReleaseMappings orchestrates the rest) ===
    let deferred_phases = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(
            AnalysisComputation::ComputeReleaseMappings,
        )],
    )];

    Result::pipeline(
        computation,
        spawn,
        deferred_phases,
    )
}

// ============================================================================
// Stage 2: ScoreReleaseCandidates
// ============================================================================

/// Load a release from the MB cache and resolve its locale-aware artist name.
fn load_release_with_artist(
    read_only_db: &ReadOnlyDb<'_>,
    release_id: &str,
    preferred_locales: &[String],
) -> Option<(MbRelease, String)> {
    let release = match read_only_db.get_mb_release_cache(release_id) {
        Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
            Ok(r) if !r.media.is_empty() => r,
            _ => return None,
        },
        _ => return None,
    };

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
        preferred_locales,
    );

    Some((release, resolved_artist))
}

/// Parse candidate rows into lookup structures for scoring.
///
/// Returns `(candidate_inodes, corpus_info, dir_file_counts)`:
/// - `candidate_inodes`: best recording match per inode
/// - `corpus_info`: file metadata (tags, duration, parent dir) per inode
/// - `dir_file_counts`: total audio file count per directory
fn build_candidate_structures(
    candidate_rows: &[crate::db::queries::external::PackingCandidateRow],
) -> (
    HashMap<i64, RecordingMatch>,
    HashMap<i64, CorpusFileInfo>,
    HashMap<String, usize>,
) {
    let mut candidate_inodes: HashMap<i64, RecordingMatch> = HashMap::new();
    let mut corpus_info: HashMap<i64, CorpusFileInfo> = HashMap::new();
    let mut dir_file_counts: HashMap<String, usize> = HashMap::new();

    for row in candidate_rows {
        dir_file_counts
            .entry(row.parent_dir.clone())
            .or_insert(row.dir_file_count as usize);
        candidate_inodes
            .entry(row.inode)
            .or_insert_with(|| RecordingMatch {
                recording_id: row.recording_id.clone(),
                confidence: row.confidence,
            });

        corpus_info.entry(row.inode).or_insert_with(|| {
            let tag_iter = [
                ("TITLE", &row.tag_title),
                ("ARTIST", &row.tag_artist),
                ("ALBUM", &row.tag_album),
                ("TRACKNUMBER", &row.tag_tracknumber),
            ]
            .into_iter()
            .filter_map(|(k, v)| v.as_ref().map(|val| (k.to_owned(), val.clone())));
            CorpusFileInfo {
                parent_dir: row.parent_dir.clone(),
                tags: TagSet::new(tag_iter),
                duration_ms: row.duration_ms,
            }
        });
    }

    (candidate_inodes, corpus_info, dir_file_counts)
}

/// Score each (inode, track_slot) pairing against the release tracklist.
fn score_candidates_against_tracklist(
    candidate_inodes: &HashMap<i64, RecordingMatch>,
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    release: &MbRelease,
    release_id: &str,
    resolved_artist: &str,
    duration_tolerance_pct: f64,
    candidate_weights: &crate::config::PackingWeights,
) -> Vec<CandidateAssignment> {
    let mut candidates = Vec::new();

    for (inode, rec_match) in candidate_inodes {
        let corpus = corpus_info.get(inode);

        for medium in &release.media {
            for track in &medium.tracks {
                if track.recording.id == rec_match.recording_id {
                    let (score, breakdown) = compute_score(
                        rec_match,
                        track,
                        resolved_artist,
                        &release.title,
                        corpus,
                        duration_tolerance_pct,
                        candidate_weights,
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

    candidates
}

/// Run directory-constrained packing: Hungarian per directory, pick best, write score rows.
///
/// Applies pinned release constraints, filters candidates to the winning directory,
/// and returns `(score_rows, assigned_inodes, target_dirs, optimal_pairs)`.
#[allow(clippy::type_complexity)]
fn run_directory_constrained_packing(
    mut candidates: Vec<CandidateAssignment>,
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    release: &MbRelease,
    dir_file_counts: &HashMap<String, usize>,
    pinned_dirs_for_release: &[String],
) -> (
    Vec<PackingScoreRow>,
    HashSet<i64>,
    TargetDirs,
    HashSet<(i64, (u32, u32))>,
) {
    let mut dir_candidate_inodes: HashMap<String, HashSet<i64>> = HashMap::new();

    for candidate in &candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            dir_candidate_inodes
                .entry(corpus.parent_dir.clone())
                .or_default()
                .insert(candidate.inode);
        }
    }

    // Pinned release constraint: restrict to pinned dirs only
    if !pinned_dirs_for_release.is_empty() {
        dir_candidate_inodes.retain(|dir, _| pinned_dirs_for_release.contains(dir));
        for pinned_dir in pinned_dirs_for_release {
            dir_candidate_inodes.entry(pinned_dir.clone()).or_default();
        }
    }

    let (target_dirs, optimal_pairs) = score_all_directories(
        &candidates,
        corpus_info,
        &dir_candidate_inodes,
        &release.media,
        dir_file_counts,
    );

    // Filter candidates to winning directory only (per-medium aware)
    candidates.retain(|c| {
        corpus_info
            .get(&c.inode)
            .map(|ci| target_dirs.contains(&ci.parent_dir, c.medium_pos))
            .unwrap_or(false)
    });

    // Build score rows, marking optimal assignments — consume candidates by value
    let mut score_rows: Vec<PackingScoreRow> = Vec::new();
    let mut assigned_inodes: HashSet<i64> = HashSet::new();

    for candidate in candidates {
        let slot = (candidate.medium_pos, candidate.track_pos);
        let is_optimal = optimal_pairs.contains(&(candidate.inode, slot));

        if is_optimal {
            assigned_inodes.insert(candidate.inode);
        }

        let breakdown_bytes = bincode::serialize(&candidate.breakdown).unwrap_or_default();

        score_rows.push(PackingScoreRow {
            release_id: candidate.release_id,
            inode: candidate.inode,
            recording_id: candidate.recording_id,
            medium_pos: candidate.medium_pos as i32,
            track_pos: candidate.track_pos as i32,
            track_title: candidate.track_title,
            medium_format: candidate.medium_format,
            track_number: candidate.track_number,
            score: candidate.score,
            score_breakdown: breakdown_bytes,
            is_optimal,
            match_method: 0,
            fingerprint_hex: None,
            raw_duration_ms: None,
        });
    }

    (score_rows, assigned_inodes, target_dirs, optimal_pairs)
}

/// Fill unfilled track slots via elimination matching within the target directory.
///
/// Uses title pre-assignment for high-confidence 1:1 matches, then Hungarian
/// assignment on the remainder. Returns `(elimination_count, additional_score_rows)`.
#[allow(clippy::too_many_arguments)]
fn run_elimination_phase(
    read_only_db: &ReadOnlyDb<'_>,
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    release: &MbRelease,
    release_id: &str,
    resolved_artist: &str,
    target_dirs: &TargetDirs,
    optimal_pairs: &HashSet<(i64, (u32, u32))>,
    duration_tolerance_pct: f64,
    title_preassign_threshold: f64,
    elimination_weights: &crate::config::PackingWeights,
) -> (u32, Vec<PackingScoreRow>) {
    // Build per-directory AcoustID inodes + global filled slots from optimal pairs
    let mut dir_acoustid_inodes: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut all_filled_slots: HashSet<(u32, u32)> = HashSet::new();

    for &(inode, slot) in optimal_pairs {
        all_filled_slots.insert(slot);
        if let Some(corpus) = corpus_info.get(&inode) {
            dir_acoustid_inodes
                .entry(corpus.parent_dir.clone())
                .or_default()
                .insert(inode);
        }
    }

    // Enumerate unfilled slots
    let mut unfilled: Vec<(u32, u32, &musicbrainz::MbTrack)> = Vec::new();
    for medium in &release.media {
        for track in &medium.tracks {
            let slot = (medium.position, track.position);
            if !all_filled_slots.contains(&slot) {
                unfilled.push((medium.position, track.position, track));
            }
        }
    }

    if unfilled.is_empty() {
        return (0, Vec::new());
    }

    // Collect unassigned audio files from target dirs only.
    // For per-medium dirs, only include files from the dir assigned to a medium
    // that still has unfilled slots (prevents cross-medium contamination).
    let mut all_unassigned: Vec<(i64, String, Option<String>, Option<i64>)> = Vec::new();
    let unfilled_media: HashSet<u32> = unfilled.iter().map(|(m, _, _)| *m).collect();
    let mut scanned_dirs: HashSet<String> = HashSet::new();
    for &medium_pos in &unfilled_media {
        if let Some(dir) = target_dirs.dir_for_medium(medium_pos) {
            if scanned_dirs.insert(dir.to_string()) {
                let acoustid_inodes =
                    dir_acoustid_inodes.get(dir).cloned().unwrap_or_default();
                match read_only_db.get_unassigned_audio_in_directory(dir, &acoustid_inodes) {
                    Ok(files) => all_unassigned.extend(files),
                    Err(_) => continue,
                }
            }
        }
    }

    if all_unassigned.is_empty() {
        return (0, Vec::new());
    }

    // For PerMedium targets, map each file to its medium based on parent dir.
    // This prevents cross-medium contamination in elimination.
    let dir_to_medium: HashMap<String, u32> = match target_dirs {
        TargetDirs::PerMedium(mapping) => {
            mapping.iter().map(|(mp, d)| (d.clone(), *mp)).collect()
        }
        _ => HashMap::new(),
    };
    let file_medium: Vec<Option<u32>> = all_unassigned
        .iter()
        .map(|(_, path, _, _)| {
            if dir_to_medium.is_empty() {
                None
            } else {
                Path::new(path)
                    .parent()
                    .and_then(|p| p.to_str())
                    .and_then(|parent| dir_to_medium.get(parent).copied())
            }
        })
        .collect();

    // Pre-load tags for each unassigned file
    let unassigned_tags: Vec<TagSet> = all_unassigned
        .iter()
        .map(|(inode, _, _, _)| {
            let raw = read_only_db.get_tags::<crate::zones::CorpusZone>(*inode).unwrap_or_default();
            TagSet::new(raw.into_iter().map(|t| (t.tag_name, t.tag_value)))
        })
        .collect();

    let mut elimination_count = 0u32;
    let mut score_rows = Vec::new();

    // Phase 1: High-confidence title pre-assignment
    // Lock in files where title similarity is unambiguously high and 1:1.
    let title_sims = compute_title_similarity_matrix(&unassigned_tags, &unfilled);
    let preassignments = find_unambiguous_preassignments(
        &title_sims, title_preassign_threshold, &file_medium, &unfilled,
    );

    let mut preassigned_files: HashSet<usize> = HashSet::new();
    let mut preassigned_slots: HashSet<usize> = HashSet::new();

    for &(ui, fi) in &preassignments {
        preassigned_files.insert(ui);
        preassigned_slots.insert(fi);

        let (inode, _path, fingerprint_hex, dur_ms) = &all_unassigned[ui];
        let (medium_pos, track_pos, track) = &unfilled[fi];
        let tags = &unassigned_tags[ui];

        let mut breakdown = compute_elimination_breakdown(
            tags, track, resolved_artist, &release.title,
            *dur_ms, duration_tolerance_pct,
        );
        breakdown.title_match = title_sims[ui][fi];
        score_rows.push(build_elimination_score_row(
            release_id, *inode, track, *medium_pos, *track_pos,
            &release.media, &breakdown, elimination_weights,
            fingerprint_hex.clone(), *dur_ms,
        ));

        elimination_count += 1;
    }

    // Phase 2: Hungarian assignment on remaining files × slots
    let n_files = all_unassigned.len();
    let n_slots = unfilled.len();
    let remaining_unassigned: Vec<usize> = (0..n_files)
        .filter(|ui| !preassigned_files.contains(ui))
        .collect();
    let remaining_unfilled: Vec<usize> = (0..n_slots)
        .filter(|fi| !preassigned_slots.contains(fi))
        .collect();

    let hungarian_rows = run_elimination_hungarian(
        &remaining_unassigned, &remaining_unfilled,
        &all_unassigned, &unfilled, &unassigned_tags, &file_medium,
        release_id, release, resolved_artist,
        duration_tolerance_pct, elimination_weights,
    );
    elimination_count += hungarian_rows.len() as u32;
    score_rows.extend(hungarian_rows);

    (elimination_count, score_rows)
}

/// Execute ScoreReleaseCandidates — score all candidate inodes for one release.
///
/// Reads pre-filtered candidates from `release_packing_candidates` (written by Stage 1),
/// loads the release tracklist, scores each (inode, track_slot) pairing, solves optimal
/// per-release assignment via Hungarian algorithm, then fills remaining slots via
/// elimination matching. Writes results to `release_packing_scores`.
pub fn execute_score_release_candidates(
    ctx: &ComputationContext<'_>,
    release_id: &str,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = AnalysisComputation::ScoreReleaseCandidates {
        release_id: release_id.to_string(),
    };

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);
    let duration_tolerance_pct = config.opinions.release_packing.duration_tolerance_pct;
    let candidate_weights = &config.opinions.release_packing.candidate_weights;
    let elimination_weights = &config.opinions.release_packing.elimination_weights;
    let title_preassign_threshold = config.opinions.release_packing.title_preassign_threshold;
    let preferred_locales = &config.opinions.external_matching.preferred_locales;

    // Load release tracklist and resolve locale-aware artist name
    let (release, resolved_artist) = match load_release_with_artist(
        read_only_db, release_id, preferred_locales,
    ) {
        Some(pair) => pair,
        None => {
            return Result::failure(
                computation,
                format!("Release {} not in cache or has no parseable tracklist", release_id),
            );
        }
    };

    // Load pre-filtered candidates from intermediate table (written by Stage 1)
    let candidate_rows = match read_only_db.get_packing_candidates_for_release(release_id) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query packing candidates: {}", e),
            );
        }
    };

    if candidate_rows.is_empty() {
        return Result::success(computation, Vec::new());
    }

    let (candidate_inodes, corpus_info, dir_file_counts) =
        build_candidate_structures(&candidate_rows);

    let candidates = score_candidates_against_tracklist(
        &candidate_inodes,
        &corpus_info,
        &release,
        release_id,
        &resolved_artist,
        duration_tolerance_pct,
        candidate_weights,
    );

    // Compute pinned directory constraint
    let pinned_dirs_for_release: Vec<String> = config
        .source_dirs
        .iter()
        .filter(|sd| sd.pinned_release.as_deref() == Some(release_id))
        .map(|sd| sd.path.display().to_string())
        .collect();

    let (mut score_rows, assigned_inodes, target_dirs, optimal_pairs) =
        run_directory_constrained_packing(
            candidates,
            &corpus_info,
            &release,
            &dir_file_counts,
            &pinned_dirs_for_release,
        );

    // Elimination: fill unfilled slots from target directory
    let (elimination_count, elimination_rows) = run_elimination_phase(
        read_only_db,
        &corpus_info,
        &release,
        release_id,
        &resolved_artist,
        &target_dirs,
        &optimal_pairs,
        duration_tolerance_pct,
        title_preassign_threshold,
        elimination_weights,
    );
    score_rows.extend(elimination_rows);

    if !score_rows.is_empty() {
        sender.write_packing_scores(score_rows, witness);
    }

    log_general(format!(
        "[COMPUTE] ScoreReleaseCandidates {}: {} optimal AcoustID picks, {} elimination picks",
        release_id,
        assigned_inodes.len(),
        elimination_count
    ));

    Result::success(computation, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(title: &str, rec_title: &str, position: u32, length: Option<i64>) -> musicbrainz::MbTrack {
        musicbrainz::MbTrack {
            position,
            number: position.to_string(),
            title: title.to_string(),
            length,
            recording: musicbrainz::MbTrackRecording {
                id: format!("rec-{}", position),
                title: rec_title.to_string(),
                length,
            },
        }
    }

    fn tags_with(pairs: &[(&str, &str)]) -> TagSet {
        TagSet::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    // --- compute_title_similarity_matrix ---

    #[test]
    fn test_title_similarity_matrix_exact_match() {
        let tags = vec![
            tags_with(&[("TITLE", "Song A")]),
            tags_with(&[("TITLE", "Song B")]),
        ];
        let track_a = make_track("Song A", "Song A", 1, None);
        let track_b = make_track("Song B", "Song B", 2, None);
        let unfilled = vec![(1u32, 1u32, &track_a), (1, 2, &track_b)];

        let matrix = compute_title_similarity_matrix(&tags, &unfilled);

        assert_eq!(matrix.len(), 2);
        assert!((matrix[0][0] - 1.0).abs() < 1e-10); // Song A → Song A
        assert!((matrix[1][1] - 1.0).abs() < 1e-10); // Song B → Song B
        assert!(matrix[0][1] < 0.9); // Song A → Song B should be low
    }

    #[test]
    fn test_title_similarity_matrix_empty_tags() {
        let tags: Vec<TagSet> = vec![TagSet::empty()];
        let track = make_track("Track", "Track", 1, None);
        let unfilled = vec![(1u32, 1u32, &track)];

        let matrix = compute_title_similarity_matrix(&tags, &unfilled);
        assert!((matrix[0][0]).abs() < 1e-10); // No TITLE tag → 0.0
    }

    // --- find_unambiguous_preassignments ---

    #[test]
    fn test_preassignment_unambiguous_1to1() {
        // File 0 matches only slot 0 (sim=1.0), File 1 matches only slot 1
        let title_sims = vec![
            vec![1.0, 0.0],
            vec![0.0, 1.0],
        ];
        let track_a = make_track("A", "A", 1, None);
        let track_b = make_track("B", "B", 2, None);
        let unfilled = vec![(1u32, 1u32, &track_a), (1, 2, &track_b)];
        let file_medium = vec![None, None];

        let result = find_unambiguous_preassignments(&title_sims, 0.9, &file_medium, &unfilled);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&(0, 0)));
        assert!(result.contains(&(1, 1)));
    }

    #[test]
    fn test_preassignment_ambiguous_skipped() {
        // File 0 matches both slots → ambiguous, no assignment
        let title_sims = vec![
            vec![1.0, 1.0],
        ];
        let track_a = make_track("A", "A", 1, None);
        let track_b = make_track("B", "B", 2, None);
        let unfilled = vec![(1u32, 1u32, &track_a), (1, 2, &track_b)];
        let file_medium = vec![None];

        let result = find_unambiguous_preassignments(&title_sims, 0.9, &file_medium, &unfilled);
        assert!(result.is_empty());
    }

    #[test]
    fn test_preassignment_respects_medium_affinity() {
        // File 0 is on medium 1, but slot 0 is on medium 2 → blocked
        let title_sims = vec![
            vec![1.0, 0.5],
        ];
        let track_a = make_track("A", "A", 1, None);
        let track_b = make_track("B", "B", 2, None);
        let unfilled = vec![(2u32, 1u32, &track_a), (1, 2, &track_b)];
        let file_medium = vec![Some(1u32)]; // File belongs to medium 1

        let result = find_unambiguous_preassignments(&title_sims, 0.4, &file_medium, &unfilled);
        // File 0 can only match slot 1 (medium 1), and if that's unambiguous...
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (0, 1));
    }

    #[test]
    fn test_preassignment_below_threshold_ignored() {
        let title_sims = vec![
            vec![0.5], // below threshold
        ];
        let track = make_track("A", "A", 1, None);
        let unfilled = vec![(1u32, 1u32, &track)];
        let file_medium = vec![None];

        let result = find_unambiguous_preassignments(&title_sims, 0.9, &file_medium, &unfilled);
        assert!(result.is_empty());
    }

    // --- build_elimination_score_row ---

    #[test]
    fn test_build_elimination_score_row_fields() {
        let track = make_track("Test Track", "Test Track", 3, Some(200000));
        let media = vec![musicbrainz::MbMedium {
            position: 1,
            format: Some("CD".to_string()),
            tracks: vec![track.clone()],
        }];
        let breakdown = PackingScoreBreakdown {
            acoustid_confidence: 0.0,
            duration_match: 1.0,
            title_match: 0.9,
            artist_match: 0.8,
            album_match: 0.7,
            track_number_match: 1.0,
        };
        let weights = crate::config::PackingWeights {
            acoustid_confidence: 0.0,
            duration_match: 1.0,
            title_match: 1.0,
            artist_match: 1.0,
            album_match: 1.0,
            track_number_match: 1.0,
        };

        let row = build_elimination_score_row(
            "rel-123", 42, &track, 1, 3, &media, &breakdown, &weights,
            Some("abc123".to_string()), Some(200000),
        );

        assert_eq!(row.release_id, "rel-123");
        assert_eq!(row.inode, 42);
        assert_eq!(row.recording_id, "rec-3");
        assert_eq!(row.medium_pos, 1);
        assert_eq!(row.track_pos, 3);
        assert_eq!(row.track_title, "Test Track");
        assert_eq!(row.medium_format.as_deref(), Some("CD"));
        assert!(row.is_optimal);
        assert_eq!(row.match_method, 1);
        assert_eq!(row.fingerprint_hex.as_deref(), Some("abc123"));
        assert_eq!(row.raw_duration_ms, Some(200000));
        assert!(row.score > 0.0);
    }
}
