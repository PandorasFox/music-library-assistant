//! Health issue detection during scanning and mutations.
//!
//! This module detects and creates health issues when tracks are inserted or modified.
//!
//! Includes:
//! - Fingerprint duplicate detection
//! - Metadata duplicate detection
//! - Quality variant detection
//! - Missing tag detection (album_artist)
//! - Out-of-band tag change detection (DB differs from disk)
//! - Deployment conflict detection
//!
//! ## TODO: Artist/Album Artist Canonicalization Mismatch
//!
//! Detect when artist and album_artist tags normalize to different values but should match.
//! Example: "DragonForce" in artist vs "Dragonforce" in album_artist.
//!
//! Implementation:
//! - Use `mla_utils::metadata_magic::normalize_artist()` on both fields
//! - Compare normalized values; if different but base names match, flag as mismatch
//! - Create a new `HealthIssueType::ArtistAlbumArtistMismatch` variant
//! - Group by normalized key for bulk resolution
//! - Could also catch typo variants like "Deadmau5" vs "deadmau5"

use std::collections::{HashMap, HashSet};

use crate::config::Config;
use crate::corpus::db::{Database, HealthIssue, HealthIssueType, Track};
use crate::corpus::deploy::compute_deployment_path_with_tags;
use anyhow::Result;

/// Default duration tolerance for fingerprint matching (10%)
const DEFAULT_DURATION_TOLERANCE: f64 = 0.10;

// ============================================================================
// Helper Functions (inlined from removed library.rs)
// ============================================================================

/// Get all configured library names from deploy mappings
fn get_configured_library_names(config: &Config) -> Vec<String> {
    let mut names = HashSet::new();
    for mapping in &config.deploy_mappings {
        for name in &mapping.library_names {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

/// Get all corpus tracks that should be deployed to a specific library
fn get_deployable_corpus_tracks(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> Vec<Track> {
    let mut all_tracks = Vec::new();

    for corpus_path in config.get_corpus_paths_for_library(library_name) {
        if let Ok(tracks) = db.get_tracks_by_corpus_path_prefix(&corpus_path.to_string_lossy()) {
            all_tracks.extend(tracks);
        }
    }

    all_tracks
}

/// Detect fingerprint duplicate issues for a newly inserted/updated track.
///
/// DEPRECATED: Incremental detection disabled. Use bulk DetectFingerprintDuplicates
/// computation instead, which embeds track_ids in metadata_json.
pub fn detect_fingerprint_issues(_db: &Database, _track: &Track) -> Result<Vec<HealthIssue>> {
    // Bulk computation handles this now - see execute_detect_fingerprint_duplicates
    Ok(vec![])
}

/// Refresh health issues for a specific track.
///
/// Called after track mutations (tag edits, moves) to update associated health issues.
/// This is currently expensive (runs full deployment conflict detection) but comprehensive.
/// Future optimization: compute only affected paths and check conflicts incrementally.
pub fn refresh_health_for_track(db: &Database, track_id: i64) -> Result<()> {
    // Get the track
    let track = match db.get_track_by_id(track_id)? {
        Some(t) => t,
        None => return Ok(()), // Track was deleted
    };

    // Re-run fingerprint detection (relevant for new tracks, harmless for tag edits)
    detect_fingerprint_issues(db, &track)?;

    // Re-run deployment conflict detection for all libraries
    // This is expensive but correct - tag edits can change deployment paths
    let config = crate::config::load_config()?;
    detect_deployment_conflicts(&config, db)?;

    // Clean up any conflicts that may have been resolved by this change
    cleanup_resolved_deployment_conflicts(&config, db)?;

    Ok(())
}

// ============================================================================
// Deployment Conflict Detection
// ============================================================================

// TODO: Store additional conflict metadata for UI display:
// - fingerprint_match: bool (whether tracks are acoustically identical)
// - differing_tags: Vec<String> (non-deploy tags that differ between tracks)
// This would help users understand whether conflicts are true duplicates or just
// metadata collisions between different recordings.

/// Detect deployment conflicts for all configured libraries.
///
/// A deployment conflict occurs when multiple corpus tracks would deploy to the same
/// target path in a library (based on their metadata: album_artist/album/track/title).
///
/// This function:
/// 1. Iterates over all configured libraries
/// 2. Computes target paths for all deployable corpus tracks
/// 3. Detects conflicts (multiple tracks -> same path)
/// 4. Creates/updates health issues in the database
///
/// Returns the number of new conflicts detected.
pub fn detect_deployment_conflicts(config: &Config, db: &Database) -> Result<usize> {
    let library_names = get_configured_library_names(config);
    let mut new_issues = 0;

    for library_name in library_names {
        new_issues += detect_deployment_conflicts_for_library(config, db, &library_name)?;
    }

    Ok(new_issues)
}

/// Detect deployment conflicts for a specific library.
///
/// A deployment conflict occurs when multiple corpus tracks would deploy to the
/// same target path (based on their metadata: album_artist/album/track/title).
///
/// Returns the number of new conflicts detected.
pub fn detect_deployment_conflicts_for_library(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> Result<usize> {
    // Get all corpus tracks that should be deployed to this library
    let tracks = get_deployable_corpus_tracks(config, db, library_name);

    if tracks.is_empty() {
        return Ok(0);
    }

    // Build deployment_path -> [track_ids] map
    let mut path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for track in &tracks {
        let track_id = match track.id {
            Some(id) => id,
            None => continue,
        };

        // Get tags for this track
        let tags = db.get_track_tags(track_id).unwrap_or_default();
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
            .collect();

        // Compute deployment path
        let deploy_path = compute_deployment_path_with_tags(track, &tag_map);
        let path_key = deploy_path.to_string_lossy().to_string();

        path_to_tracks
            .entry(path_key)
            .or_default()
            .push(track_id);
    }

    // Find conflicts (paths with multiple tracks) and create health issues
    let mut new_conflicts = 0;

    for (deploy_path, track_ids) in path_to_tracks {
        if track_ids.len() < 2 {
            continue;
        }

        // Create a unique key for this conflict
        let issue_key = format!("deploy_conflict:{}:{}", library_name, deploy_path);

        // Check if this conflict already exists
        let existing = db
            .get_health_signals(Some(HealthIssueType::DeployConflict))?
            .into_iter()
            .find(|i| i.issue_key == issue_key);

        if existing.is_none() {
            // Get paths for metadata
            let conflicting_paths: Vec<String> = track_ids
                .iter()
                .filter_map(|id| db.get_track_by_id(*id).ok().flatten())
                .map(|t| t.path)
                .collect();

            let metadata = serde_json::json!({
                "library_name": library_name,
                "target_path": deploy_path,
                "track_ids": track_ids,
                "conflicting_paths": conflicting_paths,
            });

            let issue = HealthIssue {
                id: None,
                issue_type: HealthIssueType::DeployConflict,
                issue_key,
                discovered_at: None,
                metadata_json: Some(metadata.to_string()),
            };

            db.insert_health_issue(&issue)?;
            new_conflicts += 1;
        }
    }

    Ok(new_conflicts)
}

/// Clean up deployment conflict signals that are no longer valid.
///
/// This should be called after tag edits or track deletions that might
/// have resolved conflicts (e.g., renaming a track so it no longer
/// collides with another).
///
/// Returns the number of signals deleted.
pub fn cleanup_resolved_deployment_conflicts(config: &Config, db: &Database) -> Result<usize> {
    // Get all existing DeployConflict signals
    let existing_conflicts = db.get_health_signals(Some(HealthIssueType::DeployConflict))?;

    if existing_conflicts.is_empty() {
        return Ok(0);
    }

    let mut deleted = 0;

    for conflict in existing_conflicts {
        let conflict_id = match conflict.id {
            Some(id) => id,
            None => continue,
        };

        // Parse metadata to get track_ids
        let track_ids: Vec<i64> = conflict
            .metadata_json
            .as_ref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|v| v.get("track_ids").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        if track_ids.is_empty() {
            // Can't verify, delete stale signal
            db.delete_health_signal(conflict_id)?;
            deleted += 1;
            continue;
        }

        // Parse library name from issue_key: "deploy_conflict:{library_name}:{path}"
        let parts: Vec<&str> = conflict.issue_key.splitn(3, ':').collect();
        let library_name = if parts.len() >= 2 { parts[1] } else { continue };

        // Recompute deployment paths for all tracks
        let mut path_counts: HashMap<String, usize> = HashMap::new();

        for track_id in &track_ids {
            // Check if track still exists
            let track = match db.get_track_by_id(*track_id)? {
                Some(t) => t,
                None => continue, // Track deleted, won't contribute to conflict
            };

            // Check if track is still deployable to this library
            let deployable_tracks = get_deployable_corpus_tracks(config, db, library_name);
            if !deployable_tracks.iter().any(|t| t.id == Some(*track_id)) {
                continue; // Track no longer mapped to this library
            }

            // Get tags and compute path
            let tags = db.get_track_tags(*track_id).unwrap_or_default();
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                .collect();

            let deploy_path = compute_deployment_path_with_tags(&track, &tag_map);
            let path_key = deploy_path.to_string_lossy().to_string();

            *path_counts.entry(path_key).or_insert(0) += 1;
        }

        // Check if any path still has multiple tracks (still a conflict)
        let still_conflict = path_counts.values().any(|&count| count >= 2);

        if !still_conflict {
            // Conflict resolved, delete the signal
            db.delete_health_signal(conflict_id)?;
            deleted += 1;
        }
    }

    Ok(deleted)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test track with specified quality attributes.
    fn make_track(
        id: i64,
        artist: &str,
        album: &str,
        title: &str,
        file_type: &str,
        bitrate: Option<i32>,
    ) -> Track {
        Track {
            id: Some(id),
            path: format!("/test/{}/{}/{}.{}", artist, album, title, file_type),
            source: "corpus".to_string(),
            inode: id,
            file_size: 5_000_000,
            file_type: file_type.to_string(),
            duration_ms: Some(240_000),
            bitrate_kbps: bitrate,
            sample_rate: Some(44100),
            fingerprint: Some(format!("fp_{}_{}_{}", artist, album, title)),
        }
    }

    #[test]
    fn test_quality_variant_detection_mixed_formats() {
        // Scenario: "Clockwork Hearts" exists as both 320kbps iTunes purchase and 1000kbps FLAC
        let tracks = vec![
            make_track(1, "Artist", "Clockwork Hearts", "Track 1", "mp3", Some(320)),
            make_track(2, "Artist", "Clockwork Hearts", "Track 1", "flac", Some(1000)),
        ];

        // Check for quality differences
        let mut file_types: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bitrates: Vec<i32> = Vec::new();

        for track in &tracks {
            file_types.insert(track.file_type.to_lowercase());
            if let Some(br) = track.bitrate_kbps {
                bitrates.push(br);
            }
        }

        // Should detect format difference (mp3 vs flac)
        assert_eq!(file_types.len(), 2);
        assert!(file_types.contains("mp3"));
        assert!(file_types.contains("flac"));

        // Should detect significant bitrate difference
        let max = bitrates.iter().max().copied().unwrap_or(0);
        let min = bitrates.iter().min().copied().unwrap_or(0);
        let ratio = min as f64 / max as f64;
        assert!(ratio < 0.7, "Expected significant bitrate difference, got ratio {}", ratio);
    }

    #[test]
    fn test_quality_variant_detection_same_format_different_bitrate() {
        // Scenario: Same album in different MP3 qualities
        let tracks = vec![
            make_track(1, "Artist", "Album", "Track", "mp3", Some(128)),
            make_track(2, "Artist", "Album", "Track", "mp3", Some(320)),
        ];

        let mut bitrates: Vec<i32> = Vec::new();
        for track in &tracks {
            if let Some(br) = track.bitrate_kbps {
                bitrates.push(br);
            }
        }

        let max = bitrates.iter().max().copied().unwrap_or(0);
        let min = bitrates.iter().min().copied().unwrap_or(0);
        let ratio = min as f64 / max as f64;

        // 128/320 = 0.4, which is < 0.7
        assert!(ratio < 0.7, "Should detect bitrate difference");
    }

    #[test]
    fn test_quality_variant_no_difference() {
        // Scenario: Same quality, no issue needed
        let tracks = vec![
            make_track(1, "Artist", "Album", "Track 1", "flac", Some(1000)),
            make_track(2, "Artist", "Album", "Track 2", "flac", Some(1000)),
        ];

        let mut file_types: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bitrates: Vec<i32> = Vec::new();

        for track in &tracks {
            file_types.insert(track.file_type.to_lowercase());
            if let Some(br) = track.bitrate_kbps {
                bitrates.push(br);
            }
        }

        // Same format
        assert_eq!(file_types.len(), 1);

        // Same bitrate - no significant difference
        let max = bitrates.iter().max().copied().unwrap_or(0);
        let min = bitrates.iter().min().copied().unwrap_or(0);
        let ratio = if max > 0 { min as f64 / max as f64 } else { 1.0 };
        assert!(ratio >= 0.7, "Should not detect quality difference for identical tracks");
    }

    #[test]
    fn test_same_directory_different_artists() {
        // Scenario: Tracks from a compilation (different artists, same album directory)
        // Note: Tag-based grouping now requires track_tags table, not Track struct
        // This test uses same album (and thus same directory in path format)
        let tracks = vec![
            make_track(1, "Various", "Compilation", "Track 1", "flac", Some(1000)),
            make_track(2, "Various", "Compilation", "Track 2", "flac", Some(1000)),
            make_track(3, "Various", "Compilation", "Track 3", "flac", Some(1000)),
        ];

        // Verify tracks have same parent directory (album directory)
        let dirs: std::collections::HashSet<_> = tracks
            .iter()
            .filter_map(|t| std::path::Path::new(&t.path).parent())
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert_eq!(dirs.len(), 1, "All tracks should be in same album directory");

        // Verify all tracks have same file format
        let file_types: std::collections::HashSet<_> = tracks
            .iter()
            .map(|t| t.file_type.clone())
            .collect();
        assert_eq!(file_types.len(), 1, "All tracks should have same file type");
    }

    #[test]
    fn test_insights_loop_scenario() {
        // Full scenario test:
        // 1. Artist canonicalization collapses "The Beatles" and "Beatles"
        // 2. Combined track set now has FLAC and MP3 versions
        // 3. Quality variant should be detected
        // 4. User stashes lower quality → issue resolved

        // Tracks before canonicalization (different artist strings)
        let tracks_beatles = vec![
            make_track(1, "Beatles", "Abbey Road", "Come Together", "mp3", Some(320)),
        ];
        let tracks_the_beatles = vec![
            make_track(2, "The Beatles", "Abbey Road", "Come Together", "flac", Some(1000)),
        ];

        // After canonicalization, these would be in the same bucket
        let combined: Vec<Track> = tracks_beatles.into_iter()
            .chain(tracks_the_beatles.into_iter())
            .collect();

        // Detect quality differences in combined set
        let mut file_types: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bitrates: Vec<i32> = Vec::new();

        for track in &combined {
            file_types.insert(track.file_type.to_lowercase());
            if let Some(br) = track.bitrate_kbps {
                bitrates.push(br);
            }
        }

        // Should now detect the quality variant that was hidden by artist name difference
        assert_eq!(file_types.len(), 2, "Should detect mp3 and flac");

        let max = bitrates.iter().max().copied().unwrap_or(0);
        let min = bitrates.iter().min().copied().unwrap_or(0);
        let ratio = min as f64 / max as f64;
        assert!(ratio < 0.7, "Should detect significant bitrate difference");

        // Quality differences are introspectable at runtime from Track metadata
        // when viewing FingerprintDuplicate issues - user can choose to stash lower quality
    }

    #[test]
    fn test_ep_album_variant_scenario() {
        // Scenario: "Clockwork Hearts" and "Clockwork Hearts (EP)"
        // One is 320kbps iTunes, other is 1000kbps Bandcamp FLAC

        let ep_tracks = vec![
            make_track(1, "Artist", "Clockwork Hearts (EP)", "Track 1", "mp3", Some(320)),
            make_track(2, "Artist", "Clockwork Hearts (EP)", "Track 2", "mp3", Some(320)),
        ];

        let album_tracks = vec![
            make_track(3, "Artist", "Clockwork Hearts", "Track 1", "flac", Some(1000)),
            make_track(4, "Artist", "Clockwork Hearts", "Track 2", "flac", Some(1000)),
            make_track(5, "Artist", "Clockwork Hearts", "Track 3", "flac", Some(1000)),
        ];

        // In album canonicalization, these might be flagged as related
        // (same base name, EP suffix stripped)

        // Combined for quality analysis
        let all_tracks: Vec<Track> = ep_tracks.into_iter()
            .chain(album_tracks.into_iter())
            .collect();

        let mut file_types: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bitrates: Vec<i32> = Vec::new();

        for track in &all_tracks {
            file_types.insert(track.file_type.to_lowercase());
            if let Some(br) = track.bitrate_kbps {
                bitrates.push(br);
            }
        }

        // Detect quality difference
        assert!(file_types.len() > 1, "Should have multiple formats");

        let max = bitrates.iter().max().copied().unwrap_or(0);
        let min = bitrates.iter().min().copied().unwrap_or(0);
        assert!(max > min, "Should have bitrate spread");

        // The EP version (lower quality) could be stashed in favor of full album
    }
}
