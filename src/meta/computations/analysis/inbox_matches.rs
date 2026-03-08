//! Inbox-to-corpus fingerprint match detection.
//!
//! Compares inbox files against corpus files by fingerprint similarity
//! (reusing the same threshold and duration tolerance as duplicate detection).
//! Emits InboxCorpusMatchSignal for inbox files that have corpus matches,
//! including a pre-computed quality classification (Better/Equivalent/Subpar).

use std::time::Instant;

use crate::db::types::Zone;
use crate::db::write_thread;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_corpus_signals, ComputedCorpusSignal};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    CorpusMatchQuality, InboxCorpusMatch, InboxCorpusMatchData, InboxCorpusMatchSignal,
    TypedSignalWrite,
};

use super::duplicates::{fingerprint_similarity, quality_tier_of};
use super::{Computation, Result};

/// Execute DetectInboxCorpusMatches — find inbox files that match corpus files
/// by fingerprint+duration similarity.
///
/// For each inbox file with a fingerprint, compares against all corpus files
/// within duration tolerance, using the same similarity threshold as duplicate
/// detection. Emits InboxCorpusMatchSignal for matches.
pub fn execute_detect_inbox_corpus_matches(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectInboxCorpusMatches;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Load config for similarity threshold and duration tolerance
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    let similarity_threshold = config
        .opinions
        .duplicate_analysis
        .fingerprint_similarity_threshold;
    let duration_tolerance_ms = config.opinions.duplicate_analysis.duration_tolerance_ms;
    let bitrate_fuzz_percent = config
        .opinions
        .quality_resolution
        .inbox_bitrate_fuzz_percent;

    // Get all inbox audio files with fingerprints
    let inbox_audio = match read_only_db.get_all_audio_files(Zone::Inbox, true) {
        Ok(files) => files,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query inbox audio files: {}", e),
            );
        }
    };

    let inbox_fingerprinted: Vec<_> = inbox_audio
        .into_iter()
        .filter(|af| af.audio.fingerprint.is_some() && af.audio.duration_ms.is_some())
        .collect();

    if inbox_fingerprinted.is_empty() {
        log_general("[COMPUTE] DetectInboxCorpusMatches: no fingerprinted inbox files");
        // Reconcile with empty set to clear any stale signals
        let (cleared, _, _, _) = reconcile_corpus_signals::<InboxCorpusMatchSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] DetectInboxCorpusMatches: cleared {} stale signals",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Get all corpus audio files with fingerprints
    let corpus_audio = match read_only_db.get_all_audio_files(Zone::Corpus, true) {
        Ok(files) => files,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query corpus audio files: {}", e),
            );
        }
    };

    let corpus_fingerprinted: Vec<_> = corpus_audio
        .into_iter()
        .filter(|af| af.audio.fingerprint.is_some() && af.audio.duration_ms.is_some())
        .collect();

    if corpus_fingerprinted.is_empty() {
        log_general("[COMPUTE] DetectInboxCorpusMatches: no fingerprinted corpus files");
        let (cleared, _, _, _) = reconcile_corpus_signals::<InboxCorpusMatchSignal>(
            read_only_db,
            &sender,
            Vec::new(),
            witness,
        );
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] DetectInboxCorpusMatches: cleared {} stale signals",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    log_general(format!(
        "[COMPUTE] DetectInboxCorpusMatches: {} inbox files, {} corpus files",
        inbox_fingerprinted.len(),
        corpus_fingerprinted.len()
    ));

    // Sort corpus files by duration for efficient range scanning
    let mut corpus_sorted: Vec<_> = corpus_fingerprinted.iter().collect();
    corpus_sorted.sort_by_key(|af| af.audio.duration_ms.unwrap_or(0));

    let mut computed: Vec<ComputedCorpusSignal> = Vec::new();

    // For each inbox file, find corpus files within duration tolerance
    for inbox_file in &inbox_fingerprinted {
        let inbox_fp = inbox_file.audio.fingerprint.as_ref().unwrap();
        let inbox_duration = inbox_file.audio.duration_ms.unwrap();

        let mut corpus_matches = Vec::new();

        // Binary search for the start of the duration window
        let min_duration = inbox_duration - duration_tolerance_ms;
        let max_duration = inbox_duration + duration_tolerance_ms;

        let start_idx =
            corpus_sorted.partition_point(|af| af.audio.duration_ms.unwrap_or(0) < min_duration);

        // Scan forward through the duration window
        for corpus_file in corpus_sorted[start_idx..].iter() {
            let corpus_duration = corpus_file.audio.duration_ms.unwrap_or(0);
            if corpus_duration > max_duration {
                break; // Past the window
            }

            let corpus_fp = corpus_file.audio.fingerprint.as_ref().unwrap();
            let similarity = fingerprint_similarity(inbox_fp, corpus_fp);

            if similarity >= similarity_threshold {
                corpus_matches.push(InboxCorpusMatch {
                    corpus_inode: corpus_file.inode(),
                    corpus_path: corpus_file.path().to_string(),
                    similarity,
                });
            }
        }

        if !corpus_matches.is_empty() {
            let inode = inbox_file.inode();

            // Classify inbox quality relative to best corpus match
            let inbox_tier = quality_tier_of(
                &inbox_file.audio.file_type,
                inbox_file.audio.bitrate_kbps,
                inbox_file.audio.sample_rate,
            );

            let best_corpus_tier = corpus_matches
                .iter()
                .filter_map(|cm| {
                    // Look up the corpus file's audio info from the sorted vec
                    corpus_fingerprinted
                        .iter()
                        .find(|cf| cf.inode() == cm.corpus_inode)
                        .map(|cf| {
                            quality_tier_of(
                                &cf.audio.file_type,
                                cf.audio.bitrate_kbps,
                                cf.audio.sample_rate,
                            )
                        })
                })
                .max();

            let classification = match best_corpus_tier {
                Some(corpus_tier) => {
                    // Apply bitrate fuzz for same-format, same-samplerate comparisons
                    if inbox_tier.format_class == corpus_tier.format_class
                        && inbox_tier.sample_rate == corpus_tier.sample_rate
                        && bitrate_fuzz_percent > 0.0
                    {
                        let max_br = inbox_tier.bitrate.max(corpus_tier.bitrate) as f64;
                        let diff = (inbox_tier.bitrate - corpus_tier.bitrate).unsigned_abs() as f64;
                        if max_br > 0.0 && (diff / max_br * 100.0) <= bitrate_fuzz_percent {
                            CorpusMatchQuality::Equivalent
                        } else {
                            match inbox_tier.cmp(&corpus_tier) {
                                std::cmp::Ordering::Greater => CorpusMatchQuality::Better,
                                std::cmp::Ordering::Equal => CorpusMatchQuality::Equivalent,
                                std::cmp::Ordering::Less => CorpusMatchQuality::Subpar,
                            }
                        }
                    } else {
                        match inbox_tier.cmp(&corpus_tier) {
                            std::cmp::Ordering::Greater => CorpusMatchQuality::Better,
                            std::cmp::Ordering::Equal => CorpusMatchQuality::Equivalent,
                            std::cmp::Ordering::Less => CorpusMatchQuality::Subpar,
                        }
                    }
                }
                // No quality info for corpus matches — default to Equivalent (safe to stash)
                None => CorpusMatchQuality::Equivalent,
            };

            computed.push(ComputedCorpusSignal::new(
                inode,
                TypedSignalWrite::InboxCorpusMatch(InboxCorpusMatchSignal {
                    inode,
                    path: inbox_file.path().to_string(),
                    classification,
                    data: InboxCorpusMatchData {
                        corpus_matches,
                        classification,
                    },
                }),
            ));
        }
    }

    let match_count = computed.len();
    let (cleared, new_count, updated, unchanged) = reconcile_corpus_signals::<InboxCorpusMatchSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectInboxCorpusMatches: {} inbox files matched corpus (cleared={}, new={}, updated={}, unchanged={})",
        match_count, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
