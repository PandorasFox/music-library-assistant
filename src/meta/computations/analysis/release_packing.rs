//! Release bin-packing computation.
//!
//! Assigns corpus files to MusicBrainz releases by building a bipartite graph
//! of inodes ↔ recordings ↔ releases, scoring each possible (inode, track)
//! assignment, and greedily assigning files to their best-matching release.
//!
//! Manual trigger only — not part of ScheduleContentAnalysis.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::db::queries::external::ExternalMatchRow;
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::db::write_thread;
use crate::external::musicbrainz::{self, MbArtistCredit, MbRelease};
use crate::logging::log_general;
use crate::meta::computations::helpers::{ComputedCorpusSignal, reconcile_corpus_signals};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::{
    PackingScoreBreakdown, ReleasePackingData, ReleasePackingSignal, TypedSignalWrite,
};

use super::{Computation, Result};

// ============================================================================
// Internal types
// ============================================================================

/// Corpus file metadata needed for scoring.
struct CorpusFileInfo {
    path: String,
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
    /// How many distinct releases this inode had candidates for.
    alternatives_count: u16,
}

// ============================================================================
// Computation entry point
// ============================================================================

/// Execute PackReleases — bin-pack corpus files into MusicBrainz releases.
pub fn execute_pack_releases(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::PackReleases;

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

    // Load config for filtering thresholds.
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
    let min_confidence = config.opinions.release_packing.min_confidence;
    let preferred_locales = config.opinions.external_matching.preferred_locales.clone();

    let source_key = ExternalSource::AcoustID.to_key();

    // === STEP 1: Load all external matches (corpus only) ===
    let all_rows = match read_only_db.get_external_matches_for_derivation(source_key) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query external matches: {}", e),
            );
        }
    };

    let best_per_inode = group_best_per_inode(all_rows);

    if best_per_inode.is_empty() {
        let (cleared, _, _, _) =
            reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, Vec::new(), witness);
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
        best_per_inode.len()
    ));

    // === STEP 2: Load corpus metadata (tags + duration + path) ===
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
                    path,
                    parent_dir,
                    tags,
                    duration_ms: af.audio.duration_ms,
                },
            )
        })
        .collect();

    // === STEP 3: Parse recordings, filter, collect release IDs ===
    let mut inode_recordings: HashMap<i64, Vec<RecordingMatch>> = HashMap::new();
    let mut all_release_ids: HashSet<String> = HashSet::new();
    let mut recording_releases: HashMap<String, Vec<String>> = HashMap::new();
    let mut filtered_duration = 0usize;
    let mut filtered_confidence = 0usize;
    let mut recording_parse_failures = 0usize;

    for (inode, row) in &best_per_inode {
        // Filter by confidence
        if row.confidence < min_confidence {
            filtered_confidence += 1;
            continue;
        }

        // Parse MB recording cache to get releases
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

        // Filter by duration mismatch
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

        // Collect release IDs from this recording
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

    log_general(format!(
        "[COMPUTE] PackReleases: {} inodes after filtering (duration={}, confidence={}, parse_fail={}), {} release IDs to load",
        inode_recordings.len(), filtered_duration, filtered_confidence, recording_parse_failures,
        all_release_ids.len()
    ));

    if inode_recordings.is_empty() {
        let (cleared, _, _, _) =
            reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, Vec::new(), witness);
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] PackReleases: cleared {} stale signals (all filtered)",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // === STEP 4: Load release tracklists ===
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
        release_tracklists.len(), missing_tracklists
    ));

    if release_tracklists.is_empty() {
        log_general("[COMPUTE] PackReleases: no releases with tracklist data, skipping");
        reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, Vec::new(), witness);
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // === STEP 4b: Load artist data for locale-aware name resolution ===
    let release_artist_data: HashMap<String, Vec<(String, Option<musicbrainz::MbArtist>)>> =
        if preferred_locales.is_empty() {
            HashMap::new()
        } else {
            // Collect unique artist IDs across all release credits.
            let mut artist_ids_seen: HashSet<String> = HashSet::new();
            for release in release_tracklists.values() {
                for credit in &release.artist_credit {
                    artist_ids_seen.insert(credit.artist.id.clone());
                }
            }
            // Bulk-load artist cache.
            let mut artist_cache: HashMap<String, musicbrainz::MbArtist> = HashMap::new();
            for artist_id in &artist_ids_seen {
                if let Ok(Some((json, _))) = read_only_db.get_mb_artist_cache(artist_id) {
                    if let Ok(artist) = musicbrainz::parse_artist(&json) {
                        artist_cache.insert(artist_id.clone(), artist);
                    }
                }
            }
            // Build per-release artist data vectors.
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

    // === STEP 5: Build candidate assignments ===
    let mut candidates: Vec<CandidateAssignment> = Vec::new();
    // Track how many distinct releases each inode has candidates for.
    let mut inode_release_set: HashMap<i64, HashSet<String>> = HashMap::new();

    for (inode, rec_matches) in &inode_recordings {
        let corpus = corpus_info.get(inode);

        for rec_match in rec_matches {
            let release_ids = match recording_releases.get(&rec_match.recording_id) {
                Some(ids) => ids,
                None => continue,
            };

            for release_id in release_ids {
                let release = match release_tracklists.get(release_id) {
                    Some(r) => r,
                    None => continue,
                };

                let resolved_artist = localized_release_artist(
                    &release.artist_credit,
                    &release_artist_data,
                    release_id,
                    &preferred_locales,
                );

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

                            inode_release_set
                                .entry(*inode)
                                .or_default()
                                .insert(release_id.clone());

                            candidates.push(CandidateAssignment {
                                inode: *inode,
                                recording_id: rec_match.recording_id.clone(),
                                release_id: release_id.clone(),
                                medium_pos: medium.position,
                                track_pos: track.position,
                                medium_format: medium.format.clone(),
                                track_number: track.number.clone(),
                                track_title: track.title.clone(),
                                score,
                                breakdown,
                                alternatives_count: 0, // Set later
                            });
                        }
                    }
                }
            }
        }
    }

    // Set alternatives_count on each candidate.
    for candidate in &mut candidates {
        candidate.alternatives_count = inode_release_set
            .get(&candidate.inode)
            .map(|s| s.len() as u16)
            .unwrap_or(1);
    }

    log_general(format!(
        "[COMPUTE] PackReleases: {} candidate assignments across {} inodes",
        candidates.len(),
        inode_recordings.len()
    ));

    // === STEP 6: Compute directory cohesion ===
    // Group inodes by parent directory, count per-release candidates.
    let mut dir_inodes: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut dir_release_counts: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for candidate in &candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            dir_inodes
                .entry(corpus.parent_dir.clone())
                .or_default()
                .insert(candidate.inode);
            *dir_release_counts
                .entry(corpus.parent_dir.clone())
                .or_default()
                .entry(candidate.release_id.clone())
                .or_default() += 1;
        }
    }

    // Update scores with directory cohesion.
    for candidate in &mut candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            let dir_size = dir_inodes
                .get(&corpus.parent_dir)
                .map(|s| s.len())
                .unwrap_or(1);
            let release_count = dir_release_counts
                .get(&corpus.parent_dir)
                .and_then(|m| m.get(&candidate.release_id))
                .copied()
                .unwrap_or(0);

            candidate.breakdown.directory_cohesion =
                release_count as f64 / dir_size.max(1) as f64;
            candidate.score = weighted_composite(&candidate.breakdown);
        }
    }

    // === STEP 7: Greedy assignment ===
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut assigned_inodes: HashSet<i64> = HashSet::new();
    let mut occupied_slots: HashSet<(String, u32, u32)> = HashSet::new();
    let mut assignments: Vec<CandidateAssignment> = Vec::new();

    for candidate in candidates {
        if assigned_inodes.contains(&candidate.inode) {
            continue;
        }
        let slot = (
            candidate.release_id.clone(),
            candidate.medium_pos,
            candidate.track_pos,
        );
        if occupied_slots.contains(&slot) {
            continue;
        }
        assigned_inodes.insert(candidate.inode);
        occupied_slots.insert(slot);
        assignments.push(candidate);
    }

    // === STEP 8: Compute per-release coverage ===
    let mut release_total_tracks: HashMap<&str, u32> = HashMap::new();
    let mut release_filled_tracks: HashMap<String, u32> = HashMap::new();

    for (release_id, release) in &release_tracklists {
        let total: u32 = release.media.iter().map(|m| m.tracks.len() as u32).sum();
        release_total_tracks.insert(release_id.as_str(), total);
    }
    for assignment in &assignments {
        *release_filled_tracks
            .entry(assignment.release_id.clone())
            .or_default() += 1;
    }

    // === STEP 9: Emit signals ===
    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();

    for assignment in &assignments {
        let release = match release_tracklists.get(&assignment.release_id) {
            Some(r) => r,
            None => continue,
        };

        let filled = release_filled_tracks
            .get(&assignment.release_id)
            .copied()
            .unwrap_or(0);
        let total = release_total_tracks
            .get(assignment.release_id.as_str())
            .copied()
            .unwrap_or(1);

        let corpus = match corpus_info.get(&assignment.inode) {
            Some(c) => c,
            None => continue,
        };

        let signal = ReleasePackingSignal {
            inode: assignment.inode,
            path: corpus.path.clone(),
            data: ReleasePackingData {
                release_id: assignment.release_id.clone(),
                release_title: release.title.clone(),
                release_artist: localized_release_artist(&release.artist_credit, &release_artist_data, &assignment.release_id, &preferred_locales),
                track_position: assignment.track_pos,
                medium_position: assignment.medium_pos,
                medium_format: assignment.medium_format.clone(),
                track_number: assignment.track_number.clone(),
                recording_id: assignment.recording_id.clone(),
                track_title: assignment.track_title.clone(),
                score: assignment.score,
                score_breakdown: assignment.breakdown.clone(),
                alternatives_count: assignment.alternatives_count,
                release_coverage: filled as f32 / total.max(1) as f32,
            },
        };

        computed.push(ComputedCorpusSignal::new(
            signal.inode,
            TypedSignalWrite::ReleasePacking(signal),
        ));
    }

    let unique_releases = release_filled_tracks.len();
    let (cleared, new, updated, unchanged) =
        reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, computed, witness);

    log_general(format!(
        "[COMPUTE] PackReleases: {} assigned to {} releases, {} missing tracklists | \
         signals: cleared={}, new={}, updated={}, unchanged={}",
        assignments.len(),
        unique_releases,
        missing_tracklists,
        cleared,
        new,
        updated,
        unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Helpers
// ============================================================================

/// Group external match rows by inode, taking the first per inode (highest confidence).
fn group_best_per_inode(rows: Vec<ExternalMatchRow>) -> Vec<(i64, ExternalMatchRow)> {
    let mut best: Vec<(i64, ExternalMatchRow)> = Vec::new();
    let mut last_inode: Option<i64> = None;

    for row in rows {
        if last_inode == Some(row.inode) {
            continue;
        }
        last_inode = Some(row.inode);
        best.push((row.inode, row));
    }

    best
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

    // Duration match: compare corpus duration against track.length (or recording.length).
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
        _ => 0.5, // Unknown — neutral score
    };

    // Tag similarity: weighted combination of title, artist, and album comparisons.
    // Title (0.4): compare corpus TITLE against track title + recording title fallback.
    let title_sim = corpus
        .and_then(|c| c.tags.get("TITLE"))
        .and_then(|v| v.first())
        .map(|corpus_title| {
            let track_sim = strsim::normalized_levenshtein(corpus_title, &track.title);
            // Recording title is the canonical form and may match better than the
            // release-specific track title (e.g. different punctuation or subtitle).
            let rec_sim = strsim::normalized_levenshtein(corpus_title, &track.recording.title);
            track_sim.max(rec_sim)
        })
        .unwrap_or(0.0);

    // Artist (0.35): compare corpus ARTIST against release artist credit.
    let artist_sim = corpus
        .and_then(|c| c.tags.get("ARTIST"))
        .and_then(|v| v.first())
        .map(|corpus_artist| strsim::normalized_levenshtein(corpus_artist, release_artist))
        .unwrap_or(0.0);

    // Album (0.25): compare corpus ALBUM against release title.
    let album_sim = corpus
        .and_then(|c| c.tags.get("ALBUM"))
        .and_then(|v| v.first())
        .map(|corpus_album| strsim::normalized_levenshtein(corpus_album, release_title))
        .unwrap_or(0.0);

    let tag_similarity = 0.4 * title_sim + 0.35 * artist_sim + 0.25 * album_sim;

    // Track number match: check if corpus TRACKNUMBER matches track position.
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
    0.35 * b.acoustid_confidence
        + 0.20 * b.duration_match
        + 0.20 * b.tag_similarity
        + 0.10 * b.track_number_match
        + 0.15 * b.directory_cohesion
}

/// Resolve a release's artist credit string using locale preferences.
///
/// If `preferred_locales` is empty or no artist data is cached, falls back to
/// the as-credited names (identical to the old non-locale-aware behavior).
fn localized_release_artist(
    credits: &[MbArtistCredit],
    release_artist_data: &HashMap<String, Vec<(String, Option<musicbrainz::MbArtist>)>>,
    release_id: &str,
    preferred_locales: &[String],
) -> String {
    if let Some(artists) = release_artist_data.get(release_id) {
        musicbrainz::join_artist_credits_localized(credits, artists, preferred_locales)
    } else {
        // No cached artist data — join credits by their as-credited names.
        musicbrainz::join_artist_credits_localized(credits, &[], preferred_locales)
    }
}
