//! Component discovery, knot extraction, and per-component MIS solving.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::db::write_thread::{self};
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    KnotAssignment, KnotClassification, KnotProposalEntry, MatchMethod, PackedReleaseCategory,
    PackedReleaseData, PackedReleaseSignal, PackingKnotData, PackingKnotSignal,
    PackingScoreBreakdown, ReleasePackingData, ReleasePackingSignal, TypedSignalWrite,
};

use super::mis::MisCandidate;
use super::types::{
    ComponentData, Proposal, ProposalTier, SharedComponentData,
};
use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};

/// Find connected components in a conflict graph over proposals.
///
/// Two proposals conflict if they share any inode. Returns a list of
/// components, each being a sorted Vec of indices into the input slice.
pub(super) fn find_conflict_components(proposals: &[Proposal]) -> Vec<Vec<usize>> {
    let n = proposals.len();
    if n == 0 {
        return Vec::new();
    }

    // Build conflict adjacency
    let mut inode_to_idx: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in proposals.iter().enumerate() {
        for &inode in &p.inode_set {
            inode_to_idx.entry(inode).or_default().push(i);
        }
    }

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for indices in inode_to_idx.values() {
        if indices.len() > 1 {
            for &i in indices {
                for &j in indices {
                    if i != j {
                        adj[i].push(j);
                    }
                }
            }
        }
    }
    for neighbors in &mut adj {
        neighbors.sort_unstable();
        neighbors.dedup();
    }

    // BFS connected components
    let mut visited = vec![false; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut comp = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited[start] = true;
        while let Some(node) = queue.pop_front() {
            comp.push(node);
            for &neighbor in &adj[node] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        comp.sort_unstable();
        components.push(comp);
    }

    components
}

/// Emit signals for a single isolated proposal (component of size 1).
///
/// Used by tier orchestrators to handle trivially-selected proposals without
/// spawning a computation. Returns the number of inode signals emitted.
pub(super) fn emit_isolated_proposal_signals(
    proposal: &Proposal,
    tier: ProposalTier,
    manifest_map: &HashMap<&str, (&str, &str, i32)>,
    corpus_paths: &HashMap<i64, String>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
) -> usize {
    let release_id = &proposal.rows[0].release_id;
    let (release_title, release_artist, total_tracks) =
        match manifest_map.get(release_id.as_str()) {
            Some(&(t, a, tt)) => (t.to_string(), a.to_string(), tt),
            None => return 0,
        };

    let filled = proposal.rows.len() as u32;
    let category = match tier {
        ProposalTier::Perfect => PackedReleaseCategory::Perfect,
        ProposalTier::FullMatch => PackedReleaseCategory::FullMatch,
        ProposalTier::Incomplete => PackedReleaseCategory::Incomplete,
        ProposalTier::Single => PackedReleaseCategory::Single,
    };

    log_general(format!(
        "[PACKING-PICK] tier={} release={} score={:.4} inodes={} filled={}/{}",
        tier.as_str(), release_id, proposal.total_score, proposal.inode_set.len(), filled, total_tracks,
    ));

    let mut signals_batch: Vec<TypedSignalWrite> = Vec::new();
    let mut pending_submissions: Vec<write_thread::PendingAcoustIdSubmission> = Vec::new();

    // PackedRelease aggregate
    let packed_key = format!("{}:{}", category.key_prefix(), release_id);
    signals_batch.push(TypedSignalWrite::PackedRelease(PackedReleaseSignal {
        key: packed_key,
        data: PackedReleaseData {
            release_id: release_id.clone(),
            release_title: release_title.clone(),
            release_artist: release_artist.clone(),
            category,
            assigned_count: filled,
            total_tracks: total_tracks as u32,
        },
    }));

    // Per-inode ReleasePackingSignal
    let mut count = 0usize;
    for row in &proposal.rows {
        let path = match corpus_paths.get(&row.inode) {
            Some(p) => p.clone(),
            None => continue,
        };

        let breakdown: PackingScoreBreakdown = bincode::deserialize(&row.score_breakdown)
            .unwrap_or(PackingScoreBreakdown {
                acoustid_confidence: 0.0,
                duration_match: 0.0,
                title_match: 0.0,
                artist_match: 0.0,
                album_match: 0.0,
                track_number_match: 0.0,
            });

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
                alternatives_count: 1, // isolated — no alternatives
                release_coverage: filled as f32 / total_tracks.max(1) as f32,
                match_method: if row.match_method == 1 {
                    MatchMethod::Elimination
                } else {
                    MatchMethod::AcoustId
                },
            },
        }));
        count += 1;

        if row.match_method == 1 {
            if let (Some(fp_hex), Some(dur_ms)) = (&row.fingerprint_hex, row.raw_duration_ms) {
                pending_submissions.push(write_thread::PendingAcoustIdSubmission {
                    fingerprint: fp_hex.clone(),
                    recording_id: row.recording_id.clone(),
                    duration_ms: dur_ms,
                    source: "elimination".to_string(),
                });
            }
        }
    }

    if !signals_batch.is_empty() {
        sender.write_typed_signal_batch(signals_batch, witness);
    }
    if !pending_submissions.is_empty() {
        sender.write_pending_acoustid_submissions(pending_submissions, witness);
    }

    count
}

/// Emit knot signals for extracted knot components. Returns the selected pool indices.
pub(super) fn emit_knot_component_signals(
    knot_id: usize,
    component_proposals: &[&Proposal],
    tier: ProposalTier,
    classification: KnotClassification,
    ratio: f64,
    manifest_map: &HashMap<&str, (&str, &str, i32)>,
    corpus_paths: &HashMap<i64, String>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
) -> Vec<usize> {
    // Collect contested inodes
    let mut comp_inodes: HashSet<i64> = HashSet::new();
    for p in component_proposals {
        comp_inodes.extend(&p.inode_set);
    }

    // Sort proposals by score descending, greedily pick non-conflicting
    let mut order: Vec<usize> = (0..component_proposals.len()).collect();
    order.sort_by(|&a, &b| {
        component_proposals[b]
            .total_score
            .partial_cmp(&component_proposals[a].total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut knot_claimed: HashSet<i64> = HashSet::new();
    let mut selected_local_indices: Vec<usize> = Vec::new();
    let mut captured_proposals: Vec<(usize, bool)> = Vec::new();

    for &local_idx in &order {
        let p = component_proposals[local_idx];
        let selected = p.inode_set.iter().all(|i| !knot_claimed.contains(i));
        if selected {
            knot_claimed.extend(&p.inode_set);
            selected_local_indices.push(local_idx);
        }
        captured_proposals.push((local_idx, selected));
    }

    // Log knot proposals
    for &(local_idx, selected) in &captured_proposals {
        let p = component_proposals[local_idx];
        log_general(format!(
            "[PACKING-KNOT] tier={} knot={} release={} score={:.4} inodes={} selected={}",
            tier.as_str(), knot_id, p.rows[0].release_id, p.total_score,
            p.inode_set.len(), selected,
        ));
    }

    let mut signals_batch: Vec<TypedSignalWrite> = Vec::new();
    let mut pending_submissions: Vec<write_thread::PendingAcoustIdSubmission> = Vec::new();

    // Emit PackingKnot signal
    let proposals_data: Vec<KnotProposalEntry> = captured_proposals
        .iter()
        .map(|&(local_idx, selected)| {
            let proposal = component_proposals[local_idx];
            let (release_title, release_artist, total_tracks) =
                match manifest_map.get(proposal.rows[0].release_id.as_str()) {
                    Some(&(t, a, tt)) => (t.to_string(), a.to_string(), tt),
                    None => (String::new(), String::new(), proposal.total_tracks),
                };
            let assignments = proposal
                .rows
                .iter()
                .map(|row| {
                    let breakdown: PackingScoreBreakdown =
                        bincode::deserialize(&row.score_breakdown).unwrap_or(
                            PackingScoreBreakdown {
                                acoustid_confidence: 0.0,
                                duration_match: 0.0,
                                title_match: 0.0,
                                artist_match: 0.0,
                                album_match: 0.0,
                                track_number_match: 0.0,
                            },
                        );
                    KnotAssignment {
                        inode: row.inode,
                        recording_id: row.recording_id.clone(),
                        medium_pos: row.medium_pos,
                        track_pos: row.track_pos,
                        track_title: row.track_title.clone(),
                        score: row.score,
                        score_breakdown: breakdown,
                        match_method: row.match_method,
                    }
                })
                .collect();
            KnotProposalEntry {
                release_id: proposal.rows[0].release_id.clone(),
                release_title,
                release_artist,
                total_tracks,
                total_score: proposal.total_score,
                selected,
                assignments,
            }
        })
        .collect();

    let mut contested: Vec<i64> = comp_inodes.into_iter().collect();
    contested.sort_unstable();

    let key = format!("{}:{}", tier.as_str(), knot_id);
    signals_batch.push(TypedSignalWrite::PackingKnot(PackingKnotSignal {
        key,
        data: PackingKnotData {
            tier: tier.as_str().to_string(),
            knot_id,
            classification,
            ratio,
            contested_inodes: contested,
            proposals: proposals_data,
        },
    }));

    // Emit PackedRelease + ReleasePacking for greedy-selected proposals
    let category = match tier {
        ProposalTier::Perfect => PackedReleaseCategory::Perfect,
        ProposalTier::FullMatch => PackedReleaseCategory::FullMatch,
        ProposalTier::Incomplete => PackedReleaseCategory::Incomplete,
        ProposalTier::Single => PackedReleaseCategory::Single,
    };

    for &local_idx in &selected_local_indices {
        let proposal = component_proposals[local_idx];
        let release_id = &proposal.rows[0].release_id;
        let (release_title, release_artist, total_tracks) =
            match manifest_map.get(release_id.as_str()) {
                Some(&(t, a, tt)) => (t.to_string(), a.to_string(), tt),
                None => continue,
            };

        let filled = proposal.rows.len() as u32;

        log_general(format!(
            "[PACKING-PICK] tier={} release={} score={:.4} inodes={} filled={}/{} (knot-greedy)",
            tier.as_str(), release_id, proposal.total_score, proposal.inode_set.len(), filled, total_tracks,
        ));

        let packed_key = format!("{}:{}", category.key_prefix(), release_id);
        signals_batch.push(TypedSignalWrite::PackedRelease(PackedReleaseSignal {
            key: packed_key,
            data: PackedReleaseData {
                release_id: release_id.clone(),
                release_title: release_title.clone(),
                release_artist: release_artist.clone(),
                category,
                assigned_count: filled,
                total_tracks: total_tracks as u32,
            },
        }));

        for row in &proposal.rows {
            let path = match corpus_paths.get(&row.inode) {
                Some(p) => p.clone(),
                None => continue,
            };

            let breakdown: PackingScoreBreakdown = bincode::deserialize(&row.score_breakdown)
                .unwrap_or(PackingScoreBreakdown {
                    acoustid_confidence: 0.0,
                    duration_match: 0.0,
                    title_match: 0.0,
                    artist_match: 0.0,
                    album_match: 0.0,
                    track_number_match: 0.0,
                });

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
                    alternatives_count: component_proposals.len() as u16,
                    release_coverage: filled as f32 / total_tracks.max(1) as f32,
                    match_method: if row.match_method == 1 {
                        MatchMethod::Elimination
                    } else {
                        MatchMethod::AcoustId
                    },
                },
            }));

            if row.match_method == 1 {
                if let (Some(fp_hex), Some(dur_ms)) = (&row.fingerprint_hex, row.raw_duration_ms) {
                    pending_submissions.push(write_thread::PendingAcoustIdSubmission {
                        fingerprint: fp_hex.clone(),
                        recording_id: row.recording_id.clone(),
                        duration_ms: dur_ms,
                        source: "elimination".to_string(),
                    });
                }
            }
        }
    }

    if !signals_batch.is_empty() {
        sender.write_typed_signal_batch(signals_batch, witness);
    }
    if !pending_submissions.is_empty() {
        sender.write_pending_acoustid_submissions(pending_submissions, witness);
    }

    selected_local_indices
}

/// Orchestrate a "partial" tier (FullMatch, Incomplete, or Single).
///
/// Pipeline: cull tainted → dedup by inode signature → extract knots →
/// find components → emit isolated directly → spawn N component solvers.
///
/// Returns (spawned_computations, log_message).
pub(super) fn orchestrate_partial_tier(
    pool: Vec<Proposal>,
    tier: ProposalTier,
    assigned_inodes: &HashSet<i64>,
    knot_ratio: f64,
    knot_size_limit: usize,
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
) -> (Vec<AnalysisComputation>, String) {
    let pool_size = pool.len();

    // --- Step 1: Cull tainted proposals ---
    let effective: Vec<Proposal> = pool
        .into_iter()
        .filter(|p| p.inode_set.iter().all(|i| !assigned_inodes.contains(i)))
        .collect();
    let culled = pool_size - effective.len();

    // --- Step 2: Dedup by inode signature ---
    // Multiple proposals wanting the exact same set of inodes keep only best-scoring.
    let mut sig_best: HashMap<Vec<i64>, (usize, f64)> = HashMap::new();
    for (i, p) in effective.iter().enumerate() {
        let mut sig: Vec<i64> = p.inode_set.iter().copied().collect();
        sig.sort_unstable();
        match sig_best.entry(sig) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((i, p.total_score));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if p.total_score > e.get().1 {
                    e.insert((i, p.total_score));
                }
            }
        }
    }
    let keep_indices: HashSet<usize> = sig_best.values().map(|(idx, _)| *idx).collect();
    let dedup_removed = effective.len() - keep_indices.len();

    // Collect kept proposals (preserving order for determinism)
    let mut deduped: Vec<Proposal> = Vec::with_capacity(keep_indices.len());
    for (i, p) in effective.into_iter().enumerate() {
        if keep_indices.contains(&i) {
            deduped.push(p);
        }
    }

    log_general(format!(
        "[COMPUTE] {} partial: pool={}, culled={} (tainted), deduped={} (identical sigs), {} pristine remain",
        tier.as_str(), pool_size, culled, dedup_removed, deduped.len(),
    ));

    if deduped.is_empty() {
        return (
            Vec::new(),
            format!(
                "pool={}, culled={}, deduped={}, 0 eligible",
                pool_size, culled, dedup_removed
            ),
        );
    }

    // --- Step 3: Find components, extract knots ---
    let components = find_conflict_components(&deduped);

    // Load manifest + corpus paths for signal emission
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
    let mut knot_component_count = 0usize;
    let mut knot_proposal_count = 0usize;
    let mut knot_id = 0usize;

    for component in &components {
        if component.len() == 1 {
            // Isolated node — emit directly
            isolated_signals += emit_isolated_proposal_signals(
                &deduped[component[0]],
                tier,
                &manifest_map,
                &corpus_paths,
                sender,
                witness,
            );
            isolated_count += 1;
            continue;
        }

        // Check knot extraction threshold
        let mut comp_inodes: HashSet<i64> = HashSet::new();
        for &idx in component {
            comp_inodes.extend(&deduped[idx].inode_set);
        }
        let ratio = component.len() as f64 / comp_inodes.len().max(1) as f64;

        let is_knot_by_ratio = knot_ratio > 0.0 && ratio >= knot_ratio;
        let is_knot_by_size = knot_size_limit > 0 && component.len() > knot_size_limit;

        if is_knot_by_ratio || is_knot_by_size {
            // Extract knot: greedy best-scorer, emit signals directly
            let classification = if is_knot_by_ratio {
                KnotClassification::ByRatio
            } else {
                KnotClassification::BySize
            };

            let component_proposals: Vec<&Proposal> =
                component.iter().map(|&i| &deduped[i]).collect();

            emit_knot_component_signals(
                knot_id,
                &component_proposals,
                tier,
                classification,
                ratio,
                &manifest_map,
                &corpus_paths,
                sender,
                witness,
            );

            knot_component_count += 1;
            knot_proposal_count += component.len();
            knot_id += 1;
        } else {
            // Clean component — spawn solver
            let component_proposals: Vec<Proposal> = component
                .iter()
                .map(|&i| {
                    let p = &deduped[i];
                    Proposal {
                        total_tracks: p.total_tracks,
                        rows: p.rows.clone(),
                        inode_set: p.inode_set.clone(),
                        total_score: p.total_score,
                        tier: p.tier,
                    }
                })
                .collect();

            spawned.push(AnalysisComputation::ResolvePackingComponent {
                data: SharedComponentData::new(ComponentData {
                    proposals: component_proposals,
                    tier,
                    corpus_paths: corpus_paths.clone(),
                    manifest_map: manifest_map_owned.clone(),
                }),
            });
        }
    }

    if knot_component_count > 0 {
        log_general(format!(
            "[COMPUTE] {} partial: extracted {} knot components ({} proposals; thresholds: ratio={:.1}, size={})",
            tier.as_str(), knot_component_count, knot_proposal_count, knot_ratio, knot_size_limit,
        ));
    }

    let log_msg = format!(
        "pool={}, culled={}, deduped={}, {} eligible, {} components ({} isolated → {} signals, {} knots → {} proposals, {} spawned)",
        pool_size, culled, dedup_removed, deduped.len(), components.len(),
        isolated_count, isolated_signals, knot_component_count, knot_proposal_count, spawned.len(),
    );

    (spawned, log_msg)
}

// ============================================================================
// Per-component MIS solver
// ============================================================================

/// Execute ResolvePackingComponent — solve MIS for one connected component.
///
/// Self-contained: builds local conflict graph, solves, emits signals.
/// Spawned in parallel by tier orchestrators.
pub(crate) fn execute_resolve_packing_component(
    shared: &SharedComponentData,
    _read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::ResolvePackingComponent {
        data: shared.clone(),
    };
    let component = shared.take();
    let proposals = component.proposals;
    let tier = component.tier;
    let corpus_paths = component.corpus_paths;
    let manifest_map_owned = component.manifest_map;

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

    if proposals.is_empty() {
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Build MIS candidates from proposals
    let mis_candidates: Vec<MisCandidate> = proposals
        .iter()
        .map(|p| MisCandidate {
            inode_set: p.inode_set.clone(),
            score: p.total_score,
        })
        .collect();

    let mis_result = super::mis::solve_maximum_independent_set(&mis_candidates);

    // Build local alternatives_count: how many proposals in THIS component
    // contain each inode
    let mut local_inode_proposals: HashMap<i64, u16> = HashMap::new();
    for p in &proposals {
        for &inode in &p.inode_set {
            *local_inode_proposals.entry(inode).or_insert(0) += 1;
        }
    }

    // Build manifest_map ref from owned data
    let manifest_map: HashMap<&str, (&str, &str, i32)> = manifest_map_owned
        .iter()
        .map(|(k, (t, a, tt))| (k.as_str(), (t.as_str(), a.as_str(), *tt)))
        .collect();

    let category = match tier {
        ProposalTier::Perfect => PackedReleaseCategory::Perfect,
        ProposalTier::FullMatch => PackedReleaseCategory::FullMatch,
        ProposalTier::Incomplete => PackedReleaseCategory::Incomplete,
        ProposalTier::Single => PackedReleaseCategory::Single,
    };

    let mut total_assigned = 0usize;
    let mut pending_submissions: Vec<write_thread::PendingAcoustIdSubmission> = Vec::new();
    let mut signals_batch: Vec<TypedSignalWrite> = Vec::new();

    for (idx, selected) in mis_result.selected.iter().enumerate() {
        if !selected {
            continue;
        }
        let proposal = &proposals[idx];
        let release_id = &proposal.rows[0].release_id;
        let (release_title, release_artist, total_tracks) =
            match manifest_map.get(release_id.as_str()) {
                Some(&(t, a, tt)) => (t.to_string(), a.to_string(), tt),
                None => continue,
            };

        let filled = proposal.rows.len() as u32;

        log_general(format!(
            "[PACKING-PICK] tier={} release={} score={:.4} inodes={} filled={}/{}",
            tier.as_str(), release_id, proposal.total_score, proposal.inode_set.len(), filled, total_tracks,
        ));

        // Emit PackedRelease aggregate signal
        let packed_key = format!("{}:{}", category.key_prefix(), release_id);
        let packed_signal = PackedReleaseSignal {
            key: packed_key,
            data: PackedReleaseData {
                release_id: release_id.clone(),
                release_title: release_title.clone(),
                release_artist: release_artist.clone(),
                category,
                assigned_count: filled,
                total_tracks: total_tracks as u32,
            },
        };
        signals_batch.push(TypedSignalWrite::PackedRelease(packed_signal));

        // Emit per-inode ReleasePackingSignal
        for row in &proposal.rows {
            let path = match corpus_paths.get(&row.inode) {
                Some(p) => p.clone(),
                None => continue,
            };

            let alternatives_count = local_inode_proposals
                .get(&row.inode)
                .copied()
                .unwrap_or(1);

            let breakdown: PackingScoreBreakdown = bincode::deserialize(&row.score_breakdown)
                .unwrap_or(PackingScoreBreakdown {
                    acoustid_confidence: 0.0,
                    duration_match: 0.0,
                    title_match: 0.0,
                    artist_match: 0.0,
                    album_match: 0.0,
                    track_number_match: 0.0,
                });

            let signal = ReleasePackingSignal {
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
                    alternatives_count,
                    release_coverage: filled as f32 / total_tracks.max(1) as f32,
                    match_method: if row.match_method == 1 {
                        MatchMethod::Elimination
                    } else {
                        MatchMethod::AcoustId
                    },
                },
            };
            signals_batch.push(TypedSignalWrite::ReleasePacking(signal));
            total_assigned += 1;

            // Record AcoustID submission for elimination matches
            if row.match_method == 1 {
                if let (Some(fp_hex), Some(dur_ms)) = (&row.fingerprint_hex, row.raw_duration_ms) {
                    pending_submissions.push(write_thread::PendingAcoustIdSubmission {
                        fingerprint: fp_hex.clone(),
                        recording_id: row.recording_id.clone(),
                        duration_ms: dur_ms,
                        source: "elimination".to_string(),
                    });
                }
            }
        }
    }

    // Send all signals in a batch
    if !signals_batch.is_empty() {
        sender.write_typed_signal_batch(signals_batch, witness);
    }

    // Record AcoustID submissions
    if !pending_submissions.is_empty() {
        sender.write_pending_acoustid_submissions(pending_submissions, witness);
    }

    log_general(format!(
        "[COMPUTE] ResolvePackingComponent: tier={} proposals={} selected={} coverage={} signals={}",
        tier.as_str(),
        proposals.len(),
        mis_result.selected_count,
        mis_result.selected_coverage,
        total_assigned,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(inodes: &[i64]) -> Proposal {
        Proposal {
            total_tracks: inodes.len() as i32,
            rows: Vec::new(),
            inode_set: inodes.iter().copied().collect(),
            total_score: 1.0,
            tier: ProposalTier::FullMatch,
        }
    }

    #[test]
    fn test_components_deterministic() {
        let proposals = vec![
            proposal(&[1, 2]),
            proposal(&[2, 3]),
            proposal(&[4, 5]),
            proposal(&[5, 6]),
            proposal(&[7]),
        ];
        let first = find_conflict_components(&proposals);
        for _ in 0..100 {
            assert_eq!(find_conflict_components(&proposals), first);
        }
    }

    #[test]
    fn test_components_all_isolated() {
        let proposals = vec![
            proposal(&[1]),
            proposal(&[2]),
            proposal(&[3]),
        ];
        let components = find_conflict_components(&proposals);
        assert_eq!(components.len(), 3);
        for comp in &components {
            assert_eq!(comp.len(), 1);
        }
    }

    #[test]
    fn test_components_chain() {
        // A shares inode with B, B shares with C → one component
        let proposals = vec![
            proposal(&[1, 2]),
            proposal(&[2, 3]),
            proposal(&[3, 4]),
        ];
        let components = find_conflict_components(&proposals);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 3);
    }

    #[test]
    fn test_components_two_groups() {
        let proposals = vec![
            proposal(&[1, 2]),
            proposal(&[2, 3]),
            proposal(&[10, 11]),
            proposal(&[11, 12]),
        ];
        let components = find_conflict_components(&proposals);
        assert_eq!(components.len(), 2);
        // Each component has 2 proposals
        assert_eq!(components[0].len(), 2);
        assert_eq!(components[1].len(), 2);
    }

    #[test]
    fn test_components_empty() {
        let components = find_conflict_components(&[]);
        assert!(components.is_empty());
    }
}
