//! Inbox tag health detection.
//!
//! Three inbox tag computations:
//!
//! 1. **Canonicity** — Compares inbox tag values against the corpus's established
//!    vocabulary. Surfaces signals when inbox files use spellings that differ from
//!    what the corpus has canonicalized.
//!
//! 2. **Missing tags** — Detects inbox files missing required tags. Simplified
//!    version of corpus missing tag detection: no ExpectedMissingTag suppression,
//!    no MissingAlbumSingle routing.
//!
//! 3. **Compound tags** — Single-pass detection of compound tag values in inbox
//!    files (collaboration keywords + per-tag separators). No orchestrator/dirty
//!    tracking needed since inbox is small.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A list of (tag_name, normalization_fn) pairs used for inbox tag canonicity checks.
type TagNormalizer<'a> = Vec<(&'a str, Box<dyn Fn(&str) -> String>)>;

use crate::corpus::health::normalization::{
    normalize_album, normalize_album_artist, normalize_artist, normalize_genre,
};
use crate::zones::TaggedZone;
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_aggregate_signals, ComputedAggregateSignal};
use crate::meta::computations::traits::ComputationContext;
use crate::meta::signals::data::{
    InboxCompoundTagSignal, InboxMissingTagSignal,
    InboxTagCanonicityData, InboxTagCanonicitySignal, MissingTagData};
use crate::meta::signals::registry::TypedSignalWrite;

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
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectInboxTagCanonicity;

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);

    let strip_format_suffixes = config.opinions.canonicalization.strip_album_format_suffixes;

    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();

    let tag_fields: TagNormalizer<'_> = vec![
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
        let inbox_values = match read_only_db.get_distinct_tag_values_for::<crate::zones::InboxZone>(tag_name) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if inbox_values.is_empty() {
            continue;
        }

        // Get corpus values: (value, count)
        let corpus_values = match read_only_db.get_distinct_tag_values_for::<crate::zones::CorpusZone>(tag_name) {
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
            let any_canonical = corpus_variants
                .iter()
                .any(|(v, _)| read_only_db.is_canonical_tag(tag_name, v).unwrap_or(false))
                || read_only_db
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

        // Build computed signals for each normalized group with mismatches
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
            let variant_refs: Vec<&str> = inbox_variants.iter().map(|(v, _)| v.as_str()).collect();
            let inbox_inodes = read_only_db
                .get_inodes_for_tag_values_in::<crate::zones::InboxZone>(tag_name, &variant_refs)
                .unwrap_or_default();

            let key = format!("{}:{}", tag_name, norm_key);

            computed.push(ComputedAggregateSignal::new(
                key.clone(),
                TypedSignalWrite::InboxTagCanonicity(InboxTagCanonicitySignal {
                    key,
                    tag_name: tag_name.to_string(),
                    data: InboxTagCanonicityData {
                        inbox_variants,
                        inbox_inodes,
                        corpus_variants,
                    },
                }),
            ));
        }
    }

    let stats = reconcile_aggregate_signals::<InboxTagCanonicitySignal>(
        read_only_db, &sender, computed, witness,
    );

    log_general(format!(
        "[COMPUTE] DetectInboxTagCanonicity: {} signals ({})",
        stats.active(), stats,
    ));

    Result::success(computation, Vec::new())
}

// ============================================================================
// Inbox Missing Tags Detection
// ============================================================================

/// Execute DetectInboxMissingTags — detect inbox files missing required tags.
///
/// Simplified adaptation of corpus `execute_detect_missing_tags`:
/// - Uses `get_inbox_audio_files_with_tag_presence()` (inbox zone)
/// - Same required_tags config, same album/directory grouping
/// - No ExpectedMissingTag suppression (inbox files haven't been triaged)
/// - No MissingAlbumSingle special case (handled post-intake in corpus)
/// - Skips ALBUM_ARTIST from required set when `album_artist_only_required_if_compilation`
///   is true (will be caught post-intake)
pub fn execute_detect_inbox_missing_tags(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectInboxMissingTags;

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);

    let mut required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_uppercase())
        .collect();

    // Remove ALBUM_ARTIST from required set when compilation-only — caught post-intake
    if config
        .opinions
        .health_detection
        .album_artist_only_required_if_compilation
    {
        required_tags.remove("ALBUM_ARTIST");
    }

    let tracks_with_tags = match read_only_db.get_audio_files_with_tag_presence_for::<crate::zones::InboxZone>() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query inbox files with tag presence: {}", e),
            );
        }
    };

    // Group by album/directory, same as corpus missing tag detection
    let mut groups: HashMap<String, (HashSet<String>, Vec<i64>)> = HashMap::new();

    for (inode, path, album, present_tags_str, _artist, _title) in tracks_with_tags {
        let present_tags: HashSet<String> = present_tags_str
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let missing: HashSet<String> = required_tags.difference(&present_tags).cloned().collect();

        if missing.is_empty() {
            continue;
        }

        let key = if let Some(album_name) = album {
            format!("inbox:album={}", album_name)
        } else {
            let parent = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            format!("inbox:dir={}", parent)
        };

        let entry = groups
            .entry(key)
            .or_insert_with(|| (HashSet::new(), Vec::new()));
        entry.0.extend(missing);
        entry.1.push(inode);
    }

    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();
    for (key, (missing_tags, inodes)) in groups {
        let mut missing_list: Vec<String> = missing_tags.into_iter().collect();
        missing_list.sort();
        let signal = TypedSignalWrite::InboxMissingTag(InboxMissingTagSignal {
            key: key.clone(),
            data: MissingTagData {
                missing_tags: missing_list,
                inodes,
            },
        });
        computed.push(ComputedAggregateSignal::new(key, signal));
    }

    let stats = reconcile_aggregate_signals::<InboxMissingTagSignal>(
        read_only_db, &sender, computed, witness,
    );

    log_general(format!(
        "[COMPUTE] DetectInboxMissingTags: {} signals ({})",
        stats.active(), stats,
    ));

    Result::success(computation, Vec::new())
}

// ============================================================================
// Inbox Compound Tag Detection
// ============================================================================

/// Execute DetectInboxCompoundTags — single-pass compound tag detection for inbox files.
///
/// Unlike corpus compound tags (orchestrator + per-inode), inbox is small enough
/// for a single pass. For each healthy inbox inode:
/// 1. Get tags via `get_tags_for_zone(inode, Zone::Inbox)`
/// 2. Check collaboration keywords + per-tag separators from config
/// 3. Enrich `matching_parts` against corpus vocabulary
/// 4. Write/clear per-inode InboxCompoundTagSignal
/// 5. Clean up signals for inodes no longer in inbox healthy set
pub fn execute_detect_inbox_compound_tags(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectInboxCompoundTags;

    let sender = require_sender!(computation);

    let config = require_config!(ctx, computation);

    let tag_splitting = &config.opinions.tag_splitting;
    let collab_keywords: Vec<String> = tag_splitting
        .collaboration_keywords
        .iter()
        .cloned()
        .collect();

    // Get all inbox healthy inodes
    let inbox_healthy_inodes: HashSet<i64> = read_only_db
        .corpus_signal_all_inodes::<crate::meta::signals::data::InboxHealthySignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Get existing signal inodes for cleanup
    let existing_signal_inodes: HashSet<i64> = read_only_db
        .corpus_signal_all_inodes::<InboxCompoundTagSignal>()
        .unwrap_or_default()
        .into_iter()
        .collect();

    // Cache of known corpus tag values per tag name
    let mut tag_values_cache: HashMap<String, HashSet<String>> = HashMap::new();

    let mut emitted = 0usize;
    let mut cleared = 0usize;

    for &inode in &inbox_healthy_inodes {
        let tags = read_only_db
            .get_tags::<crate::zones::InboxZone>(inode)
            .unwrap_or_default();
        if tags.is_empty() {
            if existing_signal_inodes.contains(&inode) {
                sender.clear_corpus_signal::<InboxCompoundTagSignal>(inode, witness);
                cleared += 1;
            }
            continue;
        }

        let inbox_path = read_only_db
            .get_path_for_inode::<crate::zones::InboxZone>(inode)
            .unwrap_or_default()
            .unwrap_or_default();

        let compounds = super::detect_compounds_in_tags(
            &tags, tag_splitting, &collab_keywords, &mut tag_values_cache, read_only_db,
        );

        if compounds.is_empty() {
            if existing_signal_inodes.contains(&inode) {
                sender.clear_corpus_signal::<InboxCompoundTagSignal>(inode, witness);
                cleared += 1;
            }
            continue;
        }

        sender.write_typed_signal(
            crate::zones::InboxZone::compound_tag_signal(inode, inbox_path, compounds),
            witness,
        );
        emitted += 1;
    }

    // Clean up signals for inodes no longer in inbox healthy set
    for &inode in &existing_signal_inodes {
        if !inbox_healthy_inodes.contains(&inode) {
            sender.clear_corpus_signal::<InboxCompoundTagSignal>(inode, witness);
            cleared += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] DetectInboxCompoundTags: emitted={}, cleared={}",
        emitted, cleared
    ));

    Result::success(computation, Vec::new())
}
