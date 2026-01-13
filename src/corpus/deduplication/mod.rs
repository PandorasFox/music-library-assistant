//! Deduplication Module
//!
//! Provides fingerprint-based deduplication for surgical duplicate removal.
//!
//! ## Module Structure
//!
//! - `types` - Core data structures (DirectorySetCluster, ClusterDecision, DeduplicationSession)
//! - `filters` - False-positive filtering (same album detection, duration tolerance)
//! - `quality` - Quality-based comparison and auto-resolution
//! - `clustering` - Directory set clustering (non-transitive)
//! - `fingerprint` - Fingerprint duplicate detection and path divergence
//! - `sleuthing` - Directory-based duplicate discovery (picker workflow)
//! - `resolution` - Conflict resolution and stash operations
//! - `auto_ignore` - Auto-ignore state tracking for UI workflows

mod auto_ignore;
mod clustering;
mod filters;
mod fingerprint;
mod quality;
mod resolution;
mod sleuthing;
mod types;

// Re-export core types
pub use auto_ignore::AutoIgnoreState;
pub use clustering::compute_directory_set_clusters;
pub use filters::{durations_within_tolerance, is_same_album_different_tracks};
pub use fingerprint::find_fingerprint_duplicates;
pub use quality::{compare_track_quality, QualityVerdict};
pub use resolution::generate_cluster_changes;
pub use sleuthing::find_duplicates_between_directories;
pub use types::{ClusterDecision, DeduplicationSession, DirectorySetCluster};

use std::collections::HashMap;

use crate::corpus::db::Track;

/// A set of tracks sharing the same fingerprint across multiple directories.
/// Used to represent duplicate detection results.
#[derive(Debug, Clone)]
pub struct ConflictSet {
    pub fingerprint: String,
    pub conflict_dirs: Vec<String>,
    pub tracks_by_dir: HashMap<String, Vec<Track>>,
    pub match_score: f64, // Always 100.0 for exact fingerprint match
}

/// Check if all remaining clusters qualify for bulk review.
/// Returns true if all remaining clusters are 2-directory, single-file pairs.
pub fn should_offer_bulk_review(clusters: &[DirectorySetCluster], current_index: usize) -> bool {
    if current_index >= clusters.len() {
        return false;
    }

    clusters[current_index..].iter().all(|c| {
        c.magnitude == 2 && c.fingerprints.len() == 1
    })
}

/// Find the common path prefix where duplicates diverge.
/// Used to display context about where the conflict originates.
pub fn find_divergence_root(conflict_sets: &[ConflictSet]) -> String {
    if conflict_sets.is_empty() {
        return String::new();
    }

    // Collect all paths
    let all_paths: Vec<&str> = conflict_sets
        .iter()
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
        let matching_len = first
            .chars()
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
    fn test_should_offer_bulk_review_not_yet() {
        let clusters = vec![
            DirectorySetCluster {
                directory_set: vec!["a".into(), "b".into(), "c".into()],
                fingerprints: vec!["fp1".into()],
                file_count: 3,
                magnitude: 3,
                tracks_by_dir: HashMap::new(),
            },
            DirectorySetCluster {
                directory_set: vec!["a".into(), "b".into()],
                fingerprints: vec!["fp2".into()],
                file_count: 2,
                magnitude: 2,
                tracks_by_dir: HashMap::new(),
            },
        ];

        assert!(!should_offer_bulk_review(&clusters, 0));
        assert!(should_offer_bulk_review(&clusters, 1));
    }

    #[test]
    fn test_should_offer_bulk_review_complete() {
        let clusters = vec![DirectorySetCluster {
            directory_set: vec!["a".into(), "b".into()],
            fingerprints: vec!["fp1".into()],
            file_count: 2,
            magnitude: 2,
            tracks_by_dir: HashMap::new(),
        }];

        assert!(!should_offer_bulk_review(&clusters, 5));
    }

    #[test]
    fn test_find_divergence_root() {
        let mut cs = make_conflict_set(
            "fp1",
            &[
                "web/rips/spotify/Tracks-DaB",
                "web/rips/spotify/Tracks-trans",
            ],
        );
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
                artist: None,
                album: None,
                album_artist: None,
                title: None,
                track_number: None,
                genre: None,
                duration_ms: None,
                bitrate_kbps: None,
                sample_rate: None,
                fingerprint: None,
                isrc: None,
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
                artist: None,
                album: None,
                album_artist: None,
                title: None,
                track_number: None,
                genre: None,
                duration_ms: None,
                bitrate_kbps: None,
                sample_rate: None,
                fingerprint: None,
                isrc: None,
            }],
        );

        let root = find_divergence_root(&[cs]);
        assert_eq!(root, "/corpus/web/rips/spotify/");
    }
}
