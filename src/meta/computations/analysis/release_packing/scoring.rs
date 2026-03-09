//! Scoring functions for release packing candidates.

use crate::config::PackingWeights;
use crate::external::musicbrainz;
use crate::meta::signals::data::PackingScoreBreakdown;

use super::types::{CorpusFileInfo, RecordingMatch};

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

    // Duration match
    let mb_duration_ms = track.length.or(track.recording.length);
    let duration_match = match (corpus.and_then(|c| c.duration_ms), mb_duration_ms) {
        (Some(corpus_dur), Some(mb_dur)) if mb_dur > 0 => {
            let ratio = (corpus_dur as f64 - mb_dur as f64).abs() / mb_dur as f64;
            if ratio > duration_tolerance_pct {
                0.0
            } else {
                1.0 - (ratio / duration_tolerance_pct)
            }
        }
        _ => 0.5,
    };

    // Tag similarity
    let title_sim = corpus
        .and_then(|c| c.tags.get("TITLE"))
        .and_then(|v| v.first())
        .map(|corpus_title| {
            let track_sim = strsim::normalized_levenshtein(corpus_title, &track.title);
            let rec_sim = strsim::normalized_levenshtein(corpus_title, &track.recording.title);
            track_sim.max(rec_sim)
        })
        .unwrap_or(0.0);

    let artist_sim = corpus
        .and_then(|c| c.tags.get("ARTIST"))
        .and_then(|v| v.first())
        .map(|corpus_artist| strsim::normalized_levenshtein(corpus_artist, release_artist))
        .unwrap_or(0.0);

    let album_sim = corpus
        .and_then(|c| c.tags.get("ALBUM"))
        .and_then(|v| v.first())
        .map(|corpus_album| strsim::normalized_levenshtein(corpus_album, release_title))
        .unwrap_or(0.0);

    // Track number match
    let track_number_match = corpus
        .and_then(|c| c.tags.get("TRACKNUMBER"))
        .and_then(|v| v.first())
        .and_then(|tn| tn.parse::<u32>().ok())
        .map(|tn| if tn == track.position { 1.0 } else { 0.0 })
        .unwrap_or(0.0);

    let breakdown = PackingScoreBreakdown {
        acoustid_confidence,
        duration_match,
        title_match: title_sim,
        artist_match: artist_sim,
        album_match: album_sim,
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
    use std::collections::HashMap;

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
            tags: {
                let mut t = HashMap::new();
                t.insert("TITLE".to_string(), vec!["Test Track".to_string()]);
                t.insert("ARTIST".to_string(), vec!["Test Artist".to_string()]);
                t.insert("ALBUM".to_string(), vec!["Test Album".to_string()]);
                t.insert("TRACKNUMBER".to_string(), vec!["1".to_string()]);
                t
            },
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
            tags: HashMap::new(),
            duration_ms: Some(210000), // 5% off
        };
        let (_, breakdown) = compute_score(
            &rec, &track, "", "", Some(&corpus), 0.10, &uniform_weights(),
        );
        // 5% deviation with 10% tolerance: ratio=0.05, score = 1.0 - (0.05/0.10) = 0.5
        assert!((breakdown.duration_match - 0.5).abs() < 1e-10);
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
            tags: HashMap::new(),
            duration_ms: Some(230000), // 15% off
        };
        let (_, breakdown) = compute_score(
            &rec, &track, "", "", Some(&corpus), 0.10, &uniform_weights(),
        );
        assert!((breakdown.duration_match).abs() < 1e-10); // 0.0
    }
}
