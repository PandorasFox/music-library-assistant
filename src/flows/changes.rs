//! Decision Execution Module
//!
//! Executes pending operator decisions (deletes, tag edits, deploys).

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::decisions::{DecisionType, PendingDecision};
use crate::corpus::db::Database;

/// Result of executing decisions
#[derive(Debug, Default)]
pub struct ExecutionReport {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Execute a single decision on the filesystem
fn execute_single_decision(
    decision: &PendingDecision,
    db: &Database,
    dry_run: bool,
) -> Result<bool> {
    use crate::config;

    let _ = config::log_message(&format!(
        "[execute_single_decision] type={:?} source={} target={} dry_run={}",
        decision.decision_type,
        decision.source_path,
        decision.target_path.as_deref().unwrap_or("(none)"),
        dry_run
    ));

    match decision.decision_type {
        DecisionType::Move => {
            let target = decision
                .target_path
                .as_ref()
                .context("Move requires target_path")?;

            let _ = config::log_message(&format!(
                "[MOVE] source={} target={}",
                decision.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&decision.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[MOVE] ERROR: Source does not exist: {}",
                    decision.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", decision.source_path);
            }
            let _ = config::log_message(&format!(
                "[MOVE] Source exists: {} (size={} bytes)",
                decision.source_path,
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
                decision.source_path, target
            ));
            fs::rename(&decision.source_path, target)
                .with_context(|| format!("Failed to move {} to {}", decision.source_path, target))?;
            let _ = config::log_message("[MOVE] SUCCESS");
            Ok(true)
        }

        DecisionType::Delete => {
            // "Delete" means move to stash, not actual deletion
            let target = decision
                .target_path
                .as_ref()
                .context("Delete requires target_path (stash location)")?;

            let _ = config::log_message(&format!(
                "[DELETE/STASH] source={} target={}",
                decision.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&decision.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[DELETE] ERROR: Source does not exist: {}",
                    decision.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", decision.source_path);
            }
            let _ = config::log_message(&format!(
                "[DELETE] Source exists: {} (size={} bytes)",
                decision.source_path,
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
                decision.source_path, target
            ));
            fs::rename(&decision.source_path, target).with_context(|| {
                format!(
                    "Failed to move {} to stash at {}",
                    decision.source_path, target
                )
            })?;
            let _ = config::log_message("[DELETE] File move SUCCESS");

            // Remove from corpus index
            let _ = config::log_message(&format!(
                "[DELETE] Removing from index: {}",
                decision.source_path
            ));
            match db.delete_track_by_path(&decision.source_path) {
                Ok(true) => {
                    let _ = config::log_message("[DELETE] Removed from index successfully");
                }
                Ok(false) => {
                    let _ =
                        config::log_message("[DELETE] Track was not in index (already removed?)");
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

        DecisionType::TagEdit => {
            // Tag edits use metadata JSON: { "tags": [["field", "value"], ...], "track_id": N }
            let metadata_json = decision
                .metadata
                .as_ref()
                .context("TagEdit requires metadata")?;

            let metadata: serde_json::Value = serde_json::from_str(metadata_json)
                .with_context(|| format!("Invalid metadata JSON: {}", metadata_json))?;

            let tags_array = metadata
                .get("tags")
                .and_then(|v| v.as_array())
                .context("metadata must have 'tags' array")?;

            let track_id = metadata
                .get("track_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

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
                decision.source_path, track_id, tags
            ));

            if dry_run {
                let _ = config::log_message("[TAG_EDIT] DRY RUN - skipping actual write");
                return Ok(true);
            }

            // Write tags to disk and update database
            let path = Path::new(&decision.source_path);
            crate::corpus::metadata::write_tags(path, &tags, track_id, "")
                .with_context(|| format!("Failed to write tags to {}", decision.source_path))?;

            let _ = config::log_message("[TAG_EDIT] SUCCESS");
            Ok(true)
        }

        DecisionType::Deploy => {
            let target = decision
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
            fs::hard_link(&decision.source_path, target).with_context(|| {
                format!(
                    "Failed to deploy {} to {}",
                    decision.source_path, target
                )
            })?;
            Ok(true)
        }

        DecisionType::Undeploy => {
            // Remove the deployed hard link
            if dry_run {
                return Ok(true);
            }

            fs::remove_file(&decision.source_path)
                .with_context(|| format!("Failed to undeploy {}", decision.source_path))?;
            Ok(true)
        }

        DecisionType::Redeploy => {
            // Relocate existing library link to new path (stale path fix)
            // source_path = current library path, target_path = expected library path
            // Both are hard links to the same corpus file, so we can just rename
            let target = decision
                .target_path
                .as_ref()
                .context("Redeploy requires target_path")?;

            let _ = config::log_message(&format!(
                "[REDEPLOY] source={} target={}",
                decision.source_path, target
            ));

            // Check if source exists
            let source_path = Path::new(&decision.source_path);
            if !source_path.exists() {
                let _ = config::log_message(&format!(
                    "[REDEPLOY] ERROR: Source does not exist: {}",
                    decision.source_path
                ));
                anyhow::bail!("Source file does not exist: {}", decision.source_path);
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
                decision.source_path, target
            ));
            fs::rename(&decision.source_path, target).with_context(|| {
                format!(
                    "Failed to redeploy {} to {}",
                    decision.source_path, target
                )
            })?;
            let _ = config::log_message("[REDEPLOY] SUCCESS");
            Ok(true)
        }

        DecisionType::DropIndex => {
            // Remove entry from tracks table (file already missing from disk)
            let _ = config::log_message(&format!(
                "[DROP_INDEX] Removing from index: {}",
                decision.source_path
            ));
            let _ = config::log_message(&format!(
                "[DROP_INDEX] Path length: {} bytes",
                decision.source_path.len()
            ));

            if dry_run {
                return Ok(true);
            }

            match db.delete_track_by_path(&decision.source_path) {
                Ok(true) => {
                    let _ = config::log_message("[DROP_INDEX] Removed from index successfully");
                }
                Ok(false) => {
                    let _ = config::log_message(
                        "[DROP_INDEX] Track was not in index (already removed?)",
                    );
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
    }
}

/// Execute pending decisions
pub fn execute_decisions(
    db: &Database,
    decisions: &[PendingDecision],
    dry_run: bool,
) -> Result<ExecutionReport> {
    use crate::config;

    let _ = config::log_message(&format!(
        "=== execute_decisions called: {} decisions, dry_run={} ===",
        decisions.len(),
        dry_run
    ));

    let mut report = ExecutionReport::default();

    for (i, decision) in decisions.iter().enumerate() {
        let _ = config::log_message(&format!(
            "[execute_decisions] Processing decision {}/{}",
            i + 1,
            decisions.len(),
        ));

        match execute_single_decision(decision, db, dry_run) {
            Ok(true) => {
                let _ = config::log_message(&format!(
                    "[execute_decisions] Decision {} succeeded",
                    i + 1
                ));
                report.succeeded += 1;
            }
            Ok(false) => {
                let _ = config::log_message(&format!(
                    "[execute_decisions] Decision {} returned false (skipped)",
                    i + 1
                ));
                report.skipped += 1;
            }
            Err(e) => {
                let _ = config::log_message(&format!(
                    "[execute_decisions] Decision {} FAILED: {}",
                    i + 1,
                    e
                ));
                report.failed += 1;
                report.errors.push(format!("{}: {}", decision.source_path, e));
            }
        }
    }

    let _ = config::log_message(&format!(
        "=== execute_decisions complete: succeeded={} failed={} skipped={} ===",
        report.succeeded, report.failed, report.skipped
    ));

    Ok(report)
}
