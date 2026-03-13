//! Stage 3: ComputeReleaseMappings and tier orchestrators (MapPerfect/FullMatch/Incomplete/Single).

use std::collections::{HashMap, HashSet};

use crate::db::write_thread::{self};
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::computations::{Computation, PipelineStage};
use crate::meta::signals::data::{
    AlternativeReleasePackingSignal, PackedReleaseSignal, PackingKnotSignal,
    PinnedReleaseConflictData, PinnedReleaseConflictSignal, ReleasePackingSignal, VariousArtistsOverrideSignal};
use crate::meta::signals::registry::TypedSignalWrite;

use super::components::{
    emit_isolated_proposal_signals, find_conflict_components,
    orchestrate_partial_tier,
};
use super::types::{
    build_manifest_map, build_manifest_map_owned,
    classify_proposal, AlternativeRelease, ComponentData, Proposal, ProposalTier,
    ReleaseMappingState, SharedComponentData, SharedMappingState,
};
use crate::config::ReleasePackingOpinions;
use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};

// ============================================================================
// Stage 3: ComputeReleaseMappings
// ============================================================================

/// Execute ComputeReleaseMappings — Stage 3a orchestrator.
///
/// Loads scoring data, classifies proposals into quality tiers, packages
/// state, and defers MIS rounds as separate computations for visibility.
pub fn execute_compute_release_mappings(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::ComputeReleaseMappings;

    let sender = require_sender!(computation);

    // Bulk-clear all packing-related signal tables for clean re-emission.
    // Each tier will emit its signals per-component as it solves.
    sender.clear_signal_table::<ReleasePackingSignal>(witness);
    sender.clear_aggregate_signal_table::<PackedReleaseSignal>(witness);
    sender.clear_aggregate_signal_table::<PackingKnotSignal>(witness);
    sender.clear_aggregate_signal_table::<AlternativeReleasePackingSignal>(witness);
    sender.clear_aggregate_signal_table::<VariousArtistsOverrideSignal>(witness);
    sender.clear_aggregate_signal_table::<PinnedReleaseConflictSignal>(witness);
    write_thread::wait_for_queue_drain();

    // Read all optimal picks from scoring table
    let optimal_scores = match read_only_db.get_optimal_packing_scores() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
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
                format!("Failed to read manifest: {}", e),
            );
        }
    };

    let manifest_map = build_manifest_map(&manifest);

    // Media count lookup for pinned release conflict detection
    let release_media_counts: HashMap<&str, i32> = manifest
        .iter()
        .map(|r| (r.release_id.as_str(), r.media_count))
        .collect();

    if optimal_scores.is_empty() {
        return Result::success(computation, Vec::new());
    }

    // Load directory metadata for tier classification
    let inode_dir_map: HashMap<i64, (String, i32)> = match read_only_db.get_candidate_inode_dirs() {
        Ok(rows) => rows
            .into_iter()
            .map(|(inode, dir, count)| (inode, (dir, count)))
            .collect(),
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query candidate inode dirs: {}", e),
            );
        }
    };

    // Build proposals from optimal scores and classify into tiers
    let mut proposals_map: HashMap<&str, Vec<crate::db::queries::external::OptimalPackingScoreRow>> = HashMap::new();
    for row in &optimal_scores {
        proposals_map
            .entry(&row.release_id)
            .or_default()
            .push(row.clone());
    }

    let mut perfect_pool: Vec<Proposal> = Vec::new();
    let mut full_match_pool: Vec<Proposal> = Vec::new();
    let mut incomplete_pool: Vec<Proposal> = Vec::new();
    let mut single_pool: Vec<Proposal> = Vec::new();

    for (release_id, rows) in proposals_map {
        let total_tracks = manifest_map
            .get(release_id)
            .map(|&(_, _, t)| t)
            .unwrap_or(0);

        let inode_set: HashSet<i64> = rows.iter().map(|r| r.inode).collect();
        let total_score: f64 = rows.iter().map(|r| r.score).sum();

        let media_count = rows
            .iter()
            .map(|r| r.medium_pos)
            .collect::<HashSet<_>>()
            .len();

        let tier = classify_proposal(&rows, total_tracks, media_count, &inode_dir_map);

        let proposal = Proposal {
            total_tracks,
            rows,
            inode_set,
            total_score,
            tier,
        };

        match proposal.tier {
            ProposalTier::Perfect => perfect_pool.push(proposal),
            ProposalTier::FullMatch => full_match_pool.push(proposal),
            ProposalTier::Incomplete => incomplete_pool.push(proposal),
            ProposalTier::Single => single_pool.push(proposal),
        }
    }

    // === Pre-accept pinned release proposals ===
    // Pinned proposals are operator decisions — they win unconditionally.
    // We emit their signals now and remove them from tier pools before MIS.
    //
    // Conflict detection: if a release is pinned by more directories than it has
    // media (e.g., 1-medium release pinned by 2 dirs), that's an invariant violation.
    // We emit a hard-stop PinnedReleaseConflict signal and skip that release entirely.
    let pinned_release_dirs: HashMap<String, Vec<String>> = match crate::config::load_config() {
        Ok(cfg) => {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for sd in &cfg.source_dirs {
                if let Some(ref release_id) = sd.pinned_release {
                    map.entry(release_id.clone())
                        .or_default()
                        .push(format!("corpus/{}", sd.path.display()));
                }
            }
            map
        }
        Err(_) => HashMap::new(),
    };

    if !pinned_release_dirs.is_empty() {
        // Detect conflicts: more pinning dirs than release media
        let mut conflicted_releases: HashSet<String> = HashSet::new();
        for (release_id, dirs) in &pinned_release_dirs {
            let media_count = release_media_counts
                .get(release_id.as_str())
                .copied()
                .unwrap_or(1);
            if dirs.len() as i32 > media_count {
                conflicted_releases.insert(release_id.clone());
                let (title, artist) = manifest_map
                    .get(release_id.as_str())
                    .map(|&(t, a, _)| (t.to_string(), a.to_string()))
                    .unwrap_or_else(|| (release_id.clone(), String::new()));
                log_general(format!(
                    "[COMPUTE] ComputeReleaseMappings: CONFLICT — release {} pinned by {} dirs but has only {} media",
                    release_id, dirs.len(), media_count,
                ));
                sender.write_typed_signal(
                    TypedSignalWrite::PinnedReleaseConflict(PinnedReleaseConflictSignal {
                        key: release_id.clone(),
                        data: PinnedReleaseConflictData {
                            release_id: release_id.clone(),
                            release_title: title,
                            release_artist: artist,
                            media_count,
                            directories: dirs.clone(),
                            reason: format!(
                                "Release has {} media but is pinned by {} directories. Pick one directory or reorganize to conform to multi-disc layout.",
                                media_count, dirs.len()
                            ),
                        },
                    }),
                    witness,
                );
            }
        }

        // Build set of non-conflicted pinned release IDs
        let pinned_release_ids: HashSet<String> = pinned_release_dirs
            .keys()
            .filter(|id| !conflicted_releases.contains(*id))
            .cloned()
            .collect();

        if !pinned_release_ids.is_empty() {
            // Load data needed for signal emission
            let corpus_paths: HashMap<i64, String> = read_only_db
                .get_packing_inode_paths()
                .unwrap_or_default()
                .into_iter()
                .collect();

            let mut pinned_inodes: HashSet<i64> = HashSet::new();
            let mut pinned_accepted = 0usize;

            // Drain pinned proposals from all pools and emit signals
            let drain_pinned = |pool: &mut Vec<Proposal>, tier: ProposalTier| -> Vec<Proposal> {
                let mut pinned = Vec::new();
                pool.retain(|p| {
                    if !p.rows.is_empty() && pinned_release_ids.contains(&p.rows[0].release_id) {
                        pinned.push(Proposal {
                            total_tracks: p.total_tracks,
                            rows: p.rows.clone(),
                            inode_set: p.inode_set.clone(),
                            total_score: p.total_score,
                            tier,
                        });
                        false
                    } else {
                        true
                    }
                });
                pinned
            };

            let mut all_pinned: Vec<(Proposal, ProposalTier)> = Vec::new();
            for p in drain_pinned(&mut perfect_pool, ProposalTier::Perfect) {
                all_pinned.push((p, ProposalTier::Perfect));
            }
            for p in drain_pinned(&mut full_match_pool, ProposalTier::FullMatch) {
                all_pinned.push((p, ProposalTier::FullMatch));
            }
            for p in drain_pinned(&mut incomplete_pool, ProposalTier::Incomplete) {
                all_pinned.push((p, ProposalTier::Incomplete));
            }
            for p in drain_pinned(&mut single_pool, ProposalTier::Single) {
                all_pinned.push((p, ProposalTier::Single));
            }

            for (proposal, tier) in &all_pinned {
                emit_isolated_proposal_signals(
                    proposal,
                    *tier,
                    &manifest_map,
                    &corpus_paths,
                    &[], // no siblings for pinned proposals
                    &sender,
                    witness,
                    0.0, // no low-confidence downgrade for pinned
                    0.0,
                );
                pinned_inodes.extend(&proposal.inode_set);
                pinned_accepted += 1;
            }

            if pinned_accepted > 0 {
                log_general(format!(
                    "[COMPUTE] ComputeReleaseMappings: pre-accepted {} pinned proposal(s) ({} inodes)",
                    pinned_accepted,
                    pinned_inodes.len(),
                ));

                // Reverse constraint: reject non-pinned proposals that overlap with pinned dirs
                let reject_overlapping = |pool: &mut Vec<Proposal>| {
                    pool.retain(|p| !p.inode_set.iter().any(|i| pinned_inodes.contains(i)));
                };
                reject_overlapping(&mut perfect_pool);
                reject_overlapping(&mut full_match_pool);
                reject_overlapping(&mut incomplete_pool);
                reject_overlapping(&mut single_pool);
            }
        }

        // Also reject proposals for conflicted releases — neither dir gets packed
        if !conflicted_releases.is_empty() {
            let reject_conflicted = |pool: &mut Vec<Proposal>| {
                pool.retain(|p| {
                    p.rows.is_empty() || !conflicted_releases.contains(&p.rows[0].release_id)
                });
            };
            reject_conflicted(&mut perfect_pool);
            reject_conflicted(&mut full_match_pool);
            reject_conflicted(&mut incomplete_pool);
            reject_conflicted(&mut single_pool);
        }
    }

    // Sort each pool for deterministic MIS input: highest score first, then release_id
    let sort_pool = |pool: &mut Vec<Proposal>| {
        pool.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.rows[0].release_id.cmp(&b.rows[0].release_id))
        });
    };
    sort_pool(&mut perfect_pool);
    sort_pool(&mut full_match_pool);
    sort_pool(&mut incomplete_pool);
    sort_pool(&mut single_pool);

    // Log tier distribution
    let tier_summary = |pool: &[Proposal]| -> (usize, i32) {
        (pool.len(), pool.iter().map(|p| p.total_tracks).sum())
    };
    let (p_count, p_tracks) = tier_summary(&perfect_pool);
    let (f_count, f_tracks) = tier_summary(&full_match_pool);
    let (i_count, i_tracks) = tier_summary(&incomplete_pool);
    let (s_count, _) = tier_summary(&single_pool);

    log_general(format!(
        "[COMPUTE] ComputeReleaseMappings: {} proposals classified — \
         {} Perfect ({} tracks), {} FullMatch ({} tracks), \
         {} Incomplete ({} tracks), {} Single",
        p_count + f_count + i_count + s_count,
        p_count,
        p_tracks,
        f_count,
        f_tracks,
        i_count,
        i_tracks,
        s_count,
    ));

    // Load config for MIS parameters
    let rp = match crate::config::load_config() {
        Ok(c) => c.opinions.release_packing.clone(),
        Err(_) => ReleasePackingOpinions::default(),
    };

    // Package state and defer Round 1
    let state = SharedMappingState::new(ReleaseMappingState {
        perfect_pool,
        full_match_pool,
        incomplete_pool,
        single_pool,
        knot_ratio: rp.packing_knot_ratio,
        knot_size_limit: rp.packing_knot_size_limit,
        singles_before_incompletes: rp.singles_before_incompletes,
        allow_resolve_knots_with_discographies: rp.allow_resolve_knots_with_discographies,
        low_confidence_max_acoustid_ratio: rp.low_confidence_max_acoustid_ratio,
        low_confidence_max_album_match: rp.low_confidence_max_album_match,
    });

    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(
            AnalysisComputation::MapPerfectReleases { state },
        )],
    )];

    Result::pipeline(
        computation,
        Vec::new(),
        deferred,
    )
}

/// Execute MapPerfectReleases — Stage 3b orchestrator.
///
/// Filters to proposals with entirely unclaimed inodes, finds connected
/// components, emits isolated nodes directly, spawns per-component solvers
/// for the rest. Defers MapFullMatchReleases as next barrier phase.
pub(crate) fn execute_map_perfect_releases(
    shared: &SharedMappingState,
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::MapPerfectReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

    let sender = require_sender!(computation);

    let assigned_inodes = read_only_db
        .get_assigned_packing_inodes()
        .unwrap_or_default();

    // Filter: proposals whose entire inode set is unclaimed
    let eligible: Vec<Proposal> = std::mem::take(&mut state.perfect_pool)
        .into_iter()
        .filter(|p| {
            !p.inode_set.is_empty() && p.inode_set.iter().all(|i| !assigned_inodes.contains(i))
        })
        .collect();

    if eligible.is_empty() {
        log_general("[COMPUTE] MapPerfectReleases: 0 eligible, skipping");
        let next_state = SharedMappingState::new(state);
        return Result::pipeline(
            computation,
            Vec::new(),
            vec![(
                PipelineStage::Resolve,
                vec![Computation::Analysis(
                    AnalysisComputation::MapFullMatchReleases { state: next_state },
                )],
            )],
        );
    }

    // Build inode-signature groups for alternative detection (bookkeeping only, no filtering)
    let eligible_siblings = build_signature_siblings(&eligible, read_only_db);

    // Find connected components
    let components = find_conflict_components(&eligible);

    // Load manifest + corpus paths for isolated-node emission
    let manifest = read_only_db.get_packing_manifest().unwrap_or_default();
    let manifest_map = build_manifest_map(&manifest);
    let corpus_paths: HashMap<i64, String> = read_only_db
        .get_packing_inode_paths()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let manifest_map_owned = build_manifest_map_owned(&manifest);

    let mut spawned: Vec<AnalysisComputation> = Vec::new();
    let mut isolated_count = 0usize;
    let mut isolated_signals = 0usize;

    for component in &components {
        if component.len() == 1 {
            let eligible_idx = component[0];
            let siblings = eligible_siblings.get(&eligible_idx).map(|v| v.as_slice()).unwrap_or(&[]);
            // Isolated node — emit directly (Perfect tier never triggers low-confidence)
            isolated_signals += emit_isolated_proposal_signals(
                &eligible[eligible_idx],
                ProposalTier::Perfect,
                &manifest_map,
                &corpus_paths,
                siblings,
                &sender,
                witness,
                0.0, // Perfect tier won't downgrade
                0.0,
            );
            isolated_count += 1;
        } else {
            // Multi-node component — spawn solver
            // Remap eligible indices to component-local indices for signature_siblings
            let mut component_siblings: HashMap<usize, Vec<AlternativeRelease>> = HashMap::new();
            let component_proposals: Vec<Proposal> =
                component.iter().enumerate().map(|(local_idx, &eligible_idx)| {
                    if let Some(sibs) = eligible_siblings.get(&eligible_idx) {
                        component_siblings.insert(local_idx, sibs.clone());
                    }
                    let p = &eligible[eligible_idx];
                    Proposal {
                        total_tracks: p.total_tracks,
                        rows: p.rows.clone(),
                        inode_set: p.inode_set.clone(),
                        total_score: p.total_score,
                        tier: p.tier,
                    }
                }).collect();

            spawned.push(AnalysisComputation::ResolvePackingComponent {
                data: SharedComponentData::new(ComponentData {
                    proposals: component_proposals,
                    tier: ProposalTier::Perfect,
                    corpus_paths: corpus_paths.clone(),
                    manifest_map: manifest_map_owned.clone(),
                    signature_siblings: component_siblings,
                }),
            });
        }
    }

    log_general(format!(
        "[COMPUTE] MapPerfectReleases: {} eligible, {} components ({} isolated → {} signals, {} spawned)",
        eligible.len(),
        components.len(),
        isolated_count,
        isolated_signals,
        spawned.len(),
    ));

    let next_state = SharedMappingState::new(state);
    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(
            AnalysisComputation::MapFullMatchReleases { state: next_state },
        )],
    )];

    Result::pipeline(
        computation,
        spawned,
        deferred,
    )
}

/// Execute MapFullMatchReleases — Stage 3c orchestrator.
///
/// Culls tainted proposals, deduplicates by inode signature, extracts knots,
/// finds connected components among clean proposals, spawns per-component
/// solvers. Defers next tier based on singles_before_incompletes config.
pub(crate) fn execute_map_full_match_releases(
    shared: &SharedMappingState,
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::MapFullMatchReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

    let sender = require_sender!(computation);

    let assigned_inodes = read_only_db
        .get_assigned_packing_inodes()
        .unwrap_or_default();

    let pool = std::mem::take(&mut state.full_match_pool);
    let (spawned, log_msg) = orchestrate_partial_tier(
        pool,
        ProposalTier::FullMatch,
        &assigned_inodes,
        state.knot_ratio,
        state.knot_size_limit,
        state.allow_resolve_knots_with_discographies,
        read_only_db,
        &sender,
        witness,
        state.low_confidence_max_acoustid_ratio,
        state.low_confidence_max_album_match,
    );

    log_general(format!("[COMPUTE] MapFullMatchReleases: {}", log_msg));

    // Dynamic chain: FullMatch → first of (Incomplete, Singles) based on config
    let next_computation = if state.singles_before_incompletes {
        AnalysisComputation::MapSingleReleases {
            state: SharedMappingState::new(state),
        }
    } else {
        AnalysisComputation::MapIncompleteReleases {
            state: SharedMappingState::new(state),
        }
    };
    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(next_computation)],
    )];

    Result::pipeline(
        computation,
        spawned,
        deferred,
    )
}

/// Execute MapIncompleteReleases — orchestrator for partial-coverage proposals.
pub(crate) fn execute_map_incomplete_releases(
    shared: &SharedMappingState,
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::MapIncompleteReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

    let sender = require_sender!(computation);

    let assigned_inodes = read_only_db
        .get_assigned_packing_inodes()
        .unwrap_or_default();

    let pool = std::mem::take(&mut state.incomplete_pool);
    let (spawned, log_msg) = orchestrate_partial_tier(
        pool,
        ProposalTier::Incomplete,
        &assigned_inodes,
        state.knot_ratio,
        state.knot_size_limit,
        state.allow_resolve_knots_with_discographies,
        read_only_db,
        &sender,
        witness,
        state.low_confidence_max_acoustid_ratio,
        state.low_confidence_max_album_match,
    );

    log_general(format!("[COMPUTE] MapIncompleteReleases: {}", log_msg));

    // Dynamic chain: if singles already ran, go to gap analysis;
    // otherwise, singles run next.
    let next_computation = if state.singles_before_incompletes {
        AnalysisComputation::EmitUnmatchedSignals
    } else {
        AnalysisComputation::MapSingleReleases {
            state: SharedMappingState::new(state),
        }
    };
    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(next_computation)],
    )];

    Result::pipeline(
        computation,
        spawned,
        deferred,
    )
}

/// Execute MapSingleReleases — orchestrator for single-track releases.
///
/// Uses the same orchestrate_partial_tier pipeline. Each single proposal
/// has a 1-element inode set, so the MIS solver naturally picks the
/// best-scoring release per unclaimed inode.
pub(crate) fn execute_map_single_releases(
    shared: &SharedMappingState,
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = AnalysisComputation::MapSingleReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

    let sender = require_sender!(computation);

    let assigned_inodes = read_only_db
        .get_assigned_packing_inodes()
        .unwrap_or_default();

    let pool = std::mem::take(&mut state.single_pool);
    let (spawned, log_msg) = orchestrate_partial_tier(
        pool,
        ProposalTier::Single,
        &assigned_inodes,
        state.knot_ratio,
        state.knot_size_limit,
        state.allow_resolve_knots_with_discographies,
        read_only_db,
        &sender,
        witness,
        state.low_confidence_max_acoustid_ratio,
        state.low_confidence_max_album_match,
    );

    log_general(format!("[COMPUTE] MapSingleReleases: {}", log_msg));

    // Dynamic chain: if incompletes already ran, go to gap analysis;
    // otherwise, incompletes run next.
    let next_computation = if !state.singles_before_incompletes {
        AnalysisComputation::EmitUnmatchedSignals
    } else {
        AnalysisComputation::MapIncompleteReleases {
            state: SharedMappingState::new(state),
        }
    };
    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(next_computation)],
    )];

    Result::pipeline(
        computation,
        spawned,
        deferred,
    )
}

/// Build per-proposal-index alternative siblings from inode signature groups.
///
/// Groups proposals by sorted inode set. For each group with >1 member,
/// the best-scorer is the "keeper" and the rest become its `AlternativeRelease` siblings.
/// Returns a map from keeper index to its siblings.
fn build_signature_siblings(
    proposals: &[Proposal],
    read_only_db: &ReadOnlyDb<'_>,
) -> HashMap<usize, Vec<AlternativeRelease>> {
    let mut sig_groups: HashMap<Vec<i64>, Vec<usize>> = HashMap::new();
    for (i, p) in proposals.iter().enumerate() {
        let mut sig: Vec<i64> = p.inode_set.iter().copied().collect();
        sig.sort_unstable();
        sig_groups.entry(sig).or_default().push(i);
    }

    let manifest = read_only_db.get_packing_manifest().unwrap_or_default();
    let manifest_map: HashMap<&str, (&str, &str)> = manifest
        .iter()
        .map(|r| (r.release_id.as_str(), (r.release_title.as_str(), r.release_artist.as_str())))
        .collect();

    let mut result: HashMap<usize, Vec<AlternativeRelease>> = HashMap::new();

    for group in sig_groups.values() {
        if group.len() < 2 {
            continue;
        }
        // Find best-scorer in group
        let best_idx = *group
            .iter()
            .max_by(|&&a, &&b| {
                proposals[a]
                    .total_score
                    .partial_cmp(&proposals[b].total_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let siblings: Vec<AlternativeRelease> = group
            .iter()
            .filter(|&&i| i != best_idx)
            .filter_map(|&i| {
                let p = &proposals[i];
                let release_id = &p.rows[0].release_id;
                let (title, artist) = manifest_map
                    .get(release_id.as_str())
                    .map(|&(t, a)| (t.to_string(), a.to_string()))
                    .unwrap_or_default();
                Some(AlternativeRelease {
                    release_id: release_id.clone(),
                    release_title: title,
                    release_artist: artist,
                    total_score: p.total_score,
                })
            })
            .collect();

        if !siblings.is_empty() {
            result.insert(best_idx, siblings);
        }
    }

    result
}
