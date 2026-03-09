//! Hungarian (Kuhn-Munkres) algorithm and directory-constrained assignment.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::external::musicbrainz;

use super::types::{CandidateAssignment, CorpusFileInfo};

/// Kuhn-Munkres (Hungarian) algorithm on an n×n cost matrix.
///
/// Returns `col_to_row` assignment where `col_to_row[j]` is the 1-indexed row
/// assigned to column j (0 = unassigned). The matrix must be square.
#[allow(clippy::needless_range_loop)]
pub(super) fn kuhn_munkres(cost: &[Vec<f64>], n: usize) -> Vec<usize> {
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
pub(super) fn hungarian_assignment(
    candidates: &[CandidateAssignment],
) -> HashSet<(i64, (u32, u32))> {
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
pub(super) enum TargetDirs {
    /// One directory for all media (single-medium or multi-medium fallback).
    Single(String),
    /// Per-medium directory assignments from sibling group detection.
    PerMedium(HashMap<u32, String>),
    /// No valid target directory found.
    Empty,
}

impl TargetDirs {
    /// Check if a candidate (inode in a given dir, targeting a given medium) belongs.
    pub fn contains(&self, parent_dir: &str, medium_pos: u32) -> bool {
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
    pub fn dir_for_medium(&self, medium_pos: u32) -> Option<&str> {
        match self {
            TargetDirs::Single(d) => Some(d.as_str()),
            TargetDirs::PerMedium(m) => m.get(&medium_pos).map(|d| d.as_str()),
            TargetDirs::Empty => None,
        }
    }
}

pub(super) fn select_target_directory(
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
pub(super) fn localized_release_artist(
    credits: &[musicbrainz::MbArtistCredit],
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
