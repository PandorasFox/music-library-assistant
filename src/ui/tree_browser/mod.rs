//! Tree Browser Module
//!
//! Unified tree browser with variant modes. Currently only CorpusBrowser exists.
//!
//! ## Usage
//!
//! ```ignore
//! // Create a corpus browser
//! let browser = TreeBrowserState::corpus_browser(corpus_root, CorpusBrowserConfig::default());
//! ```

mod actions;
mod config;
mod entry;
mod input;
mod navigator;
mod render;
pub mod variants;

use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::ui::input::InputAction;

pub use actions::TreeBrowserAction;
pub use config::CorpusBrowserConfig;
pub use entry::TreeEntry;
pub use navigator::{EntryFilter, TreeNavigator};
pub use variants::{BrowserVariant, CorpusBrowserVariant};

/// Unified tree browser state.
///
/// Instantiate with a specific variant - no unvarianted mode exists.
#[derive(Debug)]
pub struct TreeBrowserState {
    /// Shared navigation state
    navigator: TreeNavigator,
    /// Variant-specific state and behavior
    variant: BrowserVariant,
}

impl TreeBrowserState {
    /// Create a CorpusBrowser for browsing files and directories.
    ///
    /// Features:
    /// - Shows both files and directories (full width)
    /// - Type-to-jump search for directories
    /// - Enter launches tag editor (directory = bulk edit, file = single edit)
    /// - Part of lateral view ring (Tab/Shift-Tab cycling)
    ///
    /// `root` is the archive root (shows zone dirs at depth 0).
    /// `corpus_dir` is the corpus subdirectory (for C key and initial focus).
    /// `primary_zone_paths` lists corpus/inbox/stash for dimming non-zone dirs.
    pub fn corpus_browser(
        root: PathBuf,
        variant_config: CorpusBrowserConfig,
        deploy_source_paths: Vec<PathBuf>,
        corpus_dir: PathBuf,
        primary_zone_paths: Vec<PathBuf>,
    ) -> Self {
        let filter = if variant_config.show_files {
            EntryFilter::with_files()
        } else {
            EntryFilter::directories_only()
        };

        let mut navigator = TreeNavigator::new(
            root,
            filter,
            false,
            deploy_source_paths,
            primary_zone_paths,
        );
        navigator.focus_and_expand(&corpus_dir);
        let variant = BrowserVariant::CorpusBrowser(CorpusBrowserVariant::new(variant_config, corpus_dir));

        Self { navigator, variant }
    }

    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> TreeBrowserAction {
        input::handle_input(action, &mut self.navigator, &mut self.variant)
    }

    /// Render the browser.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        render::render(f, area, &mut self.navigator, &mut self.variant);
    }

    // =========================================================================
    // Filtering
    // =========================================================================

    /// Apply a filter condition, showing only matching files and their ancestors.
    ///
    /// Queries the database for tracks matching the filter condition, then
    /// computes the set of matching paths plus all ancestor directories.
    /// Apply filter results (matching paths) to the tree browser.
    ///
    /// Call with paths computed via the cache thread. If empty, the
    /// filter is not applied (existing view retained).
    pub fn apply_filter_results(&mut self, matching_paths: Vec<PathBuf>) {
        if matching_paths.is_empty() {
            return;
        }

        // Build set of matching paths plus all ancestor directories
        let mut all_paths: HashSet<PathBuf> = HashSet::new();
        let root = self.navigator.root_path().clone();

        for path in &matching_paths {
            all_paths.insert(path.clone());

            // Add all ancestor directories up to (but not including) root
            let mut current = path.parent();
            while let Some(parent) = current {
                if parent == root {
                    break;
                }
                all_paths.insert(parent.to_path_buf());
                current = parent.parent();
            }
        }

        // Apply filter to navigator
        self.navigator.set_path_filter(all_paths);
    }

    /// Clear any active filter, restoring full tree view.
    pub fn clear_filter(&mut self) {
        self.navigator.clear_path_filter();
    }

    /// Get the path of the currently selected entry (if any).
    pub fn selected_path(&self) -> Option<&std::path::Path> {
        self.navigator.current_entry().map(|e| e.path.as_path())
    }

    // =========================================================================
    // Config Panel
    // =========================================================================

    /// Set the config panel state and switch focus to it.
    pub fn set_config_panel(&mut self, panel: variants::corpus::DirConfigPanelState) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.config_panel = Some(panel);
        v.set_focus_config_panel();
    }

    /// Clear the config panel and return focus to tree.
    pub fn clear_config_panel(&mut self) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.config_panel = None;
        v.set_focus_tree();
    }

    /// Get a reference to the config panel state, if open.
    pub fn config_panel(&self) -> Option<&variants::corpus::DirConfigPanelState> {
        let BrowserVariant::CorpusBrowser(ref v) = self.variant;
        v.config_panel.as_ref()
    }

    /// Set pending edit paths for visual markers.
    pub fn set_pending_edit_paths(&mut self, paths: HashSet<std::path::PathBuf>) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.set_pending_edit_paths(paths);
    }
}
