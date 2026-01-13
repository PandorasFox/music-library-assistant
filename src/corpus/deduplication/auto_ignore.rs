//! Auto-ignore state tracking for deduplication UI workflows.
//!
//! Tracks skip patterns to offer auto-ignoring of frequently skipped directories.

use std::collections::{HashMap, HashSet};

/// Tracks skip patterns and manages auto-ignore functionality.
/// After 5 consecutive skips of a directory, prompts user to auto-ignore it.
#[derive(Debug, Clone)]
pub struct AutoIgnoreState {
    skip_counts: HashMap<String, usize>,
    ignored_dirs: HashSet<String>,
}

impl AutoIgnoreState {
    /// Create a new auto-ignore state tracker.
    pub fn new() -> Self {
        Self {
            skip_counts: HashMap::new(),
            ignored_dirs: HashSet::new(),
        }
    }

    /// Record a skip for a directory.
    /// Returns true if should prompt user about auto-ignore (hit 5-skip threshold).
    pub fn record_skip(&mut self, dir: &str) -> bool {
        let count = self.skip_counts.entry(dir.to_string()).or_insert(0);
        *count += 1;
        *count == 5
    }

    /// Record a win (user picked this directory) - resets skip counter.
    pub fn record_win(&mut self, dir: &str) {
        self.skip_counts.remove(dir);
    }

    /// Add directory to ignore list.
    pub fn add_ignored(&mut self, dir: String) {
        self.ignored_dirs.insert(dir);
    }

    /// Check if directory should be auto-ignored.
    pub fn should_ignore(&self, dir: &str) -> bool {
        self.ignored_dirs.contains(dir)
    }

    /// Check if entire cluster should be skipped (all dirs ignored).
    pub fn should_skip_cluster(&self, cluster_dirs: &[String]) -> bool {
        cluster_dirs.iter().all(|d| self.should_ignore(d))
    }

    /// Get list of currently ignored directories (for display).
    pub fn get_ignored_dirs(&self) -> Vec<String> {
        self.ignored_dirs.iter().cloned().collect()
    }
}

impl Default for AutoIgnoreState {
    fn default() -> Self {
        Self::new()
    }
}
