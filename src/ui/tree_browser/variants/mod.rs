//! Browser Variants
//!
//! Each variant provides a distinct, purpose-tuned mode for the tree browser.
//! Currently only CorpusBrowser exists - DirectorySelector was removed as vestigial.

mod corpus;

pub use corpus::CorpusBrowserVariant;

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
}

impl BrowserVariant {
    /// Get the entry filter for this variant.
    pub fn entry_filter(&self) -> EntryFilter {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.entry_filter(),
        }
    }

    /// Called when cursor moves - allows variants to update state.
    pub fn on_cursor_move(&mut self, nav: &TreeNavigator) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.on_cursor_move(nav),
        }
    }

    /// Handle Escape key - return true if handled (don't propagate Cancel).
    pub fn handle_escape(&mut self, nav: &mut TreeNavigator) -> bool {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_escape(nav),
        }
    }

    /// Check if variant wants to capture navigation keys (arrows, hjkl).
    /// When true, navigation keys are delegated to the variant instead of tree browser.
    pub fn wants_navigation_keys(&self) -> bool {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.wants_navigation_keys(),
        }
    }

    /// Handle variant-specific key input.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_key(key, nav),
        }
    }

    /// Render variant-specific overlays (search popup, match modal, etc.).
    pub fn render_overlays(&self, f: &mut Frame, area: Rect) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.render_overlays(f, area),
        }
    }
}
