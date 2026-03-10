//! External match signal derivation.
//!
//! Compares AcoustID match metadata against corpus tags, emitting
//! ExternalMatch signals that classify each match as ExactMatch,
//! ContentDiff, or MetadataOnly.

use std::collections::HashMap;

use crate::db::queries::external::ExternalMatchRow;
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::external::acoustid::{self, AcoustIdRecording, AcoustIdResponse};
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_corpus_signals, ComputedCorpusSignal};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::{
    ExternalMatchData, ExternalMatchSignal, ExternalTagDiff, MatchClassification};
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

/// Execute DeriveExternalMatches — compare AcoustID metadata against corpus tags.
pub fn execute_derive_external_matches(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::DeriveExternalMatches;

    let sender = require_sender!(computation);

    let source_key = ExternalSource::AcoustID.to_key();

    // 1. Query all external matches for corpus files (ordered by inode, confidence DESC).
    let all_rows = match read_only_db.get_external_matches_for_derivation(source_key) {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query external matches: {}", e),
            );
        }
    };

    if all_rows.is_empty() {
        // No external matches — reconcile with empty set to clear stale signals.
        let (cleared, _, _, _) = reconcile_corpus_signals::<ExternalMatchSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] DeriveExternalMatches: cleared {} stale signals (no external matches)",
                cleared
            ));
        }
        return Result::success(computation, Vec::new());
    }

    // 2. Group by inode, take first per group (highest confidence).
    let best_per_inode = group_best_per_inode(all_rows);

    log_general(format!(
        "[COMPUTE] DeriveExternalMatches: {} inodes with external matches",
        best_per_inode.len()
    ));

    // 3. Load all corpus audio files with tags for O(1) lookup.
    let files_with_tags = match read_only_db.get_all_audio_files_with_tags(Zone::Corpus, false) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query corpus files: {}", e),
            );
        }
    };

    let tag_map: HashMap<i64, HashMap<String, Vec<String>>> = files_with_tags
        .into_iter()
        .map(|(af, tags)| (af.inode(), tags))
        .collect();

    // 4. For each inode with external matches: parse, compare, classify.
    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();
    let mut exact = 0;
    let mut content_diff = 0;
    let mut metadata_only = 0;
    let mut parse_failures = 0;

    for (inode, row) in &best_per_inode {
        // Parse raw_response JSON → typed AcoustIdResponse.
        let response = match &row.raw_response {
            Some(raw) => match serde_json::from_slice::<AcoustIdResponse>(raw) {
                Ok(r) => r,
                Err(_) => {
                    parse_failures += 1;
                    continue;
                }
            },
            None => {
                parse_failures += 1;
                continue;
            }
        };

        // Find the matched recording in the response.
        let recording = match acoustid::find_recording_in_response(&response, &row.recording_id) {
            Some(r) => r,
            None => {
                parse_failures += 1;
                continue;
            }
        };

        // Count total candidates across all results for this response.
        let total_candidates: usize = response.results.iter().map(|r| r.recordings.len()).sum();

        // Get corpus tags for this inode.
        let corpus_tags = tag_map.get(inode);

        // Compare tags and classify.
        let (classification, diffs) = compare_recording_to_corpus(recording, corpus_tags);

        // Pick best release (closest ALBUM match, or first if no ALBUM tag).
        let (release_id, release_group_id) = pick_best_release(recording, corpus_tags);

        match classification {
            MatchClassification::ExactMatch => exact += 1,
            MatchClassification::ContentDiff => content_diff += 1,
            MatchClassification::MetadataOnly => metadata_only += 1,
        }

        let signal = ExternalMatchSignal {
            inode: *inode,
            path: row.path.clone(),
            data: ExternalMatchData {
                source: ExternalSource::AcoustID as u8,
                recording_id: row.recording_id.clone(),
                confidence: row.confidence,
                classification,
                diffs,
                total_candidates,
                release_id,
                release_group_id,
            },
        };

        computed.push(ComputedCorpusSignal::new(
            signal.inode,
            TypedSignalWrite::ExternalMatch(signal),
        ));
    }

    let (cleared, new, updated, unchanged) =
        reconcile_corpus_signals::<ExternalMatchSignal>(read_only_db, &sender, computed, witness);

    log_general(format!(
        "[COMPUTE] DeriveExternalMatches: exact={}, content_diff={}, metadata_only={}, parse_failures={} | \
         signals: cleared={}, new={}, updated={}, unchanged={}",
        exact, content_diff, metadata_only, parse_failures,
        cleared, new, updated, unchanged
    ));

    Result::success(computation, Vec::new())
}

/// Group external match rows by inode, taking the first per inode (highest confidence).
fn group_best_per_inode(rows: Vec<ExternalMatchRow>) -> Vec<(i64, ExternalMatchRow)> {
    let mut best: Vec<(i64, ExternalMatchRow)> = Vec::new();
    let mut last_inode: Option<i64> = None;

    for row in rows {
        if last_inode == Some(row.inode) {
            continue; // Already took highest confidence for this inode.
        }
        last_inode = Some(row.inode);
        best.push((row.inode, row));
    }

    best
}

/// Compare a recording's metadata against corpus tags.
///
/// Returns the overall classification and per-tag diffs.
/// No normalization — raw string equality per the plan.
fn compare_recording_to_corpus(
    recording: &AcoustIdRecording,
    corpus_tags: Option<&HashMap<String, Vec<String>>>,
) -> (MatchClassification, Vec<ExternalTagDiff>) {
    let mut diffs = Vec::new();
    let mut has_content_diff = false;
    let mut has_metadata_only = false;

    let empty_tags: HashMap<String, Vec<String>> = HashMap::new();
    let tags = corpus_tags.unwrap_or(&empty_tags);

    // TITLE comparison.
    if let Some(ref ext_title) = recording.title {
        let corpus_title = tags.get("TITLE").and_then(|v| v.first());
        match corpus_title {
            Some(ct) if ct == ext_title => {} // ExactMatch for this tag.
            Some(ct) => {
                has_content_diff = true;
                diffs.push(ExternalTagDiff {
                    tag_name: "TITLE".to_string(),
                    external_value: ext_title.clone(),
                    corpus_value: Some(ct.clone()),
                });
            }
            None => {
                has_metadata_only = true;
                diffs.push(ExternalTagDiff {
                    tag_name: "TITLE".to_string(),
                    external_value: ext_title.clone(),
                    corpus_value: None,
                });
            }
        }
    }

    // ARTIST comparison.
    if !recording.artists.is_empty() {
        let ext_artist_names: Vec<&str> =
            recording.artists.iter().map(|a| a.name.as_str()).collect();

        let corpus_artists = tags.get("ARTIST");
        match corpus_artists {
            Some(ca) if !ca.is_empty() => {
                if ca.len() == 1 {
                    // Single corpus ARTIST value: join external artists with joinphrase.
                    let joined = join_artists_with_joinphrase(&recording.artists);
                    if ca[0] != joined {
                        has_content_diff = true;
                        diffs.push(ExternalTagDiff {
                            tag_name: "ARTIST".to_string(),
                            external_value: joined,
                            corpus_value: Some(ca[0].clone()),
                        });
                    }
                } else {
                    // Multiple corpus ARTIST values: compare as sets.
                    let mut ext_set: Vec<&str> = ext_artist_names.clone();
                    ext_set.sort();
                    let mut corpus_set: Vec<&str> = ca.iter().map(|s| s.as_str()).collect();
                    corpus_set.sort();

                    if ext_set != corpus_set {
                        has_content_diff = true;
                        let joined = join_artists_with_joinphrase(&recording.artists);
                        diffs.push(ExternalTagDiff {
                            tag_name: "ARTIST".to_string(),
                            external_value: joined,
                            corpus_value: Some(ca.join("; ")),
                        });
                    }
                }
            }
            _ => {
                has_metadata_only = true;
                let joined = join_artists_with_joinphrase(&recording.artists);
                diffs.push(ExternalTagDiff {
                    tag_name: "ARTIST".to_string(),
                    external_value: joined,
                    corpus_value: None,
                });
            }
        }
    }

    // ALBUM comparison (best-matching release title).
    if !recording.releases.is_empty() {
        let corpus_album = tags.get("ALBUM").and_then(|v| v.first());
        let best_release_title =
            pick_best_release_title(recording, corpus_album.map(|s| s.as_str()));

        if let Some(ext_album) = best_release_title {
            match corpus_album {
                Some(ca) if ca == &ext_album => {} // ExactMatch.
                Some(ca) => {
                    has_content_diff = true;
                    diffs.push(ExternalTagDiff {
                        tag_name: "ALBUM".to_string(),
                        external_value: ext_album,
                        corpus_value: Some(ca.clone()),
                    });
                }
                None => {
                    has_metadata_only = true;
                    diffs.push(ExternalTagDiff {
                        tag_name: "ALBUM".to_string(),
                        external_value: ext_album,
                        corpus_value: None,
                    });
                }
            }
        }
    }

    // Overall classification: worst across all compared tags.
    let classification = if has_content_diff {
        MatchClassification::ContentDiff
    } else if has_metadata_only {
        MatchClassification::MetadataOnly
    } else {
        MatchClassification::ExactMatch
    };

    (classification, diffs)
}

/// Join artist names using their joinphrase fields.
fn join_artists_with_joinphrase(artists: &[crate::external::acoustid::AcoustIdArtist]) -> String {
    let mut result = String::new();
    for (i, artist) in artists.iter().enumerate() {
        result.push_str(&artist.name);
        if i < artists.len() - 1 {
            if let Some(ref jp) = artist.joinphrase {
                result.push_str(jp);
            }
        }
    }
    result
}

/// Pick the release whose title best matches corpus ALBUM tag.
///
/// If corpus has an ALBUM tag, pick the release with an exact title match
/// (or the first release if none match exactly).
/// Returns (release_id, release_group_id).
fn pick_best_release(
    recording: &AcoustIdRecording,
    corpus_tags: Option<&HashMap<String, Vec<String>>>,
) -> (Option<String>, Option<String>) {
    if recording.releases.is_empty() {
        // Fall back to release groups if no releases.
        let rg_id = recording.releasegroups.first().map(|rg| rg.id.clone());
        return (None, rg_id);
    }

    let corpus_album = corpus_tags
        .and_then(|t| t.get("ALBUM"))
        .and_then(|v| v.first());

    let best = if let Some(ca) = corpus_album {
        // Prefer exact title match.
        recording
            .releases
            .iter()
            .find(|r| r.title.as_deref() == Some(ca.as_str()))
            .unwrap_or(&recording.releases[0])
    } else {
        &recording.releases[0]
    };

    let release_id = Some(best.id.clone());

    // Find release group for the chosen release (if available).
    let release_group_id = recording.releasegroups.first().map(|rg| rg.id.clone());

    (release_id, release_group_id)
}

/// Pick the best release title for ALBUM comparison.
///
/// If corpus has an ALBUM tag, prefer a release whose title matches.
/// Otherwise, take the first release's title.
fn pick_best_release_title(
    recording: &AcoustIdRecording,
    corpus_album: Option<&str>,
) -> Option<String> {
    if recording.releases.is_empty() {
        return None;
    }

    if let Some(ca) = corpus_album {
        // Check for exact match first.
        if let Some(r) = recording
            .releases
            .iter()
            .find(|r| r.title.as_deref() == Some(ca))
        {
            return r.title.clone();
        }
    }

    // Fall back to first release with a title.
    recording.releases.iter().find_map(|r| r.title.clone())
}
