//! Tag detection executors.
//!
//! Missing tags, tag canonicalization, inconsistent album artist, and
//! compound tag detection.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    TypedSignalWrite, TagCanonicitySignal, TagCanonicityData,
    InconsistentAlbumArtistSignal, InconsistentAlbumArtistData,
    MissingTagSignal, MissingTagData,
    CompoundTagSignal, CompoundTagEntry as TypedCompoundEntry,
};
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Missing Tags Detection
// ============================================================================

/// Execute DetectMissingTags - detect tracks missing required tags.
pub fn execute_detect_missing_tags(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use std::collections::HashSet;

    let computation = Computation::DetectMissingTags;

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

    let mut required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_uppercase())
        .collect();

    // When album_artist_only_required_if_compilation is true, pull ALBUM_ARTIST
    // out of the base required set and only enforce it on compilation albums.
    let album_artist_key = "ALBUM_ARTIST".to_string();
    let elide_album_artist = config.opinions.health_detection.album_artist_only_required_if_compilation
        && required_tags.remove(&album_artist_key);

    let compilation_albums: HashSet<String> = if elide_album_artist {
        read_only_db.get_compilation_albums().unwrap_or_default()
    } else {
        HashSet::new()
    };

    // Clear all existing MissingTag signals (routes through db_thread)
    sender.clear_all_of_aggregate_type::<MissingTagSignal>(witness);

    let tracks_with_tags = match read_only_db.get_audio_files_with_tag_presence() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query files with tag presence: {}", e),
            );
        }
    };

    let mut groups: HashMap<String, (HashSet<String>, Vec<i64>)> = HashMap::new();

    for (inode, path, album, present_tags_str) in tracks_with_tags {

        let present_tags: HashSet<String> = present_tags_str
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let mut missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

        // Re-require ALBUM_ARTIST for compilation albums
        if elide_album_artist {
            let is_compilation = album.as_ref()
                .map(|a| compilation_albums.contains(a))
                .unwrap_or(false);
            if is_compilation && !present_tags.contains(&album_artist_key) {
                missing.insert(album_artist_key.clone());
            }
        }

        if missing.is_empty() {
            continue;
        }

        let key = if let Some(album_name) = album {
            format!("album={}", album_name)
        } else {
            let parent = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            format!("dir={}", parent)
        };

        let entry = groups.entry(key).or_insert_with(|| (HashSet::new(), Vec::new()));
        entry.0.extend(missing);
        entry.1.push(inode);
    }

    let mut total_groups = 0;

    for (key, (missing_tags, inodes)) in groups {
        total_groups += 1;

        let mut missing_list: Vec<String> = missing_tags.into_iter().collect();
        missing_list.sort();

        sender.write_typed_signal(TypedSignalWrite::MissingTag(MissingTagSignal {
            key: key.clone(),
            data: MissingTagData {
                missing_tags: missing_list,
                inodes,
            },
        }), witness);
    }

    log_general(format!(
        "[COMPUTE] DetectMissingTags: {} groups with missing tags",
        total_groups
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Tag Canonicalization Detection
// ============================================================================

/// Execute DetectTagCanonicalizations - detect tag canonicalization opportunities.
///
/// Emits TagCanonicity aggregate signals for each detected collision cluster.
/// Respects `strip_album_format_suffixes` from config for album collision detection.
/// Skips collision groups where any variant has a CanonicalTag signal.
pub fn execute_detect_tag_canonicalizations(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::collision::{
        get_album_artist_collisions, get_album_collisions, get_artist_collisions,
        get_genre_collisions,
    };

    let computation = Computation::DetectTagCanonicalizations;

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

    // Clear stale TagCanonicity signals before re-detecting
    sender.clear_all_of_aggregate_type::<TagCanonicitySignal>(witness);

    let mut signal_count = 0;

    // Helper to emit signals for a set of collisions
    let emit_collision_signals = |collisions: Vec<crate::corpus::health::collision::TagCollision>,
                                   sender: &crate::db_thread::SignalWriteSender,
                                   witness: &ComputationWitness,
                                   count: &mut usize| {
        for collision in collisions {
            // Skip groups where any variant has a CanonicalTag signal
            let any_canonical = collision.variants.iter().any(|v| {
                read_only_db.is_canonical_tag(&collision.tag_name, v).unwrap_or(false)
            });
            if any_canonical {
                continue;
            }

            // Get inodes for all variants in this collision
            let variant_refs: Vec<&str> = collision.variants.iter().map(|s| s.as_str()).collect();
            let inodes = read_only_db
                .get_inodes_for_tag_values(&collision.tag_name, &variant_refs)
                .unwrap_or_default();

            // Build sorted variant tuples (count DESC)
            let mut variants: Vec<(String, usize)> = collision
                .variant_counts
                .into_iter()
                .collect();
            variants.sort_by(|a, b| b.1.cmp(&a.1));

            // Signal key: "{tag_name}:{normalized_key}"
            let key = format!("{}:{}", collision.tag_name, collision.normalized_key);

            sender.write_typed_signal(TypedSignalWrite::TagCanonicity(TagCanonicitySignal {
                key,
                tag_name: collision.tag_name,
                data: TagCanonicityData { variants, inodes },
            }), witness);

            *count += 1;
        }
    };

    // Detect and emit signals for each tag type
    if let Ok(collisions) = get_artist_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_album_artist_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_album_collisions(read_only_db, strip_format_suffixes) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_genre_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    log_general(format!(
        "[COMPUTE] DetectTagCanonicalizations: emitted {} TagCanonicity signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Computation type identifier for dirty inode tracking.
const COMPOUND_TAG_COMPUTATION: &str = "compound_tag";

/// Execute DetectCompoundTagValues - orchestrator for compound tag detection.
///
/// Uses incremental dirty-inode tracking: only spawns DetectCompoundTagsForInode
/// for inodes that have been marked dirty (tags changed since last computation).
/// Migration v5→v6 seeds all corpus inodes as dirty for initial population.
pub fn execute_detect_compound_tag_values(
    read_only_db: &ReadOnlyDb<'_>,
    _witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectCompoundTagValues;

    // Query dirty inodes instead of all corpus inodes
    let dirty_inodes = match read_only_db.get_dirty_inodes(COMPOUND_TAG_COMPUTATION) {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get dirty inodes: {}", e),
            );
        }
    };

    // Skip if no dirty inodes
    if dirty_inodes.is_empty() {
        log_general("[COMPUTE] DetectCompoundTagValues: no dirty inodes, skipping");
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Note: We do NOT clear existing signals here. Each per-inode computation will
    // emit or clear its own signal as appropriate. Signals for unchanged inodes
    // (not in dirty list) are preserved.

    // Spawn per-inode computations only for dirty inodes
    let spawn: Vec<Computation> = dirty_inodes
        .into_iter()
        .map(|inode| Computation::DetectCompoundTagsForInode { inode })
        .collect();

    log_general(format!(
        "[COMPUTE] DetectCompoundTagValues: spawning {} per-inode computations",
        spawn.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, spawn)
}

/// Determine the display label for a collaboration keyword match.
///
/// Scans the value for the first matching keyword (case-insensitive) and returns
/// its canonical form with trailing dot (e.g., "feat.", "ft.", "vs.").
fn determine_collab_separator_label(value: &str, keywords: &[String]) -> String {
    let lower = value.to_lowercase();
    for kw in keywords {
        let kw_lower = kw.to_lowercase();
        // Check for " keyword." or " keyword " (with preceding space)
        if lower.contains(&format!(" {}.", kw_lower)) || lower.contains(&format!(" {} ", kw_lower)) {
            // Return canonical form: keyword + dot (unless it's a full word like "featuring"/"with")
            return if kw_lower == "featuring" || kw_lower == "with" {
                kw_lower
            } else {
                format!("{}.", kw_lower)
            };
        }
    }
    // Fallback — shouldn't happen if detect_featuring_pattern matched
    "feat.".to_string()
}

/// Execute DetectCompoundTagsForInode - detect compound tags for a single inode.
///
/// Checks all tags for separator patterns and featuring patterns, emitting
/// a per-file CompoundTag signal if any compound values are found. Clears the
/// dirty flag after processing regardless of outcome.
pub fn execute_detect_compound_tags_for_inode(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::compound::{CompoundTagValue, detect_featuring_pattern};

    let computation = Computation::DetectCompoundTagsForInode { inode };

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

    // Get the file path for this inode (needed for signal key)
    let corpus_path = match read_only_db.get_corpus_path_for_inode(inode) {
        Ok(Some(path)) => path,
        Ok(None) => {
            // File no longer in corpus - clear dirty and skip silently
            sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);
            return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
        }
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get path for inode {}: {}", inode, e),
            );
        }
    };

    // Get tags for this inode
    let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
    if tags.is_empty() {
        // No tags - clear any existing signal and dirty flag
        sender.clear_corpus_signal::<CompoundTagSignal>(inode, witness);
        sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Get tag splitting config from opinions
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(_) => {
            sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);
            return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
        }
    };
    let tag_split_rules = &config.opinions.tag_splitting.tag_split_rules;

    // Cache of known tag values per tag name — used by SeparatorIfKnown during
    // rule matching and by the matching_parts pass afterward.
    let mut tag_values_cache: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();

    let mut compounds: Vec<TypedCompoundEntry> = Vec::new();

    for tag in &tags {
        let tag_name_upper = tag.tag_name.to_uppercase();

        // Check if this value is whitelisted as canonical
        if read_only_db.is_canonical_tag(&tag.tag_name, &tag.tag_value).unwrap_or(false) {
            continue;
        }

        // Walk the priority chain of split rules for this tag
        if let Some(rules) = tag_split_rules.get(&tag_name_upper) {
            for rule in rules {
                let matched = match rule {
                    crate::config::SplitRule::Separator(sep) => {
                        if CompoundTagValue::is_compound(&tag.tag_value, sep) {
                            let split_parts = CompoundTagValue::split_value(&tag.tag_value, sep);
                            Some(TypedCompoundEntry {
                                tag_name: tag.tag_name.clone(),
                                compound_value: tag.tag_value.clone(),
                                split_parts,
                                separator: sep.clone(),
                                matching_parts: Vec::new(),
                            })
                        } else {
                            None
                        }
                    }
                    crate::config::SplitRule::SeparatorIfKnown(sep) => {
                        if CompoundTagValue::is_compound(&tag.tag_value, sep) {
                            let split_parts = CompoundTagValue::split_value(&tag.tag_value, sep);
                            // Only match if at least one part is already known
                            let existing = tag_values_cache.entry(tag_name_upper.clone()).or_insert_with(|| {
                                read_only_db
                                    .get_distinct_tag_values(&tag_name_upper)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|(value, _count)| value)
                                    .collect()
                            });
                            if split_parts.iter().any(|part| existing.contains(part)) {
                                Some(TypedCompoundEntry {
                                    tag_name: tag.tag_name.clone(),
                                    compound_value: tag.tag_value.clone(),
                                    split_parts,
                                    separator: sep.clone(),
                                    matching_parts: Vec::new(),
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    crate::config::SplitRule::CollaborationKeywords(keywords) => {
                        if let Some((main_part, secondary_parts)) =
                            detect_featuring_pattern(&tag.tag_value, keywords)
                        {
                            let mut split_parts = vec![main_part];
                            split_parts.extend(secondary_parts);
                            let separator = determine_collab_separator_label(&tag.tag_value, keywords);
                            Some(TypedCompoundEntry {
                                tag_name: tag.tag_name.clone(),
                                compound_value: tag.tag_value.clone(),
                                split_parts,
                                separator,
                                matching_parts: Vec::new(),
                            })
                        } else {
                            None
                        }
                    }
                };
                if let Some(entry) = matched {
                    compounds.push(entry);
                    break; // First matching rule wins
                }
            }
        }
    }

    // If no compounds found, clear any existing signal
    if compounds.is_empty() {
        sender.clear_corpus_signal::<CompoundTagSignal>(inode, witness);
        sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Populate matching_parts for each compound by checking which split parts
    // exist as standalone values in the corpus. This enables "safe split" detection.
    for compound in &mut compounds {
        let tag_name = compound.tag_name.to_uppercase();

        // Get or fetch existing values for this tag type (reuses cache from above)
        let existing_values = tag_values_cache.entry(tag_name.clone()).or_insert_with(|| {
            read_only_db
                .get_distinct_tag_values(&tag_name)
                .unwrap_or_default()
                .into_iter()
                .map(|(value, _count)| value)
                .collect()
        });

        // Find which split parts exist as standalone values
        compound.matching_parts = compound
            .split_parts
            .iter()
            .filter(|part| existing_values.contains(*part))
            .cloned()
            .collect();
    }

    // Emit per-file CompoundTag signal
    sender.write_typed_signal(TypedSignalWrite::CompoundTag(CompoundTagSignal {
        inode,
        path: corpus_path,
        compounds,
    }), witness);

    // Clear dirty flag after successful processing
    sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Inconsistent Album Artist Detection
// ============================================================================

/// Execute DetectInconsistentAlbumArtist - detect albums with inconsistent album_artist tags.
///
/// Emits InconsistentAlbumArtist aggregate signals for albums where:
/// - Multiple artists are present on the same album
/// - album_artist tags are missing or inconsistent
pub fn execute_detect_inconsistent_album_artist(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::album_artist_detection::detect_inconsistent_album_artist;

    let computation = Computation::DetectInconsistentAlbumArtist;

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

    // Clear stale InconsistentAlbumArtist signals before re-detecting
    sender.clear_all_of_aggregate_type::<InconsistentAlbumArtistSignal>(witness);

    let issues = match detect_inconsistent_album_artist(read_only_db) {
        Ok(i) => i,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to detect inconsistent album_artist: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for issue in issues {
        // Convert HashMaps to sorted Vec<(String, usize)> tuples
        let mut artist_variants: Vec<(String, usize)> = issue
            .artist_variants
            .into_iter()
            .collect();
        artist_variants.sort_by(|a, b| b.1.cmp(&a.1));

        let mut album_artist_variants: Vec<(String, usize)> = issue
            .album_artist_variants
            .into_iter()
            .collect();
        album_artist_variants.sort_by(|a, b| b.1.cmp(&a.1));

        sender.write_typed_signal(TypedSignalWrite::InconsistentAlbumArtist(InconsistentAlbumArtistSignal {
            key: issue.normalized_album,
            data: InconsistentAlbumArtistData {
                album: issue.album,
                artist_variants,
                album_artist_variants,
                inodes: issue.inodes,
            },
        }), witness);

        signal_count += 1;
    }

    log_general(format!(
        "[COMPUTE] DetectInconsistentAlbumArtist: emitted {} signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
