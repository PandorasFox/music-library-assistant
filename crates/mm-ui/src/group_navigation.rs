//! Group navigation for cluster-nav and group-review resolution modals.
//!
//! Resolution data types that represent one group within a navigable family
//! implement [`GroupNavigation`]. This enables the modal skeleton to show
//! "3 of 12" indicators and handle next/prev group shortcuts, which translate
//! to route changes (load-anchor updates, not render-state changes).

use crate::input::InputAction;

/// Data that knows it's one of N navigable groups.
///
/// Implement on resolution data types for cluster-nav and group-review modals.
/// The modal skeleton uses this to display group position and handle
/// navigation shortcuts.
pub trait GroupNavigation {
    /// Total number of groups in this family.
    fn group_count(&self) -> usize;

    /// Index of the currently loaded group (0-based).
    fn current_group(&self) -> usize;

    /// Whether there's a next group to navigate to.
    fn has_next(&self) -> bool {
        self.current_group() + 1 < self.group_count()
    }

    /// Whether there's a previous group to navigate back to.
    fn has_prev(&self) -> bool {
        self.current_group() > 0
    }
}

/// Result of group-aware input handling.
///
/// Extends the normal modal action flow with group navigation requests.
/// `NavigateNext` / `NavigatePrev` are route-level events — the client
/// translates them to route changes that trigger a new data load.
pub enum GroupInputResult<A> {
    /// Normal modal action (button confirm, cancel, etc.)
    Action(A),
    /// Navigate to the next group (route change).
    NavigateNext,
    /// Navigate to the previous group (route change).
    NavigatePrev,
    /// Input consumed by normal modal handling.
    Consumed,
    /// Input not handled.
    Unhandled,
}

/// Handle group navigation shortcuts.
///
/// Call this before `handle_frame_input()` in modals with group navigation.
/// Returns `NavigateNext` / `NavigatePrev` for CycleNext/CyclePrev (Tab/Shift+Tab),
/// or `None` if the input isn't a group navigation shortcut.
pub fn try_group_navigate<G: GroupNavigation>(
    data: &G,
    action: &InputAction,
) -> Option<GroupInputResult<()>> {
    match action {
        InputAction::CycleNext if data.has_next() => {
            Some(GroupInputResult::NavigateNext)
        }
        InputAction::CyclePrev if data.has_prev() => {
            Some(GroupInputResult::NavigatePrev)
        }
        // Consume cycle actions even when at bounds (don't let them bubble)
        InputAction::CycleNext | InputAction::CyclePrev => {
            Some(GroupInputResult::Consumed)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestGroups {
        current: usize,
        total: usize,
    }

    impl GroupNavigation for TestGroups {
        fn group_count(&self) -> usize { self.total }
        fn current_group(&self) -> usize { self.current }
    }

    #[test]
    fn has_next_and_prev() {
        let g = TestGroups { current: 0, total: 5 };
        assert!(g.has_next());
        assert!(!g.has_prev());

        let g = TestGroups { current: 2, total: 5 };
        assert!(g.has_next());
        assert!(g.has_prev());

        let g = TestGroups { current: 4, total: 5 };
        assert!(!g.has_next());
        assert!(g.has_prev());
    }

    #[test]
    fn single_group_no_navigation() {
        let g = TestGroups { current: 0, total: 1 };
        assert!(!g.has_next());
        assert!(!g.has_prev());
    }

    #[test]
    fn try_navigate_next() {
        let g = TestGroups { current: 0, total: 3 };
        let result = try_group_navigate(&g, &InputAction::CycleNext);
        assert!(matches!(result, Some(GroupInputResult::NavigateNext)));
    }

    #[test]
    fn try_navigate_prev() {
        let g = TestGroups { current: 2, total: 3 };
        let result = try_group_navigate(&g, &InputAction::CyclePrev);
        assert!(matches!(result, Some(GroupInputResult::NavigatePrev)));
    }

    #[test]
    fn cycle_at_bounds_consumed() {
        let g = TestGroups { current: 2, total: 3 };
        let result = try_group_navigate(&g, &InputAction::CycleNext);
        assert!(matches!(result, Some(GroupInputResult::Consumed)));

        let g = TestGroups { current: 0, total: 3 };
        let result = try_group_navigate(&g, &InputAction::CyclePrev);
        assert!(matches!(result, Some(GroupInputResult::Consumed)));
    }

    #[test]
    fn non_cycle_returns_none() {
        let g = TestGroups { current: 1, total: 3 };
        assert!(try_group_navigate(&g, &InputAction::NavUp).is_none());
        assert!(try_group_navigate(&g, &InputAction::Confirm).is_none());
    }
}
