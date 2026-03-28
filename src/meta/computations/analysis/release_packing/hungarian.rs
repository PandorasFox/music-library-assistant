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

/// Target directory constraint for release packing.
#[derive(Debug)]
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

/// Run Hungarian for every candidate directory, return the best TargetDirs + optimal pairs.
///
/// For multi-medium releases, also tries sibling directory mappings.
///
/// Directory selection prioritizes track coverage (most assigned slots), then prefers
/// snug-fitting directories (fewest leftover files) at equal coverage.
/// Deterministic tiebreak chain:
///   most assigned slots → fewest leftovers → highest score → lexicographic dir path.
#[allow(clippy::type_complexity)]
pub(super) fn score_all_directories(
    candidates: &[CandidateAssignment],
    corpus_info: &HashMap<i64, CorpusFileInfo>,
    dir_candidate_inodes: &HashMap<String, HashSet<i64>>,
    media: &[musicbrainz::MbMedium],
    dir_file_counts: &HashMap<String, usize>,
) -> (TargetDirs, HashSet<(i64, (u32, u32))>) {
    if candidates.is_empty() || dir_candidate_inodes.is_empty() {
        return (TargetDirs::Empty, HashSet::new());
    }

    // (assigned_count, leftovers, score, dir_key, target_dirs, pairs)
    type BestCandidate = (usize, usize, f64, String, TargetDirs, HashSet<(i64, (u32, u32))>);
    let mut best: Option<BestCandidate> = None;

    // Coverage first: most assigned slots wins. At equal coverage, prefer fewer
    // leftover files (snug-fitting directories). Then score, then lexicographic.
    let is_better = |leftovers: usize,
                     score: f64,
                     count: usize,
                     dir_key: &str,
                     best: &Option<BestCandidate>|
     -> bool {
        match best {
            None => true,
            Some((best_count, best_left, best_score, ref best_key, _, _)) => {
                if count > *best_count {
                    true
                } else if count == *best_count {
                    if leftovers < *best_left {
                        true
                    } else if leftovers == *best_left {
                        if score > *best_score {
                            true
                        } else if (score - *best_score).abs() < f64::EPSILON {
                            dir_key < best_key.as_str()
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    };

    // Multi-medium: try sibling directory mapping first
    if media.len() > 1 {
        if let Some(mapping) =
            find_sibling_dir_mapping(candidates, corpus_info, dir_candidate_inodes)
        {
            let sibling_dirs = TargetDirs::PerMedium(mapping.clone());
            let filtered: Vec<&CandidateAssignment> = candidates
                .iter()
                .filter(|c| {
                    corpus_info
                        .get(&c.inode)
                        .map(|ci| sibling_dirs.contains(&ci.parent_dir, c.medium_pos))
                        .unwrap_or(false)
                })
                .collect();
            let filtered_owned: Vec<CandidateAssignment> = filtered
                .iter()
                .map(|c| CandidateAssignment {
                    inode: c.inode,
                    recording_id: c.recording_id.clone(),
                    release_id: c.release_id.clone(),
                    medium_pos: c.medium_pos,
                    track_pos: c.track_pos,
                    medium_format: c.medium_format.clone(),
                    track_number: c.track_number.clone(),
                    track_title: c.track_title.clone(),
                    score: c.score,
                    breakdown: c.breakdown.clone(),
                })
                .collect();
            let pairs = hungarian_assignment(&filtered_owned);
            let total_score: f64 = pairs
                .iter()
                .filter_map(|(inode, slot)| {
                    filtered_owned
                        .iter()
                        .find(|c| c.inode == *inode && (c.medium_pos, c.track_pos) == *slot)
                        .map(|c| c.score)
                })
                .sum();
            // Dir key for sibling: sorted mapping values
            let mut dir_key_parts: Vec<&str> = mapping.values().map(|s| s.as_str()).collect();
            dir_key_parts.sort();
            let dir_key = dir_key_parts.join("|");

            // Leftovers = sum of sibling dir file counts − assigned slots
            let total_files: usize = mapping
                .values()
                .map(|d| dir_file_counts.get(d).copied().unwrap_or(0))
                .sum();
            let leftovers = total_files.saturating_sub(pairs.len());

            if is_better(leftovers, total_score, pairs.len(), &dir_key, &best) {
                best = Some((pairs.len(), leftovers, total_score, dir_key, sibling_dirs, pairs));
            }
        }
    }

    // Try each individual directory
    let mut dirs: Vec<&String> = dir_candidate_inodes.keys().collect();
    dirs.sort();

    for dir in dirs {
        let target = TargetDirs::Single(dir.clone());
        let filtered: Vec<CandidateAssignment> = candidates
            .iter()
            .filter(|c| {
                corpus_info
                    .get(&c.inode)
                    .map(|ci| ci.parent_dir == *dir)
                    .unwrap_or(false)
            })
            .map(|c| CandidateAssignment {
                inode: c.inode,
                recording_id: c.recording_id.clone(),
                release_id: c.release_id.clone(),
                medium_pos: c.medium_pos,
                track_pos: c.track_pos,
                medium_format: c.medium_format.clone(),
                track_number: c.track_number.clone(),
                track_title: c.track_title.clone(),
                score: c.score,
                breakdown: c.breakdown.clone(),
            })
            .collect();

        if filtered.is_empty() {
            continue;
        }

        let pairs = hungarian_assignment(&filtered);
        let total_score: f64 = pairs
            .iter()
            .filter_map(|(inode, slot)| {
                filtered
                    .iter()
                    .find(|c| c.inode == *inode && (c.medium_pos, c.track_pos) == *slot)
                    .map(|c| c.score)
            })
            .sum();

        let file_count = dir_file_counts.get(dir.as_str()).copied().unwrap_or(0);
        let leftovers = file_count.saturating_sub(pairs.len());

        if is_better(leftovers, total_score, pairs.len(), dir.as_str(), &best) {
            best = Some((
                pairs.len(),
                leftovers,
                total_score,
                dir.clone(),
                target,
                pairs,
            ));
        }
    }

    match best {
        Some((_, _, _, _, target_dirs, pairs)) => (target_dirs, pairs), // (count, leftovers, score, key, dirs, pairs)
        None => (TargetDirs::Empty, HashSet::new()),
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
    // Deterministic tiebreak: most candidates, then lexicographic smallest parent path
    let best_group = parent_groups
        .into_iter()
        .filter(|(_, dirs)| dirs.len() >= 2)
        .max_by(|(parent_a, dirs_a), (parent_b, dirs_b)| {
            let count_a: usize = dirs_a
                .iter()
                .map(|d| dir_candidate_inodes.get(d).map(|s| s.len()).unwrap_or(0))
                .sum();
            let count_b: usize = dirs_b
                .iter()
                .map(|d| dir_candidate_inodes.get(d).map(|s| s.len()).unwrap_or(0))
                .sum();
            count_a.cmp(&count_b).then(parent_b.cmp(parent_a))
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
    // Deterministic tiebreak: highest count, then lexicographic smallest dir
    let mut mapping: HashMap<u32, String> = HashMap::new();
    for (medium_pos, dir_counts) in &medium_dir_counts {
        let mut sorted: Vec<(&&str, &usize)> = dir_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        if let Some((&best_dir, _)) = sorted.first() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::tags::TagSet;
    use crate::meta::signals::data::PackingScoreBreakdown;

    fn dummy_breakdown() -> PackingScoreBreakdown {
        PackingScoreBreakdown {
            acoustid_confidence: 0.0,
            duration_match: 0.0,
            title_match: 0.0,
            artist_match: 0.0,
            album_match: 0.0,
            track_number_match: 0.0,
        }
    }

    fn make_candidate(inode: i64, medium: u32, track: u32, score: f64) -> CandidateAssignment {
        CandidateAssignment {
            inode,
            recording_id: format!("rec-{}", inode),
            release_id: "rel-1".to_string(),
            medium_pos: medium,
            track_pos: track,
            medium_format: None,
            track_number: track.to_string(),
            track_title: format!("Track {}", track),
            score,
            breakdown: dummy_breakdown(),
        }
    }

    #[test]
    fn test_hungarian_deterministic() {
        // Same input, 100 iterations → identical output
        let candidates = vec![
            make_candidate(100, 1, 1, 0.9),
            make_candidate(100, 1, 2, 0.3),
            make_candidate(200, 1, 1, 0.4),
            make_candidate(200, 1, 2, 0.8),
            make_candidate(300, 1, 3, 0.7),
        ];
        let first = hungarian_assignment(&candidates);
        for _ in 0..100 {
            assert_eq!(hungarian_assignment(&candidates), first);
        }
    }

    #[test]
    fn test_hungarian_simple_assignment() {
        // Known optimal: inode 100 → slot (1,1), inode 200 → slot (1,2)
        let candidates = vec![
            make_candidate(100, 1, 1, 0.9),
            make_candidate(100, 1, 2, 0.1),
            make_candidate(200, 1, 1, 0.1),
            make_candidate(200, 1, 2, 0.9),
        ];
        let result = hungarian_assignment(&candidates);
        assert!(result.contains(&(100, (1, 1))));
        assert!(result.contains(&(200, (1, 2))));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_hungarian_rectangular_more_inodes() {
        // 3 inodes, 2 slots → only 2 assigned
        let candidates = vec![
            make_candidate(100, 1, 1, 0.5),
            make_candidate(200, 1, 2, 0.8),
            make_candidate(300, 1, 1, 0.9),
            make_candidate(300, 1, 2, 0.3),
        ];
        let result = hungarian_assignment(&candidates);
        assert_eq!(result.len(), 2);
        // inode 300 should get slot (1,1) with score 0.9, inode 200 gets (1,2)
        assert!(result.contains(&(300, (1, 1))));
        assert!(result.contains(&(200, (1, 2))));
    }

    #[test]
    fn test_hungarian_rectangular_more_slots() {
        // 1 inode, 3 slots → only 1 assigned (best slot)
        let candidates = vec![
            make_candidate(100, 1, 1, 0.3),
            make_candidate(100, 1, 2, 0.9),
            make_candidate(100, 1, 3, 0.5),
        ];
        let result = hungarian_assignment(&candidates);
        assert_eq!(result.len(), 1);
        assert!(result.contains(&(100, (1, 2))));
    }

    #[test]
    fn test_directory_scoring_best_wins() {
        // Dir A has 2 candidates with high scores, Dir B has 1 with low score.
        // Both dirs have file counts matching their candidate counts (no leftovers).
        let candidates = vec![
            make_candidate(100, 1, 1, 0.9),
            make_candidate(200, 1, 2, 0.8),
            make_candidate(300, 1, 1, 0.2),
        ];
        let mut corpus_info = HashMap::new();
        corpus_info.insert(100, CorpusFileInfo {
            parent_dir: "/music/album_a".to_string(),
            tags: TagSet::empty(),
            duration_ms: None,
        });
        corpus_info.insert(200, CorpusFileInfo {
            parent_dir: "/music/album_a".to_string(),
            tags: TagSet::empty(),
            duration_ms: None,
        });
        corpus_info.insert(300, CorpusFileInfo {
            parent_dir: "/music/album_b".to_string(),
            tags: TagSet::empty(),
            duration_ms: None,
        });

        let mut dir_inodes = HashMap::new();
        dir_inodes.insert("/music/album_a".to_string(), [100i64, 200].iter().copied().collect());
        dir_inodes.insert("/music/album_b".to_string(), [300i64].iter().copied().collect());

        let mut dir_file_counts = HashMap::new();
        dir_file_counts.insert("/music/album_a".to_string(), 2);
        dir_file_counts.insert("/music/album_b".to_string(), 1);

        let media = vec![]; // single-medium fallback

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/album_a"),
            _ => panic!("Expected Single target dir"),
        }
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn test_directory_scoring_tiebreak_deterministic() {
        // Two dirs with identical scores and file counts → lexicographic smallest wins
        let candidates = vec![
            make_candidate(100, 1, 1, 0.5),
            make_candidate(200, 1, 1, 0.5),
        ];
        let mut corpus_info = HashMap::new();
        corpus_info.insert(100, CorpusFileInfo {
            parent_dir: "/music/zzz".to_string(),
            tags: TagSet::empty(),
            duration_ms: None,
        });
        corpus_info.insert(200, CorpusFileInfo {
            parent_dir: "/music/aaa".to_string(),
            tags: TagSet::empty(),
            duration_ms: None,
        });

        let mut dir_inodes = HashMap::new();
        dir_inodes.insert("/music/zzz".to_string(), [100i64].iter().copied().collect());
        dir_inodes.insert("/music/aaa".to_string(), [200i64].iter().copied().collect());

        let mut dir_file_counts = HashMap::new();
        dir_file_counts.insert("/music/zzz".to_string(), 1);
        dir_file_counts.insert("/music/aaa".to_string(), 1);

        let media = vec![];

        let first = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );
        for _ in 0..100 {
            let (target, pairs) = score_all_directories(
                &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
            );
            match &target {
                TargetDirs::Single(d) => assert_eq!(d, "/music/aaa"),
                _ => panic!("Expected Single target dir"),
            }
            assert_eq!(pairs, first.1);
        }
    }

    #[test]
    fn test_directory_scoring_empty_input() {
        let (target, pairs) = score_all_directories(
            &[], &HashMap::new(), &HashMap::new(), &[], &HashMap::new(),
        );
        assert!(matches!(target, TargetDirs::Empty));
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_directory_scoring_coverage_over_snugness() {
        // Glitch Mob scenario: 11-track release, two candidate directories.
        // Dir A: 10 files, 10 match recordings on the release (0 leftovers)
        // Dir B: 23 files, 11 match all 11 tracks on the release (12 leftovers)
        // Dir B should win: full coverage (11/11) beats snug fit (10/11).
        let mut candidates = Vec::new();
        let mut corpus_info = HashMap::new();

        // Dir A: 10 files matching tracks 1-3,5-11 (missing track 4)
        for (i, track) in [1, 2, 3, 5, 6, 7, 8, 9, 10, 11].iter().enumerate() {
            let inode = 1000 + i as i64;
            candidates.push(make_candidate(inode, 1, *track, 0.85));
            corpus_info.insert(inode, CorpusFileInfo {
                parent_dir: "/corpus/see-without-eyes".to_string(),
                tags: TagSet::empty(),
                duration_ms: None,
            });
        }

        // Dir B: 23 files, 11 of which match all 11 tracks on this release
        for track in 1..=11 {
            let inode = 2000 + track as i64;
            candidates.push(make_candidate(inode, 1, track, 0.85));
            corpus_info.insert(inode, CorpusFileInfo {
                parent_dir: "/corpus/see-without-eyes-deluxe".to_string(),
                tags: TagSet::empty(),
                duration_ms: None,
            });
        }

        let mut dir_inodes = HashMap::new();
        dir_inodes.insert(
            "/corpus/see-without-eyes".to_string(),
            (1000..1010).collect(),
        );
        dir_inodes.insert(
            "/corpus/see-without-eyes-deluxe".to_string(),
            (2001..2012).collect(),
        );

        let mut dir_file_counts = HashMap::new();
        dir_file_counts.insert("/corpus/see-without-eyes".to_string(), 10);
        dir_file_counts.insert("/corpus/see-without-eyes-deluxe".to_string(), 23);

        let media = vec![];

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Dir B wins: 11 assigned (full coverage) beats Dir A: 10 assigned.
        // Snugness only breaks ties at equal coverage.
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/corpus/see-without-eyes-deluxe"),
            _ => panic!("Expected Single target dir, got {:?}", target),
        }
        assert_eq!(pairs.len(), 11);
    }

    #[test]
    fn test_directory_scoring_snugness_breaks_coverage_tie() {
        // Two directories both fill all 3 slots, but Dir A has 3 files (snug)
        // while Dir B has 5 files (2 leftovers). Dir A should win on snugness.
        let candidates = vec![
            make_candidate(100, 1, 1, 0.9),
            make_candidate(101, 1, 2, 0.9),
            make_candidate(102, 1, 3, 0.9),
            make_candidate(200, 1, 1, 0.9),
            make_candidate(201, 1, 2, 0.9),
            make_candidate(202, 1, 3, 0.9),
        ];

        let mut corpus_info = HashMap::new();
        for i in 100..=102 {
            corpus_info.insert(i, CorpusFileInfo {
                parent_dir: "/music/snug".to_string(),
                tags: TagSet::empty(),
                duration_ms: None,
            });
        }
        for i in 200..=202 {
            corpus_info.insert(i, CorpusFileInfo {
                parent_dir: "/music/loose".to_string(),
                tags: TagSet::empty(),
                duration_ms: None,
            });
        }

        let mut dir_inodes = HashMap::new();
        dir_inodes.insert("/music/snug".to_string(), [100i64, 101, 102].iter().copied().collect());
        dir_inodes.insert("/music/loose".to_string(), [200i64, 201, 202].iter().copied().collect());

        let mut dir_file_counts = HashMap::new();
        dir_file_counts.insert("/music/snug".to_string(), 3);
        dir_file_counts.insert("/music/loose".to_string(), 5);

        let media = vec![];

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Both fill 3 slots. Dir A wins on snugness: 0 leftovers < 2 leftovers.
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/snug"),
            _ => panic!("Expected Single target dir"),
        }
        assert_eq!(pairs.len(), 3);
    }

    // ====================================================================
    // Scenario tests: real-world-inspired multi-medium and single-dir cases
    // ====================================================================

    /// Helper: build candidates + corpus_info for files in a single directory.
    /// Returns (candidates, corpus_info entries) for the given tracks.
    fn dir_candidates(
        dir: &str,
        inode_start: i64,
        tracks: &[(u32, u32)], // (medium_pos, track_pos)
        score: f64,
    ) -> (Vec<CandidateAssignment>, Vec<(i64, CorpusFileInfo)>) {
        let mut cands = Vec::new();
        let mut infos = Vec::new();
        for (i, &(medium, track)) in tracks.iter().enumerate() {
            let inode = inode_start + i as i64;
            cands.push(make_candidate(inode, medium, track, score));
            infos.push((inode, CorpusFileInfo {
                parent_dir: dir.to_string(),
                tags: TagSet::empty(),
                duration_ms: None,
            }));
        }
        (cands, infos)
    }

    fn make_media(track_counts: &[usize]) -> Vec<musicbrainz::MbMedium> {
        track_counts.iter().enumerate().map(|(i, &count)| {
            musicbrainz::MbMedium {
                position: (i + 1) as u32,
                format: Some("CD".to_string()),
                tracks: (1..=count).map(|t| musicbrainz::MbTrack {
                    id: String::new(),
                    position: t as u32,
                    number: t.to_string(),
                    title: format!("Track {}", t),
                    length: Some(240000),
                    recording: musicbrainz::MbTrackRecording {
                        id: format!("rec-{}-{}", i + 1, t),
                        title: format!("Track {}", t),
                        length: Some(240000),
                    },
                }).collect(),
                data_tracks: Vec::new(),
            }
        }).collect()
    }

    #[test]
    fn test_portal2_multi_chapter_single_dir() {
        // Portal 2 soundtrack: 3 "volumes" (media) on MusicBrainz, but all
        // files live in a single directory. Should pack as Single target dir
        // with full coverage — the sibling mapping should NOT win here because
        // there are no sibling dirs to split across.
        let vol1: Vec<(u32, u32)> = (1..=22).map(|t| (1, t)).collect();
        let vol2: Vec<(u32, u32)> = (1..=22).map(|t| (2, t)).collect();
        let vol3: Vec<(u32, u32)> = (1..=23).map(|t| (3, t)).collect();

        let mut all_tracks = Vec::new();
        all_tracks.extend_from_slice(&vol1);
        all_tracks.extend_from_slice(&vol2);
        all_tracks.extend_from_slice(&vol3);

        let (candidates, infos) = dir_candidates(
            "/music/Portal 2 OST",
            1000,
            &all_tracks,
            0.92,
        );

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();
        let total_files = all_tracks.len(); // 67 files, all matched

        let dir_inodes: HashMap<String, HashSet<i64>> = [(
            "/music/Portal 2 OST".to_string(),
            (1000..1000 + total_files as i64).collect(),
        )].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [(
            "/music/Portal 2 OST".to_string(),
            total_files,
        )].into_iter().collect();

        let media = make_media(&[22, 22, 23]); // 3 volumes

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Single dir should win — it covers all 67 tracks
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/Portal 2 OST"),
            other => panic!("Expected Single target dir, got {:?}", other),
        }
        assert_eq!(pairs.len(), 67);
    }

    #[test]
    fn test_multi_disc_sibling_dirs() {
        // Classic 2-CD album: CD1 in disc1/, CD2 in disc2/, both under same parent.
        // Should pick the PerMedium sibling mapping over any single dir.
        let cd1_tracks: Vec<(u32, u32)> = (1..=12).map(|t| (1, t)).collect();
        let cd2_tracks: Vec<(u32, u32)> = (1..=10).map(|t| (2, t)).collect();

        let (mut candidates, mut infos) = dir_candidates(
            "/music/album/disc1", 1000, &cd1_tracks, 0.95,
        );
        let (cands2, infos2) = dir_candidates(
            "/music/album/disc2", 2000, &cd2_tracks, 0.93,
        );
        candidates.extend(cands2);
        infos.extend(infos2);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/album/disc1".to_string(), (1000..1012).collect()),
            ("/music/album/disc2".to_string(), (2000..2010).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/album/disc1".to_string(), 12),
            ("/music/album/disc2".to_string(), 10),
        ].into_iter().collect();

        let media = make_media(&[12, 10]);

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Sibling mapping should win with full 22-track coverage
        match &target {
            TargetDirs::PerMedium(mapping) => {
                assert_eq!(mapping.get(&1).unwrap(), "/music/album/disc1");
                assert_eq!(mapping.get(&2).unwrap(), "/music/album/disc2");
            }
            other => panic!("Expected PerMedium target, got {:?}", other),
        }
        assert_eq!(pairs.len(), 22);
    }

    #[test]
    fn test_multi_disc_sibling_beats_single_dir_partial() {
        // 2-CD release (12+10 = 22 tracks). Dir "complete" has 20 files from both
        // discs mixed together (covers 20/22). Sibling dirs disc1+disc2 cover all 22.
        // Sibling should win on coverage even though "complete" has fewer leftovers.
        let cd1_tracks: Vec<(u32, u32)> = (1..=12).map(|t| (1, t)).collect();
        let cd2_tracks: Vec<(u32, u32)> = (1..=10).map(|t| (2, t)).collect();

        // Sibling dirs with full coverage
        let (mut candidates, mut infos) = dir_candidates(
            "/music/album/disc1", 1000, &cd1_tracks, 0.90,
        );
        let (cands2, infos2) = dir_candidates(
            "/music/album/disc2", 2000, &cd2_tracks, 0.90,
        );
        candidates.extend(cands2);
        infos.extend(infos2);

        // "complete" dir: 20 files covering 20/22 tracks (missing medium 2 tracks 9-10)
        let mut mixed: Vec<(u32, u32)> = (1..=12).map(|t| (1, t)).collect();
        mixed.extend((1..=8).map(|t| (2, t)));
        let (cands3, infos3) = dir_candidates(
            "/music/album-complete", 3000, &mixed, 0.90,
        );
        candidates.extend(cands3);
        infos.extend(infos3);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/album/disc1".to_string(), (1000..1012).collect()),
            ("/music/album/disc2".to_string(), (2000..2010).collect()),
            ("/music/album-complete".to_string(), (3000..3020).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/album/disc1".to_string(), 12),
            ("/music/album/disc2".to_string(), 10),
            ("/music/album-complete".to_string(), 20),
        ].into_iter().collect();

        let media = make_media(&[12, 10]);

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Siblings cover 22 tracks vs "complete" covers 20 → siblings win on coverage
        match &target {
            TargetDirs::PerMedium(mapping) => {
                assert_eq!(mapping.len(), 2);
            }
            other => panic!("Expected PerMedium target, got {:?}", other),
        }
        assert_eq!(pairs.len(), 22);
    }

    #[test]
    fn test_standard_vs_deluxe_same_album() {
        // 10-track standard edition in a 10-file dir, same release also has
        // candidates from a 15-file deluxe dir (which only matches 10/10 tracks
        // since extra files are bonus content not on this release).
        // Standard dir should win: same coverage (10), fewer leftovers (0 vs 5).
        let tracks: Vec<(u32, u32)> = (1..=10).map(|t| (1, t)).collect();

        let (mut candidates, mut infos) = dir_candidates(
            "/music/album-standard", 1000, &tracks, 0.92,
        );
        let (cands2, infos2) = dir_candidates(
            "/music/album-deluxe", 2000, &tracks, 0.92,
        );
        candidates.extend(cands2);
        infos.extend(infos2);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/album-standard".to_string(), (1000..1010).collect()),
            ("/music/album-deluxe".to_string(), (2000..2010).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/album-standard".to_string(), 10),
            ("/music/album-deluxe".to_string(), 15), // 5 bonus tracks
        ].into_iter().collect();

        let media = vec![];

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Both fill 10 slots, but standard has 0 leftovers vs deluxe 5
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/album-standard"),
            other => panic!("Expected Single target dir, got {:?}", other),
        }
        assert_eq!(pairs.len(), 10);
    }

    #[test]
    fn test_jazz_reissue_coverage_wins() {
        // Jazz album: MB has 8-track original and 12-track reissue.
        // User has the 12-track reissue in a 12-file dir.
        // For the 12-track MB release, dir with 12 files should pack perfectly.
        // For the 8-track MB release, dir with 12 files should still win (8 assigned,
        // 4 leftovers) over any hypothetical tighter dir.
        let tracks_12: Vec<(u32, u32)> = (1..=12).map(|t| (1, t)).collect();

        let (candidates, infos) = dir_candidates(
            "/jazz/coltrane-reissue", 1000, &tracks_12, 0.88,
        );
        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [(
            "/jazz/coltrane-reissue".to_string(),
            (1000..1012).collect(),
        )].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [(
            "/jazz/coltrane-reissue".to_string(),
            12,
        )].into_iter().collect();

        let media = vec![];

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/jazz/coltrane-reissue"),
            other => panic!("Expected Single target dir, got {:?}", other),
        }
        assert_eq!(pairs.len(), 12);
    }

    #[test]
    fn test_dir_with_extra_files_still_wins_on_coverage() {
        // Dir A has 15 audio files, 8 match this release's 8 tracks (7 leftovers).
        // Dir B has 5 audio files, 5 match 5 of the 8 tracks (0 leftovers).
        // Dir A should win: 8 assigned > 5 assigned.
        let all_8: Vec<(u32, u32)> = (1..=8).map(|t| (1, t)).collect();
        let first_5: Vec<(u32, u32)> = (1..=5).map(|t| (1, t)).collect();

        let (mut candidates, mut infos) = dir_candidates(
            "/music/big-dir", 1000, &all_8, 0.85,
        );
        let (cands2, infos2) = dir_candidates(
            "/music/small-dir", 2000, &first_5, 0.85,
        );
        candidates.extend(cands2);
        infos.extend(infos2);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/big-dir".to_string(), (1000..1008).collect()),
            ("/music/small-dir".to_string(), (2000..2005).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/big-dir".to_string(), 15),
            ("/music/small-dir".to_string(), 5),
        ].into_iter().collect();

        let media = vec![];

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Coverage wins: 8 > 5
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/big-dir"),
            other => panic!("Expected Single target dir, got {:?}", other),
        }
        assert_eq!(pairs.len(), 8);
    }

    #[test]
    fn test_multi_chapter_single_dir_beats_scattered_siblings() {
        // Multi-chapter release (3 media, 5+5+5 tracks). All 15 files live in
        // one dir, but a sibling group of 2 dirs under a different parent also
        // has some candidates (7 total across 2 dirs for medium 1 and 2 only).
        // The single dir covering all 15 should win over the partial sibling group.
        let all_tracks: Vec<(u32, u32)> = (1..=3).flat_map(|m| {
            (1..=5).map(move |t| (m, t))
        }).collect();

        let (mut candidates, mut infos) = dir_candidates(
            "/music/full-ost", 1000, &all_tracks, 0.90,
        );

        // Partial sibling: dir_a has medium 1 tracks 1-4, dir_b has medium 2 tracks 1-3
        let (cands_a, infos_a) = dir_candidates(
            "/other/part/a", 2000, &[(1,1),(1,2),(1,3),(1,4)], 0.90,
        );
        let (cands_b, infos_b) = dir_candidates(
            "/other/part/b", 3000, &[(2,1),(2,2),(2,3)], 0.90,
        );
        candidates.extend(cands_a);
        candidates.extend(cands_b);
        infos.extend(infos_a);
        infos.extend(infos_b);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/full-ost".to_string(), (1000..1015).collect()),
            ("/other/part/a".to_string(), (2000..2004).collect()),
            ("/other/part/b".to_string(), (3000..3003).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/full-ost".to_string(), 15),
            ("/other/part/a".to_string(), 4),
            ("/other/part/b".to_string(), 3),
        ].into_iter().collect();

        let media = make_media(&[5, 5, 5]);

        let (target, pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        // Single dir with 15/15 coverage wins over sibling group with 7/15
        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/full-ost"),
            other => panic!("Expected Single target, got {:?}", other),
        }
        assert_eq!(pairs.len(), 15);
    }

    #[test]
    fn test_score_tiebreak_at_equal_coverage_and_leftovers() {
        // Two dirs, both fill 5 slots, both have 5 files (0 leftovers).
        // Dir A has higher total score → Dir A wins.
        let tracks: Vec<(u32, u32)> = (1..=5).map(|t| (1, t)).collect();

        let (mut candidates, mut infos) = dir_candidates(
            "/music/high-score", 1000, &tracks, 0.95,
        );
        let (cands2, infos2) = dir_candidates(
            "/music/low-score", 2000, &tracks, 0.70,
        );
        candidates.extend(cands2);
        infos.extend(infos2);

        let corpus_info: HashMap<i64, CorpusFileInfo> = infos.into_iter().collect();

        let dir_inodes: HashMap<String, HashSet<i64>> = [
            ("/music/high-score".to_string(), (1000..1005).collect()),
            ("/music/low-score".to_string(), (2000..2005).collect()),
        ].into_iter().collect();

        let dir_file_counts: HashMap<String, usize> = [
            ("/music/high-score".to_string(), 5),
            ("/music/low-score".to_string(), 5),
        ].into_iter().collect();

        let media = vec![];

        let (target, _pairs) = score_all_directories(
            &candidates, &corpus_info, &dir_inodes, &media, &dir_file_counts,
        );

        match &target {
            TargetDirs::Single(d) => assert_eq!(d, "/music/high-score"),
            other => panic!("Expected Single target, got {:?}", other),
        }
    }
}
