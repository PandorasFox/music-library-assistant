//! Health filtering functions for distinguishing legitimate variants from duplicates.
//!
//! These filters are applied during health issue detection to avoid false positives.
//!
//! Note: Tag-based checks (track_number, album) require loading tags from track_tags table.
//! For now, directory-based heuristics are used.

use crate::corpus::db::Track;
use std::collections::HashSet;
use std::path::Path;

/// Check if tracks are from the same album but with different filenames.
/// This indicates they're different songs that happen to share a fingerprint
/// (e.g., ambient albums with similar-sounding tracks).
///
/// Returns `true` if:
/// - All tracks are in the same directory
/// - Tracks have different filenames (heuristic for different songs)
///
/// Note: Track number comparison requires loading tags from database.
/// This function uses filename-based heuristics instead.
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

    // Check if filenames are different (heuristic for different track numbers)
    let filenames: HashSet<_> = tracks
        .iter()
        .filter_map(|t| Path::new(&t.path).file_name())
        .map(|f| f.to_string_lossy().to_string())
        .collect();

    // If we have different filenames in the same directory, these are different songs
    filenames.len() > 1
}

/// Check if all tracks in a group have durations within a tolerance of each other.
///
/// Tracks with significantly different durations are likely not true duplicates
/// despite sharing a fingerprint. Default tolerance is 10%.
pub fn durations_within_tolerance(tracks: &[Track], tolerance_percent: f64) -> bool {
    if tracks.len() < 2 {
        return true;
    }

    // Get all durations
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

    // Calculate variance as percentage
    let variance = (max_dur - min_dur) / min_dur;

    // Return true if variance is within tolerance
    variance <= tolerance_percent
}

/// Detect if tracks represent a legitimate re-release rather than a duplicate.
///
/// A re-release is when the same audio appears on different albums
/// (e.g., Carpenter Brut "Looking For Tracy Tzu" on EP II and TRILOGY).
///
/// Returns `true` if:
/// - Tracks share the same fingerprint
/// - Tracks are in different directories (heuristic for different albums)
/// - This is not a "same album different tracks" case
///
/// Note: Album name comparison requires loading tags from database.
/// This function uses directory-based heuristics instead.
pub fn is_legitimate_rerelease(tracks: &[Track]) -> bool {
    if tracks.len() < 2 {
        return false;
    }

    // First, rule out the "same album different tracks" case
    if is_same_album_different_tracks(tracks) {
        return false;
    }

    // Collect parent directories
    let dirs: HashSet<_> = tracks
        .iter()
        .filter_map(|t| Path::new(&t.path).parent())
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    // Re-release if tracks are in different directories
    // (directory structure indicates separate releases)
    dirs.len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(path: &str) -> Track {
        Track {
            id: None,
            path: path.to_string(),
            source: "test".to_string(),
            inode: 1,
            file_size: 1000,
            file_type: "flac".to_string(),
            duration_ms: Some(180000),
            bitrate_kbps: Some(320),
            sample_rate: Some(44100),
            fingerprint: Some(vec![0xabc123]),
        }
    }

    #[test]
    fn test_same_album_different_tracks() {
        // Same directory, different filenames - should return true
        let tracks = vec![
            make_track("/music/album/01.flac"),
            make_track("/music/album/02.flac"),
        ];
        assert!(is_same_album_different_tracks(&tracks));

        // Different directories - should return false
        let tracks = vec![
            make_track("/music/album1/01.flac"),
            make_track("/music/album2/01.flac"),
        ];
        assert!(!is_same_album_different_tracks(&tracks));

        // Same directory, same filename - should return false
        let tracks = vec![
            make_track("/music/album/song.flac"),
            make_track("/music/album/song.flac"),
        ];
        assert!(!is_same_album_different_tracks(&tracks));
    }

    #[test]
    fn test_legitimate_rerelease() {
        // Same song on different albums (different directories)
        let tracks = vec![
            make_track("/music/EP II/01.flac"),
            make_track("/music/TRILOGY/05.flac"),
        ];
        assert!(is_legitimate_rerelease(&tracks));

        // Same directory - not a re-release
        let tracks = vec![
            make_track("/music/album/song.flac"),
            make_track("/music/album/song_copy.flac"),
        ];
        assert!(!is_legitimate_rerelease(&tracks));
    }

    #[test]
    fn test_durations_within_tolerance() {
        // Within 10% tolerance
        let mut t1 = make_track("/a.flac");
        let mut t2 = make_track("/b.flac");
        t1.duration_ms = Some(180000);
        t2.duration_ms = Some(185000);
        assert!(durations_within_tolerance(&[t1.clone(), t2.clone()], 0.10));

        // Outside 10% tolerance
        t2.duration_ms = Some(220000);
        assert!(!durations_within_tolerance(&[t1, t2], 0.10));
    }
}
