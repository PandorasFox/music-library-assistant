//! Directory Selector Variant
//!
//! Browse directories with multi-select checkboxes, return selected paths.
//! Used for scoped operations like fingerprint-based deduplication.

use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};

use crate::ui::tree_browser::actions::TreeBrowserAction;
use crate::ui::tree_browser::config::DirectorySelectorConfig;
use crate::ui::tree_browser::entry::TreeEntry;
use crate::ui::tree_browser::navigator::{EntryFilter, TreeNavigator};

/// Directory selector variant state.
#[derive(Debug)]
pub struct DirectorySelectorVariant {
    /// Variant-specific configuration
    config: DirectorySelectorConfig,
    /// Paths that have been selected (space-toggled)
    selected_paths: HashSet<PathBuf>,
}

impl DirectorySelectorVariant {
    /// Create a new directory selector variant.
    pub fn new(config: DirectorySelectorConfig) -> Self {
        Self {
            config,
            selected_paths: HashSet::new(),
        }
    }

    /// Get the entry filter for directory selector (directories only).
    pub fn entry_filter(&self) -> EntryFilter {
        EntryFilter::directories_only()
    }

    /// Handle variant-specific keys.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        match key.code {
            KeyCode::Char(' ') => {
                self.toggle_selection(nav);
                TreeBrowserAction::None
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.select_all(nav);
                TreeBrowserAction::None
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.deselect_all();
                TreeBrowserAction::None
            }
            KeyCode::Enter => self.get_proceed_action(nav),
            _ => TreeBrowserAction::None,
        }
    }

    // =========================================================================
    // Selection Management
    // =========================================================================

    /// Toggle selection on current entry.
    pub fn toggle_selection(&mut self, nav: &TreeNavigator) {
        if let Some(entry) = nav.current_entry() {
            let path = entry.path.clone();

            // Can't toggle ancestors of selected paths
            if self.is_ancestor_of_selected(&path) {
                return;
            }

            if self.selected_paths.contains(&path) {
                self.selected_paths.remove(&path);
            } else {
                if !self.config.multi_select {
                    self.selected_paths.clear();
                }
                self.selected_paths.insert(path);
            }
        }
    }

    /// Select all visible entries.
    pub fn select_all(&mut self, nav: &TreeNavigator) {
        if self.config.multi_select {
            for entry in nav.entries() {
                if self.is_selectable(&entry.path) {
                    self.selected_paths.insert(entry.path.clone());
                }
            }
        }
    }

    /// Deselect all entries.
    pub fn deselect_all(&mut self) {
        self.selected_paths.clear();
    }

    /// Check if a path is selected.
    pub fn is_selected(&self, path: &PathBuf) -> bool {
        self.selected_paths.contains(path)
    }

    /// Check if path is an ancestor of any selected path.
    pub fn is_ancestor_of_selected(&self, path: &PathBuf) -> bool {
        self.selected_paths
            .iter()
            .any(|selected| selected.starts_with(path) && selected != path)
    }

    /// Check if path is selectable (not an ancestor of a selection).
    pub fn is_selectable(&self, path: &PathBuf) -> bool {
        !self.is_ancestor_of_selected(path)
    }

    /// Get all selected paths.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_paths.iter().cloned().collect()
    }

    /// Get the selection count.
    pub fn selection_count(&self) -> usize {
        self.selected_paths.len()
    }

    // =========================================================================
    // Actions
    // =========================================================================

    /// Get action for Enter key.
    fn get_proceed_action(&self, nav: &TreeNavigator) -> TreeBrowserAction {
        if self.selected_paths.is_empty() {
            // No explicit selection - use current cursor position
            if let Some(entry) = nav.current_entry() {
                TreeBrowserAction::SelectPaths(vec![entry.path.clone()])
            } else {
                TreeBrowserAction::Cancel
            }
        } else {
            TreeBrowserAction::SelectPaths(self.selected_paths())
        }
    }

    // =========================================================================
    // Rendering Helpers
    // =========================================================================

    /// Get the selection marker for an entry.
    pub fn selection_marker(&self, entry: &TreeEntry) -> &'static str {
        if self.is_selected(&entry.path) {
            "[x]"
        } else if self.is_ancestor_of_selected(&entry.path) {
            "[-]"
        } else {
            "[ ]"
        }
    }

    /// Get config reference.
    pub fn config(&self) -> &DirectorySelectorConfig {
        &self.config
    }
}
