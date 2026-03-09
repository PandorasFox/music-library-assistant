//! Release bin-packing staged pipeline.
//!
//! See `docs/RELEASE_PACKING_ALGORITHM.md` for the full algorithm reference.
//! Keep that document in sync with any changes to scoring, staging, or classification logic.
//!
//! Multi-stage pipeline for assigning corpus files to MusicBrainz releases:
//!
//! ```text
//! Stage 1: PackReleases (orchestrator)
//!     │  Loads external matches, identifies releases, writes manifest
//!     │  Spawns N ScoreReleaseCandidates
//!     │  Defers ComputeReleaseMappings
//!     ▼
//! Stage 2: ScoreReleaseCandidates { release_id } × N  (parallel)
//!     │  AcoustID Hungarian matching + per-release elimination
//!     │  Each release builds its maximally-packed proposal independently
//!     │  Writes results to intermediate table
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3a: ComputeReleaseMappings (orchestrator)
//!     │  Classifies proposals into quality tiers, defers MIS rounds
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3b: MapPerfectReleases — MIS on 1:1 dir↔release proposals
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3c: MapFullMatchReleases — MIS on cross-dir/extra-file proposals
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stages 3d/3e: MapIncompleteReleases + MapSingleReleases
//!     │  Order configurable via `singles-before-incompletes`:
//!     │    false (default): Incomplete → Singles
//!     │    true:            Singles → Incomplete
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 4: EmitUnmatchedSignals
//!        Unmatched corpus tracks, unfilled release slots
//! ```
//!
//! Each tier emits PackedRelease, ReleasePacking, and PackingKnot signals
//! per-component immediately after MIS solving. Inter-tier inode tracking
//! reads assigned inodes from the DB (signal_release_packing table).
//! State flows between MIS rounds via `SharedMappingState` (Arc<Mutex<Option<Box>>>).
//!
//! Manual trigger only — not part of ScheduleContentAnalysis.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;

use crate::config::PackingWeights;
use crate::db::queries::external::{ExternalMatchRow, OptimalPackingScoreRow};
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
    KnotAssignment, KnotClassification, KnotProposalEntry, MatchMethod, PackedReleaseCategory,
    PackedReleaseData, PackedReleaseSignal,
    PackingKnotData, PackingKnotSignal, PackingScoreBreakdown, ReleasePackingData,
    ReleasePackingSignal, TypedSignalWrite, UnfilledReleaseSlotData, UnfilledReleaseSlotSignal,
    UnmatchedCorpusTrackData, UnmatchedCorpusTrackSignal,
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
// Proposal types (Stage 3 conflict resolution)
// ============================================================================

/// Quality tier for a release proposal. Determines which MIS round it enters.
/// Each tier is a separate pool — proposals enter exactly one pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProposalTier {
    /// Every slot filled, 1:1 dir↔release (per-medium for multi-medium), no leftover files.
    Perfect,
    /// Every slot filled, but cross-directory or directory has extra files.
    FullMatch,
    /// Some slots filled but not all (includes near-misses).
    Incomplete,
    /// Single-track release.
    Single,
}

impl ProposalTier {
    fn as_str(&self) -> &'static str {
        match self {
            ProposalTier::Perfect => "perfect",
            ProposalTier::FullMatch => "full_match",
            ProposalTier::Incomplete => "incomplete",
            ProposalTier::Single => "single",
        }
    }
}

/// A release-level proposal: a complete assignment of inodes to track slots.
/// Proposals are the unit of selection in MIS rounds — they stay intact.
pub(crate) struct Proposal {
    pub total_tracks: i32,
    pub rows: Vec<OptimalPackingScoreRow>,
    pub inode_set: HashSet<i64>,
    pub total_score: f64,
    pub tier: ProposalTier,
}

/// Arc<Mutex<Option<Box<...>>>> wrapper that derives Clone + Debug + Serialize + Deserialize.
///
/// Clone is cheap (Arc refcount). Serialize/Deserialize skip the inner state
/// (these variants are transient pipeline state, never persisted).
#[derive(Clone)]
pub(crate) struct SharedMappingState(
    std::sync::Arc<std::sync::Mutex<Option<Box<ReleaseMappingState>>>>,
);

impl SharedMappingState {
    pub fn new(state: ReleaseMappingState) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(
            state,
        )))))
    }

    /// Take the state out of the box. Panics if called twice (state already consumed).
    pub fn take(&self) -> ReleaseMappingState {
        *self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("ReleaseMappingState consumed twice")
    }
}

impl std::fmt::Debug for SharedMappingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedMappingState(..)")
    }
}

impl serde::Serialize for SharedMappingState {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl<'de> serde::Deserialize<'de> for SharedMappingState {
    fn deserialize<D: serde::Deserializer<'de>>(
        _deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "SharedMappingState cannot be deserialized",
        ))
    }
}

/// Data for a single connected component to be solved independently.
pub(crate) struct ComponentData {
    pub proposals: Vec<Proposal>,
    pub tier: ProposalTier,
    pub corpus_paths: HashMap<i64, String>,
    pub manifest_map: HashMap<String, (String, String, i32)>,
}

/// Arc<Mutex<Option<Box<...>>>> wrapper for component data.
/// Same transient-pipeline-state pattern as SharedMappingState.
#[derive(Clone)]
pub(crate) struct SharedComponentData(
    std::sync::Arc<std::sync::Mutex<Option<Box<ComponentData>>>>,
);

impl SharedComponentData {
    pub fn new(data: ComponentData) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(
            data,
        )))))
    }

    pub fn take(&self) -> ComponentData {
        *self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("ComponentData consumed twice")
    }
}

impl std::fmt::Debug for SharedComponentData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedComponentData(..)")
    }
}

impl serde::Serialize for SharedComponentData {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl<'de> serde::Deserialize<'de> for SharedComponentData {
    fn deserialize<D: serde::Deserializer<'de>>(
        _deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "SharedComponentData cannot be deserialized",
        ))
    }
}

/// Shared state passed between MIS round computations via boxed move semantics.
///
/// Each round takes ownership via `SharedMappingState::take()`, runs its MIS,
/// and packages the remaining state for the next round. Signal emission happens
/// per-component within each tier — no accumulation across rounds.
pub(crate) struct ReleaseMappingState {
    /// Proposal pools — each consumed by its corresponding tier orchestrator.
    pub perfect_pool: Vec<Proposal>,
    pub full_match_pool: Vec<Proposal>,
    pub incomplete_pool: Vec<Proposal>,
    pub single_pool: Vec<Proposal>,
    /// Knot extraction threshold: connected components where
    /// proposals/inodes >= this ratio are too tangled for MIS (many releases
    /// competing over few files). Extracted and resolved by best-scorer.
    pub knot_ratio: f64,
    /// Maximum component size before knot extraction kicks in regardless of ratio.
    pub knot_size_limit: usize,
    /// When true, run Singles before Incompletes in MIS ordering.
    /// Singles claim one inode each (no MIS needed), preventing single-file
    /// incompletes from competing in the expensive Incomplete MIS round.
    pub singles_before_incompletes: bool,
}

/// Default knot extraction ratio. Components with proposals/inodes >= this
/// are extracted from MIS and resolved by best score.
const DEFAULT_KNOT_RATIO: f64 = 3.0;

/// Default knot size limit. Components larger than this are extracted
/// regardless of ratio — too large for BnB to solve in reasonable time.
const DEFAULT_KNOT_SIZE_LIMIT: usize = 50;

/// Classify a proposal into a quality tier based on slot coverage and directory purity.
///
/// `media_count` is the number of MbMedium entries for this release (from the tracklist).
/// `inode_dir_map` maps each inode to its (parent_dir, dir_file_count).
fn classify_proposal(
    rows: &[OptimalPackingScoreRow],
    total_tracks: i32,
    media_count: usize,
    inode_dir_map: &HashMap<i64, (String, i32)>,
) -> ProposalTier {
    if total_tracks <= 1 {
        return ProposalTier::Single;
    }
    if (rows.len() as i32) < total_tracks {
        return ProposalTier::Incomplete;
    }

    // All slots filled — check directory purity for Perfect vs FullMatch.
    //
    // Perfect requires:
    //   Single-medium: exactly 1 directory, dir_file_count == total_tracks
    //   Multi-medium: each medium's inodes from exactly 1 directory, each directory
    //     maps to exactly 1 medium, dir_file_count == medium track count,
    //     all directories are siblings (same parent)

    // Build dir → inodes and dir → media mapping
    let mut dir_inodes: HashMap<&str, Vec<i64>> = HashMap::new();
    let mut dir_media: HashMap<&str, HashSet<i32>> = HashMap::new();
    let mut medium_dirs: HashMap<i32, HashSet<&str>> = HashMap::new();

    for row in rows {
        if let Some((dir, _)) = inode_dir_map.get(&row.inode) {
            dir_inodes.entry(dir.as_str()).or_default().push(row.inode);
            dir_media
                .entry(dir.as_str())
                .or_default()
                .insert(row.medium_pos);
            medium_dirs
                .entry(row.medium_pos)
                .or_default()
                .insert(dir.as_str());
        }
    }

    if dir_inodes.is_empty() {
        return ProposalTier::FullMatch;
    }

    if media_count <= 1 {
        // Single-medium: Perfect iff exactly 1 directory, file count matches total tracks
        if dir_inodes.len() == 1 {
            let dir = *dir_inodes.keys().next().unwrap();
            let dir_file_count = inode_dir_map
                .values()
                .find(|(d, _)| d.as_str() == dir)
                .map(|(_, c)| *c)
                .unwrap_or(0);
            if dir_file_count == total_tracks {
                return ProposalTier::Perfect;
            }
        }
        return ProposalTier::FullMatch;
    }

    // Multi-medium: each medium must map to exactly 1 directory and vice versa
    for medium_dir_set in medium_dirs.values() {
        if medium_dir_set.len() != 1 {
            return ProposalTier::FullMatch;
        }
    }
    for dir_medium_set in dir_media.values() {
        if dir_medium_set.len() != 1 {
            return ProposalTier::FullMatch;
        }
    }

    // Check that each directory's file count matches its medium's track count
    // and all directories are siblings (same parent)
    let mut parents: HashSet<&str> = HashSet::new();
    for (dir, inodes) in &dir_inodes {
        let dir_file_count = inode_dir_map
            .values()
            .find(|(d, _)| d.as_str() == *dir)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        if dir_file_count != inodes.len() as i32 {
            return ProposalTier::FullMatch;
        }
        if let Some(parent) = Path::new(dir).parent() {
            parents.insert(parent.to_str().unwrap_or(""));
        }
    }
    if parents.len() > 1 {
        return ProposalTier::FullMatch;
    }

    ProposalTier::Perfect
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

/// Entry for the MIS solver: an inode set and associated score.
struct MisCandidate {
    inode_set: HashSet<i64>,
    score: f64,
}

/// Result of solving a Maximum Independent Set problem.
struct MisResult {
    /// Which candidates were selected (parallel to input slice).
    selected: Vec<bool>,
    /// Total number of selected candidates.
    selected_count: usize,
    /// Total coverage: sum of inode_set.len() for selected candidates.
    selected_coverage: usize,
}

/// Solve Maximum Independent Set: select candidates whose inode sets are
/// pairwise disjoint, maximizing total coverage (sum of inode set sizes).
/// Tiebreak on total score.
///
/// Uses exhaustive bitmask enumeration for components ≤ 25, branch-and-bound
/// for larger components.
fn solve_maximum_independent_set(candidates: &[MisCandidate]) -> MisResult {
    let n = candidates.len();
    if n == 0 {
        return MisResult {
            selected: Vec::new(),
            selected_count: 0,
            selected_coverage: 0,
        };
    }

    // Build conflict adjacency: edge between candidates that share any inode
    // Uses sorted Vec<usize> instead of HashSet for deterministic iteration order.
    let mut inode_to_idx: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, cand) in candidates.iter().enumerate() {
        for &inode in &cand.inode_set {
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
    drop(inode_to_idx);

    // Find connected components via BFS
    let mut component_id: Vec<Option<usize>> = vec![None; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if component_id[start].is_some() {
            continue;
        }
        let cid = components.len();
        let mut comp = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        component_id[start] = Some(cid);
        while let Some(node) = queue.pop_front() {
            comp.push(node);
            for &neighbor in &adj[node] {
                if component_id[neighbor].is_none() {
                    component_id[neighbor] = Some(cid);
                    queue.push_back(neighbor);
                }
            }
        }
        comp.sort_unstable();
        components.push(comp);
    }

    let mut selected = vec![false; n];
    let mut selected_count = 0usize;
    let mut selected_coverage = 0usize;

    // Log component distribution for visibility
    {
        let isolated = components.iter().filter(|c| c.len() == 1).count();
        let small = components
            .iter()
            .filter(|c| (2..=5).contains(&c.len()))
            .count();
        let bitmask = components
            .iter()
            .filter(|c| (6..=25).contains(&c.len()))
            .count();
        let bnb = components.iter().filter(|c| c.len() > 25).count();
        let bnb_sizes: Vec<usize> = components
            .iter()
            .filter(|c| c.len() > 25)
            .map(|c| c.len())
            .collect();
        log_general(format!(
            "[COMPUTE] MIS: {} components (isolated={}, small={}, bitmask={}, bnb={}{})",
            components.len(),
            isolated,
            small,
            bitmask,
            bnb,
            if bnb_sizes.is_empty() {
                String::new()
            } else {
                format!(" sizes={:?}", bnb_sizes)
            },
        ));
    }

    for (ci, component) in components.iter().enumerate() {
        if component.len() == 1 {
            selected[component[0]] = true;
            selected_count += 1;
            selected_coverage += candidates[component[0]].inode_set.len();
            continue;
        }

        if component.len() <= 25 {
            // Exhaustive bitmask enumeration
            let k = component.len();
            let mut best_mask: u32 = 0;
            let mut best_coverage: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;

            for mask in 1u32..(1u32 << k) {
                let mut claimed: HashSet<i64> = HashSet::new();
                let mut feasible = true;
                let mut coverage = 0usize;
                let mut score = 0.0f64;

                for (bit, &gi) in component.iter().enumerate() {
                    if mask & (1 << bit) == 0 {
                        continue;
                    }
                    if candidates[gi]
                        .inode_set
                        .iter()
                        .any(|inode| claimed.contains(inode))
                    {
                        feasible = false;
                        break;
                    }
                    claimed.extend(&candidates[gi].inode_set);
                    coverage += candidates[gi].inode_set.len();
                    score += candidates[gi].score;
                }

                if feasible
                    && (coverage > best_coverage
                        || (coverage == best_coverage && score > best_score)
                        || (coverage == best_coverage
                            && score == best_score
                            && mask < best_mask))
                {
                    best_coverage = coverage;
                    best_score = score;
                    best_mask = mask;
                }
            }

            for (bit, &gi) in component.iter().enumerate() {
                if best_mask & (1 << bit) != 0 {
                    selected[gi] = true;
                    selected_count += 1;
                    selected_coverage += candidates[gi].inode_set.len();
                }
            }
        } else {
            // Branch-and-bound for larger components
            let comp_size = component.len();
            let comp_edges: usize = component
                .iter()
                .map(|&gi| adj[gi].iter().filter(|&&n| component.contains(&n)).count())
                .sum::<usize>()
                / 2;
            log_general(format!(
                "[COMPUTE] MIS: solving BnB component {}/{} (nodes={}, edges={})",
                ci + 1,
                components.len(),
                comp_size,
                comp_edges,
            ));

            let mut global_to_local: HashMap<usize, usize> = HashMap::new();
            for (li, &gi) in component.iter().enumerate() {
                global_to_local.insert(gi, li);
            }
            let mut local_adj: Vec<Vec<usize>> = vec![Vec::new(); comp_size];
            for (li, &gi) in component.iter().enumerate() {
                for &neighbor in &adj[gi] {
                    if let Some(&ln) = global_to_local.get(&neighbor) {
                        local_adj[li].push(ln);
                    }
                }
                local_adj[li].sort_unstable();
                local_adj[li].dedup();
            }

            let local_scores: Vec<f64> = component.iter().map(|&gi| candidates[gi].score).collect();
            let local_coverages: Vec<usize> = component
                .iter()
                .map(|&gi| candidates[gi].inode_set.len())
                .collect();
            let local_inode_sets: Vec<&HashSet<i64>> = component
                .iter()
                .map(|&gi| &candidates[gi].inode_set)
                .collect();

            let mut best_coverage: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;
            let mut best_selected: Vec<bool> = vec![false; comp_size];

            struct BnBState {
                candidates: Vec<usize>,
                selected: Vec<bool>,
                selected_coverage: usize,
                selected_score: f64,
                claimed_inodes: HashSet<i64>,
            }

            let initial_candidates: Vec<usize> = (0..comp_size).collect();
            let mut stack: Vec<BnBState> = vec![BnBState {
                candidates: initial_candidates,
                selected: vec![false; comp_size],
                selected_coverage: 0,
                selected_score: 0.0,
                claimed_inodes: HashSet::new(),
            }];
            let bnb_start = Instant::now();
            let mut iterations = 0u64;

            while let Some(state) = stack.pop() {
                iterations += 1;
                // Upper bound: current coverage + all remaining candidates' coverage
                let remaining_max_coverage: usize =
                    state.candidates.iter().map(|&c| local_coverages[c]).sum();
                if state.selected_coverage + remaining_max_coverage < best_coverage {
                    continue;
                }
                if state.selected_coverage + remaining_max_coverage == best_coverage {
                    let remaining_max_score: f64 =
                        state.candidates.iter().map(|&c| local_scores[c]).sum();
                    if state.selected_score + remaining_max_score <= best_score {
                        continue;
                    }
                }

                if state.candidates.is_empty() {
                    let dominated = state.selected_coverage > best_coverage
                        || (state.selected_coverage == best_coverage
                            && state.selected_score > best_score)
                        || (state.selected_coverage == best_coverage
                            && state.selected_score == best_score
                            && state.selected < best_selected);
                    if dominated {
                        best_coverage = state.selected_coverage;
                        best_score = state.selected_score;
                        best_selected = state.selected.clone();
                    }
                    continue;
                }

                let pivot = *state
                    .candidates
                    .iter()
                    .max_by_key(|&&c| {
                        let degree = state
                            .candidates
                            .iter()
                            .filter(|&&other| local_adj[c].contains(&other))
                            .count();
                        (degree, std::cmp::Reverse(c))
                    })
                    .unwrap();

                // Branch B: EXCLUDE pivot (push first so INCLUDE is explored first)
                {
                    let new_candidates: Vec<usize> = state
                        .candidates
                        .iter()
                        .copied()
                        .filter(|&c| c != pivot)
                        .collect();
                    stack.push(BnBState {
                        candidates: new_candidates,
                        selected: state.selected.clone(),
                        selected_coverage: state.selected_coverage,
                        selected_score: state.selected_score,
                        claimed_inodes: state.claimed_inodes.clone(),
                    });
                }

                // Branch A: INCLUDE pivot
                {
                    let neighbors: HashSet<usize> = local_adj[pivot].iter().copied().collect();
                    let pivot_inodes = local_inode_sets[pivot];
                    if !pivot_inodes
                        .iter()
                        .any(|i| state.claimed_inodes.contains(i))
                    {
                        let new_candidates: Vec<usize> = state
                            .candidates
                            .iter()
                            .copied()
                            .filter(|&c| c != pivot && !neighbors.contains(&c))
                            .collect();
                        let mut new_selected = state.selected.clone();
                        new_selected[pivot] = true;
                        let mut new_claimed = state.claimed_inodes.clone();
                        new_claimed.extend(pivot_inodes);
                        stack.push(BnBState {
                            candidates: new_candidates,
                            selected: new_selected,
                            selected_coverage: state.selected_coverage + local_coverages[pivot],
                            selected_score: state.selected_score + local_scores[pivot],
                            claimed_inodes: new_claimed,
                        });
                    }
                }
            }

            let bnb_elapsed = bnb_start.elapsed();
            let bnb_selected: usize = best_selected.iter().filter(|&&s| s).count();
            log_general(format!(
                "[COMPUTE] MIS: BnB component done: {} selected, coverage={}, {:.1}s, {} iterations",
                bnb_selected, best_coverage, bnb_elapsed.as_secs_f64(), iterations,
            ));

            for (li, &gi) in component.iter().enumerate() {
                if best_selected[li] {
                    selected[gi] = true;
                    selected_count += 1;
                    selected_coverage += candidates[gi].inode_set.len();
                }
            }
        }
    }

    MisResult {
        selected,
        selected_count,
        selected_coverage,
    }
}

/// Compute the scoring components for a candidate assignment.
fn compute_score(
    rec_match: &RecordingMatch,
    track: &musicbrainz::MbTrack,
    release_artist: &str,
    release_title: &str,
    corpus: Option<&CorpusFileInfo>,
    duration_tolerance_pct: f64,
    weights: &PackingWeights,
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
            let rec_sim = strsim::normalized_levenshtein(corpus_title, &track.recording.title);
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
        title_match: title_sim,
        artist_match: artist_sim,
        album_match: album_sim,
        track_number_match,
    };

    let score = weighted_composite(&breakdown, weights);
    (score, breakdown)
}

/// Compute weighted composite score from breakdown components.
fn weighted_composite(b: &PackingScoreBreakdown, w: &PackingWeights) -> f64 {
    w.acoustid_confidence * b.acoustid_confidence
        + w.duration_match * b.duration_match
        + w.title_match * b.title_match
        + w.artist_match * b.artist_match
        + w.album_match * b.album_match
        + w.track_number_match * b.track_number_match
}

/// Kuhn-Munkres (Hungarian) algorithm on an n×n cost matrix.
///
/// Returns `col_to_row` assignment where `col_to_row[j]` is the 1-indexed row
/// assigned to column j (0 = unassigned). The matrix must be square.
#[allow(clippy::needless_range_loop)]
fn kuhn_munkres(cost: &[Vec<f64>], n: usize) -> Vec<usize> {
    let inf = f64::MAX / 2.0;
    let mut u = vec![0.0f64; n + 1];
    let mut v = vec![0.0f64; n + 1];
    let mut col_to_row = vec![0usize; n + 1];

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

        loop {
            let prev = links[j0];
            col_to_row[j0] = col_to_row[prev];
            j0 = prev;
            if j0 == 0 {
                break;
            }
        }
    }

    col_to_row
}

/// Optimal bipartite assignment via Hungarian algorithm (per-release).
///
/// Returns set of (inode, (medium_pos, track_pos)) pairs that maximize total score.
/// Matrix sizes are small (99.6% of releases have <100 score entries), so O(n³)
/// is negligible.
#[allow(clippy::needless_range_loop)] // result extraction uses 1-based index arithmetic from kuhn_munkres
fn hungarian_assignment(candidates: &[CandidateAssignment]) -> HashSet<(i64, (u32, u32))> {
    let mut inode_set: Vec<i64> = candidates.iter().map(|c| c.inode).collect();
    inode_set.sort();
    inode_set.dedup();
    let inode_idx: HashMap<i64, usize> =
        inode_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let mut slot_set: Vec<(u32, u32)> = candidates
        .iter()
        .map(|c| (c.medium_pos, c.track_pos))
        .collect();
    slot_set.sort();
    slot_set.dedup();
    let slot_idx: HashMap<(u32, u32), usize> =
        slot_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let n_rows = inode_set.len();
    let n_cols = slot_set.len();

    if n_rows == 0 || n_cols == 0 {
        return HashSet::new();
    }

    let n = n_rows.max(n_cols);
    let mut cost = vec![vec![0.0f64; n]; n];
    for candidate in candidates {
        let r = inode_idx[&candidate.inode];
        let c = slot_idx[&(candidate.medium_pos, candidate.track_pos)];
        if candidate.score > -cost[r][c] {
            cost[r][c] = -candidate.score;
        }
    }

    let col_to_row = kuhn_munkres(&cost, n);

    let mut result = HashSet::new();
    for j in 1..=n {
        let row = col_to_row[j];
        if row == 0 {
            continue;
        }
        let row_idx = row - 1;
        let col_idx = j - 1;
        if row_idx < n_rows && col_idx < n_cols && cost[row_idx][col_idx] < 0.0 {
            result.insert((inode_set[row_idx], slot_set[col_idx]));
        }
    }

    result
}

/// Select the best target directory (or sibling directories for multi-medium) for packing.
///
/// All packing is constrained to one directory (or sibling dirs). This replaces the
/// old `directory_cohesion` scoring gradient with a structural constraint.
///
/// **Single-medium** (`media.len() <= 1`):
/// - Prefer: directory where `dir_file_count == total_tracks` with most AcoustID candidates
/// - Fallback: directory with the most AcoustID candidates
/// - Tiebreaker: highest sum of candidate scores
/// - Returns `TargetDirs::Single(dir)`.
///
/// **Multi-medium** (`media.len() > 1`):
/// - Find sibling directories (same parent) with candidates, assign each medium to
///   the sibling dir where most of its AcoustID candidates live.
/// - Returns `TargetDirs::PerMedium(mapping)` with per-medium directory affinity.
/// - Fallback: treat like single-medium (pick one dir with most candidates).
enum TargetDirs {
    /// One directory for all media (single-medium or multi-medium fallback).
    Single(String),
    /// Per-medium directory assignments from sibling group detection.
    PerMedium(HashMap<u32, String>),
    /// No valid target directory found.
    Empty,
}

impl TargetDirs {
    /// Check if a candidate (inode in a given dir, targeting a given medium) belongs.
    fn contains(&self, parent_dir: &str, medium_pos: u32) -> bool {
        match self {
            TargetDirs::Single(d) => d == parent_dir,
            TargetDirs::PerMedium(m) => m
                .get(&medium_pos)
                .map(|d| d == parent_dir)
                .unwrap_or(false),
            TargetDirs::Empty => false,
        }
    }

    /// Get the target directory for a specific medium (for elimination scanning).
    fn dir_for_medium(&self, medium_pos: u32) -> Option<&str> {
        match self {
            TargetDirs::Single(d) => Some(d.as_str()),
            TargetDirs::PerMedium(m) => m.get(&medium_pos).map(|d| d.as_str()),
            TargetDirs::Empty => None,
        }
    }
}

fn select_target_directory(
    candidates: &[CandidateAssignment],
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    dir_candidate_inodes: &HashMap<String, HashSet<i64>>,
    dir_total: &HashMap<String, i32>,
    media: &[musicbrainz::MbMedium],
) -> TargetDirs {
    let total_tracks: usize = media.iter().map(|m| m.tracks.len()).sum();
    let media_count = media.len();

    if total_tracks == 0 || dir_candidate_inodes.is_empty() {
        return TargetDirs::Empty;
    }

    // Multi-medium: try sibling directory group detection first
    if media_count > 1 {
        if let Some(mapping) =
            find_sibling_dir_mapping(candidates, corpus_info, dir_candidate_inodes)
        {
            return TargetDirs::PerMedium(mapping);
        }
        // Fallback: treat like single-medium below
    }

    // Single-medium (or multi-medium fallback): pick best single directory
    // Score each directory: (exact_file_count_match, candidate_count, score_sum)
    let mut dir_scores: Vec<(&str, bool, usize, f64)> = dir_candidate_inodes
        .iter()
        .map(|(dir, inodes)| {
            let file_count = dir_total.get(dir).copied().unwrap_or(0);
            let exact_match = file_count == total_tracks as i32;
            let candidate_count = inodes.len();
            let score_sum: f64 = candidates
                .iter()
                .filter(|c| {
                    corpus_info
                        .get(&c.inode)
                        .map(|ci| ci.parent_dir == *dir)
                        .unwrap_or(false)
                })
                .map(|c| c.score)
                .sum();
            (dir.as_str(), exact_match, candidate_count, score_sum)
        })
        .collect();

    // Sort: exact match first, then most candidates, then highest score sum
    dir_scores.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.2.cmp(&a.2))
            .then(
                b.3.partial_cmp(&a.3)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    if let Some((dir, _, _, _)) = dir_scores.first() {
        TargetDirs::Single(dir.to_string())
    } else {
        TargetDirs::Empty
    }
}

/// Find sibling directories (same parent) with AcoustID candidates for a multi-medium
/// release, then assign each medium to the sibling dir where most of its candidates live.
///
/// Returns `medium_pos → dir` mapping if a valid sibling group is found.
fn find_sibling_dir_mapping(
    candidates: &[CandidateAssignment],
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    dir_candidate_inodes: &HashMap<String, HashSet<i64>>,
) -> Option<HashMap<u32, String>> {
    // Group directories by parent
    let mut parent_groups: HashMap<String, Vec<String>> = HashMap::new();
    for dir in dir_candidate_inodes.keys() {
        if let Some(parent) = Path::new(dir).parent().and_then(|p| p.to_str()) {
            parent_groups
                .entry(parent.to_string())
                .or_default()
                .push(dir.clone());
        }
    }

    // Pick the parent group with the most total candidates, requiring ≥2 sibling dirs
    let best_group = parent_groups
        .into_iter()
        .filter(|(_, dirs)| dirs.len() >= 2)
        .max_by_key(|(_, dirs)| {
            dirs.iter()
                .map(|d| dir_candidate_inodes.get(d).map(|s| s.len()).unwrap_or(0))
                .sum::<usize>()
        });

    let (_, sibling_dirs) = best_group?;
    let sibling_set: HashSet<&str> = sibling_dirs.iter().map(|s| s.as_str()).collect();

    // For each candidate in a sibling dir, tally which medium it belongs to
    // medium_pos → dir → candidate_count
    let mut medium_dir_counts: HashMap<u32, HashMap<&str, usize>> = HashMap::new();
    for c in candidates {
        if let Some(ci) = corpus_info.get(&c.inode) {
            if sibling_set.contains(ci.parent_dir.as_str()) {
                *medium_dir_counts
                    .entry(c.medium_pos)
                    .or_default()
                    .entry(&ci.parent_dir)
                    .or_insert(0) += 1;
            }
        }
    }

    // Assign each medium to its highest-count sibling dir
    let mut mapping: HashMap<u32, String> = HashMap::new();
    for (medium_pos, dir_counts) in &medium_dir_counts {
        if let Some((&best_dir, _)) = dir_counts.iter().max_by_key(|(_, &count)| count) {
            mapping.insert(*medium_pos, best_dir.to_string());
        }
    }

    // Must have at least 2 distinct dirs mapped to be worth using as sibling group
    let distinct_dirs: HashSet<&str> = mapping.values().map(|s| s.as_str()).collect();
    if distinct_dirs.len() < 2 {
        return None;
    }

    Some(mapping)
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
    // All packing is constrained to a single directory (or sibling dirs for multi-medium).
    // Step 1: Select target directory based on file count match and candidate density.
    // Step 2: Filter candidates to target directory only.
    // Step 3: Run Hungarian on the filtered candidates.

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
        dir_total
            .entry(row.parent_dir.clone())
            .or_insert(row.dir_file_count);
    }

    let target_dirs = select_target_directory(
        &candidates,
        &corpus_info,
        &dir_candidate_inodes,
        &dir_total,
        &release.media,
    );

    // Filter candidates to target directory only (per-medium aware)
    candidates.retain(|c| {
        corpus_info
            .get(&c.inode)
            .map(|ci| target_dirs.contains(&ci.parent_dir, c.medium_pos))
            .unwrap_or(false)
    });

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





// ============================================================================
// Component discovery (shared by all tier orchestrators)
// ============================================================================

/// Find connected components in a conflict graph over proposals.
///
/// Two proposals conflict if they share any inode. Returns a list of
/// components, each being a sorted Vec of indices into the input slice.
fn find_conflict_components(proposals: &[Proposal]) -> Vec<Vec<usize>> {
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
fn emit_isolated_proposal_signals(
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
fn emit_knot_component_signals(
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

// ============================================================================
// Per-component MIS solver
// ============================================================================

/// Execute ResolvePackingComponent — solve MIS for one connected component.
///
/// Self-contained: builds local conflict graph, solves, emits signals.
/// Spawned in parallel by tier orchestrators.
pub(crate) fn execute_resolve_packing_component(
    shared: &SharedComponentData,
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
    let manifest_map = component.manifest_map;

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

    let mis_result = solve_maximum_independent_set(&mis_candidates);

    // Build local alternatives_count: how many proposals in THIS component
    // contain each inode
    let mut local_inode_proposals: HashMap<i64, u16> = HashMap::new();
    for p in &proposals {
        for &inode in &p.inode_set {
            *local_inode_proposals.entry(inode).or_insert(0) += 1;
        }
    }

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
                Some((t, a, tt)) => (t.clone(), a.clone(), *tt),
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
    let mut proposals_map: HashMap<&str, Vec<OptimalPackingScoreRow>> = HashMap::new();
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

/// Orchestrate a "partial" tier (FullMatch, Incomplete, or Single).
///
/// Pipeline: cull tainted → dedup by inode signature → extract knots →
/// find components → emit isolated directly → spawn N component solvers.
///
/// Returns (spawned_computations, log_message).
fn orchestrate_partial_tier(
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

    // Load manifest + corpus paths once for all signal emission (isolated + knots + spawned)
    let owned_manifest: HashMap<String, (String, String, i32)> = read_only_db
        .get_packing_manifest()
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.release_id, (r.release_title, r.release_artist, r.total_tracks)))
        .collect();
    let borrowed_manifest: HashMap<&str, (&str, &str, i32)> = owned_manifest
        .iter()
        .map(|(k, (t, a, tt))| (k.as_str(), (t.as_str(), a.as_str(), *tt)))
        .collect();
    let corpus_paths: HashMap<i64, String> = read_only_db
        .get_packing_inode_paths()
        .unwrap_or_default()
        .into_iter()
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
                &borrowed_manifest,
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
                &borrowed_manifest,
                &corpus_paths,
                sender,
                witness,
            );

            knot_component_count += 1;
            knot_proposal_count += component.len();
            knot_id += 1;
        } else {
            // Clean component — spawn solver with in-band data
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
                    manifest_map: owned_manifest.clone(),
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

    // Load manifest + corpus paths once for all signal emission (isolated + spawned)
    let owned_manifest: HashMap<String, (String, String, i32)> = read_only_db
        .get_packing_manifest()
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.release_id, (r.release_title, r.release_artist, r.total_tracks)))
        .collect();
    let borrowed_manifest: HashMap<&str, (&str, &str, i32)> = owned_manifest
        .iter()
        .map(|(k, (t, a, tt))| (k.as_str(), (t.as_str(), a.as_str(), *tt)))
        .collect();
    let corpus_paths: HashMap<i64, String> = read_only_db
        .get_packing_inode_paths()
        .unwrap_or_default()
        .into_iter()
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
                &borrowed_manifest,
                &corpus_paths,
                &sender,
                witness,
            );
            isolated_count += 1;
        } else {
            // Multi-node component — spawn solver with in-band data
            let component_proposals: Vec<Proposal> =
                component.iter().map(|&i| {
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
                    manifest_map: owned_manifest.clone(),
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

// ============================================================================
// Stage 4: EmitUnmatchedSignals (gap analysis)
// ============================================================================

/// Execute EmitUnmatchedSignals — emit unmatched corpus track and unfilled slot signals.
pub fn execute_emit_unmatched_signals(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::EmitUnmatchedSignals;

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
                inode_recordings
                    .entry(inode)
                    .or_default()
                    .push(recording_id);
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

    // Also emit unmatched signals for fingerprinted corpus files that never entered
    // the candidate pipeline (no AcoustID match → no recordings → not in inode_recordings).
    match read_only_db.get_fingerprinted_corpus_inodes() {
        Ok(fingerprinted) => {
            for (inode, path) in fingerprinted {
                if assigned_inodes.contains(&inode) || inode_recordings.contains_key(&inode) {
                    continue;
                }
                let signal = UnmatchedCorpusTrackSignal {
                    inode,
                    path,
                    data: UnmatchedCorpusTrackData {
                        recording_ids: Vec::new(),
                        considered_release_ids: Vec::new(),
                    },
                };
                unmatched_signals.push(ComputedCorpusSignal::new(
                    inode,
                    TypedSignalWrite::UnmatchedCorpusTrack(signal),
                ));
            }
        }
        Err(e) => {
            log_general(format!(
                "[COMPUTE] EmitUnmatchedSignals: failed to query fingerprinted inodes: {}",
                e
            ));
        }
    }

    let (uc_cleared, uc_new, uc_updated, uc_unchanged) =
        reconcile_corpus_signals::<UnmatchedCorpusTrackSignal>(
            read_only_db,
            &sender,
            unmatched_signals,
            witness,
        );

    // === Unfilled release slots ===
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

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
