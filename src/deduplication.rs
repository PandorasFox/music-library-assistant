//! Deduplication Module
//!
//! Provides fingerprint-based deduplication for surgical duplicate removal.
//!
//! NOTE: Core functions implemented but UI integration pending.
//! See docs/FUTURE_FEATURES.md and FINGERPRINT_DEDUP_INTEGRATION.md.

#![allow(dead_code)]

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;
use crate::db::{ChangeStatus, ChangeType, Database, PendingChange, Track};

#[derive(Debug, Clone)]
pub struct ConflictSet {
    pub fingerprint: String,
    pub conflict_dirs: Vec<String>, // ["playlist-1", "playlist-2"]
    pub tracks_by_dir: HashMap<String, Vec<Track>>,
    pub match_score: f64, // Always 100.0 for exact fingerprint match
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub total_sets: usize,
    pub resolved_count: usize,
    pub skipped_count: usize,
    pub files_moved: usize,
    pub files_kept: usize,
}

#[derive(Debug, Clone)]
pub struct DirectoryCluster {
    pub directories: Vec<String>,           // Directory names in this cluster
    pub shared_fingerprints: Vec<String>,   // Fingerprints shared by these dirs
    pub track_count: usize,                 // Total tracks across all dirs in cluster
    pub example_track: String,              // Sample filename for display
}

/// Tracks skip patterns and manages auto-ignore functionality
/// After 5 consecutive skips of a directory, prompts user to auto-ignore it
#[derive(Debug, Clone)]
pub struct AutoIgnoreState {
    skip_counts: HashMap<String, usize>, // dir -> consecutive skip count
    ignored_dirs: HashSet<String>,       // Directories to auto-skip
}

impl AutoIgnoreState {
    /// Create a new auto-ignore state tracker
    pub fn new() -> Self {
        Self {
            skip_counts: HashMap::new(),
            ignored_dirs: HashSet::new(),
        }
    }

    /// Record a skip for a directory
    /// Returns: true if should prompt user about auto-ignore (hit 5-skip threshold)
    pub fn record_skip(&mut self, dir: &str) -> bool {
        let count = self.skip_counts.entry(dir.to_string()).or_insert(0);
        *count += 1;
        *count == 5 // Prompt on 5th consecutive skip
    }

    /// Record a win (user picked this directory) - resets skip counter
    pub fn record_win(&mut self, dir: &str) {
        self.skip_counts.remove(dir);
    }

    /// Add directory to ignore list
    pub fn add_ignored(&mut self, dir: String) {
        self.ignored_dirs.insert(dir);
    }

    /// Check if directory should be auto-ignored
    pub fn should_ignore(&self, dir: &str) -> bool {
        self.ignored_dirs.contains(dir)
    }

    /// Check if entire cluster should be skipped (all dirs ignored)
    pub fn should_skip_cluster(&self, cluster_dirs: &[String]) -> bool {
        cluster_dirs.iter().all(|d| self.should_ignore(d))
    }

    /// Get list of currently ignored directories (for display)
    pub fn get_ignored_dirs(&self) -> Vec<String> {
        self.ignored_dirs.iter().cloned().collect()
    }
}

impl Default for AutoIgnoreState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Directory Set Clustering (Non-Transitive)
// ============================================================================

/// A cluster of duplicates sharing EXACTLY this set of directories.
/// Unlike DirectoryCluster (which uses transitive BFS), this groups conflicts
/// by their exact directory set - duplicates in {A, B} are kept separate from
/// those in {A, B, C}.
#[derive(Debug, Clone)]
pub struct DirectorySetCluster {
    /// Sorted directory names (e.g., ["Tracks-DaB", "Tracks-trans"])
    pub directory_set: Vec<String>,
    /// Fingerprints that exist in EXACTLY these directories
    pub fingerprints: Vec<String>,
    /// Total file count across all directories
    pub file_count: usize,
    /// Number of directories (cluster magnitude)
    pub magnitude: usize,
}

/// Decision state for a single cluster
#[derive(Debug, Clone)]
pub struct ClusterDecision {
    /// The cluster this decision is for
    pub cluster: DirectorySetCluster,
    /// Directory chosen as keeper (None = not decided yet)
    pub keeper_dir: Option<String>,
    /// Generated pending changes (populated after decision)
    pub pending_changes: Vec<PendingChange>,
}

/// Session state for the deduplication workflow
#[derive(Debug, Clone)]
pub struct DeduplicationSession {
    /// Change session ID for tracking in database
    pub session_id: String,
    /// Clusters sorted by magnitude desc, then file_count desc
    pub clusters: Vec<DirectorySetCluster>,
    /// Current cluster index being reviewed
    pub current_index: usize,
    /// Completed decisions
    pub decisions: Vec<ClusterDecision>,
    /// Whether bulk phase is complete (individual conflicts remain)
    pub bulk_phase_complete: bool,
    /// Common divergence root path (e.g., "web/rips/spotify/")
    pub divergence_root: String,
    /// Original conflict sets (needed for change generation)
    pub conflict_sets: Vec<ConflictSet>,
    /// Auto-ignore state
    pub auto_ignore: AutoIgnoreState,
}

impl DeduplicationSession {
    /// Get the current cluster being reviewed, if any
    pub fn current_cluster(&self) -> Option<&DirectorySetCluster> {
        self.clusters.get(self.current_index)
    }

    /// Check if all clusters have been processed
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.clusters.len()
    }

    /// Get total pending changes across all decisions
    pub fn total_pending_changes(&self) -> usize {
        self.decisions.iter()
            .map(|d| d.pending_changes.len())
            .sum()
    }

    /// Get count of files that will be kept
    pub fn files_to_keep(&self) -> usize {
        let mut kept = 0;
        for decision in &self.decisions {
            if let Some(keeper) = &decision.keeper_dir {
                // Count files in keeper directory for this cluster's conflicts
                for cs in &self.conflict_sets {
                    let mut cs_dirs: Vec<_> = cs.conflict_dirs.clone();
                    cs_dirs.sort();
                    if cs_dirs == decision.cluster.directory_set {
                        if let Some(tracks) = cs.tracks_by_dir.get(keeper) {
                            kept += tracks.len();
                        }
                    }
                }
            }
        }
        kept
    }

    /// Get statistics by keeper directory
    pub fn keeper_stats(&self) -> HashMap<String, usize> {
        let mut stats: HashMap<String, usize> = HashMap::new();
        for decision in &self.decisions {
            if let Some(keeper) = &decision.keeper_dir {
                // Count files kept in this directory
                for cs in &self.conflict_sets {
                    let mut cs_dirs: Vec<_> = cs.conflict_dirs.clone();
                    cs_dirs.sort();
                    if cs_dirs == decision.cluster.directory_set {
                        if let Some(tracks) = cs.tracks_by_dir.get(keeper) {
                            *stats.entry(keeper.clone()).or_default() += tracks.len();
                        }
                    }
                }
            }
        }
        stats
    }
}

/// Compute directory set clusters (NON-TRANSITIVE).
/// Groups conflicts by their EXACT directory set, not transitive closure.
///
/// For example, if you have:
/// - Track A with copies in {dir1, dir2}
/// - Track B with copies in {dir1, dir2, dir3}
///
/// These form TWO separate clusters, not one.
pub fn compute_directory_set_clusters(
    conflict_sets: &[ConflictSet],
) -> Vec<DirectorySetCluster> {
    // Group by sorted directory set
    let mut by_dir_set: HashMap<Vec<String>, Vec<&ConflictSet>> = HashMap::new();
    for cs in conflict_sets {
        let mut dir_set = cs.conflict_dirs.clone();
        dir_set.sort();
        by_dir_set.entry(dir_set).or_default().push(cs);
    }

    // Build and sort clusters
    let mut clusters: Vec<DirectorySetCluster> = by_dir_set
        .into_iter()
        .map(|(dir_set, conflicts)| {
            DirectorySetCluster {
                magnitude: dir_set.len(),
                fingerprints: conflicts.iter().map(|c| c.fingerprint.clone()).collect(),
                file_count: conflicts.iter()
                    .flat_map(|c| c.tracks_by_dir.values())
                    .map(|t| t.len())
                    .sum(),
                directory_set: dir_set,
            }
        })
        .collect();

    // Sort: magnitude DESC, then file_count DESC
    clusters.sort_by(|a, b| {
        b.magnitude.cmp(&a.magnitude)
            .then_with(|| b.file_count.cmp(&a.file_count))
    });

    clusters
}

/// Generate Delete pending changes for a cluster decision.
/// Creates a change for each file in non-keeper directories.
pub fn generate_cluster_changes(
    cluster: &DirectorySetCluster,
    keeper_dir: &str,
    conflict_sets: &[ConflictSet],
    corpus_root: &Path,
    lost_files_root: &Path,
    session_id: &str,
) -> Vec<PendingChange> {
    let mut changes = Vec::new();

    for cs in conflict_sets {
        // Only process conflicts matching EXACTLY this cluster's directory set
        let mut cs_dirs = cs.conflict_dirs.clone();
        cs_dirs.sort();
        if cs_dirs != cluster.directory_set {
            continue;
        }

        // Create Delete change for each non-keeper track
        for (dir, tracks) in &cs.tracks_by_dir {
            if dir == keeper_dir {
                continue; // Keep these
            }

            for track in tracks {
                let rel_path = Path::new(&track.path)
                    .strip_prefix(corpus_root)
                    .unwrap_or(Path::new(&track.path));
                let target = lost_files_root
                    .join("fingerprint-dupes")
                    .join(dir)
                    .join(rel_path);

                changes.push(PendingChange {
                    id: None,
                    session_id: session_id.to_string(),
                    change_type: ChangeType::Delete,
                    source_path: track.path.clone(),
                    target_path: Some(target.to_string_lossy().to_string()),
                    metadata_changes: None,
                    created_at: None,
                    status: ChangeStatus::Pending,
                });
            }
        }
    }

    changes
}

/// Check if remaining clusters are all 2-dir single-file conflicts.
/// Returns true when it's time to offer bulk review before individual resolution.
pub fn should_offer_bulk_review(
    clusters: &[DirectorySetCluster],
    current_index: usize,
) -> bool {
    if current_index >= clusters.len() {
        return false; // Already complete
    }

    clusters[current_index..].iter().all(|c| {
        // 2-directory cluster with exactly 2 files (1 per directory)
        c.magnitude == 2 && c.file_count == 2
    })
}

/// Find the common divergence root from all conflict sets.
/// This is the path prefix shared by all conflicting paths.
pub fn find_divergence_root(conflict_sets: &[ConflictSet]) -> String {
    if conflict_sets.is_empty() {
        return String::new();
    }

    // Collect all paths
    let all_paths: Vec<&str> = conflict_sets.iter()
        .flat_map(|cs| cs.tracks_by_dir.values())
        .flat_map(|tracks| tracks.iter())
        .map(|t| t.path.as_str())
        .collect();

    if all_paths.is_empty() {
        return String::new();
    }

    // Find longest common prefix
    let first = all_paths[0];
    let mut common_prefix_len = first.len();

    for path in &all_paths[1..] {
        let matching_len = first.chars()
            .zip(path.chars())
            .take_while(|(a, b)| a == b)
            .count();
        common_prefix_len = common_prefix_len.min(matching_len);
    }

    // Trim to last directory separator
    let prefix = &first[..common_prefix_len];
    if let Some(last_sep) = prefix.rfind('/') {
        prefix[..=last_sep].to_string()
    } else {
        String::new()
    }
}

// ============================================================================
// Original Directory Clustering (Transitive - for reference)
// ============================================================================

/// Compute non-transitive directory clusters from conflict sets
/// Returns clusters sorted by: directory count desc, then track count desc
///
/// A cluster is formed by directories that share duplicate fingerprints.
/// Uses BFS to find connected components in the directory overlap graph.
/// Non-transitive: only direct connections matter (A-B and B-C don't imply A-C).
pub fn compute_directory_clusters(
    conflict_sets: &[ConflictSet],
) -> Vec<DirectoryCluster> {
    // Step 1: Build adjacency map (dir -> set of other dirs it shares dupes with)
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();

    for conflict_set in conflict_sets {
        let dirs = &conflict_set.conflict_dirs;

        // Add each pair of directories as adjacent
        for i in 0..dirs.len() {
            for j in i+1..dirs.len() {
                adjacency.entry(dirs[i].clone())
                    .or_default()
                    .insert(dirs[j].clone());

                adjacency.entry(dirs[j].clone())
                    .or_default()
                    .insert(dirs[i].clone());
            }
        }
    }

    // Step 2: Find connected components (non-transitive clustering)
    let mut visited: HashSet<String> = HashSet::new();
    let mut clusters: Vec<DirectoryCluster> = Vec::new();

    for dir in adjacency.keys() {
        if visited.contains(dir) {
            continue;
        }

        // BFS to find all connected directories
        let mut cluster_dirs: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        queue.push_back(dir.clone());
        visited.insert(dir.clone());

        while let Some(current) = queue.pop_front() {
            cluster_dirs.insert(current.clone());

            if let Some(neighbors) = adjacency.get(&current) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        // Step 3: Find shared fingerprints for this cluster
        let mut shared_fingerprints = Vec::new();
        let mut track_count = 0;
        let mut example_track = None;

        for conflict_set in conflict_sets {
            // Check if this conflict_set involves any dirs in this cluster
            let involves_cluster = conflict_set.conflict_dirs.iter()
                .any(|d| cluster_dirs.contains(d));

            if involves_cluster {
                shared_fingerprints.push(conflict_set.fingerprint.clone());

                // Count tracks
                for (dir, tracks) in &conflict_set.tracks_by_dir {
                    if cluster_dirs.contains(dir) {
                        track_count += tracks.len();

                        if example_track.is_none() && !tracks.is_empty() {
                            example_track = Some(
                                tracks[0].path
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or("unknown")
                                    .to_string()
                            );
                        }
                    }
                }
            }
        }

        clusters.push(DirectoryCluster {
            directories: cluster_dirs.into_iter().collect(),
            shared_fingerprints,
            track_count,
            example_track: example_track.unwrap_or_else(|| "unknown".to_string()),
        });
    }

    // Step 4: Sort clusters by directory count desc, then track count desc
    clusters.sort_by(|a, b| {
        match b.directories.len().cmp(&a.directories.len()) {
            std::cmp::Ordering::Equal => b.track_count.cmp(&a.track_count),
            other => other,
        }
    });

    clusters
}

/// Find all fingerprint duplicates in selected directories
/// Groups duplicates by the first differing directory component in their paths
pub fn find_fingerprint_duplicates(
    db: &Database,
    directory_paths: &[PathBuf],
    corpus_root: &Path,
) -> Result<Vec<ConflictSet>> {
    // 1. Query tracks with fingerprints in selected directories
    let tracks = db.get_tracks_by_paths(directory_paths, "corpus")?;

    if tracks.is_empty() {
        return Ok(Vec::new());
    }

    // 2. Group by exact fingerprint string match
    let mut fp_groups: HashMap<String, Vec<Track>> = HashMap::new();
    for track in tracks {
        if let Some(fp) = &track.fingerprint {
            if !fp.is_empty() {
                fp_groups.entry(fp.clone()).or_default().push(track);
            }
        }
    }

    // 3. Filter to groups with 2+ tracks and build conflict sets
    let mut conflict_sets = Vec::new();

    for (fp, tracks) in fp_groups {
        if tracks.len() < 2 {
            continue; // Not a duplicate
        }

        // 4. Find path divergence index for each group
        let paths: Vec<PathBuf> = tracks.iter().map(|t| PathBuf::from(&t.path)).collect();

        let divergence_idx = match find_path_divergence(&paths) {
            Ok(idx) => idx,
            Err(_) => {
                // If we can't find divergence, skip this group
                config::log_message(&format!(
                    "Could not determine path divergence for fingerprint group with {} tracks",
                    tracks.len()
                ))?;
                continue;
            }
        };

        // 5. Group tracks by conflict key (differing directory name)
        let mut tracks_by_dir: HashMap<String, Vec<Track>> = HashMap::new();
        for track in tracks {
            let conflict_key =
                extract_conflict_key(Path::new(&track.path), corpus_root, divergence_idx);
            tracks_by_dir.entry(conflict_key).or_default().push(track);
        }

        // 6. Only include if 2+ conflict directories
        if tracks_by_dir.len() < 2 {
            continue; // All tracks in same directory - not a cross-directory duplicate
        }

        let conflict_dirs: Vec<String> = tracks_by_dir.keys().cloned().collect();

        conflict_sets.push(ConflictSet {
            fingerprint: fp,
            conflict_dirs,
            tracks_by_dir,
            match_score: 100.0, // Exact fingerprint match
        });
    }

    // 7. Sort by conflict count (desc), then by total files (desc)
    conflict_sets.sort_by(|a, b| {
        b.conflict_dirs
            .len()
            .cmp(&a.conflict_dirs.len())
            .then_with(|| {
                let a_total: usize = a.tracks_by_dir.values().map(|v| v.len()).sum();
                let b_total: usize = b.tracks_by_dir.values().map(|v| v.len()).sum();
                b_total.cmp(&a_total)
            })
    });

    Ok(conflict_sets)
}

/// Find where paths diverge (first component index where paths differ)
/// Example:
///   /corpus/playlist-1/artist/album/track.flac
///   /corpus/playlist-2/artist/album/track.flac
/// Returns: 2 (index of "playlist-1" vs "playlist-2")
fn find_path_divergence(paths: &[PathBuf]) -> Result<usize> {
    if paths.is_empty() {
        anyhow::bail!("Empty paths vector");
    }

    if paths.len() == 1 {
        anyhow::bail!("Only one path provided");
    }

    let components: Vec<Vec<_>> = paths.iter().map(|p| p.components().collect()).collect();

    let max_len = components.iter().map(|c| c.len()).max().unwrap_or(0);

    for idx in 0..max_len {
        let first_component = components.first().and_then(|c| c.get(idx));

        // Check if all paths have same component at this index
        let all_same = components.iter().all(|c| c.get(idx) == first_component);

        if !all_same {
            return Ok(idx);
        }
    }

    // All paths identical (shouldn't happen in practice, but handle gracefully)
    anyhow::bail!("All paths are identical");
}

/// Extract the differing directory component for grouping
/// Returns the directory name at the divergence index
fn extract_conflict_key(path: &Path, corpus_root: &Path, divergence_idx: usize) -> String {
    let relative = path.strip_prefix(corpus_root).unwrap_or(path);

    relative
        .components()
        .nth(divergence_idx)
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Auto-remove inferior bitrate duplicates
/// Returns: (tracks_moved_count, tracks_skipped_due_to_missing_bitrate)
///
/// This function processes fingerprint duplicates and automatically moves
/// lower-quality versions to lost+found. If any track in a fingerprint group
/// is missing bitrate data, the entire group is skipped and left for manual resolution.
pub fn auto_remove_inferior_bitrates(
    tracks: &[Track],
    corpus_root: &Path,
    lost_found_root: &Path,
) -> Result<(usize, usize)> {
    let mut moved_count = 0;
    let mut skipped_count = 0;

    // Group tracks by exact fingerprint
    let mut by_fingerprint: HashMap<String, Vec<&Track>> = HashMap::new();
    for track in tracks {
        if let Some(fp) = &track.fingerprint {
            if !fp.is_empty() {
                by_fingerprint.entry(fp.clone()).or_default().push(track);
            }
        }
    }

    // For each fingerprint group
    for (fingerprint, group) in by_fingerprint {
        if group.len() < 2 {
            continue; // No duplicates
        }

        // Check if all tracks have bitrate data
        let all_have_bitrate = group.iter().all(|t| t.bitrate_kbps.is_some());

        if !all_have_bitrate {
            // Skip this group - log it
            skipped_count += group.len() - 1;
            let track_paths: Vec<_> = group.iter().map(|t| &t.path).collect();
            config::log_message(&format!(
                "Bitrate auto-removal skipped (missing data): {} tracks with fingerprint {}... paths: {:?}",
                group.len(),
                &fingerprint[..20.min(fingerprint.len())],
                track_paths
            ))?;
            continue;
        }

        // Find highest bitrate
        let max_bitrate = group.iter()
            .filter_map(|t| t.bitrate_kbps)
            .max()
            .unwrap(); // Safe: we checked all_have_bitrate

        // Keep highest bitrate track(s), move rest to lost-files
        for track in group {
            if track.bitrate_kbps.unwrap() < max_bitrate {
                // Move to lost-files/fingerprint-dupes-auto/
                match move_to_lost_found_auto(track, corpus_root, lost_found_root) {
                    Ok(_) => {
                        moved_count += 1;
                        config::log_message(&format!(
                            "Auto-removed inferior bitrate: {} ({}kbps < {}kbps max)",
                            track.path,
                            track.bitrate_kbps.unwrap(),
                            max_bitrate
                        ))?;
                    }
                    Err(e) => {
                        config::log_message(&format!(
                            "Failed to auto-remove {}: {}",
                            track.path, e
                        ))?;
                        // Continue with other files even if one fails
                    }
                }
            }
        }
    }

    Ok((moved_count, skipped_count))
}

/// Move file to lost-files/fingerprint-dupes-auto/ preserving structure
/// Used for automatic inferior-bitrate removal
fn move_to_lost_found_auto(
    track: &Track,
    corpus_root: &Path,
    lost_found_root: &Path,
) -> Result<()> {
    let source_path = Path::new(&track.path);

    // Compute relative path from corpus root
    let relative_path = source_path.strip_prefix(corpus_root)
        .context("Track path not within corpus root")?;

    // Target: lost-files/fingerprint-dupes-auto/<relative-path>
    let target_dir = lost_found_root.join("fingerprint-dupes-auto");
    let target_path = target_dir.join(relative_path);

    // Create parent directories
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create auto-removal directory")?;
    }

    // Try rename first (fast), fallback to copy+delete
    fs::rename(source_path, &target_path).or_else(|_| {
        fs::copy(source_path, &target_path)
            .context("Failed to copy file")?;
        fs::remove_file(source_path)
            .context("Failed to remove original file")?;
        Ok(())
    })
}

/// Resolve a conflict by moving losers to lost+found
/// Returns statistics about the operation
pub fn resolve_conflict_set(
    conflict_set: &ConflictSet,
    winner_dir: &str,
    corpus_root: &Path,
    lost_found_root: &Path,
) -> Result<SessionStats> {
    let mut stats = SessionStats {
        total_sets: 1,
        resolved_count: 1,
        ..Default::default()
    };

    // Keep winner files
    if let Some(winner_tracks) = conflict_set.tracks_by_dir.get(winner_dir) {
        stats.files_kept += winner_tracks.len();
    }

    // Move loser files
    for (dir, tracks) in &conflict_set.tracks_by_dir {
        if dir == winner_dir {
            continue; // Skip winner
        }

        for track in tracks {
            match move_to_lost_found(track, dir, corpus_root, lost_found_root) {
                Ok(_) => {
                    stats.files_moved += 1;
                    config::log_message(&format!(
                        "Moved duplicate: {} -> lost+found/fingerprint-dupes/{}/",
                        track.path, dir
                    ))?;
                }
                Err(e) => {
                    config::log_message(&format!("Failed to move {}: {}", track.path, e))?;
                    // Continue with other files even if one fails
                }
            }
        }
    }

    Ok(stats)
}

/// Move file to lost+found preserving directory structure
/// Target path: lost+found/fingerprint-dupes/<conflict_dir>/<relative_path>
fn move_to_lost_found(
    track: &Track,
    conflict_dir: &str,
    corpus_root: &Path,
    lost_found_root: &Path,
) -> Result<()> {
    let relative_path = Path::new(&track.path)
        .strip_prefix(corpus_root)
        .context("Track path not in corpus")?;

    let target_path = lost_found_root
        .join("fingerprint-dupes")
        .join(conflict_dir)
        .join(relative_path);

    // Create parent directories
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).context("Failed to create lost+found directories")?;
    }

    // Move file (try rename first, fall back to copy+delete for cross-filesystem)
    fs::rename(&track.path, &target_path).or_else(|_| {
        fs::copy(&track.path, &target_path).context("Failed to copy file")?;
        fs::remove_file(&track.path).context("Failed to remove original file")?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_path_divergence() {
        let paths = vec![
            PathBuf::from("/corpus/playlist-1/artist/album/track.flac"),
            PathBuf::from("/corpus/playlist-2/artist/album/track.flac"),
        ];
        assert_eq!(find_path_divergence(&paths).unwrap(), 2);
    }

    #[test]
    fn test_find_path_divergence_triple() {
        let paths = vec![
            PathBuf::from("/corpus/playlist-a/artist/album/track.flac"),
            PathBuf::from("/corpus/playlist-b/artist/album/track.flac"),
            PathBuf::from("/corpus/playlist-c/artist/album/track.flac"),
        ];
        assert_eq!(find_path_divergence(&paths).unwrap(), 2);
    }

    #[test]
    fn test_extract_conflict_key() {
        let path = PathBuf::from("/corpus/playlist-winter/artist/album/track.flac");
        let corpus_root = PathBuf::from("/corpus");
        let key = extract_conflict_key(&path, &corpus_root, 0);
        assert_eq!(key, "playlist-winter");
    }

    #[test]
    fn test_extract_conflict_key_nested() {
        let path = PathBuf::from("/corpus/collection/playlist-winter/artist/album/track.flac");
        let corpus_root = PathBuf::from("/corpus");
        let key = extract_conflict_key(&path, &corpus_root, 1);
        assert_eq!(key, "playlist-winter");
    }

    // Tests for directory set clustering

    fn make_conflict_set(fingerprint: &str, dirs: &[&str]) -> ConflictSet {
        let mut tracks_by_dir = HashMap::new();
        for dir in dirs {
            tracks_by_dir.insert(
                dir.to_string(),
                vec![Track {
                    id: None,
                    path: format!("/corpus/{}/artist/track.flac", dir),
                    source: "corpus".to_string(),
                    inode: 12345,
                    file_size: 1000000,
                    file_type: "flac".to_string(),
                    artist: Some("Artist".to_string()),
                    album: Some("Album".to_string()),
                    album_artist: None,
                    title: Some("Track".to_string()),
                    track_number: Some(1),
                    duration_ms: Some(180000),
                    bitrate_kbps: Some(320),
                    sample_rate: Some(44100),
                    fingerprint: Some(fingerprint.to_string()),
                    isrc: None,
                }],
            );
        }
        ConflictSet {
            fingerprint: fingerprint.to_string(),
            conflict_dirs: dirs.iter().map(|s| s.to_string()).collect(),
            tracks_by_dir,
            match_score: 100.0,
        }
    }

    #[test]
    fn test_compute_directory_set_clusters_basic() {
        let conflicts = vec![
            make_conflict_set("fp1", &["dir_a", "dir_b"]),
            make_conflict_set("fp2", &["dir_a", "dir_b"]),
        ];

        let clusters = compute_directory_set_clusters(&conflicts);

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].magnitude, 2);
        assert_eq!(clusters[0].fingerprints.len(), 2);
        assert_eq!(clusters[0].file_count, 4); // 2 dirs * 2 conflicts
    }

    #[test]
    fn test_compute_directory_set_clusters_non_transitive() {
        // {A, B} and {B, C} should be SEPARATE clusters
        let conflicts = vec![
            make_conflict_set("fp1", &["dir_a", "dir_b"]),
            make_conflict_set("fp2", &["dir_b", "dir_c"]),
        ];

        let clusters = compute_directory_set_clusters(&conflicts);

        assert_eq!(clusters.len(), 2); // Two separate clusters
        assert!(clusters.iter().all(|c| c.magnitude == 2));
    }

    #[test]
    fn test_compute_directory_set_clusters_ordering() {
        // 3-way cluster should come before 2-way clusters
        let conflicts = vec![
            make_conflict_set("fp1", &["a", "b"]),
            make_conflict_set("fp2", &["x", "y", "z"]), // 3-way
            make_conflict_set("fp3", &["a", "b"]),      // same as fp1
        ];

        let clusters = compute_directory_set_clusters(&conflicts);

        assert_eq!(clusters.len(), 2);
        // 3-way cluster first
        assert_eq!(clusters[0].magnitude, 3);
        assert_eq!(clusters[0].directory_set, vec!["x", "y", "z"]);
        // 2-way cluster second
        assert_eq!(clusters[1].magnitude, 2);
    }

    #[test]
    fn test_should_offer_bulk_review_not_yet() {
        let clusters = vec![
            DirectorySetCluster {
                directory_set: vec!["a".into(), "b".into(), "c".into()],
                fingerprints: vec!["fp1".into()],
                file_count: 3,
                magnitude: 3,
            },
            DirectorySetCluster {
                directory_set: vec!["a".into(), "b".into()],
                fingerprints: vec!["fp2".into()],
                file_count: 2,
                magnitude: 2,
            },
        ];

        // At index 0, not all remaining are 2-dir single-file
        assert!(!should_offer_bulk_review(&clusters, 0));
        // At index 1, the remaining is 2-dir with 2 files (1 per dir)
        assert!(should_offer_bulk_review(&clusters, 1));
    }

    #[test]
    fn test_should_offer_bulk_review_complete() {
        let clusters = vec![
            DirectorySetCluster {
                directory_set: vec!["a".into(), "b".into()],
                fingerprints: vec!["fp1".into()],
                file_count: 2,
                magnitude: 2,
            },
        ];

        // Past the end
        assert!(!should_offer_bulk_review(&clusters, 5));
    }

    #[test]
    fn test_generate_cluster_changes() {
        let conflicts = vec![make_conflict_set("fp1", &["keeper", "loser"])];
        let cluster = DirectorySetCluster {
            directory_set: vec!["keeper".into(), "loser".into()],
            fingerprints: vec!["fp1".into()],
            file_count: 2,
            magnitude: 2,
        };

        let changes = generate_cluster_changes(
            &cluster,
            "keeper",
            &conflicts,
            Path::new("/corpus"),
            Path::new("/lost-files"),
            "session-123",
        );

        assert_eq!(changes.len(), 1); // Only the loser file
        assert_eq!(changes[0].change_type, ChangeType::Delete);
        assert!(changes[0].source_path.contains("loser"));
        assert!(changes[0].target_path.as_ref().unwrap().contains("fingerprint-dupes"));
    }

    #[test]
    fn test_find_divergence_root() {
        let conflicts = vec![
            make_conflict_set("fp1", &["web/rips/spotify/Tracks-DaB", "web/rips/spotify/Tracks-trans"]),
        ];
        // Adjust paths to match the expected pattern
        let mut cs = conflicts[0].clone();
        cs.tracks_by_dir.clear();
        cs.tracks_by_dir.insert(
            "Tracks-DaB".to_string(),
            vec![Track {
                id: None,
                path: "/corpus/web/rips/spotify/Tracks-DaB/artist/track.flac".to_string(),
                source: "corpus".to_string(),
                inode: 12345,
                file_size: 1000000,
                file_type: "flac".to_string(),
                artist: None, album: None, album_artist: None, title: None,
                track_number: None, duration_ms: None, bitrate_kbps: None,
                sample_rate: None, fingerprint: None, isrc: None,
            }],
        );
        cs.tracks_by_dir.insert(
            "Tracks-trans".to_string(),
            vec![Track {
                id: None,
                path: "/corpus/web/rips/spotify/Tracks-trans/artist/track.flac".to_string(),
                source: "corpus".to_string(),
                inode: 12346,
                file_size: 1000000,
                file_type: "flac".to_string(),
                artist: None, album: None, album_artist: None, title: None,
                track_number: None, duration_ms: None, bitrate_kbps: None,
                sample_rate: None, fingerprint: None, isrc: None,
            }],
        );

        let root = find_divergence_root(&[cs]);
        assert_eq!(root, "/corpus/web/rips/spotify/");
    }
}
