//! Tag detection executors.
//!
//! Missing tags, tag canonicalization, inconsistent album artist, and
//! compound tag detection.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::{AggregateSignal, AggregateSignalType, CorpusFileSignalType, SignalType};
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

    let required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    // Clear all existing MissingTag signals (routes through db_thread)
    sender.clear_signals_by_type(SignalType::MissingTag, witness);

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

        let missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

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

        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::MissingTag,
            key: key.clone(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "missing_tags": missing_list,
            }).to_string()),
        }
        .with_inodes(&inodes);

        sender.replace_aggregate_signal(signal, witness);
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

    // Clear stale TagCanonicity signals before re-detecting
    sender.clear_signals_by_type(SignalType::TagCanonicity, witness);

    let mut signal_count = 0;

    // Helper to emit signals for a set of collisions
    let emit_collision_signals = |collisions: Vec<crate::corpus::health::collision::TagCollision>,
                                   sender: &crate::db_thread::SignalWriteSender,
                                   witness: &ComputationWitness,
                                   count: &mut usize| {
        for collision in collisions {
            // Get inodes for all variants in this collision
            let variant_refs: Vec<&str> = collision.variants.iter().map(|s| s.as_str()).collect();
            let inodes = read_only_db
                .get_inodes_for_tag_values(&collision.tag_name, &variant_refs)
                .unwrap_or_default();

            // Build metadata JSON
            let variants_json: serde_json::Map<String, serde_json::Value> = collision
                .variant_counts
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
                .collect();

            let metadata = serde_json::json!({
                "tag_name": collision.tag_name,
                "variants": variants_json,
                "inodes": inodes,
            });

            // Signal key: "{tag_name}:{normalized_key}"
            let key = format!("{}:{}", collision.tag_name, collision.normalized_key);

            sender.ensure_aggregate_signal(
                AggregateSignalType::TagCanonicity,
                &key,
                Some(&metadata.to_string()),
                witness,
            );

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

    if let Ok(collisions) = get_album_collisions(read_only_db) {
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
/// On first run after migration, all inodes are marked dirty for bootstrap.
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
        sender.clear_corpus_signal(CorpusFileSignalType::CompoundTag, inode, witness);
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
    let tag_separators = &config.opinions.tag_splitting.tag_separators;

    let mut compounds: Vec<serde_json::Value> = Vec::new();

    for tag in &tags {
        let tag_name_lower = tag.tag_name.to_lowercase();

        // Check if this value is whitelisted as canonical
        if read_only_db.is_canonical_tag(&tag.tag_name, &tag.tag_value).unwrap_or(false) {
            continue;
        }

        // 1. Check separator patterns if configured for this tag
        if let Some(separators) = tag_separators.get(&tag_name_lower) {
            for separator in separators {
                if CompoundTagValue::is_compound(&tag.tag_value, separator) {
                    let split_parts = CompoundTagValue::split_value(&tag.tag_value, separator);
                    compounds.push(serde_json::json!({
                        "tag_name": tag.tag_name,
                        "compound_value": tag.tag_value,
                        "split_parts": split_parts,
                        "separator": separator,
                    }));
                    break; // Only report first matching separator per tag
                }
            }
        }

        // 2. Check featuring patterns for artist tags
        if tag_name_lower == "artist" {
            if let Some((main_artist, featured_artists)) = detect_featuring_pattern(&tag.tag_value) {
                // Don't duplicate if already caught by separator detection
                let already_found = compounds.iter().any(|c| {
                    c.get("compound_value").and_then(|v| v.as_str()) == Some(&tag.tag_value)
                });
                if !already_found {
                    let mut split_parts = vec![main_artist];
                    split_parts.extend(featured_artists);

                    // Determine separator pattern for display
                    let separator = if tag.tag_value.to_lowercase().contains(" feat") {
                        "feat."
                    } else if tag.tag_value.to_lowercase().contains(" ft") {
                        "ft."
                    } else if tag.tag_value.to_lowercase().contains(" featuring") {
                        "featuring"
                    } else if tag.tag_value.to_lowercase().contains(" vs") {
                        "vs."
                    } else if tag.tag_value.to_lowercase().contains(" with ") {
                        "with"
                    } else {
                        "feat."
                    };

                    compounds.push(serde_json::json!({
                        "tag_name": tag.tag_name,
                        "compound_value": tag.tag_value,
                        "split_parts": split_parts,
                        "separator": separator,
                    }));
                }
            }
        }
    }

    // If no compounds found, clear any existing signal
    if compounds.is_empty() {
        sender.clear_corpus_signal(CorpusFileSignalType::CompoundTag, inode, witness);
        sender.clear_dirty_inode(inode, COMPOUND_TAG_COMPUTATION, witness);
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // Populate matching_parts for each compound by checking which split parts
    // exist as standalone values in the corpus. This enables "safe split" detection.
    let mut tag_values_cache: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();

    for compound in &mut compounds {
        let tag_name = match compound.get("tag_name").and_then(|v| v.as_str()) {
            Some(name) => name.to_lowercase(),
            None => continue,
        };

        // Get or fetch existing values for this tag type
        let existing_values = tag_values_cache.entry(tag_name.clone()).or_insert_with(|| {
            read_only_db
                .get_distinct_tag_values(&tag_name)
                .unwrap_or_default()
                .into_iter()
                .map(|(value, _count)| value)
                .collect()
        });

        // Find which split parts exist as standalone values
        let split_parts = compound
            .get("split_parts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let matching_parts: Vec<String> = split_parts
            .iter()
            .filter(|part| existing_values.contains(&part.to_string()))
            .map(|s| s.to_string())
            .collect();

        compound["matching_parts"] = serde_json::json!(matching_parts);
    }

    // Emit per-file CompoundTag signal
    let metadata = serde_json::json!({
        "inode": inode,
        "compounds": compounds,
    });

    sender.ensure_corpus_signal_with_metadata(
        CorpusFileSignalType::CompoundTag,
        inode,
        &corpus_path,
        metadata,
        witness,
    );

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
    sender.clear_signals_by_type(SignalType::InconsistentAlbumArtist, witness);

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
        // Build metadata JSON
        let artist_variants_json: serde_json::Map<String, serde_json::Value> = issue
            .artist_variants
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
            .collect();

        let album_artist_variants_json: serde_json::Map<String, serde_json::Value> = issue
            .album_artist_variants
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
            .collect();

        let metadata = serde_json::json!({
            "album": issue.album,
            "artist_variants": artist_variants_json,
            "album_artist_variants": album_artist_variants_json,
            "inodes": issue.inodes,
        });

        // Signal key: normalized album name
        sender.ensure_aggregate_signal(
            AggregateSignalType::InconsistentAlbumArtist,
            &issue.normalized_album,
            Some(&metadata.to_string()),
            witness,
        );

        signal_count += 1;
    }

    log_general(format!(
        "[COMPUTE] DetectInconsistentAlbumArtist: emitted {} signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
