//! Tree Browser Module
//!
//! Unified tree browser with variant modes. No unvarianted generic mode exists -
//! only explicit, purpose-tuned instantiations.
//!
//! ## Variants
//!
//! - **CorpusBrowser**: Browse files and directories with metadata preview,
//!   type-to-jump search, and tag editor launch.
//! - **DirectorySelector**: Browse directories with multi-select checkboxes,
//!   return selected paths for scoped operations.
//!
//! ## Usage
//!
//! ```ignore
//! // Create a corpus browser
//! let browser = TreeBrowserState::corpus_browser(corpus_root, CorpusBrowserConfig::default());
//!
//! // Create a directory selector for dedup
//! let selector = TreeBrowserState::directory_selector(
//!     corpus_root,
//!     "Select Directories",
//!     DirectorySelectorConfig::for_sleuthing(),
//! );
//! ```

mod actions;
mod config;
mod entry;
mod input;
mod navigator;
mod render;
pub mod variants;

use std::path::PathBuf;

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

pub use actions::TreeBrowserAction;
pub use config::{CorpusBrowserConfig, DirectorySelectorConfig, TreeBrowserConfig};
pub use entry::TreeEntry;
pub use navigator::{EntryFilter, TreeNavigator};
pub use variants::{BrowserVariant, CorpusBrowserVariant, DirectorySelectorVariant, FileMetadata, SearchState};

/// Unified tree browser state.
///
/// Instantiate with a specific variant - no unvarianted mode exists.
#[derive(Debug)]
pub struct TreeBrowserState {
    /// Core configuration
    config: TreeBrowserConfig,
    /// Shared navigation state
    navigator: TreeNavigator,
    /// Variant-specific state and behavior
    variant: BrowserVariant,
}

impl TreeBrowserState {
    /// Create a CorpusBrowser for browsing files with metadata preview.
    ///
    /// Features:
    /// - Shows both files and directories
    /// - Right pane displays file metadata (bitrate, duration, tags)
    /// - Type-to-jump search for directories
    /// - Enter launches tag editor (directory = bulk edit, file = single edit)
    /// - Part of lateral view ring (Tab/Shift-Tab cycling)
    pub fn corpus_browser(root: PathBuf, variant_config: CorpusBrowserConfig) -> Self {
        let filter = if variant_config.show_files {
            EntryFilter::with_files()
        } else {
            EntryFilter::directories_only()
        };

        let navigator = TreeNavigator::new(root.clone(), filter, true);
        let variant = BrowserVariant::CorpusBrowser(CorpusBrowserVariant::new(variant_config));

        Self {
            config: TreeBrowserConfig::new(root, "Corpus Browser").with_lateral_ring(true),
            navigator,
            variant,
        }
    }

    /// Create a DirectorySelector for selecting directories with checkboxes.
    ///
    /// Features:
    /// - Shows only directories (no files)
    /// - Multi-select with checkboxes (Space to toggle)
    /// - Select all (A) / Deselect all (N)
    /// - Enter returns selected paths
    /// - NOT part of lateral view ring
    pub fn directory_selector(
        root: PathBuf,
        title: impl Into<String>,
        variant_config: DirectorySelectorConfig,
    ) -> Self {
        let filter = EntryFilter::directories_only();
        let navigator = TreeNavigator::new(root.clone(), filter, false);
        let variant = BrowserVariant::DirectorySelector(DirectorySelectorVariant::new(variant_config));

        Self {
            config: TreeBrowserConfig::new(root, title).with_lateral_ring(false),
            navigator,
            variant,
        }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> TreeBrowserAction {
        input::handle_key(key, &self.config, &mut self.navigator, &mut self.variant)
    }

    /// Render the browser.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        render::render(f, area, &self.config, &mut self.navigator, &mut self.variant);
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get the browser configuration.
    pub fn config(&self) -> &TreeBrowserConfig {
        &self.config
    }

    /// Get the navigator (shared navigation state).
    pub fn navigator(&self) -> &TreeNavigator {
        &self.navigator
    }

    /// Get mutable reference to navigator.
    pub fn navigator_mut(&mut self) -> &mut TreeNavigator {
        &mut self.navigator
    }

    /// Get the variant.
    pub fn variant(&self) -> &BrowserVariant {
        &self.variant
    }

    /// Get mutable reference to variant.
    pub fn variant_mut(&mut self) -> &mut BrowserVariant {
        &mut self.variant
    }

    /// Get current entry under cursor.
    pub fn current_entry(&self) -> Option<&TreeEntry> {
        self.navigator.current_entry()
    }

    /// Get current path under cursor.
    pub fn current_path(&self) -> Option<&PathBuf> {
        self.navigator.current_path()
    }
}
