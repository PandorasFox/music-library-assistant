//! Conflict resolution and stash operations.
//!
//! Functions for resolving duplicate conflicts and generating change mutations.

use std::path::Path;

use crate::config;
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};

use super::{ConflictSet, DirectorySetCluster};

/// Generate Delete pending changes for a cluster decision.
///
/// Creates a change for each file in non-keeper directories.
/// Uses cluster's tracks_by_dir directly if available, otherwise falls back
/// to searching conflict_sets (legacy mode).
pub fn generate_cluster_changes(
    cluster: &DirectorySetCluster,
    keeper_dir: &str,
    conflict_sets: &[ConflictSet],
    corpus_root: &Path,
    stash_root: &Path,
    session_id: &str,
) -> Vec<PendingChange> {
    let _ = config::log_message(&format!(
        "=== generate_cluster_changes called ===\n  keeper_dir={}\n  corpus_root={}\n  stash_root={}\n  session_id={}\n  cluster.directory_set={:?}\n  cluster.tracks_by_dir.len()={}\n  conflict_sets.len()={}",
        keeper_dir,
        corpus_root.display(),
        stash_root.display(),
        session_id,
        cluster.directory_set,
        cluster.tracks_by_dir.len(),
        conflict_sets.len()
    ));

    let mut changes = Vec::new();

    // Use cluster.tracks_by_dir directly if available (directory-set mode)
    if !cluster.tracks_by_dir.is_empty() {
        let _ = config::log_message(
            "[generate_cluster_changes] Using directory-set mode (cluster.tracks_by_dir)",
        );

        for (dir, tracks) in &cluster.tracks_by_dir {
            let _ = config::log_message(&format!(
                "[generate_cluster_changes] Processing dir={} with {} tracks (keeper={})",
                dir,
                tracks.len(),
                dir == keeper_dir
            ));

            if dir == keeper_dir {
                let _ = config::log_message(&format!(
                    "[generate_cluster_changes] KEEPING {} tracks in {}",
                    tracks.len(),
                    dir
                ));
                continue;
            }

            for track in tracks {
                let rel_path = Path::new(&track.path)
                    .strip_prefix(corpus_root)
                    .unwrap_or(Path::new(&track.path));
                let target = stash_root
                    .join("fingerprint-dupes")
                    .join(dir)
                    .join(rel_path);

                let _ = config::log_message(&format!(
                    "[generate_cluster_changes] Creating DELETE change:\n    source={}\n    target={}",
                    track.path,
                    target.display()
                ));

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

        let _ = config::log_message(&format!(
            "[generate_cluster_changes] Generated {} changes (directory-set mode)",
            changes.len()
        ));
        return changes;
    }

    // Legacy mode: search through conflict_sets
    let _ = config::log_message("[generate_cluster_changes] Using legacy mode (conflict_sets)");

    for cs in conflict_sets {
        let mut cs_dirs = cs.conflict_dirs.clone();
        cs_dirs.sort();
        if cs_dirs != cluster.directory_set {
            continue;
        }

        let _ = config::log_message(&format!(
            "[generate_cluster_changes] Matched conflict_set with fingerprint={}",
            &cs.fingerprint[..20.min(cs.fingerprint.len())]
        ));

        for (dir, tracks) in &cs.tracks_by_dir {
            if dir == keeper_dir {
                let _ = config::log_message(&format!(
                    "[generate_cluster_changes] KEEPING {} tracks in {}",
                    tracks.len(),
                    dir
                ));
                continue;
            }

            for track in tracks {
                let rel_path = Path::new(&track.path)
                    .strip_prefix(corpus_root)
                    .unwrap_or(Path::new(&track.path));
                let target = stash_root
                    .join("fingerprint-dupes")
                    .join(dir)
                    .join(rel_path);

                let _ = config::log_message(&format!(
                    "[generate_cluster_changes] Creating DELETE change:\n    source={}\n    target={}",
                    track.path,
                    target.display()
                ));

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

    let _ = config::log_message(&format!(
        "[generate_cluster_changes] Generated {} changes (legacy mode)",
        changes.len()
    ));

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::corpus::db::Track;

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
    fn test_generate_cluster_changes() {
        let conflicts = vec![make_conflict_set("fp1", &["keeper", "loser"])];
        let cluster = DirectorySetCluster {
            directory_set: vec!["keeper".into(), "loser".into()],
            fingerprints: vec!["fp1".into()],
            file_count: 2,
            magnitude: 2,
            tracks_by_dir: HashMap::new(), // Uses conflict_sets via legacy mode
        };

        let changes = generate_cluster_changes(
            &cluster,
            "keeper",
            &conflicts,
            Path::new("/corpus"),
            Path::new("/stash"),
            "session-123",
        );

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Delete);
        assert!(changes[0].source_path.contains("loser"));
        assert!(changes[0]
            .target_path
            .as_ref()
            .unwrap()
            .contains("fingerprint-dupes"));
    }
}
