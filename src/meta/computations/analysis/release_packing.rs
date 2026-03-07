//! Release bin-packing staged pipeline.
//!
//! Four-stage pipeline for assigning corpus files to MusicBrainz releases:
//!
//! ```text
//! Stage 1: PackReleases (orchestrator)
//!     │  Loads external matches, identifies releases, writes manifest
//!     │  Spawns N ScoreReleaseCandidates
//!     │  Defers [Stage 3: Resolve, Stage 4: Analyze]
//!     ▼
//! Stage 2: ScoreReleaseCandidates { release_id } × N  (parallel)
//!     │  AcoustID Hungarian matching + per-release elimination
//!     │  Each release builds its maximally-packed proposal independently
//!     │  Writes results to intermediate table
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3: ResolveReleaseConflicts
//!     │  MIS + per-inode-best resolution on fully-packed proposals
//!     │  Emits ReleasePackingSignal per assigned inode
//!     │  Records pending AcoustID submissions for elimination winners
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 4: AnalyzeReleaseGaps
//!        Unmatched corpus tracks, unfilled slots, near-miss detection
//!        Emits gap analysis signals
//! ```
//!
//! Manual trigger only — not part of ScheduleContentAnalysis.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;

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
    MatchMethod, NearMissReleaseData, NearMissReleaseSignal, PackedReleaseCategory,
    PackedReleaseData, PackedReleaseSignal, PackingScoreBreakdown, ReleasePackingData,
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
    /// Number of connected components in the conflict graph.
    component_count: usize,
    /// Size of the largest connected component.
    max_component_size: usize,
}

/// Solve Maximum Independent Set: select the maximum number of candidates
/// whose inode sets are pairwise disjoint. Tiebreak on total score.
///
/// Uses exhaustive bitmask enumeration for components ≤ 25, branch-and-bound
/// for larger components.
fn solve_maximum_independent_set(candidates: &[MisCandidate]) -> MisResult {
    let n = candidates.len();
    if n == 0 {
        return MisResult {
            selected: Vec::new(),
            selected_count: 0,
            component_count: 0,
            max_component_size: 0,
        };
    }

    // Build conflict adjacency: edge between candidates that share any inode
    let mut inode_to_idx: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, cand) in candidates.iter().enumerate() {
        for &inode in &cand.inode_set {
            inode_to_idx.entry(inode).or_default().push(i);
        }
    }

    let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for indices in inode_to_idx.values() {
        if indices.len() > 1 {
            for &i in indices {
                for &j in indices {
                    if i != j {
                        adj[i].insert(j);
                    }
                }
            }
        }
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
        components.push(comp);
    }

    let mut selected = vec![false; n];
    let mut selected_count = 0usize;
    let mut max_component_size = 0usize;

    for component in &components {
        max_component_size = max_component_size.max(component.len());

        if component.len() == 1 {
            selected[component[0]] = true;
            selected_count += 1;
            continue;
        }

        if component.len() <= 25 {
            // Exhaustive bitmask enumeration
            let k = component.len();
            let mut best_mask: u32 = 0;
            let mut best_count: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;

            for mask in 1u32..(1u32 << k) {
                let mut claimed: HashSet<i64> = HashSet::new();
                let mut feasible = true;
                let mut count = 0usize;
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
                    count += 1;
                    score += candidates[gi].score;
                }

                if feasible
                    && (count > best_count
                        || (count == best_count && score > best_score))
                {
                    best_count = count;
                    best_score = score;
                    best_mask = mask;
                }
            }

            for (bit, &gi) in component.iter().enumerate() {
                if best_mask & (1 << bit) != 0 {
                    selected[gi] = true;
                    selected_count += 1;
                }
            }
        } else {
            // Branch-and-bound for larger components
            let comp_size = component.len();

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
            }

            let local_scores: Vec<f64> =
                component.iter().map(|&gi| candidates[gi].score).collect();
            let local_inode_sets: Vec<&HashSet<i64>> =
                component.iter().map(|&gi| &candidates[gi].inode_set).collect();

            let mut best_count: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;
            let mut best_selected: Vec<bool> = vec![false; comp_size];

            struct BnBState {
                candidates: Vec<usize>,
                selected: Vec<bool>,
                selected_count: usize,
                selected_score: f64,
                claimed_inodes: HashSet<i64>,
            }

            let initial_candidates: Vec<usize> = (0..comp_size).collect();
            let mut stack: Vec<BnBState> = vec![BnBState {
                candidates: initial_candidates,
                selected: vec![false; comp_size],
                selected_count: 0,
                selected_score: 0.0,
                claimed_inodes: HashSet::new(),
            }];

            while let Some(state) = stack.pop() {
                if state.selected_count + state.candidates.len() <= best_count {
                    continue;
                }
                if state.selected_count + state.candidates.len() == best_count {
                    let remaining_max_score: f64 =
                        state.candidates.iter().map(|&c| local_scores[c]).sum();
                    if state.selected_score + remaining_max_score <= best_score {
                        continue;
                    }
                }

                if state.candidates.is_empty() {
                    if state.selected_count > best_count
                        || (state.selected_count == best_count
                            && state.selected_score > best_score)
                    {
                        best_count = state.selected_count;
                        best_score = state.selected_score;
                        best_selected = state.selected.clone();
                    }
                    continue;
                }

                let pivot = *state
                    .candidates
                    .iter()
                    .max_by_key(|&&c| {
                        state
                            .candidates
                            .iter()
                            .filter(|&&other| local_adj[c].contains(&other))
                            .count()
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
                        selected_count: state.selected_count,
                        selected_score: state.selected_score,
                        claimed_inodes: state.claimed_inodes.clone(),
                    });
                }

                // Branch A: INCLUDE pivot
                {
                    let neighbors: HashSet<usize> =
                        local_adj[pivot].iter().copied().collect();
                    let pivot_inodes = local_inode_sets[pivot];
                    if !pivot_inodes.iter().any(|i| state.claimed_inodes.contains(i)) {
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
                            selected_count: state.selected_count + 1,
                            selected_score: state.selected_score + local_scores[pivot],
                            claimed_inodes: new_claimed,
                        });
                    }
                }
            }

            for (li, &gi) in component.iter().enumerate() {
                if best_selected[li] {
                    selected[gi] = true;
                    selected_count += 1;
                }
            }
        }
    }

    MisResult {
        selected,
        selected_count,
        component_count: components.len(),
        max_component_size,
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
            let rec_sim =
                strsim::normalized_levenshtein(corpus_title, &track.recording.title);
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

    let tag_similarity = 0.4 * title_sim + 0.35 * artist_sim + 0.25 * album_sim;

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
        tag_similarity,
        track_number_match,
        directory_cohesion: 0.0, // Set later by caller
    };

    let score = weighted_composite(&breakdown);
    (score, breakdown)
}

/// Compute weighted composite score from breakdown components.
fn weighted_composite(b: &PackingScoreBreakdown) -> f64 {
    0.25 * b.acoustid_confidence
        + 0.25 * b.duration_match
        + 0.15 * b.tag_similarity
        + 0.10 * b.track_number_match
        + 0.25 * b.directory_cohesion
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
    let inode_idx: HashMap<i64, usize> = inode_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let mut slot_set: Vec<(u32, u32)> = candidates.iter().map(|c| (c.medium_pos, c.track_pos)).collect();
    slot_set.sort();
    slot_set.dedup();
    let slot_idx: HashMap<(u32, u32), usize> = slot_set.iter().enumerate().map(|(i, &v)| (v, i)).collect();

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
            (
                release_id.clone(),
                total,
                release.title.clone(),
                artist,
            )
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
        let parent_dir = corpus
            .map(|c| c.parent_dir.clone())
            .unwrap_or_default();
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
    let candidate_rows: Vec<write_thread::PackingCandidateRow> =
        deduped.into_values().collect();

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
        "[COMPUTE] PackReleases: spawning {} ScoreReleaseCandidates, deferring Resolve → Analyze",
        spawn.len()
    ));

    // === Defer Stage 3, 4 as barrier-separated phases ===
    let deferred_phases = vec![
        (
            PipelineStage::Resolve,
            vec![Computation::Analysis(
                AnalysisComputation::ResolveReleaseConflicts,
            )],
        ),
        (
            PipelineStage::Analyze,
            vec![Computation::Analysis(
                AnalysisComputation::AnalyzeReleaseGaps,
            )],
        ),
    ];

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
/// per-release assignment via greedy, and writes results to `release_packing_scores`.
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
        candidate_inodes.entry(row.inode).or_insert_with(|| RecordingMatch {
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

    // Compute directory cohesion for this release's candidates.
    // cohesion = (unique candidate inodes from this dir for this release) / (total audio files in dir)
    // dir_file_count comes from the candidates table (computed in Stage 1).
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
        dir_total.entry(row.parent_dir.clone()).or_insert(row.dir_file_count);
    }

    for candidate in &mut candidates {
        if let Some(corpus) = corpus_info.get(&candidate.inode) {
            let unique_candidates = dir_candidate_inodes
                .get(&corpus.parent_dir)
                .map(|s| s.len())
                .unwrap_or(0);
            let total_files = dir_total
                .get(&corpus.parent_dir)
                .copied()
                .unwrap_or(1)
                .max(1);

            candidate.breakdown.directory_cohesion =
                unique_candidates as f64 / total_files as f64;
            candidate.score = weighted_composite(&candidate.breakdown);
        }
    }

    // Optimal per-release assignment via Hungarian algorithm
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
    // Per-release elimination: fill unfilled slots with unassigned directory files
    // =====================================================================
    // For each directory where this release has optimal AcoustID picks, find
    // unassigned audio files and match them to unfilled track slots. Each release
    // packs independently — we only exclude THIS release's AcoustID inodes.

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

    // Enumerate unfilled slots ONCE (global across all directories)
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
        // Collect ALL unassigned audio files across all directories with AcoustID picks
        let mut all_unassigned: Vec<(i64, String, Option<String>, Option<i64>)> = Vec::new();
        for (dir, acoustid_inodes) in &dir_acoustid_inodes {
            match read_only_db.get_unassigned_audio_in_directory(dir, acoustid_inodes) {
                Ok(files) => all_unassigned.extend(files),
                Err(_) => continue,
            }
        }

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

            // Build cost matrix: composite scoring (duration 0.5, title 0.35, tracknumber 0.15)
            // Single Hungarian across all unassigned files × all unfilled slots
            let n_unassigned = all_unassigned.len();
            let n_unfilled = unfilled.len();
            let n = n_unassigned.max(n_unfilled);
            let mut cost = vec![vec![0.0f64; n]; n];

            for (ui, (_, _, _, dur_ms)) in all_unassigned.iter().enumerate() {
                let tags = &unassigned_tags[ui];
                for (fi, (_, _, track)) in unfilled.iter().enumerate() {
                    let mb_dur = track.length.or(track.recording.length);
                    let duration_match = match (*dur_ms, mb_dur) {
                        (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                            let ratio =
                                (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
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
                            let track_sim =
                                strsim::normalized_levenshtein(t, &track.title);
                            let rec_sim = strsim::normalized_levenshtein(
                                t,
                                &track.recording.title,
                            );
                            track_sim.max(rec_sim)
                        })
                        .unwrap_or(0.0);

                    let tracknumber_match = tags
                        .get("TRACKNUMBER")
                        .and_then(|v| v.first())
                        .and_then(|tn| tn.parse::<u32>().ok())
                        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
                        .unwrap_or(0.0);

                    let composite =
                        0.5 * duration_match + 0.35 * title_sim + 0.15 * tracknumber_match;
                    cost[ui][fi] = -composite;
                }
            }

            const ELIMINATION_SCORE_THRESHOLD: f64 = 0.35;

            let col_to_row = kuhn_munkres(&cost, n);
            for (j, &row) in col_to_row.iter().enumerate().skip(1) {
                if row == 0 {
                    continue;
                }
                let ui = row - 1;
                let fi = j - 1;
                if ui >= n_unassigned || fi >= n_unfilled {
                    continue;
                }
                let composite = -cost[ui][fi];
                if composite < ELIMINATION_SCORE_THRESHOLD {
                    continue;
                }

                let (inode, _path, fingerprint_hex, dur_ms) = &all_unassigned[ui];
                let (medium_pos, track_pos, track) = &unfilled[fi];
                let corpus_tags = &unassigned_tags[ui];

                // Compute full score breakdown for the elimination match
                let mb_dur = track.length.or(track.recording.length);
                let duration_match = match (*dur_ms, mb_dur) {
                    (Some(corpus_dur), Some(mb_d)) if mb_d > 0 => {
                        let ratio =
                            (corpus_dur as f64 - mb_d as f64).abs() / mb_d as f64;
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
                        let track_sim =
                            strsim::normalized_levenshtein(t, &track.title);
                        let rec_sim = strsim::normalized_levenshtein(
                            t,
                            &track.recording.title,
                        );
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

                let tag_similarity =
                    0.4 * title_sim + 0.35 * artist_sim + 0.25 * album_sim;

                let track_number_match = corpus_tags
                    .get("TRACKNUMBER")
                    .and_then(|v| v.first())
                    .and_then(|tn| tn.parse::<u32>().ok())
                    .map(|tn| if tn == *track_pos { 1.0 } else { 0.0 })
                    .unwrap_or(0.0);

                let breakdown = PackingScoreBreakdown {
                    acoustid_confidence: 0.0,
                    duration_match,
                    tag_similarity,
                    track_number_match,
                    directory_cohesion: 1.0,
                };
                let score = weighted_composite(&breakdown);
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
// Stage 3: ResolveReleaseConflicts
// ============================================================================

/// Execute ResolveReleaseConflicts — release-level priority resolution.
///
/// Reads optimal picks from all releases and selects the best non-conflicting
/// set of complete release packings. Two releases conflict if they share any
/// optimal inode. Connected components of the conflict graph are solved via
/// maximum independent set (exhaustive for ≤20 releases, greedy for larger).
///
/// Selected releases keep their full Stage 2 Hungarian packings intact.
/// Non-selected releases receive residual unclaimed inodes.
pub fn execute_resolve_release_conflicts(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::ResolveReleaseConflicts;

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
        reconcile_corpus_signals::<ReleasePackingSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Count how many releases each inode appears in (for alternatives_count)
    let mut inode_release_set: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in &optimal_scores {
        inode_release_set
            .entry(row.inode)
            .or_default()
            .insert(row.release_id.clone());
    }

    // --- Release-level optimal resolution ---
    //
    // Stage 2 produces complete per-release packings via Hungarian. We select
    // the best packings at the release level, never remixing individual
    // assignments across releases.
    //
    // Resolution uses MIS (Maximum Independent Set) + fullness-aware assignment:
    //   Phase 1: MIS among full-capable releases (maximize full match count)
    //   Phase 2a: Iterative MIS on residual-completable releases (maximize additional full matches)
    //   Phase 2b: Per-inode-best for truly residual AcoustID inodes
    //   Phase 3: Per-inode-best for elimination gap-fill (order-independent)
    //   Phase 4: Per-inode-best for singles (order-independent, last priority)

    // Step 1: Build per-release proposals from Stage 2 optimal scores
    struct ReleaseProposal {
        rows: Vec<OptimalPackingScoreRow>,
        total_tracks: i32,
    }

    let mut proposals_map: HashMap<&str, Vec<OptimalPackingScoreRow>> = HashMap::new();
    for row in &optimal_scores {
        proposals_map
            .entry(&row.release_id)
            .or_default()
            .push(row.clone());
    }

    let mut multi_proposals: Vec<ReleaseProposal> = Vec::new();
    let mut single_proposals: Vec<ReleaseProposal> = Vec::new();

    for (release_id, rows) in proposals_map {
        let total_tracks = manifest_map
            .get(release_id)
            .map(|&(_, _, t)| t)
            .unwrap_or(0);
        let proposal = ReleaseProposal {
            rows,
            total_tracks,
        };
        if total_tracks <= 1 {
            single_proposals.push(proposal);
        } else {
            multi_proposals.push(proposal);
        }
    }

    // Step 2: Optimal assignment via MIS + per-inode-best
    //
    // Four phases, none order-dependent:
    //   Phase 1: Maximum Independent Set among full-capable releases (maximize full matches)
    //   Phase 2: Per-inode-best-score for residual AcoustID inodes
    //   Phase 3: Per-inode-best-score for elimination gap-fill
    //   Phase 4: Per-inode-best-score for singles
    let mut assigned_inodes: HashSet<i64> = HashSet::new();
    let mut assignments: Vec<OptimalPackingScoreRow> = Vec::new();

    // --- Phase 1: Maximum full matches via MIS ---
    //
    // Among releases where AcoustID count >= total_tracks ("full-capable"),
    // find the maximum set that can each get ALL their AcoustID inodes
    // without sharing any inode with another selected release.

    // Collect per-proposal AcoustID inode sets and scores
    struct ProposalMeta {
        idx: usize, // index into multi_proposals
        acoustid_inodes: HashSet<i64>,
        acoustid_score: f64,
    }

    let mut full_capable: Vec<ProposalMeta> = Vec::new();
    for (idx, prop) in multi_proposals.iter().enumerate() {
        let acoustid_inodes: HashSet<i64> = prop
            .rows
            .iter()
            .filter(|r| r.match_method == 0)
            .map(|r| r.inode)
            .collect();
        if acoustid_inodes.len() as i32 >= prop.total_tracks && prop.total_tracks > 0 {
            let acoustid_score: f64 = prop
                .rows
                .iter()
                .filter(|r| r.match_method == 0)
                .map(|r| r.score)
                .sum();
            full_capable.push(ProposalMeta {
                idx,
                acoustid_inodes,
                acoustid_score,
            });
        }
    }

    let fc_count = full_capable.len();
    let mis_candidates: Vec<MisCandidate> = full_capable
        .iter()
        .map(|meta| MisCandidate {
            inode_set: meta.acoustid_inodes.clone(),
            score: meta.acoustid_score,
        })
        .collect();

    let mis_result = solve_maximum_independent_set(&mis_candidates);

    // Lock in MIS-selected releases: all their AcoustID inodes
    for (fi, meta) in full_capable.iter().enumerate() {
        if mis_result.selected[fi] {
            for row in &multi_proposals[meta.idx].rows {
                if row.match_method == 0 {
                    assigned_inodes.insert(row.inode);
                    assignments.push(row.clone());
                }
            }
        }
    }

    log_general(format!(
        "[COMPUTE] ResolveReleaseConflicts Phase 1: {} full-capable, {} components \
         (max size {}), {} selected via MIS",
        fc_count,
        mis_result.component_count,
        mis_result.max_component_size,
        mis_result.selected_count,
    ));

    // --- Phase 2a: Iterative MIS on residual-completable releases ---
    //
    // After Phase 1, some full-capable releases lost optimal inodes to selected
    // releases. But they may have non-optimal AcoustID candidates that can fill
    // those track slots. We load ALL AcoustID candidates (not just optimal),
    // check if unclaimed candidates can cover all tracks, and run MIS to maximize
    // additional full matches before falling back to per-inode-best.
    let mut phase2a_total = 0usize;
    let mut phase2a_iterations = 0usize;

    // Track which full-capable releases have been completed by Phase 2a
    let mut phase2a_completed: HashSet<usize> = HashSet::new();

    // Identify non-selected full-capable releases that lost inodes
    let non_selected_fc: Vec<usize> = (0..full_capable.len())
        .filter(|&fi| !mis_result.selected[fi])
        .collect();

    // Load all AcoustID candidates (optimal + non-optimal) for non-selected releases
    let all_candidates_rows = if !non_selected_fc.is_empty() {
        let release_ids: Vec<&str> = non_selected_fc
            .iter()
            .map(|&fi| {
                multi_proposals[full_capable[fi].idx]
                    .rows
                    .first()
                    .map(|r| r.release_id.as_str())
                    .unwrap_or("")
            })
            .filter(|s| !s.is_empty())
            .collect();
        read_only_db
            .get_all_acoustid_candidates_for_releases(&release_ids)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Group all candidates by release_id
    let mut all_candidates_by_release: HashMap<&str, Vec<&OptimalPackingScoreRow>> =
        HashMap::new();
    for row in &all_candidates_rows {
        all_candidates_by_release
            .entry(&row.release_id)
            .or_default()
            .push(row);
    }

    loop {
        // For each non-selected full-capable release, try to build a complete
        // assignment from unclaimed AcoustID candidates
        struct ResidualRelease {
            fc_idx: usize,
            assigned_rows: Vec<OptimalPackingScoreRow>, // greedy best-per-slot
            needed_inodes: HashSet<i64>,                // inodes used in assignment
            total_score: f64,
        }

        let mut residual: Vec<ResidualRelease> = Vec::new();

        for &fi in &non_selected_fc {
            if phase2a_completed.contains(&fi) {
                continue;
            }
            let meta = &full_capable[fi];
            let total_tracks = multi_proposals[meta.idx].total_tracks;
            let release_id = match multi_proposals[meta.idx].rows.first() {
                Some(r) => r.release_id.as_str(),
                None => continue,
            };

            let candidates = match all_candidates_by_release.get(release_id) {
                Some(c) => c,
                None => continue,
            };

            // Filter to unclaimed inodes only
            let available: Vec<&&OptimalPackingScoreRow> = candidates
                .iter()
                .filter(|r| !assigned_inodes.contains(&r.inode))
                .collect();

            // Greedy assignment: for each track slot, pick the highest-scoring
            // available candidate, ensuring each inode is used at most once
            let mut slots_needed: HashSet<(i32, i32)> = HashSet::new();
            for row in &multi_proposals[meta.idx].rows {
                if row.match_method == 0 {
                    slots_needed.insert((row.medium_pos, row.track_pos));
                }
            }

            // Sort available candidates by score descending
            let mut sorted_available: Vec<&OptimalPackingScoreRow> =
                available.into_iter().copied().collect();
            sorted_available.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut used_inodes: HashSet<i64> = HashSet::new();
            let mut filled_slots: HashSet<(i32, i32)> = HashSet::new();
            let mut assigned_rows: Vec<OptimalPackingScoreRow> = Vec::new();
            let mut total_score = 0.0f64;

            for row in &sorted_available {
                let slot = (row.medium_pos, row.track_pos);
                if filled_slots.contains(&slot) || used_inodes.contains(&row.inode) {
                    continue;
                }
                if !slots_needed.contains(&slot) {
                    continue;
                }
                filled_slots.insert(slot);
                used_inodes.insert(row.inode);
                assigned_rows.push((*row).clone());
                total_score += row.score;
            }

            if filled_slots.len() as i32 >= total_tracks {
                residual.push(ResidualRelease {
                    fc_idx: fi,
                    assigned_rows,
                    needed_inodes: used_inodes,
                    total_score,
                });
            }
        }

        if residual.is_empty() {
            break;
        }

        // Build MIS candidates from residual-completable releases
        let residual_candidates: Vec<MisCandidate> = residual
            .iter()
            .map(|r| MisCandidate {
                inode_set: r.needed_inodes.clone(),
                score: r.total_score,
            })
            .collect();

        let residual_mis = solve_maximum_independent_set(&residual_candidates);

        if residual_mis.selected_count == 0 {
            break;
        }

        // Lock in selected residual releases
        for (ri, rr) in residual.iter().enumerate() {
            if residual_mis.selected[ri] {
                for row in &rr.assigned_rows {
                    assigned_inodes.insert(row.inode);
                    assignments.push(row.clone());
                }
                phase2a_completed.insert(rr.fc_idx);
            }
        }

        phase2a_total += residual_mis.selected_count;
        phase2a_iterations += 1;
    }

    if phase2a_total > 0 {
        log_general(format!(
            "[COMPUTE] ResolveReleaseConflicts Phase 2a: {} residual releases completed \
             in {} iteration(s)",
            phase2a_total, phase2a_iterations,
        ));
    }

    // --- Phase 2b: Per-inode AcoustID residual assignment ---
    //
    // For AcoustID inodes not locked by Phase 1 or 2a, each inode independently
    // goes to its highest-scoring claimant. These inodes cannot complete any
    // release, so per-inode-best is appropriate.
    {
        let mut inode_candidates: HashMap<i64, Vec<&OptimalPackingScoreRow>> = HashMap::new();
        for prop in &multi_proposals {
            for row in &prop.rows {
                if row.match_method == 0 && !assigned_inodes.contains(&row.inode) {
                    inode_candidates.entry(row.inode).or_default().push(row);
                }
            }
        }
        for candidates in inode_candidates.values() {
            let best = candidates
                .iter()
                .max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.release_id.cmp(&b.release_id))
                })
                .unwrap();
            assigned_inodes.insert(best.inode);
            assignments.push((*best).clone());
        }
    }

    // --- Phase 3: Per-inode elimination gap-fill ---
    //
    // Elimination inodes that are not yet assigned go to their
    // highest-scoring claimant. Cannot steal AcoustID assignments.
    {
        let mut inode_candidates: HashMap<i64, Vec<&OptimalPackingScoreRow>> = HashMap::new();
        for prop in &multi_proposals {
            for row in &prop.rows {
                if row.match_method == 1 && !assigned_inodes.contains(&row.inode) {
                    inode_candidates.entry(row.inode).or_default().push(row);
                }
            }
        }
        for candidates in inode_candidates.values() {
            let best = candidates
                .iter()
                .max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.release_id.cmp(&b.release_id))
                })
                .unwrap();
            assigned_inodes.insert(best.inode);
            assignments.push((*best).clone());
        }
    }

    // --- Phase 4: Per-inode singles ---
    //
    // Single-track releases claim remaining inodes. Per-inode-best, last priority.
    {
        let mut inode_candidates: HashMap<i64, Vec<&OptimalPackingScoreRow>> = HashMap::new();
        for prop in &single_proposals {
            for row in &prop.rows {
                if !assigned_inodes.contains(&row.inode) {
                    inode_candidates.entry(row.inode).or_default().push(row);
                }
            }
        }
        for candidates in inode_candidates.values() {
            let best = candidates
                .iter()
                .max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.release_id.cmp(&b.release_id))
                })
                .unwrap();
            assigned_inodes.insert(best.inode);
            assignments.push((*best).clone());
        }
    }

    // Compute per-release coverage
    let mut release_filled: HashMap<&str, u32> = HashMap::new();
    for row in &assignments {
        *release_filled.entry(&row.release_id).or_default() += 1;
    }

    // Load corpus paths (candidates + elimination-matched inodes from files table)
    let corpus_paths: HashMap<i64, String> = match read_only_db.get_packing_inode_paths() {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query candidate paths: {}", e),
            );
        }
    };

    // Emit signals
    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();

    for row in &assignments {
        let path = match corpus_paths.get(&row.inode) {
            Some(p) => p.clone(),
            None => continue,
        };

        let (release_title, release_artist, total_tracks): (String, String, i32) =
            match manifest_map.get(row.release_id.as_str()) {
                Some(&(title, artist, total)) => {
                    (title.to_string(), artist.to_string(), total)
                }
                None => continue,
            };

        let filled = release_filled
            .get(row.release_id.as_str())
            .copied()
            .unwrap_or(0);

        let alternatives_count = inode_release_set
            .get(&row.inode)
            .map(|s| s.len() as u16)
            .unwrap_or(1);

        let breakdown: PackingScoreBreakdown =
            bincode::deserialize(&row.score_breakdown).unwrap_or(PackingScoreBreakdown {
                acoustid_confidence: 0.0,
                duration_match: 0.0,
                tag_similarity: 0.0,
                track_number_match: 0.0,
                directory_cohesion: 0.0,
            });

        let signal = ReleasePackingSignal {
            inode: row.inode,
            path,
            data: ReleasePackingData {
                release_id: row.release_id.clone(),
                release_title,
                release_artist,
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

        computed.push(ComputedCorpusSignal::new(
            signal.inode,
            TypedSignalWrite::ReleasePacking(signal),
        ));
    }

    let unique_releases = release_filled.len();
    let (cleared, new, updated, unchanged) =
        reconcile_corpus_signals::<ReleasePackingSignal>(read_only_db, &sender, computed, witness);

    // Record pending AcoustID submissions for elimination-method winners
    let mut pending_submissions: Vec<write_thread::PendingAcoustIdSubmission> = Vec::new();
    for row in &assignments {
        if row.match_method != 1 {
            continue;
        }
        if let (Some(fp_hex), Some(dur_ms)) = (&row.fingerprint_hex, row.raw_duration_ms) {
            pending_submissions.push(write_thread::PendingAcoustIdSubmission {
                fingerprint: fp_hex.clone(),
                recording_id: row.recording_id.clone(),
                duration_ms: dur_ms,
                source: "elimination".to_string(),
            });
        }
    }
    let submission_count = pending_submissions.len();
    if !pending_submissions.is_empty() {
        sender.write_pending_acoustid_submissions(pending_submissions, witness);
    }

    log_general(format!(
        "[COMPUTE] ResolveReleaseConflicts: {} inodes assigned to {} releases \
         ({} multi-track, {} singles) | \
         Phase 1 MIS: {}/{} full-capable selected, {} components (max {}) | \
         Phase 2a: {} residual completed ({} iterations) | \
         {} elimination submissions | \
         signals: cleared={}, new={}, updated={}, unchanged={}",
        assignments.len(),
        unique_releases,
        multi_proposals.len(),
        single_proposals.len(),
        mis_result.selected_count,
        fc_count,
        mis_result.component_count,
        mis_result.max_component_size,
        phase2a_total,
        phase2a_iterations,
        submission_count,
        cleared,
        new,
        updated,
        unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}


// ============================================================================
// Stage 4: AnalyzeReleaseGaps (renumbered from old Stage 5)
// ============================================================================

/// Execute AnalyzeReleaseGaps — identify unmatched tracks and near-miss patterns.
pub fn execute_analyze_release_gaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = AnalysisComputation::AnalyzeReleaseGaps;

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
                inode_recordings.entry(inode).or_default().push(recording_id);
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
                "[COMPUTE] AnalyzeReleaseGaps: failed to query fingerprinted inodes: {}",
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
    for (inode, _path, release_id, medium_pos, track_pos) in &actual_assignments {
        filled_slots
            .entry(release_id.clone())
            .or_default()
            .insert((*medium_pos as i32, *track_pos as i32));
        release_assigned_inodes
            .entry(release_id.clone())
            .or_default()
            .insert(*inode);
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
            if let Some(candidate_inodes) = release_candidate_inodes.get(manifest_row.release_id.as_str()) {
                if !candidate_inodes.is_empty()
                    && candidate_inodes.iter().all(|i| full_match_inodes.contains(i))
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

    // === Packed release signals ===
    // Emit one per-release aggregate signal with typed category.
    let mut packed_signals: Vec<ComputedAggregateSignal> = Vec::new();

    for manifest_row in &manifest {
        let filled_count = filled_slots
            .get(&manifest_row.release_id)
            .map(|s| s.len() as u32)
            .unwrap_or(0);

        // Only emit for releases that have at least one assigned track
        if filled_count == 0 {
            continue;
        }

        let total = manifest_row.total_tracks as u32;
        let category = if total == 1 {
            PackedReleaseCategory::Single
        } else if filled_count >= total {
            PackedReleaseCategory::FullMatch
        } else {
            // Suppress fully-covered incomplete releases (same filter as unfilled slots)
            if let Some(candidate_inodes) = release_candidate_inodes.get(manifest_row.release_id.as_str()) {
                if !candidate_inodes.is_empty()
                    && candidate_inodes.iter().all(|i| full_match_inodes.contains(i))
                {
                    continue;
                }
            }
            PackedReleaseCategory::Incomplete
        };

        let key = format!("{}:{}", category.key_prefix(), manifest_row.release_id);
        let signal = PackedReleaseSignal {
            key: key.clone(),
            data: PackedReleaseData {
                release_id: manifest_row.release_id.clone(),
                release_title: manifest_row.release_title.clone(),
                release_artist: manifest_row.release_artist.clone(),
                category,
                assigned_count: filled_count,
                total_tracks: total,
            },
        };

        packed_signals.push(ComputedAggregateSignal::new(
            key,
            TypedSignalWrite::PackedRelease(signal),
        ));
    }

    let (pr_cleared, pr_new, pr_updated, pr_unchanged) =
        reconcile_aggregate_signals::<PackedReleaseSignal>(
            read_only_db,
            &sender,
            packed_signals,
            witness,
        );

    // === Near-miss detection ===
    // A release with (n-1)/n tracks matched, all from the same directory containing
    // exactly n audio files. The unmatched file is the likely missing track.
    let mut near_miss_signals: Vec<ComputedAggregateSignal> = Vec::new();

    for manifest_row in &manifest {
        let total = manifest_row.total_tracks as u32;
        if total < 2 {
            continue;
        }

        let filled_for_release = match filled_slots.get(&manifest_row.release_id) {
            Some(s) => s,
            None => continue,
        };
        let filled_count = filled_for_release.len() as u32;

        // Near-miss: exactly (n-1)/n filled
        if filled_count + 1 != total {
            continue;
        }

        // Get the assigned inodes for this release (from actual assignments,
        // which includes both AcoustId and Elimination matches)
        let release_inodes: Vec<i64> = actual_assignments
            .iter()
            .filter(|(_, _, rid, _, _)| rid == &manifest_row.release_id)
            .map(|(inode, _, _, _, _)| *inode)
            .collect();

        // Check directory cohesion: all assigned inodes from same directory
        let mut dirs: HashSet<String> = HashSet::new();
        for inode in &release_inodes {
            if let Some(path) = corpus_paths.get(inode) {
                if let Some(parent) = Path::new(path).parent() {
                    dirs.insert(parent.to_string_lossy().to_string());
                }
            }
        }

        if dirs.len() != 1 {
            continue;
        }
        let directory = dirs.into_iter().next().unwrap();

        // Count audio files in that directory (from corpus)
        let dir_audio_count = corpus_paths
            .values()
            .filter(|p| {
                Path::new(p.as_str())
                    .parent()
                    .map(|parent| parent.to_string_lossy() == directory)
                    .unwrap_or(false)
            })
            .count() as u32;

        // The directory should have exactly total tracks (n audio files for an n-track release)
        if dir_audio_count != total {
            continue;
        }

        // Find the unmatched file in this directory
        let assigned_set: HashSet<i64> = release_inodes.iter().copied().collect();
        let mut candidate_inode: Option<i64> = None;
        let mut candidate_path: Option<String> = None;

        for (inode, path) in &corpus_paths {
            if assigned_set.contains(inode) {
                continue;
            }
            if let Some(parent) = Path::new(path.as_str()).parent() {
                if parent.to_string_lossy() == directory {
                    candidate_inode = Some(*inode);
                    candidate_path = Some(path.clone());
                    break;
                }
            }
        }

        let (candidate_inode, candidate_path) = match (candidate_inode, candidate_path) {
            (Some(i), Some(p)) => (i, p),
            _ => continue,
        };

        // Find the missing slot
        let release = match read_only_db.get_mb_release_cache(&manifest_row.release_id) {
            Ok(Some((raw_json, _))) => match musicbrainz::parse_release(&raw_json) {
                Ok(r) => r,
                Err(_) => continue,
            },
            _ => continue,
        };

        let mut missing_slot = None;
        for medium in &release.media {
            for track in &medium.tracks {
                let slot = (medium.position as i32, track.position as i32);
                if !filled_for_release.contains(&slot) {
                    missing_slot = Some((medium.position, track.position, track.title.clone(), track.recording.id.clone()));
                    break;
                }
            }
            if missing_slot.is_some() {
                break;
            }
        }

        let (missing_medium, missing_track, missing_title, missing_recording) =
            match missing_slot {
                Some(s) => s,
                None => continue,
            };

        let key = format!("{}:{}", manifest_row.release_id, directory);
        let signal = NearMissReleaseSignal {
            key: key.clone(),
            data: NearMissReleaseData {
                release_id: manifest_row.release_id.clone(),
                release_title: manifest_row.release_title.clone(),
                release_artist: manifest_row.release_artist.clone(),
                directory,
                candidate_inode,
                candidate_path,
                missing_medium_pos: missing_medium,
                missing_track_pos: missing_track,
                missing_track_title: missing_title,
                missing_recording_id: missing_recording,
                filled_count,
                total_tracks: total,
            },
        };

        near_miss_signals.push(ComputedAggregateSignal::new(
            key,
            TypedSignalWrite::NearMissRelease(signal),
        ));
    }

    let (nm_cleared, nm_new, nm_updated, nm_unchanged) =
        reconcile_aggregate_signals::<NearMissReleaseSignal>(
            read_only_db,
            &sender,
            near_miss_signals,
            witness,
        );

    log_general(format!(
        "[COMPUTE] AnalyzeReleaseGaps: \
         unmatched_corpus: cleared={}, new={}, updated={}, unchanged={} | \
         unfilled_slots: cleared={}, new={}, updated={}, unchanged={} | \
         packed_release: cleared={}, new={}, updated={}, unchanged={} | \
         near_miss: cleared={}, new={}, updated={}, unchanged={} | \
         {} fully-covered releases suppressed",
        uc_cleared, uc_new, uc_updated, uc_unchanged, us_cleared, us_new, us_updated,
        us_unchanged, pr_cleared, pr_new, pr_updated, pr_unchanged, nm_cleared, nm_new,
        nm_updated, nm_unchanged, suppressed_covered
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
