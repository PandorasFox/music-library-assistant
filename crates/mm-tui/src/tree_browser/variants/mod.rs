//! Browser Variants
//!
//! Each variant provides a distinct, purpose-tuned mode for the tree browser.
//! Currently only CorpusBrowser exists.

pub mod corpus;

pub use corpus::CorpusBrowserVariant;

use ratatui::layout::Rect;
use ratatui::Frame;

use mm_ui::directory_browser::{BrowserAction, DirectoryBrowser};

use crate::input::InputAction;

use super::actions::TreeBrowserAction;

/// Result from variant input handling — may include a browser action alongside the domain action.
pub struct VariantInputResult {
    pub tree_action: Option<TreeBrowserAction>,
    pub browser_action: Option<BrowserAction>,
}

/// Browser variants - each is a distinct, purpose-tuned mode.
#[derive(Debug)]
pub enum BrowserVariant {
    /// Browse files+directories with metadata preview, launch tag editor
    CorpusBrowser(CorpusBrowserVariant),
}

impl BrowserVariant {
    /// Called when cursor moves - allows variants to update state.
    pub fn on_cursor_move(&mut self, browser: &DirectoryBrowser) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.on_cursor_move(browser),
        }
    }

    /// Handle Escape key - return true if handled (don't propagate Cancel).
    pub fn handle_escape(&mut self, browser: &mut DirectoryBrowser) -> bool {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_escape(browser),
        }
    }

    /// Check if variant wants to capture navigation keys (arrows).
    /// When true, navigation keys are delegated to the variant instead of tree browser.
    pub fn wants_navigation_keys(&self) -> bool {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.wants_navigation_keys(),
        }
    }

    /// Handle variant-specific input action.
    pub fn handle_input(
        &mut self,
        action: &InputAction,
        browser: &mut DirectoryBrowser,
    ) -> VariantInputResult {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.handle_input(action, browser),
        }
    }

    /// Render variant-specific overlays (search popup, match modal, etc.).
    pub fn render_overlays(&self, f: &mut Frame, area: Rect) {
        match self {
            BrowserVariant::CorpusBrowser(v) => v.render_overlays(f, area),
        }
    }
}
