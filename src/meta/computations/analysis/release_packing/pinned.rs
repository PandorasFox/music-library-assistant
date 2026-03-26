//! Incremental pinned release packing computations.
//!
//! When a release is pinned to a directory, these computations handle the
//! incremental path: check cache/scores, score if needed, emit signals,
//! and invalidate displaced releases — without re-running the full packing pipeline.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db;
use crate::external::musicbrainz;
use crate::logging::log_general;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::signals::data::{
    AlternativeReleasePackingSignal, MatchMethod, PackedReleaseCategory, PackedReleaseData,
    PackedReleaseSignal, PackingScoreBreakdown, PinnedReleasePackFailureData,
    PinnedReleasePackFailureSignal, ReleasePackingData, ReleasePackingSignal,
    UnmatchedCorpusTrackSignal, UnmatchedPosition,
};
use crate::meta::signals::registry::TypedSignalWrite;

use super::super::{Computation, Result};

// ============================================================================
// Gateway: ResolvePinForDir
// ============================================================================

/// Check cache/scores and route to the appropriate path.
pub fn execute_resolve_pin_for_dir(
    ctx: &ComputationContext,
    release_id: &str,
    dir_path: &Path,
) -> Result {
    let computation = Computation::ResolvePinForDir {
        release_id: release_id.to_string(),
        dir_path: dir_path.to_path_buf(),
    };

    let read_db = ctx.read_db;

    // Always go through warm/cold path to ensure comprehensive scoring.
    // Pre-existing scores from the full pipeline may only cover AcoustID-matched
    // recordings (e.g., Volume 3 of a 3-disc release), missing tracks that are
    // matchable by title/duration/track-number similarity.

    // Warm path: do we have MB cache data for this release?
    match read_db.get_mb_release_cache(release_id) {
        Ok(Some(_)) => {
            log_general(format!(
                "[PIN] Warm path: MB cache hit for release {} — scoring",
                release_id
            ));
            warm_path_build_candidates_and_score(ctx, release_id, dir_path, computation)
        }
        Ok(None) => {
            // Cold path: need to fetch from MusicBrainz first.
            log_general(format!(
                "[PIN] Cold path: release {} not in MB cache — requesting fetch",
                release_id
            ));
            Result::needs_fetch(
                computation,
                vec![crate::meta::computations::FetchRequest {
                    mb_release_ids: vec![release_id.to_string()],
                    then: vec![crate::meta::computations::Computation::Analysis(
                        Computation::ResolvePinForDir {
                            release_id: release_id.to_string(),
                            dir_path: dir_path.to_path_buf(),
                        },
                    )],
                }],
            )
        }
        Err(e) => Result::failure(computation, format!("MB cache query failed: {}", e)),
    }
}

/// Warm path: build candidates from cached MB data, then score.
fn warm_path_build_candidates_and_score(
    ctx: &ComputationContext,
    release_id: &str,
    dir_path: &Path,
    computation: Computation,
) -> Result {
    let read_db = ctx.read_db;
    let sender = require_sender!(computation);

    // Load release tracklist from MB cache
    let (raw_json, _) = match read_db.get_mb_release_cache(release_id) {
        Ok(Some(data)) => data,
        Ok(None) => return Result::failure(computation, "Release not in MB cache".into()),
        Err(e) => return Result::failure(computation, format!("MB cache read failed: {}", e)),
    };

    let release = match musicbrainz::parse_release(&raw_json) {
        Ok(r) => r,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to parse MB release JSON: {}", e),
            )
        }
    };

    // Count total tracks
    let total_tracks: i32 = release.media.iter().map(|m| m.tracks.len() as i32).sum();
    if total_tracks == 0 {
        return Result::failure(computation, "Release has no tracks".into());
    }

    // Build release artist string
    let release_artist = release
        .artist_credit
        .iter()
        .map(|ac| {
            format!("{}{}", ac.name, ac.joinphrase)
        })
        .collect::<String>();

    // Clean stale candidates and scores from prior runs before writing fresh data.
    // Without this, INSERT OR REPLACE would merge stale candidates (referencing ghost
    // inodes from previous indexing) with fresh synthetic candidates.
    sender.delete_packing_data_for_release(release_id, ctx.witness);

    // Write manifest entry
    let manifest_row = (
        release_id.to_string(),
        total_tracks,
        release.title.clone(),
        release_artist,
        release.media.len() as i32,
    );
    sender.write_packing_manifest(vec![manifest_row], ctx.witness);

    // Load corpus audio files in this directory
    let dir_str = dir_path.display().to_string();
    let audio_files = match read_db.get_audio_files_by_path_prefix(&dir_str) {
        Ok(files) => files,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to load corpus files for dir {:?}: {}", dir_path, e),
            )
        }
    };

    if audio_files.is_empty() {
        return Result::failure(
            computation,
            format!("No audio files found in directory {:?}", dir_path),
        );
    }

    // Build candidate rows: synthetic confidence=1.0 for all files in the dir
    let file_count = audio_files.len() as i32;
    let mut candidate_rows = Vec::new();
    for file in &audio_files {
        // Load tags for this file
        let tags = read_db
            .get_tags_for_zone(file.inode(), db::types::Zone::Corpus)
            .unwrap_or_default();

        let find_tag = |name: &str| -> Option<String> {
            tags.iter()
                .find(|t| t.tag_name.eq_ignore_ascii_case(name))
                .map(|t| t.tag_value.clone())
        };

        candidate_rows.push(db::write_thread::PackingCandidateRow {
            release_id: release_id.to_string(),
            inode: file.inode(),
            recording_id: String::new(),
            confidence: 1.0,
            path: file.path().to_string(),
            parent_dir: dir_str.clone(),
            duration_ms: file.audio.duration_ms,
            tag_title: find_tag("TITLE"),
            tag_artist: find_tag("ARTIST"),
            tag_album: find_tag("ALBUM"),
            tag_tracknumber: find_tag("TRACKNUMBER"),
            dir_file_count: file_count,
        });
    }

    sender.write_packing_candidates(candidate_rows, ctx.witness);

    log_general(format!(
        "[PIN] Built {} candidates for release {} in {:?}",
        audio_files.len(),
        release_id,
        dir_path
    ));

    // Spawn scoring (existing per-release computation), then defer commit
    use crate::meta::computations::PipelineStage;

    Result::pipeline(
        computation,
        vec![Computation::ScoreReleaseCandidates {
            release_id: release_id.to_string(),
        }],
        vec![(
            PipelineStage::Resolve,
            vec![crate::meta::computations::Computation::Analysis(
                Computation::CommitPinnedRelease {
                    release_id: release_id.to_string(),
                    dir_path: dir_path.to_path_buf(),
                },
            )],
        )],
    )
}

// ============================================================================
// Commit: emit signals and invalidate displaced releases
// ============================================================================

/// Commit a pinned release: verify full coverage, emit signals, invalidate old packings.
pub fn execute_commit_pinned_release(
    ctx: &ComputationContext,
    release_id: &str,
    dir_path: &Path,
) -> Result {
    let computation = Computation::CommitPinnedRelease {
        release_id: release_id.to_string(),
        dir_path: dir_path.to_path_buf(),
    };

    let read_db = ctx.read_db;
    let sender = require_sender!(computation);
    let dir_str = dir_path.display().to_string();

    // Load optimal scores for this release
    let scores = match read_db.get_optimal_packing_scores_for_release(release_id) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to load scores for release {}: {}", release_id, e),
            )
        }
    };

    if scores.is_empty() {
        return Result::failure(
            computation,
            format!("No packing scores for release {}", release_id),
        );
    }

    // Get release metadata
    let (release_title, release_artist, total_tracks) =
        match get_release_metadata(read_db, release_id) {
            Some(meta) => meta,
            None => {
                return Result::failure(
                    computation,
                    "Cannot determine release metadata".into(),
                )
            }
        };

    // Filter scores to inodes from the pinned directory
    let dir_inodes: HashSet<i64> = read_db
        .get_audio_files_by_path_prefix(&dir_str)
        .unwrap_or_default()
        .iter()
        .map(|f| f.inode())
        .collect();

    let dir_scores: Vec<_> = scores
        .iter()
        .filter(|s| dir_inodes.contains(&s.inode))
        .collect();

    let matched_tracks = dir_scores.len();

    // Check full coverage
    if matched_tracks < total_tracks as usize {
        log_general(format!(
            "[PIN] Pack failure: release {} matched {}/{} tracks in {:?}",
            release_id, matched_tracks, total_tracks, dir_path
        ));

        let unmatched = build_unmatched_positions(read_db, release_id, &dir_scores);

        let failure_key = format!("{}:{}", release_id, dir_str);
        sender.write_typed_signal(
            TypedSignalWrite::PinnedReleasePackFailure(PinnedReleasePackFailureSignal {
                key: failure_key,
                data: PinnedReleasePackFailureData {
                    release_id: release_id.to_string(),
                    release_title,
                    release_artist,
                    dir_path: dir_str,
                    total_tracks: total_tracks as usize,
                    matched_tracks,
                    unmatched_positions: unmatched,
                },
            }),
            ctx.witness,
        );

        return Result::success(computation, Vec::new());
    }

    // Full coverage! Emit signals and invalidate displaced releases.
    log_general(format!(
        "[PIN] Full pack: release {} covers all {} tracks — committing",
        release_id, total_tracks
    ));

    // Clear any existing PackFailure for this release+dir
    let failure_key = format!("{}:{}", release_id, dir_str);
    sender.clear_aggregate_signal::<PinnedReleasePackFailureSignal>(&failure_key, ctx.witness);

    // Find displaced releases: existing packing assignments for our claimed inodes
    let claimed_inodes: HashSet<i64> = dir_scores.iter().map(|s| s.inode).collect();
    let existing_assignments = read_db.get_release_packing_assignments().unwrap_or_default();
    let displaced_release_ids: HashSet<String> = existing_assignments
        .iter()
        .filter(|a| claimed_inodes.contains(&a.inode) && a.release_id != release_id)
        .map(|a| a.release_id.clone())
        .collect();

    // Invalidate displaced releases — full subsumption
    for displaced_id in &displaced_release_ids {
        log_general(format!(
            "[PIN] Invalidating displaced release {} (subsumed by {})",
            displaced_id, release_id
        ));

        // Clear PackedRelease for the displaced release (under any category prefix)
        for prefix in &[
            "perfect:", "full_match:", "incomplete:", "single:", "low_confidence:",
        ] {
            let key = format!("{}{}", prefix, displaced_id);
            sender.clear_aggregate_signal::<PackedReleaseSignal>(&key, ctx.witness);
        }

        // Clear AlternativeReleasePacking where displaced is the winner
        sender.clear_aggregate_by_key_prefix::<AlternativeReleasePackingSignal>(
            &format!("{}:", displaced_id),
            ctx.witness,
        );

        // Clear ReleasePacking for all inodes of the displaced release
        // (not just the overlapping ones — the release is fully evicted)
        let displaced_inodes: Vec<i64> = existing_assignments
            .iter()
            .filter(|a| a.release_id == *displaced_id)
            .map(|a| a.inode)
            .collect();
        for inode in displaced_inodes {
            // Only clear if this inode isn't being claimed by our pin
            // (our pin will overwrite via INSERT OR REPLACE)
            if !claimed_inodes.contains(&inode) {
                sender.clear_corpus_signal::<ReleasePackingSignal>(inode, ctx.witness);
            }
        }
    }

    // Build corpus_paths for signal emission
    let corpus_paths: HashMap<i64, String> = read_db
        .get_audio_files_by_path_prefix(&dir_str)
        .unwrap_or_default()
        .into_iter()
        .map(|f| (f.inode(), f.path().to_string()))
        .collect();

    // Emit signals
    let mut signals_batch: Vec<TypedSignalWrite> = Vec::new();
    let filled = dir_scores.len() as u32;

    // PackedRelease aggregate signal — pinned releases are always Perfect when fully packed
    let category = PackedReleaseCategory::Perfect;
    let packed_key = format!("{}:{}", category.key_prefix(), release_id);
    signals_batch.push(TypedSignalWrite::PackedRelease(PackedReleaseSignal {
        key: packed_key,
        data: PackedReleaseData {
            release_id: release_id.to_string(),
            release_title: release_title.clone(),
            release_artist: release_artist.clone(),
            category,
            assigned_count: filled,
            total_tracks: total_tracks as u32,
            low_confidence_acoustid_ratio: None,
            low_confidence_avg_album_match: None,
        },
    }));

    // Per-inode ReleasePackingSignal
    for row in &dir_scores {
        let path = corpus_paths
            .get(&row.inode)
            .cloned()
            .unwrap_or_else(|| format!("inode:{}", row.inode));

        let breakdown: PackingScoreBreakdown =
            bincode::deserialize(&row.score_breakdown).unwrap_or_default();

        signals_batch.push(TypedSignalWrite::ReleasePacking(ReleasePackingSignal {
            inode: row.inode,
            path,
            data: ReleasePackingData {
                release_id: row.release_id.clone(),
                release_title: release_title.clone(),
                release_artist: release_artist.clone(),
                track_position: row.track_pos as u32,
                medium_position: row.medium_pos as u32,
                medium_format: row.medium_format.clone(),
                track_number: row.track_number.clone(),
                recording_id: row.recording_id.clone(),
                track_title: row.track_title.clone(),
                score: row.score,
                score_breakdown: breakdown,
                alternatives_count: 1,
                release_coverage: filled as f32 / total_tracks.max(1) as f32,
                match_method: if row.match_method == 1 {
                    MatchMethod::Elimination
                } else {
                    MatchMethod::AcoustId
                },
            },
        }));
    }

    // Clear UnmatchedCorpusTrack signals for claimed inodes
    for inode in &claimed_inodes {
        sender.clear_corpus_signal::<UnmatchedCorpusTrackSignal>(*inode, ctx.witness);
    }

    if !signals_batch.is_empty() {
        sender.write_typed_signal_batch(signals_batch, ctx.witness);
    }

    log_general(format!(
        "[PIN] Committed release {}: {} tracks, {} displaced releases",
        release_id,
        filled,
        displaced_release_ids.len()
    ));

    Result::success(computation, Vec::new())
}

// ============================================================================
// Invalidate: clean up on unpin
// ============================================================================

/// Clean up all packing signals for an unpinned release.
pub fn execute_invalidate_pinned_release(
    ctx: &ComputationContext,
    release_id: &str,
    dir_path: &Path,
) -> Result {
    let computation = Computation::InvalidatePinnedRelease {
        release_id: release_id.to_string(),
        dir_path: dir_path.to_path_buf(),
    };

    let read_db = ctx.read_db;
    let sender = require_sender!(computation);
    let dir_str = dir_path.display().to_string();

    log_general(format!(
        "[PIN] Invalidating unpinned release {} from {:?}",
        release_id, dir_path
    ));

    // Clear PackedRelease (under any category prefix)
    for prefix in &[
        "perfect:", "full_match:", "incomplete:", "single:", "low_confidence:",
    ] {
        let key = format!("{}{}", prefix, release_id);
        sender.clear_aggregate_signal::<PackedReleaseSignal>(&key, ctx.witness);
    }

    // Clear AlternativeReleasePacking where this release is the winner
    sender.clear_aggregate_by_key_prefix::<AlternativeReleasePackingSignal>(
        &format!("{}:", release_id),
        ctx.witness,
    );

    // Clear ReleasePacking for inodes assigned to this release
    let assignments = read_db.get_release_packing_assignments().unwrap_or_default();
    for assignment in &assignments {
        if assignment.release_id == release_id {
            sender.clear_corpus_signal::<ReleasePackingSignal>(assignment.inode, ctx.witness);
        }
    }

    // Clear any PackFailure signal for this release+dir
    let failure_key = format!("{}:{}", release_id, dir_str);
    sender.clear_aggregate_signal::<PinnedReleasePackFailureSignal>(&failure_key, ctx.witness);

    Result::success(computation, Vec::new())
}

// ============================================================================
// Helpers
// ============================================================================

/// Get release metadata from manifest or MB cache.
fn get_release_metadata(
    read_db: &db::queries::ReadOnlyDb,
    release_id: &str,
) -> Option<(String, String, i32)> {
    // Try manifest first
    if let Ok(manifest) = read_db.get_packing_manifest() {
        if let Some(entry) = manifest.iter().find(|m| m.release_id == release_id) {
            return Some((
                entry.release_title.clone(),
                entry.release_artist.clone(),
                entry.total_tracks,
            ));
        }
    }

    // Fall back to MB cache
    if let Ok(Some((raw_json, _))) = read_db.get_mb_release_cache(release_id) {
        if let Ok(release) = musicbrainz::parse_release(&raw_json) {
            let total: i32 = release.media.iter().map(|m| m.tracks.len() as i32).sum();
            let artist = release
                .artist_credit
                .iter()
                .map(|ac| {
                    format!("{}{}", ac.name, ac.joinphrase)
                })
                .collect::<String>();
            return Some((release.title.clone(), artist, total));
        }
    }

    None
}

/// Build the list of unmatched track positions from MB cache data.
fn build_unmatched_positions(
    read_db: &db::queries::ReadOnlyDb,
    release_id: &str,
    matched_scores: &[&db::queries::external::OptimalPackingScoreRow],
) -> Vec<UnmatchedPosition> {
    let matched_set: HashSet<(i32, i32)> = matched_scores
        .iter()
        .map(|s| (s.medium_pos, s.track_pos))
        .collect();

    let mut unmatched = Vec::new();
    if let Ok(Some((raw_json, _))) = read_db.get_mb_release_cache(release_id) {
        if let Ok(release) = musicbrainz::parse_release(&raw_json) {
            for medium in &release.media {
                for track in &medium.tracks {
                    let medium_pos = medium.position as i32;
                    let track_pos = track.position as i32;
                    if !matched_set.contains(&(medium_pos, track_pos)) {
                        unmatched.push(UnmatchedPosition {
                            medium: medium_pos,
                            position: track_pos,
                            title: track.title.clone(),
                        });
                    }
                }
            }
        }
    }
    unmatched
}
