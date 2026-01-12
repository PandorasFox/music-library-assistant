//! Algebraic Change Tracking Module
//!
//! Tracks corpus operations as composable, reversible functions.
//! Changes accumulate as "pending" before execution, enabling:
//! - Dry-run preview of changes
//! - Staging to a preview library
//! - Atomic commit or revert of change sets
//!
//! NOTE: Much of this module is implemented but not yet wired into the UI.
//! See docs/FUTURE_FEATURES.md for planned integration.

#![allow(dead_code)]

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::{ChangeStatus, ChangeType, Database, PendingChange};

/// Summary of a change for preview display
#[derive(Debug, Clone)]
pub struct ChangePreview {
    pub change: PendingChange,
    pub source_exists: bool,
    pub target_exists: bool,
    pub is_reversible: bool,
}

/// Result of executing changes
#[derive(Debug, Default)]
pub struct ExecutionReport {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Result of creating a staging preview
#[derive(Debug)]
pub struct StagingResult {
    pub staging_root: PathBuf,
    pub links_created: usize,
    pub errors: Vec<String>,
}

/// Result of reverting changes
#[derive(Debug, Default)]
pub struct RevertReport {
    pub reverted: usize,
    pub failed: usize,
    pub not_reversible: usize,
    pub errors: Vec<String>,
}

/// Preview a single pending change
pub fn preview_change(change: &PendingChange) -> ChangePreview {
    let source_exists = Path::new(&change.source_path).exists();
    let target_exists = change
        .target_path
        .as_ref()
        .map(|p| Path::new(p).exists())
        .unwrap_or(false);

    // Most changes are reversible except for deployed files that get modified
    let is_reversible = match change.change_type {
        ChangeType::Move | ChangeType::Delete => true, // Can move back
        ChangeType::TagEdit => true,                   // Old values stored in metadata_changes
        ChangeType::Deploy => true,                    // Can remove hard link
        ChangeType::Undeploy => true,                  // Can re-create hard link
        ChangeType::DropIndex => false,               // Can't restore - file is gone
    };

    ChangePreview {
        change: change.clone(),
        source_exists,
        target_exists,
        is_reversible,
    }
}

/// Preview all pending changes
pub fn preview_changes(changes: &[PendingChange]) -> Vec<ChangePreview> {
    changes.iter().map(preview_change).collect()
}

/// Get a summary of pending changes grouped by type
pub fn summarize_changes(changes: &[PendingChange]) -> HashMap<ChangeType, usize> {
    let mut summary = HashMap::new();
    for change in changes {
        *summary.entry(change.change_type).or_insert(0) += 1;
    }
    summary
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
            // Tag edits are handled by the metadata module
            // The metadata_changes field contains the JSON diff
            if dry_run {
                return Ok(true);
            }

            // For now, tag edits are applied directly via lofty
            // This will be enhanced to use the metadata_changes JSON
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

/// Create a staging preview with hard links in a temporary directory
pub fn create_staging_preview(
    changes: &[PendingChange],
    staging_root: &Path,
) -> Result<StagingResult> {
    let mut result = StagingResult {
        staging_root: staging_root.to_path_buf(),
        links_created: 0,
        errors: Vec::new(),
    };

    // Create staging root if it doesn't exist
    fs::create_dir_all(staging_root)?;

    for change in changes {
        // Only stage Deploy changes for preview
        if change.change_type != ChangeType::Deploy {
            continue;
        }

        if let Some(target) = &change.target_path {
            // Create the same directory structure under staging root
            let relative_target = target.strip_prefix('/').unwrap_or(target.as_str());

            let staging_path = staging_root.join(relative_target);

            // Ensure parent directory exists
            if let Some(parent) = staging_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    result
                        .errors
                        .push(format!("Failed to create directory for {}: {}", target, e));
                    continue;
                }
            }

            // Create hard link
            match fs::hard_link(&change.source_path, &staging_path) {
                Ok(_) => result.links_created += 1,
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to create staging link {} -> {}: {}",
                        change.source_path,
                        staging_path.display(),
                        e
                    ));
                }
            }
        }
    }

    Ok(result)
}

/// Revert committed changes (where possible)
pub fn revert_changes(db: &Database, changes: &[PendingChange]) -> Result<RevertReport> {
    let mut report = RevertReport::default();

    for change in changes {
        // Can only revert committed changes
        if change.status != ChangeStatus::Committed {
            report.not_reversible += 1;
            continue;
        }

        let reverted = match change.change_type {
            ChangeType::Move | ChangeType::Delete => {
                // Swap source and target to reverse the move
                if let Some(target) = &change.target_path {
                    match fs::rename(target, &change.source_path) {
                        Ok(_) => true,
                        Err(e) => {
                            report
                                .errors
                                .push(format!("Failed to revert move {}: {}", target, e));
                            false
                        }
                    }
                } else {
                    report
                        .errors
                        .push(format!("Cannot revert {}: no target path", change.source_path));
                    false
                }
            }

            ChangeType::Deploy => {
                // Remove the hard link that was created
                if let Some(target) = &change.target_path {
                    match fs::remove_file(target) {
                        Ok(_) => true,
                        Err(e) => {
                            report.errors.push(format!(
                                "Failed to revert deploy {}: {}",
                                target, e
                            ));
                            false
                        }
                    }
                } else {
                    false
                }
            }

            ChangeType::Undeploy => {
                // Re-create the hard link that was removed
                if let Some(target) = &change.target_path {
                    match fs::hard_link(&change.source_path, target) {
                        Ok(_) => true,
                        Err(e) => {
                            report.errors.push(format!(
                                "Failed to revert undeploy {}: {}",
                                change.source_path, e
                            ));
                            false
                        }
                    }
                } else {
                    false
                }
            }

            ChangeType::TagEdit => {
                // Tag edits would need to restore from metadata_changes
                // For now, mark as not automatically reversible
                report.not_reversible += 1;
                continue;
            }

            ChangeType::DropIndex => {
                // DropIndex cannot be reverted - the file no longer exists
                report.not_reversible += 1;
                continue;
            }
        };

        if reverted {
            report.reverted += 1;
            // Update status in database
            if let Some(id) = change.id {
                let _ = db.update_change_status(id, ChangeStatus::Reverted);
            }
        } else {
            report.failed += 1;
        }
    }

    Ok(report)
}

/// Format a change for display
pub fn format_change(change: &PendingChange) -> String {
    match change.change_type {
        ChangeType::Move => {
            format!(
                "~ {} -> {}",
                change.source_path,
                change.target_path.as_deref().unwrap_or("?")
            )
        }
        ChangeType::Delete => {
            format!("- {} (-> stash)", change.source_path)
        }
        ChangeType::TagEdit => {
            format!("* {} [tag edit]", change.source_path)
        }
        ChangeType::Deploy => {
            format!(
                "+ {} -> {}",
                change.source_path,
                change.target_path.as_deref().unwrap_or("?")
            )
        }
        ChangeType::Undeploy => {
            format!("- {} [undeploy]", change.source_path)
        }
        ChangeType::DropIndex => {
            format!("x {} [drop from index]", change.source_path)
        }
    }
}
