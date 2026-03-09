//! Stage 1 (PackReleases) and Stage 2 (ScoreReleaseCandidates).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::db::types::Zone;
use crate::db::write_thread::{self, PackingScoreRow};
use crate::db::ReadOnlyDb;
use crate::external::musicbrainz::{self, MbRelease};
use crate::logging::log_general;
use crate::meta::computations::helpers::reconcile_corpus_signals;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::computations::{Computation, PipelineStage};
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::{PackingScoreBreakdown, ReleasePackingSignal};

use super::hungarian::{
    kuhn_munkres, localized_release_artist, score_all_directories,
    TargetDirs,
};
use super::scoring::{compute_score, weighted_composite};
use super::types::{
    group_all_per_inode, CandidateAssignment, CorpusFileInfo, RecordingMatch,
};
use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};

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
        "[COMPUTE] PackReleases: {} inodes after filtering (confidence={}, parse_fail={}), {} release IDs",
        inode_recordings.len(), filtered_confidence, recording_parse_failures,
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
            (release_id.clone(), total, release.title.clone(), artist)
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
        let parent_dir = corpus.map(|c| c.parent_dir.clone()).unwrap_or_default();
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
            release_id: release_id.clone(),
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
/// per-release assignment via Hungarian algorithm, then fills remaining slots via
/// elimination matching. Writes results to `release_packing_scores`.
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
    let candidate_weights = config.opinions.release_packing.candidate_weights.clone();
    let elimination_weights = config.opinions.release_packing.elimination_weights.clone();
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
        candidate_inodes
            .entry(row.inode)
            .or_insert_with(|| RecordingMatch {
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
                        &candidate_weights,
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

    // --- Directory-constrained packing ---
    // Run Hungarian for every candidate directory, keep the best-scoring assignment.
    // This replaces the old greedy heuristic (select_target_directory) with exhaustive
    // per-directory scoring for deterministic results.

    let mut dir_candidate_inodes: HashMap<String, HashSet<i64>> = HashMap::new();

    for candidate in &candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            dir_candidate_inodes
                .entry(corpus.parent_dir.clone())
                .or_default()
                .insert(candidate.inode);
        }
    }

    let (target_dirs, optimal_pairs) = score_all_directories(
        &candidates,
        &corpus_info,
        &dir_candidate_inodes,
        &release.media,
    );

    // Filter candidates to winning directory only (per-medium aware)
    candidates.retain(|c| {
        corpus_info
            .get(&c.inode)
            .map(|ci| target_dirs.contains(&ci.parent_dir, c.medium_pos))
            .unwrap_or(false)
    });

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
            match_method: 0,
            fingerprint_hex: None,
            raw_duration_ms: None,
        });
    }

    // =====================================================================
    // Per-release elimination: fill unfilled slots from target directory only
    // =====================================================================
    // Scan ONLY the target dir(s) for unassigned audio files to fill remaining slots.
    // This is the key constraint: elimination never reaches outside the selected directory.

    let mut elimination_count = 0u32;

    // Build per-directory AcoustID inodes + global filled slots from optimal pairs
    let mut dir_acoustid_inodes: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut all_filled_slots: HashSet<(u32, u32)> = HashSet::new();

    for candidate in &candidates {
        let slot = (candidate.medium_pos, candidate.track_pos);
        if optimal_pairs.contains(&(candidate.inode, slot)) {
            all_filled_slots.insert(slot);
            if let Some(corpus) = corpus_info.get(&candidate.inode) {
                dir_acoustid_inodes
                    .entry(corpus.parent_dir.clone())
                    .or_default()
                    .insert(candidate.inode);
            }
        }
    }

    // Enumerate unfilled slots ONCE
    let mut unfilled: Vec<(u32, u32, &musicbrainz::MbTrack)> = Vec::new();
    for medium in &release.media {
        for track in &medium.tracks {
            let slot = (medium.position, track.position);
            if !all_filled_slots.contains(&slot) {
                unfilled.push((medium.position, track.position, track));
            }
        }
    }

    if !unfilled.is_empty() {
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

        // For PerMedium targets, map each file to its medium based on parent dir.
        // This prevents cross-medium contamination in elimination: Disc 1 files
        // can only fill Medium 1 slots, Disc 2 files only Medium 2 slots.
        let dir_to_medium: HashMap<String, u32> = match &target_dirs {
            TargetDirs::PerMedium(mapping) => {
                mapping.iter().map(|(mp, d)| (d.clone(), *mp)).collect()
            }
            _ => HashMap::new(),
        };
        let file_medium: Vec<Option<u32>> = all_unassigned
            .iter()
            .map(|(_, path, _, _)| {
                if dir_to_medium.is_empty() {
                    None // Single target — no per-medium constraint
                } else {
                    Path::new(path)
                        .parent()
                        .and_then(|p| p.to_str())
                        .and_then(|parent| dir_to_medium.get(parent).copied())
                }
            })
            .collect();

        if !all_unassigned.is_empty() {
            // Pre-load tags for each unassigned file
            let unassigned_tags: Vec<HashMap<String, Vec<String>>> = all_unassigned
                .iter()
                .map(|(inode, _, _, _)| {
                    let raw = read_only_db.get_corpus_tags(*inode).unwrap_or_default();
                    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
                    for tag in raw {
                        tags.entry(tag.tag_name).or_default().push(tag.tag_value);
                    }
                    tags
                })
                .collect();

            // =============================================================
            // Phase 1: High-confidence title pre-assignment
            // =============================================================
            // Before the full Hungarian, lock in files where title similarity
            // is unambiguously high (>0.95) and the match is 1:1. This prevents
            // tracknumber from stealing slots that have clear title matches
            // when the rip's track ordering diverges from MB.

            const TITLE_PREASSIGN_THRESHOLD: f64 = 0.95;

            // Compute title similarity for every file × slot pair
            let title_sims: Vec<Vec<f64>> = unassigned_tags
                .iter()
                .map(|tags| {
                    unfilled
                        .iter()
                        .map(|(_, _, track)| {
                            tags.get("TITLE")
                                .and_then(|v| v.first())
                                .map(|t| {
                                    let track_sim = strsim::normalized_levenshtein(t, &track.title);
                                    let rec_sim =
                                        strsim::normalized_levenshtein(t, &track.recording.title);
                                    track_sim.max(rec_sim)
                                })
                                .unwrap_or(0.0)
                        })
                        .collect()
                })
                .collect();

            // Find unambiguous 1:1 matches above threshold
            let mut preassigned_files: HashSet<usize> = HashSet::new();
            let mut preassigned_slots: HashSet<usize> = HashSet::new();

            // For each file, find slots above threshold; for each slot, find files above threshold
            let n_files = all_unassigned.len();
            let n_slots = unfilled.len();

            // file_candidates[ui] = list of slot indices with sim > threshold
            // For PerMedium targets, only consider slots from the file's medium
            let file_candidates: Vec<Vec<usize>> = (0..n_files)
                .map(|ui| {
                    (0..n_slots)
                        .filter(|&fi| {
                            title_sims[ui][fi] > TITLE_PREASSIGN_THRESHOLD
                                && file_medium[ui]
                                    .map_or(true, |fm| unfilled[fi].0 == fm)
                        })
                        .collect()
                })
                .collect();

            // slot_candidates[fi] = list of file indices with sim > threshold
            let slot_candidates: Vec<Vec<usize>> = (0..n_slots)
                .map(|fi| {
                    (0..n_files)
                        .filter(|&ui| {
                            title_sims[ui][fi] > TITLE_PREASSIGN_THRESHOLD
                                && file_medium[ui]
                                    .map_or(true, |fm| unfilled[fi].0 == fm)
                        })
                        .collect()
                })
                .collect();

            // Assign where both sides have exactly one candidate (unambiguous 1:1)
            for ui in 0..n_files {
                if file_candidates[ui].len() != 1 {
                    continue;
                }
                let fi = file_candidates[ui][0];
                if slot_candidates[fi].len() != 1 {
                    continue;
                }
                // Unambiguous: this file matches exactly one slot, that slot matches exactly one file
                preassigned_files.insert(ui);
                preassigned_slots.insert(fi);

                let (inode, _path, fingerprint_hex, dur_ms) = &all_unassigned[ui];
                let (medium_pos, track_pos, track) = &unfilled[fi];
                let tags = &unassigned_tags[ui];

                // Build full breakdown for the pre-assigned match
                let mb_dur = track.length.or(track.recording.length);
                let duration_match = match (*dur_ms, mb_dur) {
                    (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                        let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                        if ratio > duration_tolerance_pct {
                            0.0
                        } else {
                            1.0 - (ratio / duration_tolerance_pct)
                        }
                    }
                    _ => 0.5,
                };

                let artist_sim = tags
                    .get("ARTIST")
                    .and_then(|v| v.first())
                    .map(|a| strsim::normalized_levenshtein(a, &resolved_artist))
                    .unwrap_or(0.0);

                let album_sim = tags
                    .get("ALBUM")
                    .and_then(|v| v.first())
                    .map(|a| strsim::normalized_levenshtein(a, &release.title))
                    .unwrap_or(0.0);

                let track_number_match = tags
                    .get("TRACKNUMBER")
                    .and_then(|v| v.first())
                    .and_then(|tn| tn.parse::<u32>().ok())
                    .map(|tn| if tn == *track_pos { 1.0 } else { 0.0 })
                    .unwrap_or(0.0);

                let breakdown = PackingScoreBreakdown {
                    acoustid_confidence: 0.0,
                    duration_match,
                    title_match: title_sims[ui][fi],
                    artist_match: artist_sim,
                    album_match: album_sim,
                    track_number_match,
                };
                let score = weighted_composite(&breakdown, &elimination_weights);
                let breakdown_bytes = bincode::serialize(&breakdown).unwrap_or_default();

                score_rows.push(PackingScoreRow {
                    release_id: release_id.to_string(),
                    inode: *inode,
                    recording_id: track.recording.id.clone(),
                    medium_pos: *medium_pos as i32,
                    track_pos: *track_pos as i32,
                    track_title: track.title.clone(),
                    medium_format: release
                        .media
                        .iter()
                        .find(|m| m.position == *medium_pos)
                        .and_then(|m| m.format.clone()),
                    track_number: track.number.clone(),
                    score,
                    score_breakdown: breakdown_bytes,
                    is_optimal: true,
                    match_method: 1,
                    fingerprint_hex: fingerprint_hex.clone(),
                    raw_duration_ms: *dur_ms,
                });

                elimination_count += 1;
            }

            // Filter out pre-assigned entries for the Hungarian pass
            let remaining_unassigned: Vec<usize> = (0..n_files)
                .filter(|ui| !preassigned_files.contains(ui))
                .collect();
            let remaining_unfilled: Vec<usize> = (0..n_slots)
                .filter(|fi| !preassigned_slots.contains(fi))
                .collect();

            // =============================================================
            // Phase 2: Hungarian assignment on remaining files × slots
            // =============================================================
            let n_unassigned = remaining_unassigned.len();
            let n_unfilled = remaining_unfilled.len();
            let n = n_unassigned.max(n_unfilled);
            let mut cost = vec![vec![0.0f64; n]; n];

            for (ri, &ui) in remaining_unassigned.iter().enumerate() {
                let tags = &unassigned_tags[ui];
                let dur_ms = &all_unassigned[ui].3;
                for (rj, &fi) in remaining_unfilled.iter().enumerate() {
                    // Per-medium affinity: prohibit cross-medium assignment
                    if let Some(fm) = file_medium[ui] {
                        if unfilled[fi].0 != fm {
                            cost[ri][rj] = 1e9;
                            continue;
                        }
                    }
                    let (_, _, track) = &unfilled[fi];
                    let mb_dur = track.length.or(track.recording.length);
                    let duration_match = match (*dur_ms, mb_dur) {
                        (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                            let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                            if ratio > duration_tolerance_pct {
                                0.0
                            } else {
                                1.0 - (ratio / duration_tolerance_pct)
                            }
                        }
                        _ => 0.5,
                    };

                    let title_sim = tags
                        .get("TITLE")
                        .and_then(|v| v.first())
                        .map(|t| {
                            let track_sim = strsim::normalized_levenshtein(t, &track.title);
                            let rec_sim = strsim::normalized_levenshtein(t, &track.recording.title);
                            track_sim.max(rec_sim)
                        })
                        .unwrap_or(0.0);

                    let artist_sim = tags
                        .get("ARTIST")
                        .and_then(|v| v.first())
                        .map(|a| strsim::normalized_levenshtein(a, &resolved_artist))
                        .unwrap_or(0.0);

                    let album_sim = tags
                        .get("ALBUM")
                        .and_then(|v| v.first())
                        .map(|a| strsim::normalized_levenshtein(a, &release.title))
                        .unwrap_or(0.0);

                    let track_number_match = tags
                        .get("TRACKNUMBER")
                        .and_then(|v| v.first())
                        .and_then(|tn| tn.parse::<u32>().ok())
                        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
                        .unwrap_or(0.0);

                    let elim_breakdown = PackingScoreBreakdown {
                        acoustid_confidence: 0.0,
                        duration_match,
                        title_match: title_sim,
                        artist_match: artist_sim,
                        album_match: album_sim,
                        track_number_match,
                    };
                    cost[ri][rj] = -weighted_composite(&elim_breakdown, &elimination_weights);
                }
            }

            let col_to_row = if n > 0 {
                kuhn_munkres(&cost, n)
            } else {
                Vec::new()
            };
            for (j, &row) in col_to_row.iter().enumerate().skip(1) {
                if row == 0 {
                    continue;
                }
                let ri = row - 1;
                let rj = j - 1;
                if ri >= n_unassigned || rj >= n_unfilled {
                    continue;
                }

                // Map back to original indices
                let ui = remaining_unassigned[ri];
                let fi = remaining_unfilled[rj];

                let (inode, _path, fingerprint_hex, dur_ms) = &all_unassigned[ui];
                let (medium_pos, track_pos, track) = &unfilled[fi];
                let corpus_tags = &unassigned_tags[ui];

                // Compute full score breakdown for the elimination match
                let mb_dur = track.length.or(track.recording.length);
                let duration_match = match (*dur_ms, mb_dur) {
                    (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                        let ratio = (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
                        if ratio > duration_tolerance_pct {
                            0.0
                        } else {
                            1.0 - (ratio / duration_tolerance_pct)
                        }
                    }
                    _ => 0.5,
                };

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
                    .map(|a| strsim::normalized_levenshtein(a, &resolved_artist))
                    .unwrap_or(0.0);

                let album_sim = corpus_tags
                    .get("ALBUM")
                    .and_then(|v| v.first())
                    .map(|a| strsim::normalized_levenshtein(a, &release.title))
                    .unwrap_or(0.0);

                let track_number_match = corpus_tags
                    .get("TRACKNUMBER")
                    .and_then(|v| v.first())
                    .and_then(|tn| tn.parse::<u32>().ok())
                    .map(|tn| if tn == *track_pos { 1.0 } else { 0.0 })
                    .unwrap_or(0.0);

                let breakdown = PackingScoreBreakdown {
                    acoustid_confidence: 0.0,
                    duration_match,
                    title_match: title_sim,
                    artist_match: artist_sim,
                    album_match: album_sim,
                    track_number_match,
                };
                let score = weighted_composite(&breakdown, &elimination_weights);
                let breakdown_bytes = bincode::serialize(&breakdown).unwrap_or_default();

                score_rows.push(PackingScoreRow {
                    release_id: release_id.to_string(),
                    inode: *inode,
                    recording_id: track.recording.id.clone(),
                    medium_pos: *medium_pos as i32,
                    track_pos: *track_pos as i32,
                    track_title: track.title.clone(),
                    medium_format: release
                        .media
                        .iter()
                        .find(|m| m.position == *medium_pos)
                        .and_then(|m| m.format.clone()),
                    track_number: track.number.clone(),
                    score,
                    score_breakdown: breakdown_bytes,
                    is_optimal: true,
                    match_method: 1,
                    fingerprint_hex: fingerprint_hex.clone(),
                    raw_duration_ms: *dur_ms,
                });

                elimination_count += 1;
            }
        }
    }

    if !score_rows.is_empty() {
        sender.write_packing_scores(score_rows, witness);
    }

    log_general(format!(
        "[COMPUTE] ScoreReleaseCandidates {}: {} candidates, {} optimal AcoustID picks, {} elimination picks",
        release_id,
        candidates.len(),
        assigned_inodes.len(),
        elimination_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
