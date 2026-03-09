//! Stage 3: ComputeReleaseMappings and tier orchestrators (MapPerfect/FullMatch/Incomplete/Single).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::db::write_thread::{self};
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::computations::{Computation, PipelineStage};
use crate::meta::signals::data::{
    PackedReleaseSignal, PackingKnotSignal, ReleasePackingSignal,
};

use super::components::{
    emit_isolated_proposal_signals, find_conflict_components,
    orchestrate_partial_tier,
};
use super::types::{
    classify_proposal, ComponentData, Proposal, ProposalTier, ReleaseMappingState,
    SharedComponentData, SharedMappingState, DEFAULT_KNOT_RATIO, DEFAULT_KNOT_SIZE_LIMIT,
};
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
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::ComputeReleaseMappings;

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

    // Bulk-clear all packing-related signal tables for clean re-emission.
    // Each tier will emit its signals per-component as it solves.
    sender.clear_signal_table::<ReleasePackingSignal>(witness);
    sender.clear_aggregate_signal_table::<PackedReleaseSignal>(witness);
    sender.clear_aggregate_signal_table::<PackingKnotSignal>(witness);
    write_thread::wait_for_queue_drain();

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
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
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
                start.elapsed().as_millis() as u64,
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
    let (knot_ratio, knot_size_limit, singles_before_incompletes) =
        match crate::config::load_config() {
            Ok(c) => {
                let em = &c.opinions.external_matching;
                (
                    em.packing_knot_ratio,
                    em.packing_knot_size_limit,
                    em.singles_before_incompletes,
                )
            }
            Err(_) => (DEFAULT_KNOT_RATIO, DEFAULT_KNOT_SIZE_LIMIT, false),
        };

    // Package state and defer Round 1
    let state = SharedMappingState::new(ReleaseMappingState {
        perfect_pool,
        full_match_pool,
        incomplete_pool,
        single_pool,
        knot_ratio,
        knot_size_limit,
        singles_before_incompletes,
    });

    let deferred = vec![(
        PipelineStage::Resolve,
        vec![Computation::Analysis(
            AnalysisComputation::MapPerfectReleases { state },
        )],
    )];

    Result::pipeline(
        computation,
        start.elapsed().as_millis() as u64,
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
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::MapPerfectReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

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
            start.elapsed().as_millis() as u64,
            Vec::new(),
            vec![(
                PipelineStage::Resolve,
                vec![Computation::Analysis(
                    AnalysisComputation::MapFullMatchReleases { state: next_state },
                )],
            )],
        );
    }

    // Find connected components
    let components = find_conflict_components(&eligible);

    // Load manifest + corpus paths for isolated-node emission
    let manifest = read_only_db.get_packing_manifest().unwrap_or_default();
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
    let corpus_paths: HashMap<i64, String> = read_only_db
        .get_packing_inode_paths()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Build owned manifest_map for component data
    let manifest_map_owned: HashMap<String, (String, String, i32)> = manifest
        .iter()
        .map(|r| {
            (
                r.release_id.clone(),
                (
                    r.release_title.clone(),
                    r.release_artist.clone(),
                    r.total_tracks,
                ),
            )
        })
        .collect();

    let mut spawned: Vec<AnalysisComputation> = Vec::new();
    let mut isolated_count = 0usize;
    let mut isolated_signals = 0usize;

    for component in &components {
        if component.len() == 1 {
            // Isolated node — emit directly
            isolated_signals += emit_isolated_proposal_signals(
                &eligible[component[0]],
                ProposalTier::Perfect,
                &manifest_map,
                &corpus_paths,
                &sender,
                witness,
            );
            isolated_count += 1;
        } else {
            // Multi-node component — spawn solver
            let component_proposals: Vec<Proposal> =
                component.iter().map(|&i| {
                    // Move proposal data out by reconstructing from eligible
                    let p = &eligible[i];
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
        start.elapsed().as_millis() as u64,
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
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::MapFullMatchReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

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
        read_only_db,
        &sender,
        witness,
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
        start.elapsed().as_millis() as u64,
        spawned,
        deferred,
    )
}

/// Execute MapIncompleteReleases — orchestrator for partial-coverage proposals.
pub(crate) fn execute_map_incomplete_releases(
    shared: &SharedMappingState,
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::MapIncompleteReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

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
        read_only_db,
        &sender,
        witness,
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
        start.elapsed().as_millis() as u64,
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
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::MapSingleReleases {
        state: shared.clone(),
    };
    let mut state = shared.take();

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
        read_only_db,
        &sender,
        witness,
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
        start.elapsed().as_millis() as u64,
        spawned,
        deferred,
    )
}
