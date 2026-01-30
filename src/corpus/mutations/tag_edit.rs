//! Tag Edit Execution
//!
//! Handles execution of tag-related mutations:
//! - TagEditDb: Database-only tag changes
//! - TagFlushToDisk: Disk-only tag writes
//! - TagEditAndFlush: Combined database + disk operations
//!
//! All disk tag operations go through `corpus::tags` module.
//! This file does NOT directly use lofty.

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::tags::{apply_edits_to_file, write_file_tags, TagSet};
use crate::witch::MutationExecutionWitness;

use super::sealed::MutationToken;
use super::types::{Mutation, MutationResult, TagEdit};

/// Execute a database-only tag edit (no disk write).
///
/// Uses read-only DB for path lookup, routes writes through signal_sender.
fn execute_db_only(
    db: &Database,
    track_id: i64,
    tag_name: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get track path from DB (read-only)
    let track = db.get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Log the edit to history via signal_sender
    sender.log_tag_edit(
        &track.path,
        tag_name,
        old_value,
        new_value,
        "mutation",
        witness,
    );

    // Update the track record via signal_sender
    if let Some(value) = new_value {
        sender.update_track_tag(&track.path, tag_name, value, witness);
    }

    Ok(())
}

/// Execute a disk-only tag write (no database update).
///
/// Reads existing tags, merges new ones in, and writes back.
/// Uses the consolidated tag writing path in corpus::tags.
fn execute_disk_only(
    path: &Path,
    tags: &[(String, String)],
    token: &MutationToken,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    // Read existing tags
    let existing = TagSet::from_file(path)
        .with_context(|| format!("Failed to read existing tags from {}", path.display()))?;

    // Merge: new tags override existing (keyed by tag name)
    let mut final_tags: std::collections::HashMap<String, String> =
        existing.into_vec().into_iter().collect();

    for (key, value) in tags {
        final_tags.insert(key.to_lowercase(), value.clone());
    }

    // Convert back to TagSet for writing
    let merged = TagSet::new(final_tags.into_iter().collect::<Vec<_>>());

    // Write to disk using the consolidated write path
    write_file_tags(path, &merged, token, witness)
}

/// Filter out no-op edits where old_value equals new_value.
///
/// These edits would have no effect and waste I/O. This commonly happens when:
/// - User opens tag editor and closes without changes
/// - User edits a value and then reverts it
/// - Bulk operations include unchanged tags
fn filter_nop_edits(edits: &[TagEdit]) -> Vec<TagEdit> {
    edits
        .iter()
        .filter(|edit| edit.old_value != edit.new_value)
        .cloned()
        .collect()
}

/// Validate that all edits with old_value have matching tags on disk.
///
/// Returns error if any expected old_value is not found, indicating
/// the file was modified externally since the editor was opened.
///
/// This ensures the read-edit-write cycle is consistent and prevents
/// applying stale edits to files that have changed.
fn validate_edits_against_current(
    edits: &[TagEdit],
    current_tags: &TagSet,
    path: &Path,
) -> Result<()> {
    for edit in edits {
        if let Some(ref expected_old) = edit.old_value {
            // Check if a tag with (name, old_value) exists in current tags
            if !current_tags.contains(&edit.tag_name, expected_old) {
                // Collect current values for this tag name for diagnostic logging
                let current_values: Vec<&str> = current_tags
                    .values_for(&edit.tag_name)
                    .collect();

                crate::logging::log_error(format!(
                    "Tag edit conflict for '{}': expected {}='{}' but not found. \
                     File may have been modified externally. Current values for '{}': {:?}",
                    path.display(),
                    edit.tag_name,
                    expected_old,
                    edit.tag_name,
                    current_values
                ));

                return Err(anyhow::anyhow!(
                    "Tag edit rejected: {}='{}' not found in current file (stale edit)",
                    edit.tag_name,
                    expected_old
                ));
            }
        }
    }
    Ok(())
}

/// Execute a combined tag edit (database + disk).
///
/// This is the proper mutation path: disk write + explicit DB update.
///
/// Supports two modes:
/// 1. **Standard edits**: Single value per tag, surgical edit-in-place
/// 2. **Multi-value edits**: Multiple values for same tag (e.g., split compound tags)
///    Detected when there are multiple edits with same tag_name and new_value != None
///
/// Before execution:
/// - Filters out NOP edits (old_value == new_value)
/// - Validates that old_values match current disk state
fn execute_combined(
    db: &Database,
    track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    // 1. Filter out no-op edits
    let edits = filter_nop_edits(edits);

    // Early return if all edits were filtered out
    if edits.is_empty() {
        crate::logging::log_general(format!(
            "Tag edit for '{}': all edits were no-ops, skipping",
            path.display()
        ));
        return Ok(());
    }

    // 2. Read current tags for validation
    let current_tags = TagSet::from_file(path)
        .with_context(|| format!("Failed to read current tags from {}", path.display()))?;

    // 3. Validate old_values match current state
    validate_edits_against_current(&edits, &current_tags, path)?;

    // 4. Create token - only possible within mutations module
    let token = MutationToken::new();

    // 5. Detect if this is a multi-value edit (same tag_name appears multiple times with new values)
    let is_multi_value = detect_multi_value_edits(&edits);

    if is_multi_value {
        execute_combined_multi_value(db, track_id, path, &edits, session_id, &token, witness)
    } else {
        execute_combined_single_value(db, track_id, path, &edits, session_id, &token, witness)
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
///
/// Uses `apply_edits_to_file()` from corpus::tags for surgical edit-in-place.
/// Routes DB writes through signal_sender.
fn execute_combined_single_value(
    db: &Database,
    _track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
    token: &MutationToken,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let resolver = paths::get_resolver();

    // Get relative path for DB operations
    let rel_path = resolver.to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path {} not in corpus root", path.display()))?;
    let rel_path_str = rel_path.to_string_lossy();

    // Verify track exists (read-only check)
    let _track = db.get_track_by_path(&rel_path_str)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", rel_path_str))?;

    // Convert TagEdit to (tag_name, old_value, new_value) triples for apply_edits_to_file
    let edit_triples: Vec<(String, Option<String>, Option<String>)> = edits
        .iter()
        .map(|e| (e.tag_name.clone(), e.old_value.clone(), e.new_value.clone()))
        .collect();

    // Apply edits to file (reads current, applies edits, writes back)
    apply_edits_to_file(path, &edit_triples, token, witness)
        .with_context(|| format!("Failed to apply tag edits to {}", path.display()))?;

    // Update database via signal_sender
    for edit in edits {
        sender.log_tag_edit(
            &rel_path_str,
            &edit.tag_name,
            edit.old_value.as_deref(),
            edit.new_value.as_deref(),
            session_id,
            witness,
        );

        if let Some(ref new_value) = edit.new_value {
            sender.update_track_tag(&rel_path_str, &edit.tag_name, new_value, witness);
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
/// Uses `write_file_tags()` from corpus::tags for proper multi-value support.
/// Routes DB writes through signal_sender.
fn execute_combined_multi_value(
    db: &Database,
    _track_id: i64,
    path: &Path,
    edits: &[TagEdit],
    session_id: &str,
    token: &MutationToken,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use std::collections::HashSet;
    use crate::corpus::paths;
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let resolver = paths::get_resolver();

    // Get relative path for DB operations
    let rel_path = resolver.to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path {} not in corpus root", path.display()))?;
    let rel_path_str = rel_path.to_string_lossy();

    // Verify track exists and get track_id for tag lookup (read-only)
    let track = db.get_track_by_path(&rel_path_str)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", rel_path_str))?;
    let track_id = track.id.ok_or_else(|| anyhow::anyhow!("Track has no id"))?;

    // 1. Read current tags
    let current_tags = TagSet::from_file(path)
        .with_context(|| format!("Failed to read tags from {}", path.display()))?;

    // 2. Determine which tag names are being replaced
    let replaced_tag_names: HashSet<String> = edits
        .iter()
        .filter(|e| e.old_value.is_some())
        .map(|e| e.tag_name.to_lowercase())
        .collect();

    // 3. Start with tags NOT being replaced
    let mut new_tags: Vec<(String, String)> = current_tags
        .into_vec()
        .into_iter()
        .filter(|(k, _)| !replaced_tag_names.contains(k))
        .collect();

    // 4. Add all new values from edits
    for edit in edits {
        if let Some(ref value) = edit.new_value {
            new_tags.push((edit.tag_name.to_lowercase(), value.clone()));
        }
    }

    // 5. Write to disk using the consolidated write path
    let final_tagset = TagSet::new(new_tags);
    write_file_tags(path, &final_tagset, token, witness)
        .context("Failed to write multi-value tags to file")?;

    // 6. Update database via signal_sender
    // Delete old values for tags being replaced
    for tag_name in &replaced_tag_names {
        sender.delete_track_tag(&rel_path_str, tag_name, witness);
    }

    // Get existing DB tags for tags NOT being replaced (read-only via track_id)
    let existing_db_tags = db.get_track_tags(track_id)
        .with_context(|| format!("Failed to read existing tags for track {}", rel_path_str))?;
    let mut final_db_tags: Vec<(String, String)> = existing_db_tags
        .into_iter()
        .filter(|t| !replaced_tag_names.contains(&t.tag_name.to_lowercase()))
        .map(|t| (t.tag_name, t.tag_value))
        .collect();

    // Add new values
    for edit in edits {
        if let Some(ref value) = edit.new_value {
            final_db_tags.push((edit.tag_name.clone(), value.clone()));
        }
    }

    // Write all tags to DB via signal_sender
    sender.set_track_tags(&rel_path_str, final_db_tags, witness);

    // 7. Log all edits to history via signal_sender
    for edit in edits {
        sender.log_tag_edit(
            &rel_path_str,
            &edit.tag_name,
            edit.old_value.as_deref(),
            edit.new_value.as_deref(),
            session_id,
            witness,
        );
    }

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
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::TagEditDb {
            track_id,
            tag_name,
            old_value,
            new_value,
        } => execute_db_only(db, *track_id, tag_name, old_value.as_deref(), new_value.as_deref(), witness),

        Mutation::TagFlushToDisk { path, tags } => {
            let token = MutationToken::new();
            execute_disk_only(path, tags, &token, witness)
        }

        Mutation::TagEditAndFlush {
            track_id,
            path,
            edits,
        } => execute_combined(db, *track_id, path, edits, session_id, witness),

        _ => Err(anyhow::anyhow!("Not a tag edit mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
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

    #[test]
    fn test_filter_nop_edits_removes_identical() {
        let edits = vec![
            TagEdit {
                tag_name: "artist".to_string(),
                old_value: Some("Same".to_string()),
                new_value: Some("Same".to_string()),
            },
            TagEdit {
                tag_name: "album".to_string(),
                old_value: Some("Old Album".to_string()),
                new_value: Some("New Album".to_string()),
            },
        ];

        let filtered = filter_nop_edits(&edits);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tag_name, "album");
    }

    #[test]
    fn test_filter_nop_edits_keeps_none_to_some() {
        let edits = vec![TagEdit {
            tag_name: "genre".to_string(),
            old_value: None,
            new_value: Some("Rock".to_string()),
        }];

        let filtered = filter_nop_edits(&edits);

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_nop_edits_keeps_some_to_none() {
        let edits = vec![TagEdit {
            tag_name: "genre".to_string(),
            old_value: Some("Rock".to_string()),
            new_value: None,
        }];

        let filtered = filter_nop_edits(&edits);

        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_nop_edits_removes_none_to_none() {
        let edits = vec![TagEdit {
            tag_name: "genre".to_string(),
            old_value: None,
            new_value: None,
        }];

        let filtered = filter_nop_edits(&edits);

        assert!(filtered.is_empty());
    }

    #[test]
    fn test_validate_edits_passes_when_old_value_matches() {
        let edits = vec![TagEdit {
            tag_name: "artist".to_string(),
            old_value: Some("Old Artist".to_string()),
            new_value: Some("New Artist".to_string()),
        }];

        let current_tags = TagSet::new(vec![("artist".to_string(), "Old Artist".to_string())]);

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edits_fails_when_old_value_missing() {
        let edits = vec![TagEdit {
            tag_name: "artist".to_string(),
            old_value: Some("Expected Artist".to_string()),
            new_value: Some("New Artist".to_string()),
        }];

        let current_tags = TagSet::new(vec![("artist".to_string(), "Different Artist".to_string())]);

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stale edit"));
    }

    #[test]
    fn test_validate_edits_case_insensitive_tag_name() {
        let edits = vec![TagEdit {
            tag_name: "ARTIST".to_string(),
            old_value: Some("Old Artist".to_string()),
            new_value: Some("New Artist".to_string()),
        }];

        let current_tags = TagSet::new(vec![("artist".to_string(), "Old Artist".to_string())]);

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edits_none_old_value_always_passes() {
        let edits = vec![TagEdit {
            tag_name: "new_tag".to_string(),
            old_value: None,
            new_value: Some("New Value".to_string()),
        }];

        // Empty current tags - new tag doesn't need to exist
        let current_tags = TagSet::empty();

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edits_multi_value_finds_specific_value() {
        // File has multiple genre tags
        let current_tags = TagSet::new(vec![
            ("genre".to_string(), "Rock".to_string()),
            ("genre".to_string(), "Metal".to_string()),
        ]);

        // Edit specifically targets "Metal"
        let edits = vec![TagEdit {
            tag_name: "genre".to_string(),
            old_value: Some("Metal".to_string()),
            new_value: Some("Jazz".to_string()),
        }];

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edits_multi_value_fails_if_specific_value_missing() {
        // File has genre=Rock only
        let current_tags = TagSet::new(vec![("genre".to_string(), "Rock".to_string())]);

        // Edit targets genre=Metal which doesn't exist
        let edits = vec![TagEdit {
            tag_name: "genre".to_string(),
            old_value: Some("Metal".to_string()),
            new_value: Some("Jazz".to_string()),
        }];

        let result = validate_edits_against_current(&edits, &current_tags, Path::new("/test.flac"));

        assert!(result.is_err());
    }
}
