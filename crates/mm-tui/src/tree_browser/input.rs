//! Input Handling
//!
//! Unified keyboard input handling for the tree browser.
//! Common navigation keys are handled here; variant-specific keys delegate to variants.

use mm_ui::directory_browser::{BrowserAction, DirectoryBrowser};

use crate::input::InputAction;

use super::actions::TreeBrowserAction;
use super::variants::BrowserVariant;

/// Result of handling input — may include both a domain action and a browser action.
pub struct InputResult {
    /// Domain action for the app layer (e.g. EditDirectory, Cancel).
    pub tree_action: Option<TreeBrowserAction>,
    /// Browser action requiring app-layer dispatch (e.g. RequestExpand, Collapse).
    pub browser_action: Option<BrowserAction>,
}

/// Handle a semantic input action for the tree browser.
pub fn handle_input(
    action: &InputAction,
    browser: &mut DirectoryBrowser,
    variant: &mut BrowserVariant,
) -> InputResult {
    // Check if variant wants to capture navigation keys (e.g., search active)
    let variant_captures_nav = variant.wants_navigation_keys();

    // Handle Escape first - variant gets priority
    if matches!(action, InputAction::Cancel) {
        if variant.handle_escape(browser) {
            return InputResult { tree_action: None, browser_action: None };
        }
        return InputResult {
            tree_action: Some(TreeBrowserAction::Cancel),
            browser_action: None,
        };
    }

    // Tab for lateral ring cycling — but NOT when variant captures navigation
    if !variant_captures_nav {
        match action {
            InputAction::CycleNext | InputAction::CyclePrev => {
                let vr = variant.handle_input(action, browser);
                return InputResult {
                    tree_action: vr.tree_action,
                    browser_action: vr.browser_action,
                };
            }
            _ => {}
        }
    }

    // If variant wants navigation keys, delegate everything to it
    if variant_captures_nav {
        let vr = variant.handle_input(action, browser);
        return InputResult {
            tree_action: vr.tree_action,
            browser_action: vr.browser_action,
        };
    }

    // Common navigation keys (only when variant doesn't capture)
    match action {
        InputAction::NavUp | InputAction::NavDown
        | InputAction::Home | InputAction::End
        | InputAction::PageUp | InputAction::PageDown => {
            browser.handle_input(action);
            variant.on_cursor_move(browser);
            return InputResult { tree_action: None, browser_action: None };
        }
        InputAction::NavRight | InputAction::NavLeft => {
            let browser_action = browser.handle_input(action);
            return InputResult { tree_action: None, browser_action };
        }
        _ => {}
    }

    // Delegate remaining keys to variant
    let vr = variant.handle_input(action, browser);
    InputResult {
        tree_action: vr.tree_action,
        browser_action: vr.browser_action,
    }
}
