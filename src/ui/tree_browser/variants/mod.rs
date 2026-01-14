//! Browser Variants
//!
//! Each variant provides a distinct, purpose-tuned mode for the tree browser.
//! No unvarianted generic mode exists - only explicit instantiations.

mod corpus;
mod selector;

pub use corpus::{CorpusBrowserVariant, FileMetadata, SearchState};
pub use selector::DirectorySelectorVariant;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use super::actions::TreeBrowserAction;
use super::navigator::{EntryFilter, TreeNavigator};

/// Browser variants - each is a distinct, purpose-tuned mode.
#[derive(Debug)]
pub enum BrowserVariant {
    /// Browse files+directories with metadata preview, launch tag editor
    CorpusBrowser(CorpusBrowserVariant),
    /// Browse directories only with multi-select, return paths
    DirectorySelector(DirectorySelectorVariant),
}

impl BrowserVariant {
    /// Get the entry filter for this variant.
    pub fn entry_filter(&self) -> EntryFilter {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.entry_filter(),
            BrowserVariant::DirectorySelector(v) => v.entry_filter(),
        }
    }

    /// Called when cursor moves - allows variants to update state.
    pub fn on_cursor_move(&mut self, nav: &TreeNavigator) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.on_cursor_move(nav),
            BrowserVariant::DirectorySelector(_) => {}
        }
    }

    /// Handle Escape key - return true if handled (don't propagate Cancel).
    pub fn handle_escape(&mut self, _nav: &mut TreeNavigator) -> bool {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_escape(),
            BrowserVariant::DirectorySelector(_) => false,
        }
    }

    /// Handle variant-specific key input.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_key(key, nav),
            BrowserVariant::DirectorySelector(v) => v.handle_key(key, nav),
        }
    }

    /// Render variant-specific overlays (search popup, match modal, etc.).
    pub fn render_overlays(&self, f: &mut Frame, area: Rect) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.render_overlays(f, area),
            BrowserVariant::DirectorySelector(_) => {}
        }
    }
}
