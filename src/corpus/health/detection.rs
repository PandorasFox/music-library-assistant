//! Health issue detection during scanning and mutations.
//!
//! This module detects and creates health issues when tracks are inserted or modified.
//!
//! TODO: Out-of-band tag change detection and resolution
//! - When tags on disk differ from indexed tags, create ChangeType::OutOfBandTagChange
//! - Resolution options: flush index to disk OR accept out-of-band changes
//! - Granularity TBD (potentially per-directory config)
//! - UI pattern similar to canon_flow (bucket selection -> confirmation -> commit)

use crate::db::{
    Database, HealthIssue, HealthIssueType, HealthIssueSeverity, Track, TrackRole,
};
use anyhow::Result;

use super::filter::{durations_within_tolerance, is_legitimate_rerelease, is_same_album_different_tracks};

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
        let variant = crate::db::KnownVariant {
            id: None,
            variant_type: crate::db::VariantType::Rerelease,
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

/// Detect metadata collision issues for a track.
///
/// Metadata collisions are tracks with the same artist/album/title but different fingerprints.
/// These may indicate mistagged files.
pub fn detect_metadata_issues(db: &Database, track: &Track) -> Result<Vec<HealthIssue>> {
    // Build metadata key from artist/album/title
    let artist = track.artist.as_deref().unwrap_or("");
    let album = track.album.as_deref().unwrap_or("");
    let title = track.title.as_deref().unwrap_or("");

    // Skip if no meaningful metadata
    if artist.is_empty() && title.is_empty() {
        return Ok(vec![]);
    }

    // Normalize key: lowercase, trimmed
    let metadata_key = format!(
        "{}|{}|{}",
        artist.to_lowercase().trim(),
        album.to_lowercase().trim(),
        title.to_lowercase().trim()
    );

    // Find tracks with same metadata
    let matching_tracks = db.get_tracks_by_metadata(artist, album, title)?;

    // Need at least 2 tracks for a collision
    if matching_tracks.len() < 2 {
        return Ok(vec![]);
    }

    // Check if fingerprints differ (indicating potential mistag, not just duplicate)
    let fingerprints: std::collections::HashSet<_> = matching_tracks
        .iter()
        .filter_map(|t| t.fingerprint.as_ref())
        .collect();

    // If all fingerprints are the same, this is a fingerprint dup, not metadata collision
    if fingerprints.len() <= 1 {
        return Ok(vec![]);
    }

    // Check if issue already exists
    if let Some(existing) = db.get_health_issue_by_key(HealthIssueType::MetadataDuplicate, &metadata_key)? {
        return Ok(vec![existing]);
    }

    // Create new health issue
    let issue = HealthIssue {
        id: None,
        issue_type: HealthIssueType::MetadataDuplicate,
        issue_key: metadata_key,
        severity: HealthIssueSeverity::ManualReview,
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

    // Re-run metadata detection
    detect_metadata_issues(db, &track)?;

    Ok(())
}

/// Determine severity of a duplicate issue based on track characteristics.
fn determine_duplicate_severity(tracks: &[Track]) -> HealthIssueSeverity {
    use crate::deduplication::{compare_track_quality, QualityVerdict};

    // Use the compare_track_quality function which takes a slice and returns a verdict
    match compare_track_quality(tracks) {
        QualityVerdict::ClearWinner(_) => HealthIssueSeverity::AutoResolvable,
        QualityVerdict::Equivalent | QualityVerdict::Indeterminate => {
            HealthIssueSeverity::ManualReview
        }
    }
}
