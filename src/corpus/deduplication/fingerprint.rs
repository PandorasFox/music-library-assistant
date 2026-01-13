//! Fingerprint-based duplicate detection.
//!
//! Functions for finding and processing fingerprint duplicates.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;
use crate::corpus::db::{Database, Track};

use super::ConflictSet;

/// Find all fingerprint duplicates in selected directories.
///
/// Uses a TWO-PASS approach:
/// 1. First pass: Find the GLOBAL minimum divergence index (highest-level conflict point)
/// 2. Second pass: Build conflict sets using that SAME index for ALL tracks
///
/// This ensures high-level sweeping decisions like "Playlist-A vs Playlist-B"
/// rather than granular "album-A vs album-B" decisions.
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

    // 3. FIRST PASS: Find the GLOBAL minimum divergence index
    let mut min_divergence_idx: Option<usize> = None;
    for tracks in fp_groups.values() {
        if tracks.len() < 2 {
            continue;
        }
        let paths: Vec<PathBuf> = tracks.iter().map(|t| PathBuf::from(&t.path)).collect();
        if let Ok(idx) = find_path_divergence(&paths) {
            min_divergence_idx = Some(match min_divergence_idx {
                Some(current) => current.min(idx),
                None => idx,
            });
        }
    }

    let global_divergence_idx = match min_divergence_idx {
        Some(idx) => idx,
        None => return Ok(Vec::new()),
    };

    config::log_message(&format!(
        "Global divergence index: {} (relative to corpus root)",
        global_divergence_idx
    ))?;

    // 4. SECOND PASS: Build conflict sets using the GLOBAL divergence index
    let mut conflict_sets = Vec::new();

    for (fp, tracks) in fp_groups {
        if tracks.len() < 2 {
            continue;
        }

        let mut tracks_by_dir: HashMap<String, Vec<Track>> = HashMap::new();
        for track in tracks {
            let conflict_key =
                extract_conflict_dir(Path::new(&track.path), corpus_root, global_divergence_idx);
            tracks_by_dir.entry(conflict_key).or_default().push(track);
        }

        if tracks_by_dir.len() < 2 {
            continue;
        }

        let conflict_dirs: Vec<String> = tracks_by_dir.keys().cloned().collect();

        conflict_sets.push(ConflictSet {
            fingerprint: fp,
            conflict_dirs,
            tracks_by_dir,
            match_score: 100.0,
        });
    }

    // 5. Sort by conflict count desc, then total files desc
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

/// Extract the directory name at the divergence index.
///
/// For files:
///   /corpus/web/playlists/Playlist-A/artist/album/track.flac
///   /corpus/web/playlists/Playlist-B/artist/album/track.flac
/// With divergence_idx=3, returns "Playlist-A" and "Playlist-B".
fn extract_conflict_dir(path: &Path, corpus_root: &Path, divergence_idx: usize) -> String {
    let relative = path.strip_prefix(corpus_root).unwrap_or(path);
    relative
        .components()
        .nth(divergence_idx)
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Find where paths diverge (first component index where paths differ).
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
        let all_same = components.iter().all(|c| c.get(idx) == first_component);

        if !all_same {
            return Ok(idx);
        }
    }

    anyhow::bail!("All paths are identical");
}

/// Auto-remove inferior bitrate duplicates.
///
/// Returns: (tracks_moved_count, tracks_skipped_due_to_missing_bitrate)
///
/// If any track in a fingerprint group is missing bitrate data, the entire
/// group is skipped and left for manual resolution.
pub fn auto_remove_inferior_bitrates(
    tracks: &[Track],
    corpus_root: &Path,
    stash_root: &Path,
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

    for (fingerprint, group) in by_fingerprint {
        if group.len() < 2 {
            continue;
        }

        let all_have_bitrate = group.iter().all(|t| t.bitrate_kbps.is_some());

        if !all_have_bitrate {
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

        let max_bitrate = group
            .iter()
            .filter_map(|t| t.bitrate_kbps)
            .max()
            .unwrap();

        for track in group {
            if track.bitrate_kbps.unwrap() < max_bitrate {
                match move_to_stash_auto(track, corpus_root, stash_root) {
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
                    }
                }
            }
        }
    }

    Ok((moved_count, skipped_count))
}

/// Move file to stash/fingerprint-dupes-auto/ preserving structure.
fn move_to_stash_auto(track: &Track, corpus_root: &Path, stash_root: &Path) -> Result<()> {
    let source_path = Path::new(&track.path);

    let relative_path = source_path
        .strip_prefix(corpus_root)
        .context("Track path not within corpus root")?;

    let target_dir = stash_root.join("fingerprint-dupes-auto");
    let target_path = target_dir.join(relative_path);

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).context("Failed to create auto-removal directory")?;
    }

    fs::rename(source_path, &target_path).or_else(|_| {
        fs::copy(source_path, &target_path).context("Failed to copy file")?;
        fs::remove_file(source_path).context("Failed to remove original file")?;
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
    fn test_extract_conflict_dir() {
        let path = PathBuf::from("/corpus/playlist-winter/artist/album/track.flac");
        let corpus_root = PathBuf::from("/corpus");
        let key = extract_conflict_dir(&path, &corpus_root, 0);
        assert_eq!(key, "playlist-winter");
    }

    #[test]
    fn test_extract_conflict_dir_nested() {
        let path = PathBuf::from("/corpus/collection/playlist-winter/artist/album/track.flac");
        let corpus_root = PathBuf::from("/corpus");
        let key = extract_conflict_dir(&path, &corpus_root, 1);
        assert_eq!(key, "playlist-winter");
    }
}
