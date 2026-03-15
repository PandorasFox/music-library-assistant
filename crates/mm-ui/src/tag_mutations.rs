//! Pure tag mutation functions (DEPRECATED).
//!
//! These functions operate on the old `TagField`/`TagChange` types.
//! New code should use `tag_set::TagSet::diff()` which produces `Vec<TagOp>` directly,
//! and `tag_set::AggregatedTagSet::from_tag_sets()` for aggregation.

use std::collections::{HashMap, HashSet};

use mm_meta::db_types::{AudioFile, Zone};
use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
use mm_meta::mutations::{Mutation, TagOp};

#[allow(deprecated)]
use crate::domain_types::{AggregatedTagField, AggregatedValue, TagChange, TagField};

// ============================================================================
// Tag Loading
// ============================================================================

/// Convert raw tag pairs (from domain query) into TagField structs.
///
/// The tag pairs come from `GetFileTagValues` which reads tags from disk
/// on the server side. This function just wraps them as editable fields.
#[deprecated(note = "use tag_set::TagSet::from_pairs() instead")]
#[allow(deprecated)]
pub fn tag_pairs_to_tag_fields(tags: Vec<(String, String)>) -> Vec<TagField> {
    let mut tag_fields: Vec<TagField> = tags
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

/// Compute all changes between original and current tag fields.
///
/// Handles multi-value tags by comparing values semantically
/// rather than by position. Detects added, removed, and modified values
/// for each tag name.
#[deprecated(note = "use tag_set::TagSet::diff() which produces TagOps directly")]
#[allow(deprecated)]
pub fn compute_changes(original: &[Vec<TagField>], current: &[Vec<TagField>]) -> Vec<TagChange> {
    let mut changes = Vec::new();

    for (track_idx, (orig_fields, curr_fields)) in original.iter().zip(current.iter()).enumerate() {
        // Build maps of tag name -> values for both original and current
        let mut orig_values: HashMap<String, Vec<&TagField>> = HashMap::new();
        let mut curr_values: HashMap<String, Vec<&TagField>> = HashMap::new();

        for field in orig_fields {
            if field.name != "New Tag" {
                orig_values
                    .entry(field.name.to_uppercase())
                    .or_default()
                    .push(field);
            }
        }

        for field in curr_fields {
            if field.name != "New Tag" {
                curr_values
                    .entry(field.name.to_uppercase())
                    .or_default()
                    .push(field);
            }
        }

        // Collect all unique tag names
        let mut all_names: HashSet<String> = orig_values.keys().cloned().collect();
        all_names.extend(curr_values.keys().cloned());

        for normalized_name in all_names {
            let orig = orig_values
                .get(&normalized_name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let curr = curr_values
                .get(&normalized_name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            // Get display name from current or original
            let display_name = curr
                .first()
                .map(|f| f.name.clone())
                .or_else(|| orig.first().map(|f| f.name.clone()))
                .unwrap_or_else(|| normalized_name.clone());

            // Check for deleted tags (marked deleted in current)
            for field in curr {
                if field.deleted {
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

            // Skip further comparison only if all current entries are explicitly deleted.
            // When curr is completely empty (tag name was renamed away), fall through
            // so the "removed values" logic below generates drop changes.
            let curr_active: Vec<_> = curr.iter().filter(|f| !f.deleted).collect();
            if curr_active.is_empty() && !orig.is_empty() && !curr.is_empty() {
                continue;
            }

            let orig_value_set: HashSet<_> = orig.iter().map(|f| &f.value).collect();
            let curr_value_set: HashSet<_> = curr_active.iter().map(|f| &f.value).collect();

            // Added values (in current but not in original)
            for field in &curr_active {
                if !orig_value_set.contains(&field.value) && !field.deleted {
                    changes.push(TagChange {
                        track_idx,
                        field_name: display_name.clone(),
                        old_value: String::new(),
                        new_value: field.value.clone(),
                    });
                }
            }

            // Removed values (in original but not in current active)
            for field in orig {
                if !curr_value_set.contains(&field.value) {
                    let is_deleted_explicitly =
                        curr.iter().any(|c| c.value == field.value && c.deleted);
                    if !is_deleted_explicitly {
                        changes.push(TagChange {
                            track_idx,
                            field_name: display_name.clone(),
                            old_value: field.value.clone(),
                            new_value: String::new(),
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

/// Convert changes to mutations.
///
/// For each change, generates an add/drop/replace TagOp with the expected
/// old value for validation. Returns a single `ApplyTagOps` mutation.
#[deprecated(note = "use tag_set::TagSet::diff() which produces TagOps directly")]
#[allow(deprecated)]
pub fn changes_to_mutations(
    changes: &[TagChange],
    audio_files: &[AudioFile],
) -> Vec<Mutation> {
    let mut ops = Vec::new();

    for change in changes {
        let Some(audio_file) = audio_files.get(change.track_idx) else {
            continue;
        };
        let inode = audio_file.inode();

        match (change.old_value.is_empty(), change.new_value.is_empty()) {
            (true, false) => {
                ops.push(TagOp::add_tag(inode, &change.field_name, &change.new_value));
            }
            (false, true) => {
                ops.push(TagOp::drop_tag(inode, &change.field_name, &change.old_value));
            }
            (false, false) => {
                ops.push(TagOp::replace_tag(
                    inode,
                    &change.field_name,
                    &change.old_value,
                    &change.new_value,
                ));
            }
            (true, true) => {}
        }
    }

    if ops.is_empty() {
        Vec::new()
    } else {
        let zone = audio_files
            .first()
            .map(|f| f.entry.zone)
            .unwrap_or(Zone::Corpus);
        vec![Mutation::ApplyTagOps(ApplyTagOpsMutation { ops, zone })]
    }
}

// ============================================================================
// Directory-Level Tag Aggregation
// ============================================================================

/// Aggregate tags across all files from pre-loaded per-file tag fields.
///
/// For each tag name (case-insensitive):
/// - If all files have the same value → `AggregatedValue::Consistent(value)`
/// - If values differ across files → `AggregatedValue::Various`
#[deprecated(note = "use tag_set::AggregatedTagSet::from_tag_sets() instead")]
#[allow(deprecated)]
pub fn aggregate_tags_from_fields(all_fields: &[Vec<TagField>]) -> Vec<AggregatedTagField> {
    if all_fields.is_empty() {
        return Vec::new();
    }

    let mut tag_values: HashMap<String, (String, HashSet<String>)> = HashMap::new();

    for fields in all_fields {
        for field in fields {
            if field.name == "New Tag" || field.value.is_empty() {
                continue;
            }

            let normalized = field.name.to_uppercase();
            tag_values
                .entry(normalized)
                .or_insert_with(|| (field.name.clone(), HashSet::new()))
                .1
                .insert(field.value.clone());
        }
    }

    let mut result: Vec<AggregatedTagField> = tag_values
        .into_iter()
        .map(|(_normalized, (display_name, values))| {
            let value = if values.len() == 1 {
                AggregatedValue::Consistent(values.into_iter().next().unwrap_or_default())
            } else {
                AggregatedValue::Various
            };

            AggregatedTagField {
                name: display_name,
                value: value.clone(),
                original_value: value,
            }
        })
        .collect();

    result.sort_by(|a, b| a.name.to_uppercase().cmp(&b.name.to_uppercase()));

    result.push(AggregatedTagField {
        name: "New Tag".to_string(),
        value: AggregatedValue::Consistent(String::new()),
        original_value: AggregatedValue::Consistent(String::new()),
    });

    result
}
