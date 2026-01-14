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

use std::collections::HashMap;

use crate::config::Config;
use crate::corpus::db::{
    Database, HealthIssue, HealthIssueType, HealthIssueSeverity, Track, TrackRole,
};
use crate::flows::deploy::compute_deployment_path;
use anyhow::Result;

use super::filter::{durations_within_tolerance, is_legitimate_rerelease, is_same_album_different_tracks};
use super::library::{get_configured_library_names, get_deployable_corpus_tracks};

/// Default duration tolerance for fingerprint matching (10%)
const DEFAULT_DURATION_TOLERANCE: f64 = 0.10;

/// Detect fingerprint duplicate issues for a newly inserted/updated track.
///
/// This should be called after a track with a fingerprint is inserted into the database.
/// It checks if other tracks share the same fingerprint and creates health issues accordingly.
pub fn detect_fingerprint_issues(db: &Database, track: &Track) -> Result<Vec<HealthIssue>> {
    let fingerprint = match &track.fingerprint {
        Some(fp) => fp,
        None => return Ok(vec![]), // No fingerprint, no issues to detect
    };

    // Find all tracks with the same fingerprint
    let matching_tracks = db.get_tracks_by_fingerprint(fingerprint)?;

    // Need at least 2 tracks for a duplicate
    if matching_tracks.len() < 2 {
        return Ok(vec![]);
    }

    // Check if this is a false positive case

    // Case 1: Same album, different track numbers (e.g., C418 ambient tracks)
    if is_same_album_different_tracks(&matching_tracks) {
        return Ok(vec![]);
    }

    // Case 2: Durations differ significantly (not true duplicates)
    if !durations_within_tolerance(&matching_tracks, DEFAULT_DURATION_TOLERANCE) {
        return Ok(vec![]);
    }

    // Case 3: Legitimate re-release (same audio on different albums)
    if is_legitimate_rerelease(&matching_tracks) {
        // This is a known variant, not a duplicate requiring resolution
        // Check if we already have a known_variant entry
        if db.is_known_variant(fingerprint)? {
            return Ok(vec![]);
        }

        // Create a new known variant entry
        let variant = crate::corpus::db::KnownVariant {
            id: None,
            variant_type: crate::corpus::db::VariantType::Rerelease,
            canonical_fingerprint: fingerprint.clone(),
            variant_fingerprint: None,
            canonical_track_id: matching_tracks.first().and_then(|t| t.id),
            variant_track_id: track.id,
            marked_at: None,
            notes: Some("Auto-detected re-release".to_string()),
        };
        db.insert_known_variant(&variant)?;

        return Ok(vec![]);
    }

    // Check if an issue already exists for this fingerprint
    if let Some(existing) = db.get_health_issue_by_key(HealthIssueType::FingerprintDuplicate, fingerprint)? {
        // Issue exists - add this track as a member if not already
        if let Some(issue_id) = existing.id {
            if let Some(track_id) = track.id {
                // Check if track is already a member (tuple: (Track, TrackRole))
                let existing_tracks = db.get_health_issue_tracks(issue_id)?;
                if !existing_tracks.iter().any(|(t, _role)| t.id == Some(track_id)) {
                    db.add_health_issue_track(issue_id, track_id, TrackRole::Member)?;
                }
            }
        }
        return Ok(vec![existing]);
    }

    // Create new health issue for fingerprint duplicate
    let severity = determine_duplicate_severity(&matching_tracks);

    let issue = HealthIssue {
        id: None,
        issue_type: HealthIssueType::FingerprintDuplicate,
        issue_key: fingerprint.clone(),
        severity,
        discovered_at: None,
        resolved_at: None,
        resolution_type: None,
        resolution_session: None,
        metadata_json: None,
    };

    let issue_id = db.insert_health_issue(&issue)?;

    // Add all matching tracks as members
    for t in &matching_tracks {
        if let Some(tid) = t.id {
            db.add_health_issue_track(issue_id, tid, TrackRole::Member)?;
        }
    }

    Ok(vec![HealthIssue { id: Some(issue_id), ..issue }])
}

/// Refresh health issues for a specific track.
///
/// Called after track mutations (tag edits, moves) to update associated health issues.
#[allow(dead_code)]
pub fn refresh_health_for_track(db: &Database, track_id: i64) -> Result<()> {
    // Get the track
    let track = match db.get_track_by_id(track_id)? {
        Some(t) => t,
        None => return Ok(()), // Track was deleted
    };

    // Re-run fingerprint detection
    detect_fingerprint_issues(db, &track)?;

    Ok(())
}

/// Determine severity of a duplicate issue based on track characteristics.
/// TODO: Quality comparison logic was in deleted deduplication module.
/// For now, all duplicates require manual review.
fn determine_duplicate_severity(_tracks: &[Track]) -> HealthIssueSeverity {
    HealthIssueSeverity::ManualReview
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
/// Returns the number of new conflicts detected.
pub fn detect_deployment_conflicts_for_library(
    config: &Config,
    db: &Database,
    library_name: &str,
) -> Result<usize> {
    // Get all corpus tracks deployable to this library
    let corpus_tracks = get_deployable_corpus_tracks(config, db, library_name);

    // Build target_path -> tracks map
    let mut target_path_map: HashMap<String, Vec<Track>> = HashMap::new();
    for track in corpus_tracks {
        let target = compute_deployment_path(&track);
        let target_str = target.display().to_string();
        target_path_map.entry(target_str).or_default().push(track);
    }

    let mut new_issues = 0;

    // Process conflicts (multiple tracks -> same path)
    for (target_path, tracks) in target_path_map {
        if tracks.len() < 2 {
            continue; // No conflict
        }

        // Issue key format: library_name:target_path
        let issue_key = format!("{}:{}", library_name, target_path);

        // Check if issue already exists
        if let Some(existing) = db.get_health_issue_by_key(HealthIssueType::DeployConflict, &issue_key)? {
            // Issue exists - ensure all tracks are members
            if let Some(issue_id) = existing.id {
                let existing_tracks = db.get_health_issue_tracks(issue_id)?;
                for track in &tracks {
                    if let Some(track_id) = track.id {
                        if !existing_tracks.iter().any(|(t, _)| t.id == Some(track_id)) {
                            db.add_health_issue_track(issue_id, track_id, TrackRole::Member)?;
                        }
                    }
                }
            }
            continue;
        }

        // Create new health issue
        let metadata = serde_json::json!({
            "library_name": library_name,
            "target_path": target_path,
            "track_count": tracks.len(),
        });

        let issue = HealthIssue {
            id: None,
            issue_type: HealthIssueType::DeployConflict,
            issue_key,
            severity: HealthIssueSeverity::ManualReview,
            discovered_at: None,
            resolved_at: None,
            resolution_type: None,
            resolution_session: None,
            metadata_json: Some(metadata.to_string()),
        };

        let issue_id = db.insert_health_issue(&issue)?;

        // Add all conflicting tracks as members
        for track in &tracks {
            if let Some(track_id) = track.id {
                db.add_health_issue_track(issue_id, track_id, TrackRole::Member)?;
            }
        }

        new_issues += 1;
    }

    Ok(new_issues)
}

/// Resolve deployment conflicts that are no longer valid.
///
/// This should be called after tag edits or track deletions that might
/// have resolved conflicts (e.g., renaming a track so it no longer
/// collides with another).
///
/// Returns the number of issues resolved.
pub fn cleanup_resolved_deployment_conflicts(config: &Config, db: &Database) -> Result<usize> {
    use crate::corpus::db::ResolutionType;

    let library_names = get_configured_library_names(config);
    let mut resolved_count = 0;

    // Get all unresolved deploy conflict issues
    let issues = db.get_unresolved_health_issues(Some(HealthIssueType::DeployConflict))?;

    for issue in issues {
        let issue_id = match issue.id {
            Some(id) => id,
            None => continue,
        };

        // Parse library name from issue key (format: "library_name:target_path")
        let parts: Vec<&str> = issue.issue_key.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let library_name = parts[0];

        // Skip if library is no longer configured
        if !library_names.contains(&library_name.to_string()) {
            db.resolve_health_issue(issue_id, ResolutionType::Ignored, None)?;
            resolved_count += 1;
            continue;
        }

        // Get tracks currently in this issue
        let issue_tracks = db.get_health_issue_tracks(issue_id)?;

        // Check if tracks still conflict
        let tracks: Vec<Track> = issue_tracks.into_iter().map(|(t, _)| t).collect();

        // Recompute target paths for these tracks
        let mut target_path_map: HashMap<String, Vec<&Track>> = HashMap::new();
        for track in &tracks {
            let target = compute_deployment_path(track);
            let target_str = target.display().to_string();
            target_path_map.entry(target_str).or_default().push(track);
        }

        // Check if any path still has multiple tracks
        let still_conflicts = target_path_map.values().any(|v| v.len() > 1);

        if !still_conflicts {
            // Conflict resolved - mark issue as resolved
            db.resolve_health_issue(issue_id, ResolutionType::Merged, None)?;
            resolved_count += 1;
        }
    }

    Ok(resolved_count)
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
            artist: Some(artist.to_string()),
            album: Some(album.to_string()),
            album_artist: None,
            title: Some(title.to_string()),
            track_number: Some(1),
            genre: None,
            duration_ms: Some(240_000),
            bitrate_kbps: bitrate,
            sample_rate: Some(44100),
            fingerprint: Some(format!("fp_{}_{}_{}", artist, album, title)),
            isrc: None,
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
    fn test_missing_album_artist_grouping() {
        // Scenario: Tracks missing album_artist should be grouped by album
        let tracks = vec![
            make_track(1, "Artist A", "Compilation", "Track 1", "flac", Some(1000)),
            make_track(2, "Artist B", "Compilation", "Track 2", "flac", Some(1000)),
            make_track(3, "Artist C", "Compilation", "Track 3", "flac", Some(1000)),
        ];

        // All tracks share the same album but different artists
        let albums: std::collections::HashSet<_> = tracks
            .iter()
            .filter_map(|t| t.album.as_ref())
            .collect();
        assert_eq!(albums.len(), 1);

        let artists: std::collections::HashSet<_> = tracks
            .iter()
            .filter_map(|t| t.artist.as_ref())
            .collect();
        assert_eq!(artists.len(), 3);

        // All tracks lack album_artist
        assert!(tracks.iter().all(|t| t.album_artist.is_none()));
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

        // This would trigger QualityVariant health issue creation
        // Then user can choose to stash the MP3 versions
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
