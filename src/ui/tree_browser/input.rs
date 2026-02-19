//! Input Handling
//!
//! Unified keyboard input handling for the tree browser.
//! Common navigation keys are handled here; variant-specific keys delegate to variants.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::actions::TreeBrowserAction;
use super::navigator::TreeNavigator;
use super::variants::BrowserVariant;

/// Handle a key event for the tree browser.
///
/// Returns the resulting action. Common navigation is handled here;
/// variant-specific behavior delegates to the variant.
pub fn handle_key(
    key: KeyEvent,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
) -> TreeBrowserAction {
    // Check if variant wants to capture navigation keys (e.g., search active)
    let variant_captures_nav = variant.wants_navigation_keys();

    // Handle Escape first - variant gets priority
    if key.code == KeyCode::Esc {
        if variant.handle_escape(nav) {
            return TreeBrowserAction::None;
        }
        return TreeBrowserAction::Cancel;
    }

    // Tab for lateral ring cycling — but NOT when variant captures navigation
    // (config panel uses Tab internally for focus switching)
    if !variant_captures_nav {
        match key.code {
            KeyCode::Tab => {
                return if key.modifiers.contains(KeyModifiers::SHIFT) {
                    TreeBrowserAction::CyclePrev
                } else {
                    TreeBrowserAction::CycleNext
                };
            }
            KeyCode::BackTab => {
                return TreeBrowserAction::CyclePrev;
            }
            _ => {}
        }
    }

    // If variant wants navigation keys, delegate everything to it
    if variant_captures_nav {
        return variant.handle_key(key, nav);
    }

    // Common navigation keys (only when variant doesn't capture)
    match key.code {
        KeyCode::Up => {
            nav.move_up();
            variant.on_cursor_move(nav);
            return TreeBrowserAction::None;
        }
        KeyCode::Down => {
            nav.move_down();
            variant.on_cursor_move(nav);
            return TreeBrowserAction::None;
        }
        KeyCode::Right => {
            nav.expand_current();
            return TreeBrowserAction::None;
        }
        KeyCode::Left => {
            nav.collapse_or_parent();
            return TreeBrowserAction::None;
        }
        _ => {}
    }

    // Delegate remaining keys to variant
    variant.handle_key(key, nav)
}
