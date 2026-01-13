//! Fingerprint match filtering.
//!
//! Filters to eliminate false positives in fingerprint-based duplicate detection.

use std::collections::HashSet;
use std::path::Path;

use crate::corpus::db::Track;

/// Check if a group of tracks represents different tracks from the same album.
///
/// This detects false positives where similar-sounding tracks (e.g., ambient music)
/// have matching fingerprints but are actually different songs.
///
/// Returns true if ALL of:
/// - All tracks are in the same directory (same album folder)
/// - Tracks have different track numbers (different songs)
pub fn is_same_album_different_tracks(tracks: &[Track]) -> bool {
    if tracks.len() < 2 {
        return false;
    }

    // Collect parent directories
    let dirs: HashSet<_> = tracks
        .iter()
        .filter_map(|t| Path::new(&t.path).parent())
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    // If tracks are in different directories, this isn't the same-album case
    if dirs.len() != 1 {
        return false;
    }

    // Check if track numbers are different
    let track_nums: HashSet<_> = tracks.iter().filter_map(|t| t.track_number).collect();

    // If we have different track numbers, these are different songs from the same album
    track_nums.len() > 1
}

/// Check if all tracks in a group have durations within a tolerance of each other.
///
/// Returns false if any track's duration differs by more than the tolerance from others.
///
/// This filters out false positives like:
/// - Radio edits vs album versions
/// - Remixes tagged as originals
/// - Different versions with similar fingerprints
pub fn durations_within_tolerance(tracks: &[Track], tolerance_percent: f64) -> bool {
    if tracks.len() < 2 {
        return true;
    }

    let durations: Vec<_> = tracks.iter().filter_map(|t| t.duration_ms).collect();

    // If no durations available, assume they're within tolerance (can't verify)
    if durations.is_empty() {
        return true;
    }

    let min_dur = *durations.iter().min().unwrap() as f64;
    let max_dur = *durations.iter().max().unwrap() as f64;

    // Avoid division by zero
    if min_dur == 0.0 {
        return true;
    }

    let variance = (max_dur - min_dur) / min_dur;
    variance <= tolerance_percent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track_with_details(
        path: &str,
        track_num: Option<i32>,
        duration_ms: Option<i64>,
    ) -> Track {
        Track {
            id: None,
            path: path.to_string(),
            source: "corpus".to_string(),
            inode: 12345,
            file_size: 1000000,
            file_type: "flac".to_string(),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
            album_artist: None,
            title: Some("Track".to_string()),
            track_number: track_num,
            genre: None,
            duration_ms,
            bitrate_kbps: Some(320),
            sample_rate: Some(44100),
            fingerprint: Some("fp123".to_string()),
            isrc: None,
        }
    }

    #[test]
    fn test_is_same_album_different_tracks_true() {
        let tracks = vec![
            make_track_with_details("/album/track17.flac", Some(17), Some(180000)),
            make_track_with_details("/album/track18.flac", Some(18), Some(180000)),
            make_track_with_details("/album/track19.flac", Some(19), Some(180000)),
        ];
        assert!(is_same_album_different_tracks(&tracks));
    }

    #[test]
    fn test_is_same_album_different_tracks_false_different_dirs() {
        let tracks = vec![
            make_track_with_details("/album-a/track.flac", Some(1), Some(180000)),
            make_track_with_details("/album-b/track.flac", Some(1), Some(180000)),
        ];
        assert!(!is_same_album_different_tracks(&tracks));
    }

    #[test]
    fn test_is_same_album_different_tracks_false_same_track_num() {
        let tracks = vec![
            make_track_with_details("/album/track.flac", Some(1), Some(180000)),
            make_track_with_details("/album/track_copy.flac", Some(1), Some(180000)),
        ];
        assert!(!is_same_album_different_tracks(&tracks));
    }

    #[test]
    fn test_durations_within_tolerance_true() {
        let tracks = vec![
            make_track_with_details("/a/track.flac", Some(1), Some(180000)),
            make_track_with_details("/b/track.flac", Some(1), Some(185000)),
        ];
        assert!(durations_within_tolerance(&tracks, 0.10));
    }

    #[test]
    fn test_durations_within_tolerance_false() {
        let tracks = vec![
            make_track_with_details("/a/track.flac", Some(1), Some(180000)),
            make_track_with_details("/b/track.flac", Some(1), Some(220000)),
        ];
        assert!(!durations_within_tolerance(&tracks, 0.10));
    }

    #[test]
    fn test_durations_within_tolerance_no_durations() {
        let tracks = vec![
            make_track_with_details("/a/track.flac", Some(1), None),
            make_track_with_details("/b/track.flac", Some(1), None),
        ];
        assert!(durations_within_tolerance(&tracks, 0.10));
    }

    #[test]
    fn test_durations_within_tolerance_edge_case_exact() {
        let tracks = vec![
            make_track_with_details("/a/track.flac", Some(1), Some(100000)),
            make_track_with_details("/b/track.flac", Some(1), Some(110000)),
        ];
        assert!(durations_within_tolerance(&tracks, 0.10));
    }
}
