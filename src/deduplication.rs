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
use crate::db::{Database, Track};

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
}
