//! Scoring functions for release packing candidates.

use crate::config::PackingWeights;
use crate::corpus::tags::TagSet;
use crate::external::musicbrainz;
use crate::meta::signals::data::PackingScoreBreakdown;

use super::types::{CorpusFileInfo, RecordingMatch};

/// Compute duration match score between a corpus file and an MB track.
///
/// Returns a score in [0.0, 1.0] where 1.0 is a perfect match, 0.0 is outside
/// tolerance, and 0.5 is the fallback when either duration is missing.
pub(super) fn compute_duration_match(
    corpus_duration_ms: Option<i64>,
    mb_duration_ms: Option<i64>,
    duration_tolerance_pct: f64,
) -> f64 {
    match (corpus_duration_ms, mb_duration_ms) {
        (Some(corpus_dur), Some(mb_dur)) if mb_dur > 0 => {
            let ratio = (corpus_dur as f64 - mb_dur as f64).abs() / mb_dur as f64;
            if ratio > duration_tolerance_pct {
                0.0
            } else {
                1.0 - (ratio / duration_tolerance_pct)
            }
        }
        _ => 0.5,
    }
}

/// Compute the best title similarity between a corpus title tag and an MB track.
///
/// Compares against both `track.title` and `track.recording.title`, returning
/// the maximum. Returns 0.0 if no corpus title tag is available.
pub(super) fn compute_title_similarity(
    tags: &TagSet,
    track: &musicbrainz::MbTrack,
) -> f64 {
    tags.get("TITLE")
        .map(|t| {
            let track_sim = strsim::normalized_levenshtein(t, &track.title);
            let rec_sim = strsim::normalized_levenshtein(t, &track.recording.title);
            track_sim.max(rec_sim)
        })
        .unwrap_or(0.0)
}

/// Compute scoring breakdown for an elimination match (no AcoustID, tag-based).
///
/// Used when filling remaining release slots via elimination matching — files
/// that weren't matched via AcoustID but reside in the target directory.
pub(super) fn compute_elimination_breakdown(
    tags: &TagSet,
    track: &musicbrainz::MbTrack,
    release_artist: &str,
    release_title: &str,
    duration_ms: Option<i64>,
    duration_tolerance_pct: f64,
) -> PackingScoreBreakdown {
    let mb_dur = track.length.or(track.recording.length);
    let duration_match = compute_duration_match(duration_ms, mb_dur, duration_tolerance_pct);
    let title_match = compute_title_similarity(tags, track);

    let artist_match = tags
        .get("ARTIST")
        .map(|a| strsim::normalized_levenshtein(a, release_artist))
        .unwrap_or(0.0);

    let album_match = tags
        .get("ALBUM")
        .map(|a| strsim::normalized_levenshtein(a, release_title))
        .unwrap_or(0.0);

    let track_number_match = tags
        .get("TRACKNUMBER")
        .and_then(|tn| tn.parse::<u32>().ok())
        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
        .unwrap_or(0.0);

    PackingScoreBreakdown {
        acoustid_confidence: 0.0,
        duration_match,
        title_match,
        artist_match,
        album_match,
        track_number_match,
    }
}

/// Compute the scoring components for a candidate assignment.
pub(super) fn compute_score(
    rec_match: &RecordingMatch,
    track: &musicbrainz::MbTrack,
    release_artist: &str,
    release_title: &str,
    corpus: Option<&CorpusFileInfo>,
    duration_tolerance_pct: f64,
    weights: &PackingWeights,
) -> (f64, PackingScoreBreakdown) {
    let acoustid_confidence = rec_match.confidence;

    let mb_duration_ms = track.length.or(track.recording.length);
    let duration_match = compute_duration_match(
        corpus.and_then(|c| c.duration_ms),
        mb_duration_ms,
        duration_tolerance_pct,
    );

    let empty_tags = TagSet::empty();
    let tags = corpus.map(|c| &c.tags).unwrap_or(&empty_tags);

    let title_match = compute_title_similarity(tags, track);

    let artist_match = tags
        .get("ARTIST")
        .map(|corpus_artist| strsim::normalized_levenshtein(corpus_artist, release_artist))
        .unwrap_or(0.0);

    let album_match = tags
        .get("ALBUM")
        .map(|corpus_album| strsim::normalized_levenshtein(corpus_album, release_title))
        .unwrap_or(0.0);

    let track_number_match = tags
        .get("TRACKNUMBER")
        .and_then(|tn| tn.parse::<u32>().ok())
        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
        .unwrap_or(0.0);

    let breakdown = PackingScoreBreakdown {
        acoustid_confidence,
        duration_match,
        title_match,
        artist_match,
        album_match,
        track_number_match,
    };

    let score = weighted_composite(&breakdown, weights);
    (score, breakdown)
}

/// Compute weighted composite score from breakdown components.
pub(super) fn weighted_composite(b: &PackingScoreBreakdown, w: &PackingWeights) -> f64 {
    w.acoustid_confidence * b.acoustid_confidence
        + w.duration_match * b.duration_match
        + w.title_match * b.title_match
        + w.artist_match * b.artist_match
        + w.album_match * b.album_match
        + w.track_number_match * b.track_number_match
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_weights() -> PackingWeights {
        PackingWeights {
            acoustid_confidence: 1.0,
            duration_match: 1.0,
            title_match: 1.0,
            artist_match: 1.0,
            album_match: 1.0,
            track_number_match: 1.0,
        }
    }

    fn make_track(title: &str, position: u32, length: Option<i64>) -> musicbrainz::MbTrack {
        musicbrainz::MbTrack {
            id: String::new(),
            position,
            number: position.to_string(),
            title: title.to_string(),
            length,
            recording: musicbrainz::MbTrackRecording {
                id: "rec-1".to_string(),
                title: title.to_string(),
                length,
            },
        }
    }

    #[test]
    fn test_weighted_composite() {
        let b = PackingScoreBreakdown {
            acoustid_confidence: 0.5,
            duration_match: 0.8,
            title_match: 0.9,
            artist_match: 0.7,
            album_match: 0.6,
            track_number_match: 1.0,
        };
        let w = PackingWeights {
            acoustid_confidence: 0.30,
            duration_match: 0.30,
            title_match: 0.10,
            artist_match: 0.05,
            album_match: 0.05,
            track_number_match: 0.20,
        };
        let expected = 0.30 * 0.5 + 0.30 * 0.8 + 0.10 * 0.9
            + 0.05 * 0.7 + 0.05 * 0.6 + 0.20 * 1.0;
        let result = weighted_composite(&b, &w);
        assert!((result - expected).abs() < 1e-10);
    }

    #[test]
    fn test_compute_score_perfect_match() {
        let rec = RecordingMatch {
            recording_id: "rec-1".to_string(),
            confidence: 0.95,
        };
        let track = make_track("Test Track", 1, Some(240000));
        let corpus = CorpusFileInfo {
            parent_dir: "/music/album".to_string(),
            tags: TagSet::new(vec![
                ("TITLE".to_string(), "Test Track".to_string()),
                ("ARTIST".to_string(), "Test Artist".to_string()),
                ("ALBUM".to_string(), "Test Album".to_string()),
                ("TRACKNUMBER".to_string(), "1".to_string()),
            ]),
            duration_ms: Some(240000),
        };

        let (score, breakdown) = compute_score(
            &rec,
            &track,
            "Test Artist",
            "Test Album",
            Some(&corpus),
            0.10,
            &uniform_weights(),
        );

        assert!((breakdown.title_match - 1.0).abs() < 1e-10);
        assert!((breakdown.duration_match - 1.0).abs() < 1e-10);
        assert!((breakdown.track_number_match - 1.0).abs() < 1e-10);
        assert!((breakdown.acoustid_confidence - 0.95).abs() < 1e-10);
        assert!(score > 0.0);
    }

    #[test]
    fn test_duration_scoring_within_tolerance() {
        let rec = RecordingMatch {
            recording_id: "rec-1".to_string(),
            confidence: 0.9,
        };
        let track = make_track("Track", 1, Some(200000));
        let corpus = CorpusFileInfo {
            parent_dir: "/music".to_string(),
            tags: TagSet::empty(),
            duration_ms: Some(210000), // 5% off
        };
        let (_, breakdown) = compute_score(
            &rec, &track, "", "", Some(&corpus), 0.10, &uniform_weights(),
        );
        // 5% deviation with 10% tolerance: ratio=0.05, score = 1.0 - (0.05/0.10) = 0.5
        assert!((breakdown.duration_match - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_compute_duration_match_perfect() {
        assert!((compute_duration_match(Some(200000), Some(200000), 0.10) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_duration_match_within_tolerance() {
        // 5% off with 10% tolerance → 0.5
        assert!((compute_duration_match(Some(210000), Some(200000), 0.10) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_compute_duration_match_outside_tolerance() {
        // 15% off with 10% tolerance → 0.0
        assert!(compute_duration_match(Some(230000), Some(200000), 0.10).abs() < 1e-10);
    }

    #[test]
    fn test_compute_duration_match_missing() {
        assert!((compute_duration_match(None, Some(200000), 0.10) - 0.5).abs() < 1e-10);
        assert!((compute_duration_match(Some(200000), None, 0.10) - 0.5).abs() < 1e-10);
        assert!((compute_duration_match(None, None, 0.10) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_elimination_breakdown_perfect_match() {
        let tags = TagSet::new(vec![
            ("TITLE".to_string(), "Test Track".to_string()),
            ("ARTIST".to_string(), "Test Artist".to_string()),
            ("ALBUM".to_string(), "Test Album".to_string()),
            ("TRACKNUMBER".to_string(), "3".to_string()),
        ]);

        let track = make_track("Test Track", 3, Some(240000));
        let breakdown = compute_elimination_breakdown(
            &tags, &track, "Test Artist", "Test Album", Some(240000), 0.10,
        );

        assert!((breakdown.acoustid_confidence).abs() < 1e-10); // always 0
        assert!((breakdown.duration_match - 1.0).abs() < 1e-10);
        assert!((breakdown.title_match - 1.0).abs() < 1e-10);
        assert!((breakdown.artist_match - 1.0).abs() < 1e-10);
        assert!((breakdown.album_match - 1.0).abs() < 1e-10);
        assert!((breakdown.track_number_match - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_elimination_breakdown_no_tags() {
        let tags = TagSet::empty();
        let track = make_track("Track", 1, Some(200000));
        let breakdown = compute_elimination_breakdown(
            &tags, &track, "Artist", "Album", Some(200000), 0.10,
        );

        assert!((breakdown.title_match).abs() < 1e-10);
        assert!((breakdown.artist_match).abs() < 1e-10);
        assert!((breakdown.album_match).abs() < 1e-10);
        assert!((breakdown.track_number_match).abs() < 1e-10);
        // duration still computes normally
        assert!((breakdown.duration_match - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_elimination_breakdown_partial_tags() {
        let tags = TagSet::new(vec![
            ("TITLE".to_string(), "Test Track".to_string()),
        ]);
        // No ARTIST, ALBUM, TRACKNUMBER

        let track = make_track("Test Track", 1, Some(200000));
        let breakdown = compute_elimination_breakdown(
            &tags, &track, "Artist", "Album", Some(200000), 0.10,
        );

        assert!((breakdown.title_match - 1.0).abs() < 1e-10);
        assert!((breakdown.artist_match).abs() < 1e-10);
        assert!((breakdown.album_match).abs() < 1e-10);
        assert!((breakdown.track_number_match).abs() < 1e-10);
    }

    #[test]
    fn test_elimination_breakdown_uses_best_title() {
        let tags = TagSet::new(vec![
            ("TITLE".to_string(), "Recording Title".to_string()),
        ]);

        // Track title differs, but recording title matches
        let track = musicbrainz::MbTrack {
            id: String::new(),
            position: 1,
            number: "1".to_string(),
            title: "Different Track Title".to_string(),
            length: Some(200000),
            recording: musicbrainz::MbTrackRecording {
                id: "rec-1".to_string(),
                title: "Recording Title".to_string(),
                length: Some(200000),
            },
        };

        let breakdown = compute_elimination_breakdown(
            &tags, &track, "", "", Some(200000), 0.10,
        );

        // Should pick recording title similarity (1.0) over track title similarity
        assert!((breakdown.title_match - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_duration_scoring_outside_tolerance() {
        let rec = RecordingMatch {
            recording_id: "rec-1".to_string(),
            confidence: 0.9,
        };
        let track = make_track("Track", 1, Some(200000));
        let corpus = CorpusFileInfo {
            parent_dir: "/music".to_string(),
            tags: TagSet::empty(),
            duration_ms: Some(230000), // 15% off
        };
        let (_, breakdown) = compute_score(
            &rec, &track, "", "", Some(&corpus), 0.10, &uniform_weights(),
        );
        assert!((breakdown.duration_match).abs() < 1e-10); // 0.0
    }
}
