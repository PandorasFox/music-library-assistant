//! Stage 4: EmitUnmatchedSignals (gap analysis).

use std::collections::{HashMap, HashSet};

use crate::db::ReadOnlyDb;
use crate::external::musicbrainz;
use crate::logging::log_general;
use crate::meta::computations::helpers::{
    reconcile_aggregate_signals, reconcile_corpus_signals, ComputedAggregateSignal,
    ComputedCorpusSignal,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    ReleasePackingSignal, UnfilledReleaseSlotData, UnfilledReleaseSlotSignal,
    UnmatchedCorpusTrackData, UnmatchedCorpusTrackSignal, UnsolvedCategory};
use crate::meta::signals::registry::TypedSignalWrite;

use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};

// ============================================================================
// Stage 4: EmitUnmatchedSignals (gap analysis)
// ============================================================================

/// Execute EmitUnmatchedSignals — emit unmatched corpus track and unfilled slot signals.
pub fn execute_emit_unmatched_signals(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::EmitUnmatchedSignals;

    let sender = require_sender!(computation);

    // Build set of assigned inodes (from ReleasePackingSignal, written by Stage 3)
    let assigned_inodes: HashSet<i64> = read_only_db
        .corpus_signal_all_inodes::<ReleasePackingSignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // === Unmatched corpus tracks ===
    // Every fingerprinted corpus inode NOT in signal_release_packing is unmatched.
    // Enriched with recording IDs (from candidates) and considered releases (from scoring)
    // when available, for UI context.

    // Enrichment: inode → recording IDs from candidates table
    let mut inode_recordings: HashMap<i64, Vec<String>> = HashMap::new();
    match read_only_db.get_candidate_inode_recordings() {
        Ok(rows) => {
            for (inode, recording_id) in rows {
                inode_recordings
                    .entry(inode)
                    .or_default()
                    .push(recording_id);
            }
        }
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query candidate inode recordings: {}", e),
            );
        }
    }

    // Enrichment: inode → considered releases from scoring table
    let optimal_scores = match read_only_db.get_optimal_packing_scores() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to read scoring table: {}", e),
            );
        }
    };

    let mut inode_considered_releases: HashMap<i64, Vec<String>> = HashMap::new();
    for row in &optimal_scores {
        inode_considered_releases
            .entry(row.inode)
            .or_default()
            .push(row.release_id.clone());
    }

    // Primary loop: all fingerprinted corpus inodes, minus assigned
    let fingerprinted = match read_only_db.get_fingerprinted_corpus_inodes() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query fingerprinted inodes: {}", e),
            );
        }
    };

    let mut unmatched_signals: Vec<ComputedCorpusSignal> = Vec::new();
    for (inode, path) in fingerprinted {
        if assigned_inodes.contains(&inode) {
            continue;
        }

        let mut recording_ids = inode_recordings
            .get(&inode)
            .cloned()
            .unwrap_or_default();
        recording_ids.sort();
        recording_ids.dedup();

        let mut considered_release_ids = inode_considered_releases
            .get(&inode)
            .cloned()
            .unwrap_or_default();
        considered_release_ids.sort();
        considered_release_ids.dedup();

        let category = if recording_ids.is_empty() {
            UnsolvedCategory::NoMatch
        } else if considered_release_ids.is_empty() {
            UnsolvedCategory::NoRelease
        } else {
            UnsolvedCategory::Conflict
        };

        let signal = UnmatchedCorpusTrackSignal {
            inode,
            path,
            category,
            data: UnmatchedCorpusTrackData {
                recording_ids,
                considered_release_ids,
            },
        };
        unmatched_signals.push(ComputedCorpusSignal::new(
            inode,
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
    let manifest = match read_only_db.get_packing_manifest() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to read manifest: {}", e),
            );
        }
    };

    // Build filled_slots from actual assignments (signal_release_packing), which
    // includes both AcoustId and Elimination assignments (all resolved in Stage 3).
    // Cannot use optimal_scores: elimination-matched inodes bypass the scoring table.
    let actual_assignments = read_only_db
        .get_release_packing_assignments()
        .unwrap_or_default();

    let mut filled_slots: HashMap<String, HashSet<(i32, i32)>> = HashMap::new();
    let mut release_assigned_inodes: HashMap<String, HashSet<i64>> = HashMap::new();
    for a in &actual_assignments {
        filled_slots
            .entry(a.release_id.clone())
            .or_default()
            .insert((a.medium_position as i32, a.track_position as i32));
        release_assigned_inodes
            .entry(a.release_id.clone())
            .or_default()
            .insert(a.inode);
    }

    // Build full_match_inodes: set of all inodes assigned to full-match releases.
    // Used to suppress gap signals for releases fully covered by other full matches.
    let mut full_match_inodes: HashSet<i64> = HashSet::new();
    for manifest_row in &manifest {
        let filled_count = filled_slots
            .get(&manifest_row.release_id)
            .map(|s| s.len() as i32)
            .unwrap_or(0);
        if filled_count >= manifest_row.total_tracks && manifest_row.total_tracks > 0 {
            if let Some(inodes) = release_assigned_inodes.get(&manifest_row.release_id) {
                full_match_inodes.extend(inodes);
            }
        }
    }

    // Build per-release candidate inode sets from optimal_scores for coverage checking.
    // A release is "fully covered" if every inode that could belong to it is already
    // assigned to a full-match release — meaning it's a cached MB entry we don't
    // actually possess as a distinct release.
    let mut release_candidate_inodes: HashMap<&str, HashSet<i64>> = HashMap::new();
    for row in &optimal_scores {
        release_candidate_inodes
            .entry(&row.release_id)
            .or_default()
            .insert(row.inode);
    }

    let mut suppressed_covered = 0usize;

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
        let total = manifest_row.total_tracks as u32;

        // Only emit unfilled slots for releases that have at least one filled slot
        if filled_count == 0 {
            continue;
        }

        // Skip fully-covered releases: if every candidate inode for this incomplete
        // release is already assigned to a full-match release, this release has no
        // unique files and represents a cached MB entry, not a real gap.
        if filled_count < total {
            if let Some(candidate_inodes) =
                release_candidate_inodes.get(manifest_row.release_id.as_str())
            {
                if !candidate_inodes.is_empty()
                    && candidate_inodes
                        .iter()
                        .all(|i| full_match_inodes.contains(i))
                {
                    suppressed_covered += 1;
                    continue;
                }
            }
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
                        total_tracks: total,
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

    log_general(format!(
        "[COMPUTE] EmitUnmatchedSignals: \
         unmatched_corpus: cleared={}, new={}, updated={}, unchanged={} | \
         unfilled_slots: cleared={}, new={}, updated={}, unchanged={} | \
         {} fully-covered releases suppressed",
        uc_cleared,
        uc_new,
        uc_updated,
        uc_unchanged,
        us_cleared,
        us_new,
        us_updated,
        us_unchanged,
        suppressed_covered
    ));

    Result::success(computation, Vec::new())
}
