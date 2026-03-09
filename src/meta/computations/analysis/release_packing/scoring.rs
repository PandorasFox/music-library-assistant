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
