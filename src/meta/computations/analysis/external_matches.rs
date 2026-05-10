//! External match signal derivation.
//!
//! For each (corpus inode → AcoustID recording_id) linkage in `external_matches`,
//! resolves the recording's metadata from `mb_recording_cache` and compares
//! against the corpus tags, emitting an `ExternalMatchSignal` classified as
//! ExactMatch, ContentDiff, or MetadataOnly.
//!
//! ## Why MB cache, not AcoustID metadata
//!
//! AcoustID returns recording MBIDs reliably but the metadata it ships alongside
//! is un-localized and picked from an arbitrary track that the recording *might*
//! be — frequently wrong. We deliberately ignore it and use the MusicBrainz cache
//! as the authoritative source of recording title, artist credits, and releases.

use std::collections::HashMap;

use crate::db::queries::external::ExternalMatchRow;
use crate::db::types::Zone;
use crate::external::musicbrainz::{parse_recording, MbArtistCredit, MbRecording, MbReleaseRef};
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_corpus_signals, ComputedCorpusSignal};
use crate::meta::computations::traits::ComputationContext;
use crate::meta::external::ExternalSource;
use crate::meta::signals::data::{
    ExternalMatchData, ExternalMatchSignal, ExternalTagDiff, MatchClassification,
};
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

/// Execute DeriveExternalMatches — compare MB-resolved recording metadata
/// (for AcoustID-linked inodes) against corpus tags.
pub fn execute_derive_external_matches(ctx: &ComputationContext<'_>) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DeriveExternalMatches;

    let sender = require_sender!(computation);

    let source_key = ExternalSource::AcoustID.to_key();

    // 1. Query all external matches for corpus files (ordered by inode, confidence DESC).
    let all_rows = match read_only_db.get_external_matches_for_corpus(source_key) {
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
        let stats = reconcile_corpus_signals::<ExternalMatchSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if stats.cleared > 0 {
            log_general(format!(
                "[COMPUTE] DeriveExternalMatches: cleared {} stale signals (no external matches)",
                stats.cleared
            ));
        }
        return Result::success(computation, Vec::new());
    }

    // 2. Group by inode → (best row, total candidate count for that inode).
    let (best_per_inode, candidates_per_inode) = group_best_per_inode(all_rows);

    log_general(format!(
        "[COMPUTE] DeriveExternalMatches: {} inodes with external matches",
        best_per_inode.len()
    ));

    // 3. Load all corpus audio files with tags for O(1) lookup.
    let files_with_tags = match read_only_db.get_all_audio_files_with_tags(Zone::Corpus, false) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(computation, format!("Failed to query corpus files: {}", e));
        }
    };

    let tag_map: HashMap<i64, HashMap<String, Vec<String>>> = files_with_tags
        .into_iter()
        .map(|(af, tags)| (af.inode(), tags))
        .collect();

    // 4. For each inode: pull recording from mb_recording_cache, compare, classify.
    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();
    let mut exact = 0;
    let mut content_diff = 0;
    let mut metadata_only = 0;
    let mut mb_cache_missing = 0;
    let mut mb_parse_failures = 0;

    for (inode, row) in &best_per_inode {
        let recording_json = match read_only_db.get_mb_recording_cache(&row.recording_id) {
            Ok(Some((json, _))) => json,
            Ok(None) => {
                // MB recording fetch is asynchronous — the AcoustID match landed
                // first, the MB recording will catch up on a later fetch cycle.
                // Don't emit a signal for now; the next derivation pass will pick it up.
                mb_cache_missing += 1;
                continue;
            }
            Err(_) => {
                mb_cache_missing += 1;
                continue;
            }
        };

        let recording = match parse_recording(&recording_json) {
            Ok(r) => r,
            Err(_) => {
                mb_parse_failures += 1;
                continue;
            }
        };

        let total_candidates = *candidates_per_inode.get(inode).unwrap_or(&1);
        let corpus_tags = tag_map.get(inode);

        let (classification, diffs) = compare_recording_to_corpus(&recording, corpus_tags);
        let (release_id, release_group_id) = pick_best_release(&recording, corpus_tags);

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

    let stats =
        reconcile_corpus_signals::<ExternalMatchSignal>(read_only_db, &sender, computed, witness);

    log_general(format!(
        "[COMPUTE] DeriveExternalMatches: exact={}, content_diff={}, metadata_only={}, \
         mb_cache_missing={}, mb_parse_failures={} | signals: {}",
        exact, content_diff, metadata_only, mb_cache_missing, mb_parse_failures, stats,
    ));

    Result::success(computation, Vec::new())
}

/// Group external match rows by inode.
///
/// Returns:
/// - `best`: one (inode, highest-confidence row) per inode (rows are pre-sorted
///   by confidence DESC, so the first occurrence wins)
/// - `total_candidates`: total row count per inode, used as the
///   `total_candidates` field on the emitted signal
fn group_best_per_inode(
    rows: Vec<ExternalMatchRow>,
) -> (Vec<(i64, ExternalMatchRow)>, HashMap<i64, usize>) {
    let mut best: Vec<(i64, ExternalMatchRow)> = Vec::new();
    let mut last_inode: Option<i64> = None;
    let mut totals: HashMap<i64, usize> = HashMap::new();

    for row in rows {
        *totals.entry(row.inode).or_insert(0) += 1;
        if last_inode == Some(row.inode) {
            continue;
        }
        last_inode = Some(row.inode);
        best.push((row.inode, row));
    }

    (best, totals)
}

/// Compare a recording's metadata against corpus tags.
///
/// Returns the overall classification and per-tag diffs.
/// No normalization — raw string equality per the plan.
fn compare_recording_to_corpus(
    recording: &MbRecording,
    corpus_tags: Option<&HashMap<String, Vec<String>>>,
) -> (MatchClassification, Vec<ExternalTagDiff>) {
    let mut diffs = Vec::new();
    let mut has_content_diff = false;
    let mut has_metadata_only = false;

    let empty_tags: HashMap<String, Vec<String>> = HashMap::new();
    let tags = corpus_tags.unwrap_or(&empty_tags);

    // TITLE comparison.
    let ext_title = &recording.title;
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

    // ARTIST comparison.
    if !recording.artist_credit.is_empty() {
        let ext_artist_names: Vec<&str> = recording
            .artist_credit
            .iter()
            .map(|c| c.name.as_str())
            .collect();

        let corpus_artists = tags.get("ARTIST");
        match corpus_artists {
            Some(ca) if !ca.is_empty() => {
                if ca.len() == 1 {
                    // Single corpus ARTIST value: join external artists with joinphrase.
                    let joined = join_mb_artist_credits(&recording.artist_credit);
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
                        let joined = join_mb_artist_credits(&recording.artist_credit);
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
                let joined = join_mb_artist_credits(&recording.artist_credit);
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

/// Join MB artist credits using their joinphrase fields.
///
/// Mirrors the prior AcoustID-based join: name1 + joinphrase1 + name2 + joinphrase2 + ... + nameN.
/// The last entry's joinphrase is omitted (MB usually leaves it empty anyway).
fn join_mb_artist_credits(credits: &[MbArtistCredit]) -> String {
    let mut result = String::new();
    for (i, credit) in credits.iter().enumerate() {
        result.push_str(&credit.name);
        if i < credits.len() - 1 {
            result.push_str(&credit.joinphrase);
        }
    }
    result
}

/// Pick the release whose title best matches corpus ALBUM tag.
///
/// If corpus has an ALBUM tag, prefer a release with an exact title match;
/// otherwise fall back to the first release. Returns (release_id, release_group_id).
fn pick_best_release(
    recording: &MbRecording,
    corpus_tags: Option<&HashMap<String, Vec<String>>>,
) -> (Option<String>, Option<String>) {
    if recording.releases.is_empty() {
        return (None, None);
    }

    let corpus_album = corpus_tags
        .and_then(|t| t.get("ALBUM"))
        .and_then(|v| v.first());

    let best: &MbReleaseRef = if let Some(ca) = corpus_album {
        recording
            .releases
            .iter()
            .find(|r| r.title.as_deref() == Some(ca.as_str()))
            .unwrap_or(&recording.releases[0])
    } else {
        &recording.releases[0]
    };

    let release_id = Some(best.id.clone());
    let release_group_id = best.release_group.as_ref().map(|rg| rg.id.clone());

    (release_id, release_group_id)
}

/// Pick the best release title for ALBUM comparison.
///
/// If corpus has an ALBUM tag, prefer a release whose title matches.
/// Otherwise, take the first release's title.
fn pick_best_release_title(
    recording: &MbRecording,
    corpus_album: Option<&str>,
) -> Option<String> {
    if recording.releases.is_empty() {
        return None;
    }

    if let Some(ca) = corpus_album {
        if let Some(r) = recording
            .releases
            .iter()
            .find(|r| r.title.as_deref() == Some(ca))
        {
            return r.title.clone();
        }
    }

    recording.releases.iter().find_map(|r| r.title.clone())
}
