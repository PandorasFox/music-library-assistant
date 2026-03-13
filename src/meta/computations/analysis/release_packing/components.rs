//! Component discovery, knot extraction, and per-component MIS solving.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::db::write_thread::{self};
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    AlternativeReleasePackingData, AlternativeReleasePackingSignal,
    KnotAssignment, KnotClassification, KnotProposalEntry, MatchMethod, PackedReleaseCategory,
    PackedReleaseData, PackedReleaseSignal, PackingKnotData, PackingKnotSignal,
    PackingScoreBreakdown, ReleasePackingData, ReleasePackingSignal,
    VariousArtistsOverrideData, VariousArtistsOverrideSignal, VariousArtistsOverrideSource};
use crate::meta::signals::registry::TypedSignalWrite;

use super::mis::MisCandidate;
use super::types::{
    build_manifest_map, build_manifest_map_owned,
    AlternativeRelease, ComponentData, Proposal, ProposalTier, SharedComponentData,
};
use crate::meta::computations::analysis::{Computation as AnalysisComputation, Result};

/// Find connected components in a conflict graph over proposals.
///
/// Two proposals conflict if they share any inode. Returns a list of
/// components, each being a sorted Vec of indices into the input slice.
pub(super) fn find_conflict_components(proposals: &[Arc<Proposal>]) -> Vec<Vec<usize>> {
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

/// Check whether a proposal should be downgraded to LowConfidence.
///
/// Only applies to FullMatch and Incomplete tiers. Returns true when the
/// AcoustID ratio (rows matched via AcoustID / total rows) and the average
/// album_match score are both below the configured thresholds.
/// Check if a proposal should be downgraded to LowConfidence.
/// Returns `Some((acoustid_ratio, avg_album_match))` if downgraded, `None` otherwise.
fn check_low_confidence(
    proposal: &Proposal,
    tier: ProposalTier,
    low_confidence_max_acoustid_ratio: f64,
    low_confidence_max_album_match: f64,
) -> Option<(f64, f64)> {
    match tier {
        ProposalTier::FullMatch | ProposalTier::Incomplete => {}
        _ => return None,
    }

    if proposal.rows.is_empty() {
        return None;
    }

    let total = proposal.rows.len() as f64;
    let acoustid_count = proposal
        .rows
        .iter()
        .filter(|r| r.match_method == 0) // 0 = AcoustId
        .count() as f64;
    let acoustid_ratio = acoustid_count / total;

    if acoustid_ratio >= low_confidence_max_acoustid_ratio {
        return None;
    }

    // Compute average album_match from score breakdowns
    let mut album_match_sum = 0.0f64;
    let mut decoded_count = 0usize;
    for row in &proposal.rows {
        if let Ok(breakdown) = bincode::deserialize::<PackingScoreBreakdown>(&row.score_breakdown) {
            album_match_sum += breakdown.album_match;
            decoded_count += 1;
        }
    }

    if decoded_count == 0 {
        return None;
    }

    let avg_album_match = album_match_sum / decoded_count as f64;
    if avg_album_match < low_confidence_max_album_match {
        Some((acoustid_ratio, avg_album_match))
    } else {
        None
    }
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
    siblings: &[AlternativeRelease],
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
    low_confidence_max_acoustid_ratio: f64,
    low_confidence_max_album_match: f64,
) -> usize {
    let release_id = &proposal.rows[0].release_id;
    let (release_title, release_artist, total_tracks) =
        match manifest_map.get(release_id.as_str()) {
            Some(&(t, a, tt)) => (t.to_string(), a.to_string(), tt),
            None => return 0,
        };

    let filled = proposal.rows.len() as u32;
    let mut category = match tier {
        ProposalTier::Perfect => PackedReleaseCategory::Perfect,
        ProposalTier::FullMatch => PackedReleaseCategory::FullMatch,
        ProposalTier::Incomplete => PackedReleaseCategory::Incomplete,
        ProposalTier::Single => PackedReleaseCategory::Single,
    };

    let lc_metrics = check_low_confidence(
        proposal,
        tier,
        low_confidence_max_acoustid_ratio,
        low_confidence_max_album_match,
    );
    if lc_metrics.is_some() {
        category = PackedReleaseCategory::LowConfidence;
    }

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
            low_confidence_acoustid_ratio: lc_metrics.map(|(r, _)| r),
            low_confidence_avg_album_match: lc_metrics.map(|(_, a)| a),
        },
    }));

    // Per-inode ReleasePackingSignal
    let mut count = 0usize;
    for row in &proposal.rows {
        let path = match corpus_paths.get(&row.inode) {
            Some(p) => p.clone(),
            None => continue,
        };

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

    // Emit alternative release signals for siblings
    let inode_count = proposal.inode_set.len() as u32;
    for sib in siblings {
        let alt_key = format!("{}:{}", release_id, sib.release_id);
        signals_batch.push(TypedSignalWrite::AlternativeReleasePacking(
            AlternativeReleasePackingSignal {
                key: alt_key,
                data: AlternativeReleasePackingData {
                    winner_release_id: release_id.clone(),
                    winner_release_title: release_title.clone(),
                    alternative_release_id: sib.release_id.clone(),
                    alternative_release_title: sib.release_title.clone(),
                    alternative_release_artist: sib.release_artist.clone(),
                    alternative_score: sib.total_score,
                    winner_score: proposal.total_score,
                    inode_count,
                },
            },
        ));
    }

    // VA override: if winner is "Various Artists", suggest a non-VA artist from siblings
    emit_va_override_from_siblings(
        release_id,
        &release_title,
        &release_artist,
        siblings,
        &mut signals_batch,
    );

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
    allow_discography_reduction: bool,
    manifest_map: &HashMap<&str, (&str, &str, i32)>,
    corpus_paths: &HashMap<i64, String>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
    low_confidence_max_acoustid_ratio: f64,
    low_confidence_max_album_match: f64,
) -> Vec<usize> {
    // Collect contested inodes
    let mut comp_inodes: HashSet<i64> = HashSet::new();
    for p in component_proposals {
        comp_inodes.extend(&p.inode_set);
    }

    // --- Discography reduction ---
    // If enabled, check for proposals whose inode_set covers ALL contested inodes.
    // If any exist, they subsume the entire knot — emit them as normal picks
    // (no knot signal, losers silently dropped).
    if allow_discography_reduction {
        let mut covering: Vec<(usize, &Proposal)> = component_proposals
            .iter()
            .enumerate()
            .filter(|(_, p)| comp_inodes.is_subset(&p.inode_set))
            .map(|(i, p)| (i, *p))
            .collect();
        if !covering.is_empty() {
            // Pick best-scoring covering proposal(s) greedily
            covering.sort_by(|a, b| {
                b.1.total_score
                    .partial_cmp(&a.1.total_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut claimed: HashSet<i64> = HashSet::new();
            let mut selected: Vec<usize> = Vec::new();
            for &(original_idx, p) in &covering {
                if p.inode_set.iter().all(|i| !claimed.contains(i)) {
                    claimed.extend(&p.inode_set);
                    selected.push(original_idx);
                }
            }

            log_general(format!(
                "[PACKING-KNOT] tier={} knot={} discography-reduction: {} of {} proposals cover all {} contested inodes, {} selected as picks",
                tier.as_str(), knot_id, covering.len(), component_proposals.len(),
                comp_inodes.len(), selected.len(),
            ));

            // Build signature groups among covering set for alternative detection
            let mut covering_sig_groups: HashMap<Vec<i64>, Vec<usize>> = HashMap::new();
            for &(orig_idx, p) in &covering {
                let mut sig: Vec<i64> = p.inode_set.iter().copied().collect();
                sig.sort_unstable();
                covering_sig_groups.entry(sig).or_default().push(orig_idx);
            }

            // Emit winners as normal picks — no knot signal
            for &original_idx in &selected {
                let p = component_proposals[original_idx];
                let mut sig: Vec<i64> = p.inode_set.iter().copied().collect();
                sig.sort_unstable();

                // Siblings = other covering proposals with the same signature, excluding self
                let siblings: Vec<AlternativeRelease> = covering_sig_groups
                    .get(&sig)
                    .map(|group| {
                        group
                            .iter()
                            .filter(|&&i| i != original_idx)
                            .filter_map(|&i| {
                                let sp = component_proposals[i];
                                let rid = &sp.rows[0].release_id;
                                let (t, a, _) = manifest_map.get(rid.as_str())?;
                                Some(AlternativeRelease {
                                    release_id: rid.clone(),
                                    release_title: t.to_string(),
                                    release_artist: a.to_string(),
                                    total_score: sp.total_score,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                emit_isolated_proposal_signals(
                    p,
                    tier,
                    manifest_map,
                    corpus_paths,
                    &siblings,
                    sender,
                    witness,
                    low_confidence_max_acoustid_ratio,
                    low_confidence_max_album_match,
                );
            }
            return selected;
        }
    }

    // --- Standard knot resolution: greedy best-scorer ---

    // Sort proposals by score descending, greedily pick non-conflicting
    let mut order: Vec<usize> = (0..component_proposals.len()).collect();
    order.sort_by(|&a, &b| {
        component_proposals[b]
            .total_score
            .partial_cmp(&component_proposals[a].total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut knot_claimed: HashSet<i64> = HashSet::new();
    let mut captured_proposals: Vec<(usize, bool)> = Vec::new();

    for &original_idx in &order {
        let p = component_proposals[original_idx];
        let selected = p.inode_set.iter().all(|i| !knot_claimed.contains(i));
        if selected {
            knot_claimed.extend(&p.inode_set);
        }
        captured_proposals.push((original_idx, selected));
    }

    // Log knot proposals
    for &(original_idx, selected) in &captured_proposals {
        let p = component_proposals[original_idx];
        log_general(format!(
            "[PACKING-KNOT] tier={} knot={} release={} score={:.4} inodes={} selected={}",
            tier.as_str(), knot_id, p.rows[0].release_id, p.total_score,
            p.inode_set.len(), selected,
        ));
    }

    let mut signals_batch: Vec<TypedSignalWrite> = Vec::new();

    // Emit PackingKnot signal
    let selected_set: HashSet<usize> = captured_proposals
        .iter()
        .filter(|(_, sel)| *sel)
        .map(|(idx, _)| *idx)
        .collect();
    let proposals_data = build_knot_proposals_data(
        component_proposals, &selected_set, manifest_map,
    );

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

    // Knots are unresolved contention — emit the knot signal only, no picks.
    // Contested inodes remain unclaimed for manual review.
    if !signals_batch.is_empty() {
        sender.write_typed_signal_batch(signals_batch, witness);
    }

    Vec::new()
}

/// Build the `KnotProposalEntry` list for a knot signal.
///
/// Each proposal gets an entry with its release metadata, assignment details,
/// and whether it was selected by the greedy resolver.
fn build_knot_proposals_data(
    component_proposals: &[&Proposal],
    selected_set: &HashSet<usize>,
    manifest_map: &HashMap<&str, (&str, &str, i32)>,
) -> Vec<KnotProposalEntry> {
    (0..component_proposals.len())
        .map(|original_idx| {
            let proposal = component_proposals[original_idx];
            let selected = selected_set.contains(&original_idx);
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
                        bincode::deserialize(&row.score_breakdown).unwrap_or_default();
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
        .collect()
}

/// Deduplicate proposals by inode signature.
///
/// Groups proposals by sorted inode set. For each group with multiple members,
/// keeps only the highest-scoring proposal. Losers become `AlternativeRelease`
/// siblings of the keeper. Requires a manifest map for release metadata lookup.
///
/// Returns `(deduped_proposals, siblings_per_deduped_index, removed_count)`.
pub(super) fn dedup_by_signature(
    proposals: Vec<Arc<Proposal>>,
    manifest_map: &HashMap<&str, (&str, &str)>,
) -> (Vec<Arc<Proposal>>, HashMap<usize, Vec<AlternativeRelease>>, usize) {
    let mut sig_groups: HashMap<Vec<i64>, Vec<usize>> = HashMap::new();
    for (i, p) in proposals.iter().enumerate() {
        let mut sig: Vec<i64> = p.inode_set.iter().copied().collect();
        sig.sort_unstable();
        sig_groups.entry(sig).or_default().push(i);
    }

    let mut keep_indices: HashSet<usize> = HashSet::new();
    let mut effective_siblings: HashMap<usize, Vec<AlternativeRelease>> = HashMap::new();

    for group in sig_groups.values() {
        let best_idx = *group
            .iter()
            .max_by(|&&a, &&b| {
                proposals[a]
                    .total_score
                    .partial_cmp(&proposals[b].total_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        keep_indices.insert(best_idx);

        if group.len() > 1 {
            let siblings: Vec<AlternativeRelease> = group
                .iter()
                .filter(|&&i| i != best_idx)
                .map(|&i| {
                    let p = &proposals[i];
                    let release_id = &p.rows[0].release_id;
                    let (title, artist) = manifest_map
                        .get(release_id.as_str())
                        .map(|&(t, a)| (t.to_string(), a.to_string()))
                        .unwrap_or_default();
                    AlternativeRelease {
                        release_id: release_id.clone(),
                        release_title: title,
                        release_artist: artist,
                        total_score: p.total_score,
                    }
                })
                .collect();
            if !siblings.is_empty() {
                effective_siblings.insert(best_idx, siblings);
            }
        }
    }

    let removed = proposals.len() - keep_indices.len();

    // Collect kept proposals preserving order, remapping sibling indices
    let mut deduped: Vec<Arc<Proposal>> = Vec::with_capacity(keep_indices.len());
    let mut deduped_siblings: HashMap<usize, Vec<AlternativeRelease>> = HashMap::new();

    for (i, p) in proposals.into_iter().enumerate() {
        if keep_indices.contains(&i) {
            let deduped_idx = deduped.len();
            if let Some(sibs) = effective_siblings.remove(&i) {
                deduped_siblings.insert(deduped_idx, sibs);
            }
            deduped.push(p);
        }
    }

    (deduped, deduped_siblings, removed)
}

/// Orchestrate a "partial" tier (FullMatch, Incomplete, or Single).
///
/// Pipeline: cull tainted → dedup by inode signature → extract knots →
/// find components → emit isolated directly → spawn N component solvers.
///
/// Returns (spawned_computations, log_message).
pub(super) fn orchestrate_partial_tier(
    pool: Vec<Arc<Proposal>>,
    tier: ProposalTier,
    assigned_inodes: &HashSet<i64>,
    knot_ratio: f64,
    knot_size_limit: usize,
    allow_discography_reduction: bool,
    read_only_db: &ReadOnlyDb<'_>,
    sender: &write_thread::SignalWriteSender,
    witness: &ComputationWitness,
    low_confidence_max_acoustid_ratio: f64,
    low_confidence_max_album_match: f64,
) -> (Vec<AnalysisComputation>, String) {
    let pool_size = pool.len();

    // --- Step 1: Cull tainted proposals ---
    let effective: Vec<Arc<Proposal>> = pool
        .into_iter()
        .filter(|p| p.inode_set.iter().all(|i| !assigned_inodes.contains(i)))
        .collect();
    let culled = pool_size - effective.len();

    // --- Step 2: Dedup by inode signature ---
    let manifest_for_siblings = read_only_db.get_packing_manifest().unwrap_or_default();
    let sibling_manifest_map: HashMap<&str, (&str, &str)> = manifest_for_siblings
        .iter()
        .map(|r| (r.release_id.as_str(), (r.release_title.as_str(), r.release_artist.as_str())))
        .collect();

    let (deduped, deduped_siblings, dedup_removed) =
        dedup_by_signature(effective, &sibling_manifest_map);

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
    let manifest_map = build_manifest_map(&manifest);
    let corpus_paths: Arc<HashMap<i64, String>> = Arc::new(read_only_db
        .get_packing_inode_paths()
        .unwrap_or_default()
        .into_iter()
        .collect());
    let manifest_map_owned: Arc<HashMap<String, (String, String, i32)>> = Arc::new(build_manifest_map_owned(&manifest));

    let mut spawned: Vec<AnalysisComputation> = Vec::new();
    let mut isolated_count = 0usize;
    let mut isolated_signals = 0usize;
    let mut knot_component_count = 0usize;
    let mut knot_proposal_count = 0usize;
    let mut knot_id = 0usize;

    for component in &components {
        if component.len() == 1 {
            let deduped_idx = component[0];
            let siblings = deduped_siblings.get(&deduped_idx).map(|v| v.as_slice()).unwrap_or(&[]);
            // Isolated node — emit directly
            isolated_signals += emit_isolated_proposal_signals(
                &deduped[deduped_idx],
                tier,
                &manifest_map,
                &corpus_paths,
                siblings,
                sender,
                witness,
                low_confidence_max_acoustid_ratio,
                low_confidence_max_album_match,
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
                component.iter().map(|&i| &*deduped[i]).collect();

            emit_knot_component_signals(
                knot_id,
                &component_proposals,
                tier,
                classification,
                ratio,
                allow_discography_reduction,
                &manifest_map,
                &corpus_paths,
                sender,
                witness,
                low_confidence_max_acoustid_ratio,
                low_confidence_max_album_match,
            );

            knot_component_count += 1;
            knot_proposal_count += component.len();
            knot_id += 1;
        } else {
            // Clean component — spawn solver
            // Remap deduped indices to component-local indices for signature_siblings
            let mut component_siblings: HashMap<usize, Vec<AlternativeRelease>> = HashMap::new();
            let component_proposals: Vec<Arc<Proposal>> = component
                .iter()
                .enumerate()
                .map(|(local_idx, &deduped_idx)| {
                    if let Some(sibs) = deduped_siblings.get(&deduped_idx) {
                        component_siblings.insert(local_idx, sibs.clone());
                    }
                    Arc::clone(&deduped[deduped_idx])
                })
                .collect();

            spawned.push(AnalysisComputation::ResolvePackingComponent {
                data: SharedComponentData::new(ComponentData {
                    proposals: component_proposals,
                    tier,
                    corpus_paths: Arc::clone(&corpus_paths),
                    manifest_map: Arc::clone(&manifest_map_owned),
                    signature_siblings: component_siblings,
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
    ctx: &ComputationContext<'_>,
    shared: &SharedComponentData,
) -> Result {
    let witness = ctx.witness;
    let computation = AnalysisComputation::ResolvePackingComponent {
        data: shared.clone(),
    };
    let component = shared.take();
    let proposals = component.proposals;
    let tier = component.tier;
    let corpus_paths = component.corpus_paths;
    let manifest_map_owned = component.manifest_map;
    let signature_siblings = component.signature_siblings;

    let sender = require_sender!(computation);

    if proposals.is_empty() {
        return Result::success(computation, Vec::new());
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

    // Load low-confidence thresholds from config
    let (lc_acoustid_ratio, lc_album_match) = match ctx.snapshot.config.as_deref() {
        Some(c) => (
            c.opinions.release_packing.low_confidence_max_acoustid_ratio,
            c.opinions.release_packing.low_confidence_max_album_match,
        ),
        None => {
            let d = crate::config::ReleasePackingOpinions::default();
            (d.low_confidence_max_acoustid_ratio, d.low_confidence_max_album_match)
        }
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

        let mut category = match tier {
            ProposalTier::Perfect => PackedReleaseCategory::Perfect,
            ProposalTier::FullMatch => PackedReleaseCategory::FullMatch,
            ProposalTier::Incomplete => PackedReleaseCategory::Incomplete,
            ProposalTier::Single => PackedReleaseCategory::Single,
        };
        let lc_metrics = check_low_confidence(proposal, tier, lc_acoustid_ratio, lc_album_match);
        if lc_metrics.is_some() {
            category = PackedReleaseCategory::LowConfidence;
        }

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
                low_confidence_acoustid_ratio: lc_metrics.map(|(r, _)| r),
                low_confidence_avg_album_match: lc_metrics.map(|(_, a)| a),
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

            let breakdown: PackingScoreBreakdown =
                bincode::deserialize(&row.score_breakdown).unwrap_or_default();

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

        // Emit alternative release signals from signature siblings
        let inode_count = proposal.inode_set.len() as u32;
        if let Some(siblings) = signature_siblings.get(&idx) {
            for sib in siblings {
                let alt_key = format!("{}:{}", release_id, sib.release_id);
                signals_batch.push(TypedSignalWrite::AlternativeReleasePacking(
                    AlternativeReleasePackingSignal {
                        key: alt_key,
                        data: AlternativeReleasePackingData {
                            winner_release_id: release_id.clone(),
                            winner_release_title: release_title.clone(),
                            alternative_release_id: sib.release_id.clone(),
                            alternative_release_title: sib.release_title.clone(),
                            alternative_release_artist: sib.release_artist.clone(),
                            alternative_score: sib.total_score,
                            winner_score: proposal.total_score,
                            inode_count,
                        },
                    },
                ));
            }

            // VA override from exact alternatives (tier 1)
            emit_va_override_from_siblings(
                release_id,
                &release_title,
                &release_artist,
                siblings,
                &mut signals_batch,
            );
        }

        // VA override from competing proposals (tier 2 fallback)
        // Only if no exact-alternative override was emitted
        if is_various_artists(&release_artist) {
            let has_exact_override = signature_siblings
                .get(&idx)
                .map(|sibs| sibs.iter().any(|s| !is_various_artists(&s.release_artist) && !s.release_artist.is_empty()))
                .unwrap_or(false);

            if !has_exact_override {
                emit_va_override_from_competing(
                    release_id,
                    &release_title,
                    &release_artist,
                    &proposal.inode_set,
                    &proposals,
                    &manifest_map,
                    &mut signals_batch,
                );
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

    Result::success(computation, Vec::new())
}

/// Check if an artist name is "Various Artists" (case-insensitive).
fn is_various_artists(artist: &str) -> bool {
    artist.eq_ignore_ascii_case("various artists")
}

/// Emit a VA override signal if the winner is "Various Artists" and a non-VA artist
/// can be found among its exact alternative siblings.
fn emit_va_override_from_siblings(
    release_id: &str,
    release_title: &str,
    release_artist: &str,
    siblings: &[AlternativeRelease],
    signals_batch: &mut Vec<TypedSignalWrite>,
) {
    if !is_various_artists(release_artist) || siblings.is_empty() {
        return;
    }

    // Find most frequent non-VA artist among siblings
    let mut artist_counts: HashMap<&str, usize> = HashMap::new();
    for sib in siblings {
        if !is_various_artists(&sib.release_artist) && !sib.release_artist.is_empty() {
            *artist_counts.entry(&sib.release_artist).or_insert(0) += 1;
        }
    }

    if let Some((&best_artist, _)) = artist_counts.iter().max_by_key(|(_, &count)| count) {
        signals_batch.push(TypedSignalWrite::VariousArtistsOverride(
            VariousArtistsOverrideSignal {
                key: release_id.to_string(),
                data: VariousArtistsOverrideData {
                    release_id: release_id.to_string(),
                    release_title: release_title.to_string(),
                    suggested_artist: best_artist.to_string(),
                    source: VariousArtistsOverrideSource::ExactAlternative,
                },
            },
        ));
    }
}

/// Emit a VA override signal from competing proposals in a component.
///
/// Used as fallback when exact alternatives don't have a non-VA artist.
/// Scans all proposals in the component for non-VA artists on releases
/// that overlap the winner's inodes.
fn emit_va_override_from_competing(
    release_id: &str,
    release_title: &str,
    release_artist: &str,
    winner_inodes: &HashSet<i64>,
    all_proposals: &[Arc<Proposal>],
    manifest_map: &HashMap<&str, (&str, &str, i32)>,
    signals_batch: &mut Vec<TypedSignalWrite>,
) {
    if !is_various_artists(release_artist) {
        return;
    }

    let mut artist_counts: HashMap<&str, usize> = HashMap::new();
    for p in all_proposals {
        let p_release_id = &p.rows[0].release_id;
        if p_release_id == release_id {
            continue;
        }
        // Must overlap winner's inodes
        if !p.inode_set.iter().any(|i| winner_inodes.contains(i)) {
            continue;
        }
        if let Some(&(_, artist, _)) = manifest_map.get(p_release_id.as_str()) {
            if !is_various_artists(artist) && !artist.is_empty() {
                *artist_counts.entry(artist).or_insert(0) += 1;
            }
        }
    }

    if let Some((&best_artist, _)) = artist_counts.iter().max_by_key(|(_, &count)| count) {
        signals_batch.push(TypedSignalWrite::VariousArtistsOverride(
            VariousArtistsOverrideSignal {
                key: release_id.to_string(),
                data: VariousArtistsOverrideData {
                    release_id: release_id.to_string(),
                    release_title: release_title.to_string(),
                    suggested_artist: best_artist.to_string(),
                    source: VariousArtistsOverrideSource::CompetingProposal,
                },
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::t;

    fn proposal(inodes: &[i64]) -> Arc<Proposal> {
        Arc::new(Proposal {
            total_tracks: inodes.len() as i32,
            rows: Vec::new(),
            inode_set: inodes.iter().copied().collect(),
            total_score: 1.0,
            tier: ProposalTier::FullMatch,
        })
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

    // --- dedup_by_signature ---

    fn proposal_with_release(inodes: &[i64], release_id: &str, score: f64) -> Arc<Proposal> {
        use crate::db::queries::external::OptimalPackingScoreRow;
        Arc::new(Proposal {
            total_tracks: inodes.len() as i32,
            rows: vec![OptimalPackingScoreRow {
                release_id: release_id.to_string(),
                inode: inodes[0],
                recording_id: "rec-1".to_string(),
                medium_pos: 1,
                track_pos: 1,
                track_title: "Track".to_string(),
                medium_format: None,
                track_number: "1".to_string(),
                score,
                score_breakdown: Vec::new(),
                match_method: 0,
                fingerprint_hex: None,
                raw_duration_ms: None,
            }],
            inode_set: inodes.iter().copied().collect(),
            total_score: score,
            tier: ProposalTier::FullMatch,
        })
    }

    #[test]
    fn test_dedup_single_proposal() {
        let proposals = vec![proposal_with_release(&[1, 2], "rel-A", 5.0)];
        let manifest: HashMap<&str, (&str, &str)> = HashMap::new();

        let (deduped, siblings, removed) = dedup_by_signature(proposals, &manifest);
        assert_eq!(deduped.len(), 1);
        assert_eq!(removed, 0);
        assert!(siblings.is_empty());
    }

    #[test]
    fn test_dedup_same_signature_keeps_best() {
        // Two proposals with same inodes {1, 2} — keep higher score
        let proposals = vec![
            proposal_with_release(&[1, 2], "rel-A", 3.0),
            proposal_with_release(&[1, 2], "rel-B", 5.0),
        ];
        let mut manifest: HashMap<&str, (&str, &str)> = HashMap::new();
        manifest.insert("rel-A", ("Album A", "Artist A"));
        manifest.insert("rel-B", ("Album B", "Artist B"));

        let (deduped, siblings, removed) = dedup_by_signature(proposals, &manifest);
        assert_eq!(deduped.len(), 1);
        assert_eq!(removed, 1);
        assert_eq!(deduped[0].rows[0].release_id, "rel-B"); // higher score
        assert_eq!(t!(siblings.get(&0)).len(), 1);
        assert_eq!(siblings[&0][0].release_id, "rel-A");
    }

    #[test]
    fn test_dedup_different_signatures_kept() {
        // Different inode sets → both kept
        let proposals = vec![
            proposal_with_release(&[1, 2], "rel-A", 5.0),
            proposal_with_release(&[3, 4], "rel-B", 5.0),
        ];
        let manifest: HashMap<&str, (&str, &str)> = HashMap::new();

        let (deduped, _, removed) = dedup_by_signature(proposals, &manifest);
        assert_eq!(deduped.len(), 2);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_dedup_siblings_populated() {
        let proposals = vec![
            proposal_with_release(&[1, 2], "rel-A", 3.0),
            proposal_with_release(&[1, 2], "rel-B", 5.0),
            proposal_with_release(&[1, 2], "rel-C", 4.0),
        ];
        let mut manifest: HashMap<&str, (&str, &str)> = HashMap::new();
        manifest.insert("rel-A", ("Album A", "Artist A"));
        manifest.insert("rel-B", ("Album B", "Artist B"));
        manifest.insert("rel-C", ("Album C", "Artist C"));

        let (deduped, siblings, removed) = dedup_by_signature(proposals, &manifest);
        assert_eq!(deduped.len(), 1);
        assert_eq!(removed, 2);
        // The winner should be rel-B (score 5.0)
        assert_eq!(deduped[0].rows[0].release_id, "rel-B");
        // Two siblings: rel-A and rel-C
        let sibs = &siblings[&0];
        assert_eq!(sibs.len(), 2);
        let sib_ids: HashSet<&str> = sibs.iter().map(|s| s.release_id.as_str()).collect();
        assert!(sib_ids.contains("rel-A"));
        assert!(sib_ids.contains("rel-C"));
    }

    // --- build_knot_proposals_data ---

    #[test]
    fn test_knot_proposals_data_selected_flag() {
        let p1 = proposal_with_release(&[1, 2], "rel-A", 5.0);
        let p2 = proposal_with_release(&[2, 3], "rel-B", 3.0);
        let proposals: Vec<&Proposal> = vec![&*p1, &*p2];

        let mut selected = HashSet::new();
        selected.insert(0usize); // Only first selected

        let mut manifest: HashMap<&str, (&str, &str, i32)> = HashMap::new();
        manifest.insert("rel-A", ("Album A", "Artist A", 2));
        manifest.insert("rel-B", ("Album B", "Artist B", 2));

        let data = build_knot_proposals_data(&proposals, &selected, &manifest);

        assert_eq!(data.len(), 2);
        assert!(data[0].selected);
        assert!(!data[1].selected);
        assert_eq!(data[0].release_id, "rel-A");
        assert_eq!(data[1].release_id, "rel-B");
    }

    #[test]
    fn test_knot_proposals_data_missing_manifest() {
        let p = proposal_with_release(&[1], "rel-X", 1.0);
        let proposals: Vec<&Proposal> = vec![&*p];
        let selected: HashSet<usize> = HashSet::new();
        let manifest: HashMap<&str, (&str, &str, i32)> = HashMap::new();

        let data = build_knot_proposals_data(&proposals, &selected, &manifest);
        assert_eq!(data[0].release_title, "");
        assert_eq!(data[0].release_artist, "");
    }
}
