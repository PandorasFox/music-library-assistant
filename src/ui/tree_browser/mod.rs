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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::corpus::db::ReadOnlyDb;
use crate::ui::filter_popup::FilterCondition;

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
    pub fn corpus_browser(root: PathBuf, variant_config: CorpusBrowserConfig, deploy_source_paths: Vec<PathBuf>) -> Self {
        let filter = if variant_config.show_files {
            EntryFilter::with_files()
        } else {
            EntryFilter::directories_only()
        };

        let navigator = TreeNavigator::new(root.clone(), filter, true, deploy_source_paths);
        let variant = BrowserVariant::CorpusBrowser(CorpusBrowserVariant::new(variant_config));

        Self { navigator, variant }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> TreeBrowserAction {
        input::handle_key(key, &mut self.navigator, &mut self.variant)
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
    pub fn apply_filter(&mut self, condition: FilterCondition, read_db: &ReadOnlyDb<'_>) {
        if !condition.is_active() {
            self.clear_filter();
            return;
        }

        // Query all audio files from database
        let audio_files = match read_db.get_all_audio_files(crate::corpus::db::types::Zone::Corpus) {
            Ok(af) => af,
            Err(_) => {
                self.clear_filter();
                return;
            }
        };

        // Get tags for each file and filter
        let mut matching_paths: Vec<PathBuf> = Vec::new();
        for audio_file in audio_files {
            // Get tags for this file and convert to HashMap (multi-value)
            let mut tags: HashMap<String, Vec<String>> = HashMap::new();
            for t in read_db.get_corpus_tags(audio_file.inode()).unwrap_or_default() {
                tags.entry(t.tag_name.to_uppercase()).or_default().push(t.tag_value);
            }

            // Check if file matches filter
            if condition.matches(
                audio_file.path(),
                &audio_file.audio.file_type,
                audio_file.audio.sample_rate,
                audio_file.audio.bitrate_kbps,
                audio_file.audio.duration_ms,
                &tags,
            ) {
                matching_paths.push(PathBuf::from(audio_file.path()));
            }
        }

        if matching_paths.is_empty() {
            // No matches - keep current view but don't apply empty filter
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
}
