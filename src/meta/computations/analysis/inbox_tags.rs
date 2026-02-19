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
use std::time::Instant;

use crate::db::ReadOnlyDb;
use crate::db::types::Zone;
use crate::corpus::health::normalization::{
    normalize_album, normalize_album_artist, normalize_artist, normalize_genre,
};
use crate::db::write_thread;
use crate::logging::log_general;
use crate::meta::computations::helpers::{ComputedAggregateSignal, reconcile_aggregate_signals};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    InboxTagCanonicityData, InboxTagCanonicitySignal,
    InboxMissingTagSignal, MissingTagData,
    InboxCompoundTagSignal, CompoundTagEntry as TypedCompoundEntry,
    TypedSignalWrite,
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

    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();

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
            let variant_refs: Vec<&str> =
                inbox_variants.iter().map(|(v, _)| v.as_str()).collect();
            let inbox_inodes = read_only_db
                .get_inbox_inodes_for_tag_values(tag_name, &variant_refs)
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

    let (cleared, new_count, updated, unchanged) =
        reconcile_aggregate_signals::<InboxTagCanonicitySignal>(
            read_only_db,
            &sender,
            computed,
            witness,
        );

    log_general(format!(
        "[COMPUTE] DetectInboxTagCanonicity: {} signals (cleared={}, new={}, updated={}, unchanged={})",
        new_count + updated + unchanged, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
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
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectInboxMissingTags;

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

    let mut required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_uppercase())
        .collect();

    // Remove ALBUM_ARTIST from required set when compilation-only — caught post-intake
    if config.opinions.health_detection.album_artist_only_required_if_compilation {
        required_tags.remove("ALBUM_ARTIST");
    }

    let tracks_with_tags = match read_only_db.get_inbox_audio_files_with_tag_presence() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
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

        let missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

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

        let entry = groups.entry(key).or_insert_with(|| (HashSet::new(), Vec::new()));
        entry.0.extend(missing);
        entry.1.push(inode);
    }

    let mut computed: Vec<ComputedAggregateSignal> = Vec::new();
    for (key, (missing_tags, inodes)) in groups {
        let mut missing_list: Vec<String> = missing_tags.into_iter().collect();
        missing_list.sort();
        let signal = TypedSignalWrite::InboxMissingTag(InboxMissingTagSignal {
            key: key.clone(),
            data: MissingTagData { missing_tags: missing_list, inodes },
        });
        computed.push(ComputedAggregateSignal::new(key, signal));
    }

    let (cleared, new_count, updated, unchanged) =
        reconcile_aggregate_signals::<InboxMissingTagSignal>(
            read_only_db,
            &sender,
            computed,
            witness,
        );

    log_general(format!(
        "[COMPUTE] DetectInboxMissingTags: {} signals (cleared={}, new={}, updated={}, unchanged={})",
        new_count + updated + unchanged, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
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
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::compound::{CompoundTagValue, detect_featuring_pattern};

    let computation = Computation::DetectInboxCompoundTags;

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

    let tag_splitting = &config.opinions.tag_splitting;
    let collab_keywords: Vec<String> = tag_splitting.collaboration_keywords.iter().cloned().collect();

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
        let tags = read_only_db.get_tags_for_zone(inode, Zone::Inbox).unwrap_or_default();
        if tags.is_empty() {
            if existing_signal_inodes.contains(&inode) {
                sender.clear_corpus_signal::<InboxCompoundTagSignal>(inode, witness);
                cleared += 1;
            }
            continue;
        }

        let inbox_path = read_only_db.get_inbox_path_for_inode(inode)
            .unwrap_or_default()
            .unwrap_or_default();

        let mut compounds: Vec<TypedCompoundEntry> = Vec::new();

        for tag in &tags {
            let tag_name_upper = tag.tag_name.to_uppercase();
            let is_artist_tag = matches!(tag_name_upper.as_str(), "ARTIST" | "ALBUMARTIST");

            // Skip if whitelisted canonical
            if read_only_db.is_canonical_tag(&tag.tag_name, &tag.tag_value).unwrap_or(false) {
                continue;
            }

            let mut matched_entry: Option<TypedCompoundEntry> = None;

            // 1. Collaboration keywords (artist tags only)
            if is_artist_tag && !collab_keywords.is_empty() {
                if let Some((main_part, secondary_parts)) =
                    detect_featuring_pattern(&tag.tag_value, &collab_keywords)
                {
                    let mut split_parts = vec![main_part];
                    split_parts.extend(secondary_parts);
                    let separator = super::determine_collab_separator_label(&tag.tag_value, &collab_keywords);
                    matched_entry = Some(TypedCompoundEntry {
                        tag_name: tag.tag_name.clone(),
                        compound_value: tag.tag_value.clone(),
                        split_parts,
                        separator,
                        matching_parts: Vec::new(),
                    });
                }
            }

            // 2. Per-tag separators (if no collab match)
            if matched_entry.is_none() {
                if let Some(separators) = tag_splitting.tag_separators.get(&tag_name_upper) {
                    for sep in separators {
                        if CompoundTagValue::is_compound(&tag.tag_value, sep) {
                            let split_parts = CompoundTagValue::split_value(&tag.tag_value, sep);
                            matched_entry = Some(TypedCompoundEntry {
                                tag_name: tag.tag_name.clone(),
                                compound_value: tag.tag_value.clone(),
                                split_parts,
                                separator: sep.clone(),
                                matching_parts: Vec::new(),
                            });
                            break;
                        }
                    }
                }
            }

            if let Some(entry) = matched_entry {
                compounds.push(entry);
            }
        }

        if compounds.is_empty() {
            if existing_signal_inodes.contains(&inode) {
                sender.clear_corpus_signal::<InboxCompoundTagSignal>(inode, witness);
                cleared += 1;
            }
            continue;
        }

        // Populate matching_parts against corpus vocabulary
        for compound in &mut compounds {
            let tag_name = compound.tag_name.to_uppercase();
            let existing_values = tag_values_cache.entry(tag_name.clone()).or_insert_with(|| {
                read_only_db
                    .get_distinct_tag_values(&tag_name)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(value, _count)| value)
                    .collect()
            });

            compound.matching_parts = compound
                .split_parts
                .iter()
                .filter(|part| existing_values.contains(*part))
                .cloned()
                .collect();
        }

        sender.write_typed_signal(TypedSignalWrite::InboxCompoundTag(InboxCompoundTagSignal {
            inode,
            path: inbox_path,
            compounds,
        }), witness);
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

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
