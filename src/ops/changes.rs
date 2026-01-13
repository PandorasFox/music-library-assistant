//! Algebraic Change Tracking Module
//!
//! Executes pending corpus changes (deletes, tag edits, deploys).

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::corpus::db::{ChangeStatus, ChangeType, Database, PendingChange};

/// Result of executing changes
#[derive(Debug, Default)]
pub struct ExecutionReport {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Execute a single change on the filesystem
fn execute_single_change(change: &PendingChange, db: &Database, dry_run: bool) -> Result<bool> {
    use crate::config;

    let _ = config::log_message(&format!(
        "[execute_single_change] type={:?} source={} target={} dry_run={}",
        change.change_type,
        change.source_path,
        change.target_path.as_deref().unwrap_or("(none)"),
        dry_run
    ));

    match change.change_type {
        ChangeType::Move => {
            let target = change
                .target_path
                .as_ref()
                .context("Move requires target_path")?;

            let _ = config::log_message(&format!(
                "[MOVE] source={} target={}",
                change.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&change.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[MOVE] ERROR: Source does not exist: {}",
                    change.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", change.source_path);
            }
            let _ = config::log_message(&format!(
                "[MOVE] Source exists: {} (size={} bytes)",
                change.source_path,
                source_path.metadata().map(|m| m.len()).unwrap_or(0)
            ));

            if dry_run {
                let _ = config::log_message("[MOVE] DRY RUN - skipping actual move");
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                let _ = config::log_message(&format!(
                    "[MOVE] Creating target parent dir: {}",
                    parent.display()
                ));
                fs::create_dir_all(parent)?;
            }

            let _ = config::log_message(&format!(
                "[MOVE] Executing fs::rename {} -> {}",
                change.source_path, target
            ));
            fs::rename(&change.source_path, target)
                .with_context(|| format!("Failed to move {} to {}", change.source_path, target))?;
            let _ = config::log_message("[MOVE] SUCCESS");
            Ok(true)
        }

        ChangeType::Delete => {
            // "Delete" means move to stash, not actual deletion
            let target = change
                .target_path
                .as_ref()
                .context("Delete requires target_path (stash location)")?;

            let _ = config::log_message(&format!(
                "[DELETE/STASH] source={} target={}",
                change.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&change.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[DELETE] ERROR: Source does not exist: {}",
                    change.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", change.source_path);
            }
            let _ = config::log_message(&format!(
                "[DELETE] Source exists: {} (size={} bytes)",
                change.source_path,
                source_path.metadata().map(|m| m.len()).unwrap_or(0)
            ));

            if dry_run {
                let _ = config::log_message("[DELETE] DRY RUN - skipping actual move");
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                let _ = config::log_message(&format!(
                    "[DELETE] Creating target parent dir: {}",
                    parent.display()
                ));
                fs::create_dir_all(parent)?;
            }

            let _ = config::log_message(&format!(
                "[DELETE] Executing fs::rename {} -> {}",
                change.source_path, target
            ));
            fs::rename(&change.source_path, target).with_context(|| {
                format!(
                    "Failed to move {} to stash at {}",
                    change.source_path, target
                )
            })?;
            let _ = config::log_message("[DELETE] File move SUCCESS");

            // Remove from corpus index
            let _ = config::log_message(&format!(
                "[DELETE] Removing from index: {}",
                change.source_path
            ));
            match db.delete_track_by_path(&change.source_path) {
                Ok(true) => {
                    let _ = config::log_message("[DELETE] Removed from index successfully");
                }
                Ok(false) => {
                    let _ = config::log_message("[DELETE] Track was not in index (already removed?)");
                }
                Err(e) => {
                    let _ = config::log_message(&format!(
                        "[DELETE] WARNING: Failed to remove from index: {}",
                        e
                    ));
                    // Don't fail the whole operation - file was moved successfully
                }
            }

            Ok(true)
        }

        ChangeType::TagEdit => {
            // Tag edits use metadata_changes JSON: { "tags": [["field", "value"], ...], "track_id": N }
            let metadata_json = change
                .metadata_changes
                .as_ref()
                .context("TagEdit requires metadata_changes")?;

            let metadata: serde_json::Value = serde_json::from_str(metadata_json)
                .with_context(|| format!("Invalid metadata JSON: {}", metadata_json))?;

            let tags_array = metadata
                .get("tags")
                .and_then(|v| v.as_array())
                .context("metadata_changes must have 'tags' array")?;

            let track_id = metadata
                .get("track_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            let session_id = &change.session_id;

            // Convert JSON array to Vec<(String, String)>
            let tags: Vec<(String, String)> = tags_array
                .iter()
                .filter_map(|item| {
                    let arr = item.as_array()?;
                    if arr.len() >= 2 {
                        Some((
                            arr[0].as_str()?.to_string(),
                            arr[1].as_str()?.to_string(),
                        ))
                    } else {
                        None
                    }
                })
                .collect();

            let _ = config::log_message(&format!(
                "[TAG_EDIT] path={} track_id={} tags={:?}",
                change.source_path, track_id, tags
            ));

            if dry_run {
                let _ = config::log_message("[TAG_EDIT] DRY RUN - skipping actual write");
                return Ok(true);
            }

            // Write tags to disk and update database
            let path = Path::new(&change.source_path);
            crate::corpus::metadata::write_tags(path, &tags, track_id, session_id)
                .with_context(|| format!("Failed to write tags to {}", change.source_path))?;

            let _ = config::log_message("[TAG_EDIT] SUCCESS");
            Ok(true)
        }

        ChangeType::Deploy => {
            let target = change
                .target_path
                .as_ref()
                .context("Deploy requires target_path")?;

            if dry_run {
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                fs::create_dir_all(parent)?;
            }

            // Create hard link
            fs::hard_link(&change.source_path, target).with_context(|| {
                format!(
                    "Failed to deploy {} to {}",
                    change.source_path, target
                )
            })?;
            Ok(true)
        }

        ChangeType::Undeploy => {
            // Remove the deployed hard link
            if dry_run {
                return Ok(true);
            }

            fs::remove_file(&change.source_path)
                .with_context(|| format!("Failed to undeploy {}", change.source_path))?;
            Ok(true)
        }

        ChangeType::Redeploy => {
            // Relocate existing library link to new path (stale path fix)
            // source_path = current library path, target_path = expected library path
            // Both are hard links to the same corpus file, so we can just rename
            let target = change
                .target_path
                .as_ref()
                .context("Redeploy requires target_path")?;

            let _ = config::log_message(&format!(
                "[REDEPLOY] source={} target={}",
                change.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&change.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[REDEPLOY] ERROR: Source does not exist: {}",
                    change.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", change.source_path);
            }

            if dry_run {
                let _ = config::log_message("[REDEPLOY] DRY RUN - skipping actual move");
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                let _ = config::log_message(&format!(
                    "[REDEPLOY] Creating target parent dir: {}",
                    parent.display()
                ));
                fs::create_dir_all(parent)?;
            }

            // Rename moves the hard link to the new path, preserving inode relationship
            let _ = config::log_message(&format!(
                "[REDEPLOY] Executing fs::rename {} -> {}",
                change.source_path, target
            ));
            fs::rename(&change.source_path, target).with_context(|| {
                format!(
                    "Failed to redeploy {} to {}",
                    change.source_path, target
                )
            })?;
            let _ = config::log_message("[REDEPLOY] SUCCESS");
            Ok(true)
        }

        ChangeType::DropIndex => {
            // Remove entry from tracks table (file already missing from disk)
            let _ = config::log_message(&format!(
                "[DROP_INDEX] Removing from index: {}",
                change.source_path
            ));
            let _ = config::log_message(&format!(
                "[DROP_INDEX] Path length: {} bytes",
                change.source_path.len()
            ));

            if dry_run {
                return Ok(true);
            }

            match db.delete_track_by_path(&change.source_path) {
                Ok(true) => {
                    let _ = config::log_message("[DROP_INDEX] Removed from index successfully");
                }
                Ok(false) => {
                    let _ = config::log_message("[DROP_INDEX] Track was not in index (already removed?)");
                }
                Err(e) => {
                    // Use {:#} to show the full error chain
                    let _ = config::log_message(&format!(
                        "[DROP_INDEX] Failed to remove from index: {:#}",
                        e
                    ));
                    return Err(e);
                }
            }
            Ok(true)
        }

        ChangeType::OutOfBandTagChange => {
            // OutOfBandTagChange is a marker for review, not directly executable.
            // When resolved, it should be converted to either:
            // - Accept corpus tags (update index) - no filesystem changes
            // - Accept index tags (write to corpus file) - requires metadata write
            //
            // TODO: Operations flow for resolving OutOfBandTagChange mutations
            // - Present list of files with tag differences
            // - Options: Accept corpus (update index), Accept index (revert corpus tags),
            //   or Manual (edit tags)
            // - Opinion system: auto-accept corpus changes for specific directories
            // - Consider batch resolution by artist/album
            let _ = config::log_message(&format!(
                "[OUT_OF_BAND_TAG_CHANGE] Skipping - requires resolution: {}",
                change.source_path
            ));
            // Skip - can't execute directly
            Ok(false)
        }
    }
}

/// Execute pending changes
pub fn execute_changes(
    db: &Database,
    changes: &[PendingChange],
    dry_run: bool,
) -> Result<ExecutionReport> {
    use crate::config;

    let _ = config::log_message(&format!(
        "=== execute_changes called: {} changes, dry_run={} ===",
        changes.len(),
        dry_run
    ));

    let mut report = ExecutionReport::default();

    for (i, change) in changes.iter().enumerate() {
        let _ = config::log_message(&format!(
            "[execute_changes] Processing change {}/{}: status={:?}",
            i + 1,
            changes.len(),
            change.status
        ));

        // Skip already committed/reverted changes
        if change.status != ChangeStatus::Pending {
            let _ = config::log_message(&format!(
                "[execute_changes] Skipping non-pending change (status={:?})",
                change.status
            ));
            report.skipped += 1;
            continue;
        }

        match execute_single_change(change, db, dry_run) {
            Ok(true) => {
                let _ = config::log_message(&format!(
                    "[execute_changes] Change {} succeeded",
                    i + 1
                ));
                report.succeeded += 1;
                // Update status in database
                if !dry_run {
                    if let Some(id) = change.id {
                        let _ = config::log_message(&format!(
                            "[execute_changes] Updating DB status for change id={}",
                            id
                        ));
                        let _ = db.update_change_status(id, ChangeStatus::Committed);
                    } else {
                        let _ = config::log_message(
                            "[execute_changes] No change ID - cannot update DB status"
                        );
                    }
                }
            }
            Ok(false) => {
                let _ = config::log_message(&format!(
                    "[execute_changes] Change {} returned false (skipped)",
                    i + 1
                ));
                report.skipped += 1;
            }
            Err(e) => {
                let _ = config::log_message(&format!(
                    "[execute_changes] Change {} FAILED: {}",
                    i + 1,
                    e
                ));
                report.failed += 1;
                report.errors.push(format!("{}: {}", change.source_path, e));
            }
        }
    }

    let _ = config::log_message(&format!(
        "=== execute_changes complete: succeeded={} failed={} skipped={} ===",
        report.succeeded, report.failed, report.skipped
    ));

    Ok(report)
}
