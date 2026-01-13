//! Quality-based duplicate resolution.
//!
//! Automatic resolution of duplicates based on file format and bitrate quality.

use std::collections::HashMap;

use crate::corpus::db::Track;

/// File format quality tiers (higher = better).
///
/// - Tier 3: Lossless formats (FLAC, WAV, APE, WV)
/// - Tier 2: High-quality lossy (OGG, OPUS, M4A/AAC at high bitrate)
/// - Tier 1: Legacy lossy (MP3, WMA, low-bitrate AAC)
fn format_quality_tier(file_type: &str) -> u8 {
    match file_type.to_lowercase().as_str() {
        // Tier 3: Lossless
        "flac" | "wav" | "ape" | "wv" | "alac" => 3,
        // Tier 2: High-quality lossy
        "ogg" | "opus" | "m4a" | "aac" => 2,
        // Tier 1: Legacy lossy
        "mp3" | "wma" => 1,
        // Unknown formats default to middle tier
        _ => 2,
    }
}

/// Result of quality-based comparison between tracks.
#[derive(Debug, Clone, PartialEq)]
pub enum QualityVerdict {
    /// One track is clearly superior (index of winner in the slice)
    ClearWinner(usize),
    /// Tracks are equivalent quality (same tier, similar bitrate)
    Equivalent,
    /// Cannot determine (missing data)
    Indeterminate,
}

/// Compare tracks by quality and return verdict.
///
/// Uses format tier first, then bitrate within same tier.
///
/// A track is a "clear winner" if:
/// - It's in a higher format tier (e.g., FLAC vs MP3), OR
/// - Same tier but significantly higher bitrate (>50% difference)
pub fn compare_track_quality(tracks: &[Track]) -> QualityVerdict {
    if tracks.len() < 2 {
        return QualityVerdict::Indeterminate;
    }

    let metrics: Vec<(usize, u8, Option<i32>)> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, format_quality_tier(&t.file_type), t.bitrate_kbps))
        .collect();

    let max_tier = metrics.iter().map(|(_, tier, _)| *tier).max().unwrap_or(0);
    let min_tier = metrics.iter().map(|(_, tier, _)| *tier).min().unwrap_or(0);

    // If there's a tier difference, the highest tier wins
    if max_tier > min_tier {
        let winners: Vec<_> = metrics
            .iter()
            .filter(|(_, tier, _)| *tier == max_tier)
            .collect();

        if winners.len() == 1 {
            return QualityVerdict::ClearWinner(winners[0].0);
        }

        // Multiple tracks at highest tier - compare bitrates
        let winner = winners
            .iter()
            .max_by_key(|(_, _, br)| br.unwrap_or(0))
            .map(|(i, _, _)| *i);

        if let Some(idx) = winner {
            return QualityVerdict::ClearWinner(idx);
        }
    }

    // Same tier - check bitrate difference
    let bitrates: Vec<_> = metrics
        .iter()
        .filter_map(|(i, _, br)| br.map(|b| (*i, b)))
        .collect();

    if bitrates.len() < 2 {
        return QualityVerdict::Indeterminate;
    }

    let max_br = bitrates.iter().map(|(_, br)| *br).max().unwrap();
    let min_br = bitrates.iter().map(|(_, br)| *br).min().unwrap();

    // >50% bitrate difference is significant
    if min_br > 0 && (max_br - min_br) as f64 / min_br as f64 > 0.5 {
        let winner_idx = bitrates
            .iter()
            .find(|(_, br)| *br == max_br)
            .map(|(i, _)| *i);

        if let Some(idx) = winner_idx {
            return QualityVerdict::ClearWinner(idx);
        }
    }

    QualityVerdict::Equivalent
}

/// Filter a group of duplicate tracks to find auto-resolvable ones.
///
/// Returns `Some((winner_track, loser_tracks))` if there's a clear quality winner.
/// Returns `None` if manual resolution is needed.
pub fn auto_resolve_by_quality(tracks: &[Track]) -> Option<(&Track, Vec<&Track>)> {
    if tracks.len() < 2 {
        return None;
    }

    match compare_track_quality(tracks) {
        QualityVerdict::ClearWinner(idx) => {
            let winner = &tracks[idx];
            let losers: Vec<_> = tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .map(|(_, t)| t)
                .collect();
            Some((winner, losers))
        }
        _ => None,
    }
}

/// Statistics from auto-resolution process.
#[derive(Debug, Clone, Default)]
pub struct AutoResolutionStats {
    pub groups_analyzed: usize,
    pub groups_auto_resolved: usize,
    pub groups_need_manual: usize,
    pub files_marked_inferior: usize,
    pub files_kept: usize,
    /// Breakdown by winning format
    pub wins_by_format: HashMap<String, usize>,
}

/// Analyze fingerprint duplicate groups and categorize by resolution type.
///
/// Returns `(auto_resolvable, manual_needed)` groups.
#[allow(clippy::type_complexity)]
pub fn categorize_duplicates_by_quality<'a>(
    groups: &'a [(&'a String, &'a Vec<&'a Track>)],
) -> (
    Vec<(&'a String, &'a Track, Vec<&'a Track>)>,
    Vec<&'a (&'a String, &'a Vec<&'a Track>)>,
) {
    let mut auto_resolvable = Vec::new();
    let mut manual_needed = Vec::new();

    for group in groups {
        let (fp, tracks) = group;
        let track_slice: Vec<Track> = tracks.iter().map(|t| (*t).clone()).collect();

        match compare_track_quality(&track_slice) {
            QualityVerdict::ClearWinner(idx) => {
                let winner = tracks[idx];
                let losers: Vec<_> = tracks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != idx)
                    .map(|(_, t)| *t)
                    .collect();
                auto_resolvable.push((*fp, winner, losers));
            }
            _ => {
                manual_needed.push(group);
            }
        }
    }

    (auto_resolvable, manual_needed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track_with_quality(path: &str, file_type: &str, bitrate: Option<i32>) -> Track {
        Track {
            id: None,
            path: path.to_string(),
            source: "corpus".to_string(),
            inode: 12345,
            file_size: 1000000,
            file_type: file_type.to_string(),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
            album_artist: None,
            title: Some("Track".to_string()),
            track_number: Some(1),
            genre: None,
            duration_ms: Some(180000),
            bitrate_kbps: bitrate,
            sample_rate: Some(44100),
            fingerprint: Some("fp123".to_string()),
            isrc: None,
        }
    }

    #[test]
    fn test_compare_quality_flac_vs_mp3() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(320)),
            make_track_with_quality("/b/track.flac", "flac", Some(900)),
        ];
        assert_eq!(compare_track_quality(&tracks), QualityVerdict::ClearWinner(1));
    }

    #[test]
    fn test_compare_quality_ogg_vs_mp3() {
        let tracks = vec![
            make_track_with_quality("/a/track.ogg", "ogg", Some(256)),
            make_track_with_quality("/b/track.mp3", "mp3", Some(320)),
        ];
        assert_eq!(compare_track_quality(&tracks), QualityVerdict::ClearWinner(0));
    }

    #[test]
    fn test_compare_quality_same_tier_big_bitrate_diff() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(128)),
            make_track_with_quality("/b/track.mp3", "mp3", Some(320)),
        ];
        assert_eq!(compare_track_quality(&tracks), QualityVerdict::ClearWinner(1));
    }

    #[test]
    fn test_compare_quality_same_tier_similar_bitrate() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(256)),
            make_track_with_quality("/b/track.mp3", "mp3", Some(320)),
        ];
        assert_eq!(compare_track_quality(&tracks), QualityVerdict::Equivalent);
    }

    #[test]
    fn test_compare_quality_missing_bitrate() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", None),
            make_track_with_quality("/b/track.mp3", "mp3", Some(320)),
        ];
        assert_eq!(
            compare_track_quality(&tracks),
            QualityVerdict::Indeterminate
        );
    }

    #[test]
    fn test_compare_quality_three_way_mixed() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(320)),
            make_track_with_quality("/b/track.ogg", "ogg", Some(256)),
            make_track_with_quality("/c/track.flac", "flac", Some(900)),
        ];
        assert_eq!(compare_track_quality(&tracks), QualityVerdict::ClearWinner(2));
    }

    #[test]
    fn test_auto_resolve_by_quality_winner() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(128)),
            make_track_with_quality("/b/track.flac", "flac", Some(900)),
        ];
        let result = auto_resolve_by_quality(&tracks);
        assert!(result.is_some());
        let (winner, losers) = result.unwrap();
        assert_eq!(winner.file_type, "flac");
        assert_eq!(losers.len(), 1);
        assert_eq!(losers[0].file_type, "mp3");
    }

    #[test]
    fn test_auto_resolve_by_quality_no_winner() {
        let tracks = vec![
            make_track_with_quality("/a/track.mp3", "mp3", Some(256)),
            make_track_with_quality("/b/track.mp3", "mp3", Some(320)),
        ];
        assert!(auto_resolve_by_quality(&tracks).is_none());
    }
}
