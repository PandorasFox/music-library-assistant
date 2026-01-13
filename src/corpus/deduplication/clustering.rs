//! Directory set clustering.
//!
//! Non-transitive clustering that groups conflicts by their EXACT directory set.

use std::collections::HashMap;

use crate::corpus::db::Track;

use super::{ConflictSet, DirectorySetCluster};

/// Compute directory set clusters (NON-TRANSITIVE).
///
/// Groups conflicts by their EXACT directory set, not transitive closure.
///
/// For example, if you have:
/// - Track A with copies in {dir1, dir2}
/// - Track B with copies in {dir1, dir2, dir3}
///
/// These form TWO separate clusters, not one.
pub fn compute_directory_set_clusters(conflict_sets: &[ConflictSet]) -> Vec<DirectorySetCluster> {
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
            // Aggregate tracks by directory across all fingerprints in this cluster
            let mut tracks_by_dir: HashMap<String, Vec<Track>> = HashMap::new();
            for cs in &conflicts {
                for (dir, tracks) in &cs.tracks_by_dir {
                    tracks_by_dir
                        .entry(dir.clone())
                        .or_default()
                        .extend(tracks.clone());
                }
            }

            DirectorySetCluster {
                magnitude: dir_set.len(),
                fingerprints: conflicts.iter().map(|c| c.fingerprint.clone()).collect(),
                file_count: tracks_by_dir.values().map(|t| t.len()).sum(),
                directory_set: dir_set,
                tracks_by_dir,
            }
        })
        .collect();

    // Sort: magnitude DESC, then fingerprint count DESC, then file_count DESC
    clusters.sort_by(|a, b| {
        b.magnitude
            .cmp(&a.magnitude)
            .then_with(|| b.fingerprints.len().cmp(&a.fingerprints.len()))
            .then_with(|| b.file_count.cmp(&a.file_count))
    });

    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    genre: None,
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
        assert_eq!(clusters[0].file_count, 4);
    }

    #[test]
    fn test_compute_directory_set_clusters_non_transitive() {
        // {A, B} and {B, C} should be SEPARATE clusters
        let conflicts = vec![
            make_conflict_set("fp1", &["dir_a", "dir_b"]),
            make_conflict_set("fp2", &["dir_b", "dir_c"]),
        ];

        let clusters = compute_directory_set_clusters(&conflicts);

        assert_eq!(clusters.len(), 2);
        assert!(clusters.iter().all(|c| c.magnitude == 2));
    }

    #[test]
    fn test_compute_directory_set_clusters_ordering() {
        // 3-way cluster should come before 2-way clusters
        let conflicts = vec![
            make_conflict_set("fp1", &["a", "b"]),
            make_conflict_set("fp2", &["x", "y", "z"]),
            make_conflict_set("fp3", &["a", "b"]),
        ];

        let clusters = compute_directory_set_clusters(&conflicts);

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].magnitude, 3);
        assert_eq!(clusters[1].magnitude, 2);
    }
}
