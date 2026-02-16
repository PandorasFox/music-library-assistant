//! Inbox-to-corpus fingerprint match detection.
//!
//! Compares inbox files against corpus files by fingerprint similarity
//! (reusing the same threshold and duration tolerance as duplicate detection).
//! Emits InboxCorpusMatchSignal for inbox files that have corpus matches.

use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::corpus::db::types::Zone;
use crate::meta::signals::data::{
    TypedSignalWrite, InboxCorpusMatchSignal, InboxCorpusMatchData, InboxCorpusMatch,
};
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::duplicates::fingerprint_similarity;
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

    let sender = match db_thread::signal_sender() {
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

    let similarity_threshold = config.opinions.duplicate_analysis.fingerprint_similarity_threshold;
    let duration_tolerance_ms = config.opinions.duplicate_analysis.duration_tolerance_ms;

    // Get all inbox audio files with fingerprints
    let inbox_audio = match read_only_db.get_all_audio_files(Zone::Inbox) {
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
        // Clear any stale inbox corpus match signals
        sender.clear_all_of_corpus_type::<InboxCorpusMatchSignal>(witness);
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Get all corpus audio files with fingerprints
    let corpus_audio = match read_only_db.get_all_audio_files(Zone::Corpus) {
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
        sender.clear_all_of_corpus_type::<InboxCorpusMatchSignal>(witness);
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

    // Clear existing InboxCorpusMatch signals (full recompute)
    sender.clear_all_of_corpus_type::<InboxCorpusMatchSignal>(witness);

    let mut match_count = 0;

    // For each inbox file, find corpus files within duration tolerance
    for inbox_file in &inbox_fingerprinted {
        let inbox_fp = inbox_file.audio.fingerprint.as_ref().unwrap();
        let inbox_duration = inbox_file.audio.duration_ms.unwrap();

        let mut corpus_matches = Vec::new();

        // Binary search for the start of the duration window
        let min_duration = inbox_duration - duration_tolerance_ms;
        let max_duration = inbox_duration + duration_tolerance_ms;

        let start_idx = corpus_sorted
            .partition_point(|af| af.audio.duration_ms.unwrap_or(0) < min_duration);

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
            match_count += 1;

            sender.write_typed_signal(
                TypedSignalWrite::InboxCorpusMatch(InboxCorpusMatchSignal {
                    inode: inbox_file.inode(),
                    path: inbox_file.path().to_string(),
                    data: InboxCorpusMatchData { corpus_matches },
                }),
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DetectInboxCorpusMatches: {} inbox files matched corpus",
        match_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
