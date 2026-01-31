//! Deploy Modal Types
//!
//! Data structures for the deploy modal, including cached signal data
//! and confirmation dialog state.

use std::collections::HashMap;
use std::path::Path;

use crate::corpus::db::types::{ConflictGroup, DeploySignalFile, LeftoverSignalFile, StaleSignalFile};
use crate::corpus::db::ReadOnlyDb;
use anyhow::Result;

/// Directory aggregate for grouped file display.
/// Sorted by count descending.
#[derive(Debug, Clone)]
pub struct DirectoryAggregate {
    /// Directory path
    pub directory: String,
    /// Number of files in this directory
    pub count: usize,
}

/// Cached data for the deploy modal.
///
/// Loaded once when the modal opens, contains all signal lists.
/// This prevents database queries during render.
#[derive(Debug, Clone, Default)]
pub struct DeployModalData {
    /// Healthy files: deployed at correct library path
    pub healthy: Vec<DeploySignalFile>,
    /// New files: ready to deploy (not yet in library)
    pub new: Vec<DeploySignalFile>,
    /// New files aggregated by directory (sorted by count desc)
    pub new_by_dir: Vec<DirectoryAggregate>,
    /// Conflict groups: multiple corpus files → same library path
    pub conflicts: Vec<ConflictGroup>,
    /// Leftover files: in library but no corpus backing
    pub leftover: Vec<LeftoverSignalFile>,
    /// Leftover files aggregated by directory (sorted by count desc)
    pub leftover_by_dir: Vec<DirectoryAggregate>,
    /// Stale files: deployed at wrong path (tags changed)
    pub stale: Vec<StaleSignalFile>,
}

impl DeployModalData {
    /// Load all deploy signal data from the database.
    ///
    /// Called once when the modal opens. All subsequent renders
    /// use this cached data.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        let healthy = read_db.get_deployed_healthy_files()?;
        let new = read_db.get_deploy_ready_files()?;
        let conflicts = read_db.get_deploy_conflict_groups()?;
        let leftover = read_db.get_library_leftover_files()?;
        let stale = read_db.get_library_stale_files()?;

        // Aggregate new files by directory (using corpus_path)
        let new_by_dir = Self::aggregate_by_directory(
            new.iter().map(|f| f.corpus_path.as_str())
        );

        // Aggregate leftover files by directory (using library_path)
        let leftover_by_dir = Self::aggregate_by_directory(
            leftover.iter().map(|f| f.library_path.as_str())
        );

        Ok(Self {
            healthy,
            new,
            new_by_dir,
            conflicts,
            leftover,
            leftover_by_dir,
            stale,
        })
    }

    /// Aggregate paths by their parent directory, sorted by count descending.
    fn aggregate_by_directory<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<DirectoryAggregate> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for path in paths {
            let dir = Path::new(path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            *counts.entry(dir).or_insert(0) += 1;
        }

        let mut aggregates: Vec<_> = counts
            .into_iter()
            .map(|(directory, count)| DirectoryAggregate { directory, count })
            .collect();

        // Sort by count descending
        aggregates.sort_by(|a, b| b.count.cmp(&a.count));
        aggregates
    }

    /// Get counts for each tab (for display in tab bar).
    /// New and Leftover show directory counts, others show file counts.
    pub fn tab_counts(&self) -> [usize; 5] {
        [
            self.healthy.len(),
            self.new_by_dir.len(),      // Directory count
            self.conflicts.len(),
            self.leftover_by_dir.len(), // Directory count
            self.stale.len(),
        ]
    }

    /// Get summary for confirmation dialog.
    pub fn summary(&self) -> DeploySummary {
        DeploySummary {
            new_count: self.new.len(),
            stale_count: self.stale.len(),
            leftover_count: self.leftover.len(),
            conflict_count: self.conflicts.len(),
            healthy_count: self.healthy.len(),
        }
    }
}

/// Summary of deploy operations for confirmation dialog.
#[derive(Debug, Clone, Default)]
pub struct DeploySummary {
    /// New files to deploy
    pub new_count: usize,
    /// Stale files to redeploy
    pub stale_count: usize,
    /// Leftover files to remove
    pub leftover_count: usize,
    /// Conflicts blocking deployment
    pub conflict_count: usize,
    /// Already healthy (no action needed)
    pub healthy_count: usize,
}

impl DeploySummary {
    /// Total operations that will be performed (excluding healthy).
    /// Conflicts are auto-resolved by picking first alphabetical path.
    pub fn total_operations(&self) -> usize {
        self.new_count + self.stale_count + self.leftover_count + self.conflict_count
    }
}

/// Buttons on the deploy confirmation modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployConfirmButton {
    /// Go back to editing/preview (safe default for Enter-triggered modal)
    Cancel,
    /// Execute the deployment
    Confirm,
    /// Discard staged changes and return to insights (safe default for Esc-triggered modal)
    Discard,
}

impl DeployConfirmButton {
    /// Cycle to next button (left direction).
    pub fn prev(self) -> Self {
        match self {
            Self::Cancel => Self::Discard,
            Self::Confirm => Self::Cancel,
            Self::Discard => Self::Confirm,
        }
    }

    /// Cycle to next button (right direction).
    pub fn next(self) -> Self {
        match self {
            Self::Cancel => Self::Confirm,
            Self::Confirm => Self::Discard,
            Self::Discard => Self::Cancel,
        }
    }
}

/// State for the confirmation dialog.
#[derive(Debug, Clone)]
pub struct DeployConfirmModal {
    pub summary: DeploySummary,
    pub selected_button: DeployConfirmButton,
}

impl DeployConfirmModal {
    /// Create modal for Enter key (wanting to confirm) - defaults to Cancel (safe).
    pub fn for_confirm(summary: DeploySummary) -> Self {
        Self {
            summary,
            selected_button: DeployConfirmButton::Cancel,
        }
    }

    /// Create modal for Esc key (wanting to leave) - defaults to Discard (safe, changes are trivial to re-stage).
    pub fn for_escape(summary: DeploySummary) -> Self {
        Self {
            summary,
            selected_button: DeployConfirmButton::Discard,
        }
    }

    /// Navigate button selection left.
    pub fn select_prev(&mut self) {
        self.selected_button = self.selected_button.prev();
    }

    /// Navigate button selection right.
    pub fn select_next(&mut self) {
        self.selected_button = self.selected_button.next();
    }
}
