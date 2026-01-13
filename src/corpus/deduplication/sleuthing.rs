//! Directory-based duplicate discovery (sleuthing).
//!
//! Find duplicates BETWEEN selected directories using the directory picker workflow.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::config;
use crate::corpus::db::{Database, Track};

use super::filters::{durations_within_tolerance, is_same_album_different_tracks};
use super::DirectorySetCluster;

/// Find duplicates BETWEEN selected directories.
///
/// Unlike `find_fingerprint_duplicates` which uses path prefix matching and
/// global divergence indices, this function treats the selected directories
/// themselves as the comparison units.
///
/// Example: Selecting `/playlists/{1,2,3,4}` finds:
/// - Files in all 4 directories (by fingerprint)
/// - Files in any 3 directories
/// - Files in any 2 directories
///
/// Returns clusters sorted by magnitude (4-dir before 3-dir before 2-dir).
pub fn find_duplicates_between_directories(
    db: &Database,
    selected_dirs: &[PathBuf],
) -> Result<Vec<DirectorySetCluster>> {
    let _ = config::log_message(&format!(
        "=== find_duplicates_between_directories called with {} directories ===",
        selected_dirs.len()
    ));
    for (i, dir) in selected_dirs.iter().enumerate() {
        let _ = config::log_message(&format!("  dir[{}]: {}", i, dir.display()));
    }

    if selected_dirs.len() < 2 {
        let _ = config::log_message("[find_duplicates] Less than 2 directories - returning empty");
        return Ok(Vec::new());
    }

    // 1. For each selected directory, get all tracks with fingerprints
    let mut fp_to_dirs: HashMap<String, HashMap<usize, Vec<Track>>> = HashMap::new();

    for (dir_idx, dir_path) in selected_dirs.iter().enumerate() {
        let _ = config::log_message(&format!(
            "[find_duplicates] Querying tracks in dir[{}]: {}",
            dir_idx,
            dir_path.display()
        ));
        let tracks = db.get_tracks_in_directory(dir_path)?;
        let _ = config::log_message(&format!(
            "[find_duplicates] Found {} tracks with fingerprints in {}",
            tracks.len(),
            dir_path.display()
        ));
        for track in tracks {
            if let Some(fp) = &track.fingerprint {
                fp_to_dirs
                    .entry(fp.clone())
                    .or_default()
                    .entry(dir_idx)
                    .or_default()
                    .push(track);
            }
        }
    }

    let _ = config::log_message(&format!(
        "[find_duplicates] Total unique fingerprints found: {}",
        fp_to_dirs.len()
    ));

    // 2. Filter to fingerprints in 2+ directories, applying false-positive filters
    let mut filtered_count = 0usize;
    let mut same_album_filtered = 0usize;
    let mut duration_filtered = 0usize;

    let duplicates: HashMap<_, _> = fp_to_dirs
        .into_iter()
        .filter(|(fp, dirs)| {
            if dirs.len() < 2 {
                return false;
            }

            let all_tracks: Vec<Track> = dirs.values().flat_map(|t| t.clone()).collect();

            if is_same_album_different_tracks(&all_tracks) {
                let _ = config::log_message(&format!(
                    "[find_duplicates] Filtered same-album-different-tracks: fp={}...",
                    &fp[..20.min(fp.len())]
                ));
                same_album_filtered += 1;
                filtered_count += 1;
                return false;
            }

            if !durations_within_tolerance(&all_tracks, 0.10) {
                let _ = config::log_message(&format!(
                    "[find_duplicates] Filtered duration-variance: fp={}...",
                    &fp[..20.min(fp.len())]
                ));
                duration_filtered += 1;
                filtered_count += 1;
                return false;
            }

            true
        })
        .collect();

    let _ = config::log_message(&format!(
        "[find_duplicates] Fingerprints appearing in 2+ directories: {} (filtered out: {} total, {} same-album, {} duration-variance)",
        duplicates.len(),
        filtered_count,
        same_album_filtered,
        duration_filtered
    ));

    if duplicates.is_empty() {
        let _ = config::log_message("[find_duplicates] No duplicates found - returning empty");
        return Ok(Vec::new());
    }

    // 3. Group fingerprints by their exact directory set
    let mut by_dir_set: HashMap<Vec<usize>, Vec<(String, HashMap<usize, Vec<Track>>)>> =
        HashMap::new();

    for (fp, dirs_map) in duplicates {
        let mut dir_set: Vec<usize> = dirs_map.keys().cloned().collect();
        dir_set.sort();
        by_dir_set.entry(dir_set).or_default().push((fp, dirs_map));
    }

    // 4. Build DirectorySetCluster for each group
    let mut clusters: Vec<DirectorySetCluster> = by_dir_set
        .into_iter()
        .map(|(dir_indices, fps_and_tracks)| {
            let dir_names: Vec<String> = dir_indices
                .iter()
                .map(|&i| {
                    selected_dirs[i]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                })
                .collect();

            let mut tracks_by_dir: HashMap<String, Vec<Track>> = HashMap::new();
            for (_, dir_tracks) in &fps_and_tracks {
                for (&dir_idx, tracks) in dir_tracks {
                    let dir_name =
                        &dir_names[dir_indices.iter().position(|&i| i == dir_idx).unwrap()];
                    tracks_by_dir
                        .entry(dir_name.clone())
                        .or_default()
                        .extend(tracks.clone());
                }
            }

            DirectorySetCluster {
                directory_set: dir_names,
                fingerprints: fps_and_tracks.iter().map(|(fp, _)| fp.clone()).collect(),
                file_count: tracks_by_dir.values().map(|t| t.len()).sum(),
                magnitude: dir_indices.len(),
                tracks_by_dir,
            }
        })
        .collect();

    // 5. Sort: magnitude DESC, then fingerprint count DESC
    clusters.sort_by(|a, b| {
        b.magnitude
            .cmp(&a.magnitude)
            .then_with(|| b.fingerprints.len().cmp(&a.fingerprints.len()))
            .then_with(|| b.file_count.cmp(&a.file_count))
    });

    let _ = config::log_message(&format!(
        "=== find_duplicates_between_directories complete: {} clusters ===",
        clusters.len()
    ));
    for (i, cluster) in clusters.iter().enumerate() {
        let _ = config::log_message(&format!(
            "  cluster[{}]: magnitude={} dirs={:?} fingerprints={} files={} tracks_by_dir.keys={:?}",
            i,
            cluster.magnitude,
            cluster.directory_set,
            cluster.fingerprints.len(),
            cluster.file_count,
            cluster.tracks_by_dir.keys().collect::<Vec<_>>()
        ));
    }

    Ok(clusters)
}
