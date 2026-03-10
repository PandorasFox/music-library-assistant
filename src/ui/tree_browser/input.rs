//! Input Handling
//!
//! Unified keyboard input handling for the tree browser.
//! Common navigation keys are handled here; variant-specific keys delegate to variants.

use crate::ui::input::InputAction;

use super::actions::TreeBrowserAction;
use super::navigator::TreeNavigator;
use super::variants::BrowserVariant;

/// Handle a semantic input action for the tree browser.
///
/// Returns the resulting action. Common navigation is handled here;
/// variant-specific behavior delegates to the variant.
pub fn handle_input(
    action: &InputAction,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
) -> TreeBrowserAction {
    // Check if variant wants to capture navigation keys (e.g., search active)
    let variant_captures_nav = variant.wants_navigation_keys();

    // Handle Escape first - variant gets priority
    if matches!(action, InputAction::Cancel) {
        if variant.handle_escape(nav) {
            return TreeBrowserAction::None;
        }
        return TreeBrowserAction::Cancel;
    }

    // Tab for lateral ring cycling — but NOT when variant captures navigation
    // (config panel uses Tab internally for focus switching)
    if !variant_captures_nav {
        match action {
            InputAction::CycleNext | InputAction::CyclePrev => {
                // Let variant try to handle Tab first (e.g., cycling MB dirs).
                // If variant returns CycleNext/CyclePrev, propagate as lateral ring.
                let result = variant.handle_input(action, nav);
                return match result {
                    TreeBrowserAction::None => TreeBrowserAction::None,
                    _ => result,
                };
            }
            _ => {}
        }
    }

    // If variant wants navigation keys, delegate everything to it
    if variant_captures_nav {
        return variant.handle_input(action, nav);
    }

    // Common navigation keys (only when variant doesn't capture)
    match action {
        InputAction::NavUp => {
            nav.move_up();
            variant.on_cursor_move(nav);
            return TreeBrowserAction::None;
        }
        InputAction::NavDown => {
            nav.move_down();
            variant.on_cursor_move(nav);
            return TreeBrowserAction::None;
        }
        InputAction::NavRight => {
            nav.expand_current();
            return TreeBrowserAction::None;
        }
        InputAction::NavLeft => {
            nav.collapse_or_parent();
            return TreeBrowserAction::None;
        }
        _ => {}
    }

    // Delegate remaining keys to variant
    variant.handle_input(action, nav)
}
