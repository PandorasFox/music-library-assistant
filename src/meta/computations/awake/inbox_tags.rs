//! Inbox tag canonicity detection.
//!
//! Compares inbox tag values against the corpus's established vocabulary.
//! Surfaces signals when inbox files use spellings that differ from what
//! the corpus has canonicalized. Resolution is one-directional: inbox
//! normalizes to corpus, never the reverse.
//!
//! Novel inbox values (no corpus equivalent at all) are NOT flagged —
//! they're new vocabulary, not mismatches.

use std::collections::HashMap;
use std::time::Instant;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::health::normalization::{
    normalize_album, normalize_album_artist, normalize_artist, normalize_genre,
};
use crate::db_thread;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    InboxTagCanonicityData, InboxTagCanonicitySignal, TypedSignalWrite,
};

use super::{Computation, Result};

/// Execute DetectInboxTagCanonicity — find inbox tag values that differ
/// from corpus canonical spellings.
///
/// For each tag field (artist, album_artist, album, genre):
/// 1. Get distinct inbox tag values and corpus tag values
/// 2. Build normalized→values map for corpus
/// 3. For each inbox value: normalize, check corpus map
/// 4. Skip if exact match exists in corpus or if any variant has CanonicalTag
/// 5. Group non-matching inbox values by normalized key
/// 6. Emit InboxTagCanonicitySignal per group with corpus variants
pub fn execute_detect_inbox_tag_canonicity(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectInboxTagCanonicity;

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

    let strip_format_suffixes = config.opinions.canonicalization.strip_album_format_suffixes;

    // Clear all stale inbox tag canonicity signals (full recompute)
    sender.clear_all_of_aggregate_type::<InboxTagCanonicitySignal>(witness);

    let mut signal_count = 0;

    let tag_fields: Vec<(&str, Box<dyn Fn(&str) -> String>)> = vec![
        ("artist", Box::new(|s: &str| normalize_artist(s))),
        (
            "album_artist",
            Box::new(|s: &str| normalize_album_artist(s)),
        ),
        (
            "album",
            Box::new(move |s: &str| normalize_album(s, strip_format_suffixes)),
        ),
        ("genre", Box::new(|s: &str| normalize_genre(s))),
    ];

    for (tag_name, normalize_fn) in &tag_fields {
        // Get inbox values: (value, count)
        let inbox_values = match read_only_db.get_distinct_inbox_tag_values(tag_name) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if inbox_values.is_empty() {
            continue;
        }

        // Get corpus values: (value, count)
        let corpus_values = match read_only_db.get_distinct_tag_values(tag_name) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if corpus_values.is_empty() {
            continue;
        }

        // Build corpus normalized→values map
        // Key: normalized form, Value: vec of (original_value, count)
        let mut corpus_by_norm: HashMap<String, Vec<(String, usize)>> = HashMap::new();
        // Also build a set of exact corpus values for quick lookup
        let mut corpus_exact: HashMap<String, usize> = HashMap::new();
        for (value, count) in &corpus_values {
            let norm = normalize_fn(value);
            corpus_by_norm
                .entry(norm)
                .or_default()
                .push((value.clone(), *count));
            corpus_exact.insert(value.clone(), *count);
        }

        // Check each inbox value against corpus
        // Group by normalized key: norm_key → vec of (inbox_value, inbox_count)
        let mut inbox_mismatches: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        for (inbox_value, inbox_count) in &inbox_values {
            let norm = normalize_fn(inbox_value);

            // Only flag if corpus has values with same normalized form
            let Some(corpus_variants) = corpus_by_norm.get(&norm) else {
                // Novel value — no corpus equivalent at all, skip
                continue;
            };

            // If exact inbox value already exists in corpus, no mismatch
            if corpus_exact.contains_key(inbox_value) {
                continue;
            }

            // Skip if any corpus variant OR the inbox value is marked CanonicalTag
            let any_canonical = corpus_variants.iter().any(|(v, _)| {
                read_only_db
                    .is_canonical_tag(tag_name, v)
                    .unwrap_or(false)
            }) || read_only_db
                .is_canonical_tag(tag_name, inbox_value)
                .unwrap_or(false);

            if any_canonical {
                continue;
            }

            // This is a mismatch — inbox has a variant spelling
            inbox_mismatches
                .entry(norm)
                .or_default()
                .push((inbox_value.clone(), *inbox_count));
        }

        // Emit signals for each normalized group with mismatches
        for (norm_key, inbox_variants) in inbox_mismatches {
            let corpus_variants = match corpus_by_norm.get(&norm_key) {
                Some(v) => {
                    let mut sorted = v.clone();
                    sorted.sort_by(|a, b| b.1.cmp(&a.1));
                    sorted
                }
                None => continue,
            };

            // Get inbox inodes for these variant values
            let variant_refs: Vec<&str> =
                inbox_variants.iter().map(|(v, _)| v.as_str()).collect();
            let inbox_inodes = read_only_db
                .get_inbox_inodes_for_tag_values(tag_name, &variant_refs)
                .unwrap_or_default();

            let key = format!("{}:{}", tag_name, norm_key);

            sender.write_typed_signal(
                TypedSignalWrite::InboxTagCanonicity(InboxTagCanonicitySignal {
                    key,
                    tag_name: tag_name.to_string(),
                    data: InboxTagCanonicityData {
                        inbox_variants,
                        inbox_inodes,
                        corpus_variants,
                    },
                }),
                witness,
            );

            signal_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] DetectInboxTagCanonicity: emitted {} InboxTagCanonicity signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
