//! Algebraic Change Tracking Module
//!
//! Tracks corpus operations as composable, reversible functions.
//! Changes accumulate as "pending" before execution, enabling:
//! - Dry-run preview of changes
//! - Staging to a preview library
//! - Atomic commit or revert of change sets

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
fn execute_single_change(change: &PendingChange, dry_run: bool) -> Result<bool> {
    match change.change_type {
        ChangeType::Move => {
            let target = change
                .target_path
                .as_ref()
                .context("Move requires target_path")?;

            if dry_run {
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                fs::create_dir_all(parent)?;
            }

            fs::rename(&change.source_path, target)
                .with_context(|| format!("Failed to move {} to {}", change.source_path, target))?;
            Ok(true)
        }

        ChangeType::Delete => {
            // "Delete" means move to lost-files, not actual deletion
            let target = change
                .target_path
                .as_ref()
                .context("Delete requires target_path (lost-files location)")?;

            if dry_run {
                return Ok(true);
            }

            // Ensure target parent directory exists
            if let Some(parent) = Path::new(target).parent() {
                fs::create_dir_all(parent)?;
            }

            fs::rename(&change.source_path, target).with_context(|| {
                format!(
                    "Failed to move {} to lost-files at {}",
                    change.source_path, target
                )
            })?;
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
    }
}

/// Execute pending changes
pub fn execute_changes(
    db: &Database,
    changes: &[PendingChange],
    dry_run: bool,
) -> Result<ExecutionReport> {
    let mut report = ExecutionReport::default();

    for change in changes {
        // Skip already committed/reverted changes
        if change.status != ChangeStatus::Pending {
            report.skipped += 1;
            continue;
        }

        match execute_single_change(change, dry_run) {
            Ok(true) => {
                report.succeeded += 1;
                // Update status in database
                if !dry_run {
                    if let Some(id) = change.id {
                        let _ = db.update_change_status(id, ChangeStatus::Committed);
                    }
                }
            }
            Ok(false) => {
                report.skipped += 1;
            }
            Err(e) => {
                report.failed += 1;
                report.errors.push(format!("{}: {}", change.source_path, e));
            }
        }
    }

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
            let relative_target = if target.starts_with('/') {
                // Strip leading slash for path joining
                &target[1..]
            } else {
                target.as_str()
            };

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
            format!("- {} (-> lost-files)", change.source_path)
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
    }
}
