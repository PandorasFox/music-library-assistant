//! Tag Mutation Functions
//!
//! Pure functions for computing tag changes and generating mutations.
//! These functions are stateless and operate on tag field data structures.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::corpus::db::types::AudioFile;
use crate::corpus::mutations::{Mutation, TagOp};
use crate::corpus::paths;

use super::types::{AggregatedTagField, AggregatedValue, TagChange, TagField};

// ============================================================================
// Tag Loading
// ============================================================================

/// Load tag fields from disk for an audio file.
///
/// All tags are loaded from the audio file and sorted alphabetically.
/// Multi-value tags (e.g., multiple genres) are loaded as separate entries.
pub fn audio_file_to_tag_fields(audio_file: &AudioFile) -> Vec<TagField> {
    use crate::corpus::tags::TagSet;

    let resolver = paths::get_resolver();
    let disk_path = resolver.resolve(Path::new(audio_file.path()));

    let tag_set = match TagSet::from_file(&disk_path) {
        Ok(tags) => tags,
        Err(e) => {
            crate::logging::log_error(format!(
                "Could not read tags from {}: {}",
                disk_path.display(), e
            ));
            TagSet::empty()
        }
    };

    // Convert to TagField - TagSet is already sorted
    let mut tag_fields: Vec<TagField> = tag_set
        .into_vec()
        .into_iter()
        .map(|(name, value)| TagField {
            name,
            value,
            editable: true,
            deleted: false,
        })
        .collect();

    // Add "New Tag" placeholder at the end
    tag_fields.push(TagField {
        name: "New Tag".to_string(),
        value: "[Press Enter to create]".to_string(),
        editable: true,
        deleted: false,
    });

    tag_fields
}

// ============================================================================
// Change Detection
// ============================================================================

/// Compute all changes between original and current tag fields
///
/// This function handles multi-value tags by comparing values semantically
/// rather than by position. It detects added, removed, and modified values
/// for each tag name.
pub fn compute_changes(original: &[Vec<TagField>], current: &[Vec<TagField>]) -> Vec<TagChange> {
    let mut changes = Vec::new();

    for (track_idx, (orig_fields, curr_fields)) in original.iter().zip(current.iter()).enumerate() {
        // Build maps of tag name -> values for both original and current
        let mut orig_values: HashMap<String, Vec<&TagField>> = HashMap::new();
        let mut curr_values: HashMap<String, Vec<&TagField>> = HashMap::new();

        for field in orig_fields {
            if field.name != "New Tag" {
                orig_values
                    .entry(field.name.to_lowercase())
                    .or_default()
                    .push(field);
            }
        }

        for field in curr_fields {
            if field.name != "New Tag" {
                curr_values
                    .entry(field.name.to_lowercase())
                    .or_default()
                    .push(field);
            }
        }

        // Collect all unique tag names
        let mut all_names: HashSet<String> = orig_values.keys().cloned().collect();
        all_names.extend(curr_values.keys().cloned());

        for normalized_name in all_names {
            let orig = orig_values.get(&normalized_name).map(|v| v.as_slice()).unwrap_or(&[]);
            let curr = curr_values.get(&normalized_name).map(|v| v.as_slice()).unwrap_or(&[]);

            // Get display name from current or original
            let display_name = curr
                .first()
                .map(|f| f.name.clone())
                .or_else(|| orig.first().map(|f| f.name.clone()))
                .unwrap_or_else(|| normalized_name.clone());

            // Check for deleted tags (marked deleted in current)
            for field in curr {
                if field.deleted {
                    // Find corresponding original value
                    let orig_value = orig
                        .iter()
                        .find(|o| o.value == field.value)
                        .map(|o| o.value.clone())
                        .unwrap_or_default();

                    changes.push(TagChange {
                        track_idx,
                        field_name: display_name.clone(),
                        old_value: orig_value,
                        new_value: field.value.clone(),
                    });
                }
            }

            // Skip further comparison if all values are deleted
            let curr_active: Vec<_> = curr.iter().filter(|f| !f.deleted).collect();
            if curr_active.is_empty() && !orig.is_empty() {
                continue; // Deletion changes already added above
            }

            // Compare active (non-deleted) values
            let orig_value_set: HashSet<_> = orig.iter().map(|f| &f.value).collect();
            let curr_value_set: HashSet<_> = curr_active.iter().map(|f| &f.value).collect();

            // Added values (in current but not in original)
            for field in &curr_active {
                if !orig_value_set.contains(&field.value) && !field.deleted {
                    changes.push(TagChange {
                        track_idx,
                        field_name: display_name.clone(),
                        old_value: String::new(), // New value, no old
                        new_value: field.value.clone(),
                    });
                }
            }

            // Removed values (in original but not in current active)
            for field in orig {
                if !curr_value_set.contains(&field.value) {
                    // Check if it's not already covered by a deletion flag
                    let is_deleted_explicitly = curr.iter().any(|c| c.value == field.value && c.deleted);
                    if !is_deleted_explicitly {
                        changes.push(TagChange {
                            track_idx,
                            field_name: display_name.clone(),
                            old_value: field.value.clone(),
                            new_value: String::new(), // Removed
                        });
                    }
                }
            }
        }
    }

    changes
}

// ============================================================================
// Mutation Generation
// ============================================================================

/// Convert changes to mutations for the daemon.
///
/// Uses incremental TagOps: for each change, generates an add/drop/replace operation
/// that includes the expected old value for validation. This prevents stale overwrites
/// when track state changed since staging.
///
/// Returns a single ApplyTagOps mutation containing all ops.
pub fn changes_to_mutations(changes: &[TagChange], audio_files: &[AudioFile], _all_tag_fields: &[Vec<TagField>]) -> Vec<Mutation> {
    let mut ops = Vec::new();

    for change in changes {
        let Some(audio_file) = audio_files.get(change.track_idx) else {
            continue;
        };
        let inode = audio_file.inode();

        match (change.old_value.is_empty(), change.new_value.is_empty()) {
            (true, false) => {
                // Old empty, new has value → add
                ops.push(TagOp::add_tag(inode, &change.field_name, &change.new_value));
            }
            (false, true) => {
                // Old has value, new empty → drop
                ops.push(TagOp::drop_tag(inode, &change.field_name, &change.old_value));
            }
            (false, false) => {
                // Both have values → replace
                ops.push(TagOp::replace_tag(
                    inode,
                    &change.field_name,
                    &change.old_value,
                    &change.new_value,
                ));
            }
            (true, true) => {
                // Both empty → no-op
            }
        }
    }

    if ops.is_empty() {
        Vec::new()
    } else {
        vec![Mutation::ApplyTagOps { ops }]
    }
}

// ============================================================================
// Directory-Level Tag Aggregation
// ============================================================================

/// Aggregate tags across all audio files in a directory.
///
/// For each tag name (case-insensitive):
/// - If all files have the same value → `AggregatedValue::Consistent(value)`
/// - If values differ across files → `AggregatedValue::Various`
///
/// Empty values are filtered out. Tags are sorted alphabetically.
/// This provides a unified view for directory-level tag editing where the user
/// can see which tags are consistent and which need attention.
pub fn aggregate_tags_across_audio_files(audio_files: &[AudioFile]) -> Vec<AggregatedTagField> {
    if audio_files.is_empty() {
        return Vec::new();
    }

    // Collect all tag values per tag name across all files
    // Key: normalized tag name, Value: (display name, set of unique non-empty values)
    let mut tag_values: HashMap<String, (String, HashSet<String>)> = HashMap::new();

    for audio_file in audio_files {
        let fields = audio_file_to_tag_fields(audio_file);
        for field in fields {
            if field.name == "New Tag" {
                continue;
            }

            // Skip empty values
            if field.value.is_empty() {
                continue;
            }

            let normalized = field.name.to_lowercase();

            tag_values
                .entry(normalized.clone())
                .or_insert_with(|| (field.name.clone(), HashSet::new()))
                .1
                .insert(field.value);
        }
    }

    // Convert to AggregatedTagField entries
    let mut result: Vec<AggregatedTagField> = tag_values
        .into_iter()
        .map(|(_normalized, (display_name, values))| {
            let value = if values.len() == 1 {
                // All files have the same value
                AggregatedValue::Consistent(values.into_iter().next().unwrap_or_default())
            } else {
                // Files have different values
                AggregatedValue::Various
            };

            AggregatedTagField {
                name: display_name,
                value: value.clone(),
                original_value: value,
            }
        })
        .collect();

    // Sort alphabetically by tag name
    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Add "New Tag" placeholder at end
    result.push(AggregatedTagField {
        name: "New Tag".to_string(),
        value: AggregatedValue::Consistent(String::new()),
        original_value: AggregatedValue::Consistent(String::new()),
    });

    result
}
