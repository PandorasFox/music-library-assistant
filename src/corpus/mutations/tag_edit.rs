//! Tag Edit Execution
//!
//! Handles execution of tag-related mutations:
//! - TagEditDb: Database-only tag changes
//! - TagFlushToDisk: Disk-only tag writes
//! - TagEditAndFlush: Combined database + disk operations

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::metadata;
use crate::witch::MutationExecutionWitness;

use super::sealed::MutationToken;
use super::types::{Mutation, MutationResult, TagEdit};

/// Execute a database-only tag edit (no disk write).
fn execute_db_only(
    db: &Database,
    track_id: i64,
    tag_name: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
) -> Result<()> {
    // Log the edit to history
    db.log_tag_edit(track_id, tag_name, old_value, new_value, "mutation")
        .context("Failed to log tag edit")?;

    // Update the track record
    if let Some(value) = new_value {
        db.update_track_tag(track_id, tag_name, value)
            .context("Failed to update track tag in database")?;
    }

    Ok(())
}

/// Execute a disk-only tag write (no database update).
fn execute_disk_only(path: &Path, tags: &[(String, String)]) -> Result<()> {
    // Read existing tags
    let existing = metadata::read_all_tags(path).unwrap_or_default();

    // Merge: new tags override existing
    let mut final_tags: std::collections::HashMap<String, String> = existing.into_iter().collect();

    for (key, value) in tags {
        final_tags.insert(key.clone(), value.clone());
    }

    // Convert back to vec for writing
    let merged: Vec<(String, String)> = final_tags.into_iter().collect();

    // Write to disk only (track_id=0, session_id="" to skip history/db)
    write_tags_to_disk_only(path, &merged)
}

/// Execute a combined tag edit (database + disk).
///
/// This is the proper mutation path: disk write + explicit DB update.
///
/// Supports two modes:
/// 1. **Standard edits**: Single value per tag, uses HashMap-based merge
/// 2. **Multi-value edits**: Multiple values for same tag (e.g., split compound tags)
///    Detected when there are multiple edits with same tag_name and new_value != None
fn execute_combined(
    db: &Database,
    track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
) -> Result<()> {
    // Create token - only possible within mutations module
    let token = MutationToken::new();

    // Detect if this is a multi-value edit (same tag_name appears multiple times with new values)
    let is_multi_value = detect_multi_value_edits(edits);

    if is_multi_value {
        execute_combined_multi_value(db, track_id, path, edits, session_id, &token)
    } else {
        execute_combined_single_value(db, track_id, path, edits, session_id, &token)
    }
}

/// Detect if edits contain multi-value operations (multiple new values for same tag).
fn detect_multi_value_edits(edits: &[TagEdit]) -> bool {
    use std::collections::HashMap;

    let mut insert_counts: HashMap<&str, usize> = HashMap::new();
    for edit in edits {
        if edit.new_value.is_some() {
            *insert_counts.entry(&edit.tag_name).or_default() += 1;
        }
    }

    // Multi-value if any tag has more than one insert
    insert_counts.values().any(|&count| count > 1)
}

/// Execute standard single-value tag edits.
fn execute_combined_single_value(
    db: &Database,
    track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
    token: &MutationToken,
) -> Result<()> {
    // 1. Read existing tags from disk
    let existing = metadata::read_all_tags(path).unwrap_or_default();
    let mut tag_map: std::collections::HashMap<String, String> = existing.into_iter().collect();

    // 2. Apply edits
    for edit in edits {
        if let Some(ref new_value) = edit.new_value {
            tag_map.insert(edit.tag_name.clone(), new_value.clone());
        } else {
            // new_value is None means delete the tag
            tag_map.remove(&edit.tag_name);
        }
    }

    // 3. Write to disk (requires token proof)
    let final_tags: Vec<(String, String)> = tag_map.into_iter().collect();
    metadata::write_tags_to_file(path, &final_tags, token)
        .context("Failed to write tags to file")?;

    // 4. Update database explicitly
    for edit in edits {
        // Log to history
        db.log_tag_edit(
            track_id,
            &edit.tag_name,
            edit.old_value.as_deref(),
            edit.new_value.as_deref(),
            session_id,
        )
        .context("Failed to log tag edit")?;

        // Update tracks table
        if let Some(ref new_value) = edit.new_value {
            db.update_track_tag(track_id, &edit.tag_name, new_value)
                .context("Failed to update track tag in database")?;
        }
    }

    Ok(())
}

/// Execute multi-value tag edits (e.g., splitting compound tags).
///
/// This handles cases like:
/// - Delete "genre" = "Rock; Metal"
/// - Insert "genre" = "Rock"
/// - Insert "genre" = "Metal"
///
/// Uses write_tags_to_file_multi_value for proper Vorbis/FLAC support.
fn execute_combined_multi_value(
    db: &Database,
    track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
    token: &MutationToken,
) -> Result<()> {
    use std::collections::HashSet;

    // 1. Collect all new values as (tag_name, value) pairs
    let new_tags: Vec<(String, String)> = edits
        .iter()
        .filter_map(|e| {
            e.new_value
                .as_ref()
                .map(|v| (e.tag_name.clone(), v.clone()))
        })
        .collect();

    // 2. Write to disk using multi-value function
    metadata::write_tags_to_file_multi_value(path, &new_tags, token)
        .context("Failed to write multi-value tags to file")?;

    // 3. Update database
    // First, delete any old values for tags being replaced
    let replaced_tags: HashSet<&str> = edits
        .iter()
        .filter(|e| e.old_value.is_some())
        .map(|e| e.tag_name.as_str())
        .collect();

    for tag_name in &replaced_tags {
        db.delete_track_tag(track_id, tag_name)
            .context("Failed to delete old tag value from database")?;
    }

    // 4. Insert all new values
    // We need to use set_track_tags which supports multiple values
    // But we need to preserve other tags not being edited
    let existing_tags = db.get_track_tags(track_id).unwrap_or_default();
    let mut final_db_tags: Vec<(String, String)> = existing_tags
        .into_iter()
        .filter(|t| !replaced_tags.contains(t.tag_name.as_str()))
        .map(|t| (t.tag_name, t.tag_value))
        .collect();

    // Add new values
    final_db_tags.extend(new_tags.clone());

    // Write all tags to DB
    db.set_track_tags(track_id, &final_db_tags)
        .context("Failed to update track tags in database")?;

    // 5. Log all edits to history
    for edit in edits {
        db.log_tag_edit(
            track_id,
            &edit.tag_name,
            edit.old_value.as_deref(),
            edit.new_value.as_deref(),
            session_id,
        )
        .context("Failed to log tag edit")?;
    }

    Ok(())
}

/// Write tags to disk only (bypassing database/history).
///
/// Used for TagFlushToDisk mutations where we only want to update the file.
/// This is public so it can be called from the executor for parallel file operations.
pub fn write_tags_to_disk_only(path: &Path, tags: &[(String, String)]) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt};

    let mut tagged_file = Probe::open(path)
        .with_context(|| format!("Failed to open file for tag writing: {}", path.display()))?
        .read()
        .with_context(|| format!("Failed to read tags from: {}", path.display()))?;

    let tag_type = tagged_file.primary_tag_type();

    // Get or create primary tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            let new_tag = Tag::new(tag_type);
            tagged_file.insert_tag(new_tag);
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Clear existing tags before writing
    tag.clear();

    // Write all tags
    for (key, value) in tags {
        match key.as_str() {
            "artist" => tag.set_artist(value.to_string()),
            "album" => tag.set_album(value.to_string()),
            "title" => tag.set_title(value.to_string()),
            "track_number" => {
                if let Ok(num) = value.parse::<u32>() {
                    tag.set_track(num);
                }
            }
            "year" | "date" => {
                if let Ok(year) = value.parse::<u32>() {
                    tag.set_year(year);
                }
            }
            "genre" => tag.set_genre(value.to_string()),
            _ => {
                let item_key = ItemKey::from_key(tag_type, key);
                tag.insert_text(item_key, value.to_string());
            }
        }
    }

    // Save to file
    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Failed to save tags to file: {}", path.display()))?;

    Ok(())
}

/// Execute a single tag edit mutation.
///
/// Convenience function for executing individual mutations outside of batch context.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &Database,
    mutation: &Mutation,
    session_id: &str,
    _witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::TagEditDb {
            track_id,
            tag_name,
            old_value,
            new_value,
        } => execute_db_only(db, *track_id, tag_name, old_value.as_deref(), new_value.as_deref()),

        Mutation::TagFlushToDisk { path, tags } => execute_disk_only(path, tags),

        Mutation::TagEditAndFlush {
            track_id,
            path,
            edits,
        } => execute_combined(db, *track_id, path, edits, session_id),

        _ => Err(anyhow::anyhow!("Not a tag edit mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    MutationResult {
        mutation: mutation.clone(),
        success,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_tag_edit_to_mutation() {
        let edit = TagEdit {
            tag_name: "artist".to_string(),
            old_value: Some("Old Artist".to_string()),
            new_value: Some("New Artist".to_string()),
        };

        let mutation = Mutation::TagEditAndFlush {
            track_id: 1,
            path: PathBuf::from("/test/file.flac"),
            edits: vec![edit],
        };

        assert!(matches!(mutation, Mutation::TagEditAndFlush { .. }));
    }
}
