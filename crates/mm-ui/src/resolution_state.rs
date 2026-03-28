//! Generic resolution modal state.
//!
//! [`ResolutionState<D, B>`] provides the shared skeleton for all resolution
//! modals: cached data, list navigation (with wizard support), focus pane,
//! button row. It implements [`ModalFrameCore`] via blanket impl, so concrete
//! modals only need to provide a data type (implementing [`ResolutionData`]),
//! a button enum (implementing [`ModalButtons`]), and backend-specific rendering.

use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
use crate::standard_list::{StandardListConfig, StandardListState};

// ============================================================================
// ResolutionData trait
// ============================================================================

/// Trait for data payloads used in resolution modals.
///
/// Provides the modal's structural metadata (layout, titles) and the
/// data-dependent accessors (list length, button context, selected path).
/// Implement this on your data type, then use `ResolutionState<YourData, YourButton>`.
pub trait ResolutionData {
    /// Context type for button enablement/labels.
    type ButtonCtx;

    /// Number of items in the primary list.
    fn list_len(&self) -> usize;

    /// Build button context from current data state.
    fn button_ctx(&self) -> Self::ButtonCtx;

    /// Path of the item at the given cursor index (for status bar).
    fn selected_path(&self, cursor: usize) -> Option<&str>;

    /// Layout for the modal content.
    fn content_layout(&self) -> ContentLayout;

    /// Title for the list section.
    fn list_title(&self) -> String;

    /// Message when the list is empty.
    fn empty_message(&self) -> &'static str {
        "No items"
    }
}

// ============================================================================
// ResolutionState
// ============================================================================

/// Generic resolution modal state.
///
/// `D` is the data payload (loaded once when the modal opens).
/// `B` is the button enum (resolution choices).
///
/// Uses `StandardListState` for list navigation, giving every resolution
/// modal wizard (Z-key) support, scroll management, and multi-select for free.
///
/// Implements [`ModalFrameCore`] automatically, so both TUI and web clients
/// get input handling for free. Backend-specific rendering is provided by
/// implementing mm-tui's `ModalFrame` trait on the concrete instantiation.
pub struct ResolutionState<D: ResolutionData, B: ModalButtons<Context = D::ButtonCtx>> {
    /// Data loaded once when the modal opens.
    pub data: D,
    /// List navigation state (cursor, scroll, wizard, multi-select).
    pub list: StandardListState,
    /// Shared frame state (focus pane, button row, click targets).
    pub frame: FrameState<B>,
}

impl<D, B> ResolutionState<D, B>
where
    D: ResolutionData,
    B: ModalButtons<Context = D::ButtonCtx>,
{
    /// Create a new resolution state with cached data. Cursor starts at 0.
    pub fn new(data: D) -> Self {
        Self {
            data,
            list: StandardListState::new(StandardListConfig::default()),
            frame: FrameState::new(),
        }
    }

    /// Create a new resolution state with custom list configuration.
    pub fn with_list_config(data: D, config: StandardListConfig) -> Self {
        Self {
            data,
            list: StandardListState::new(config),
            frame: FrameState::new(),
        }
    }

    /// Path of the currently selected item (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.data.selected_path(self.list.cursor)
    }

    /// Handle keyboard input. Returns `Some(action)` if a button was confirmed
    /// or escape was pressed, `None` for navigation/consumed/unhandled inputs.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<B::Action> {
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => Some(a),
            FrameInputResult::Consumed | FrameInputResult::Unhandled => None,
        }
    }

    /// Handle a mouse click at (x, y). Returns `Some(action)` if a button was
    /// clicked, otherwise updates cursor/focus and returns `None`.
    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<B::Action> {
        let ctx = self.data.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.data.list_len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.list.cursor = idx;
                }
            }
        }
        None
    }

    /// Reset list state (cursor, scroll) — used when advancing groups.
    pub fn reset_list(&mut self) {
        self.list.reset();
    }
}

// ============================================================================
// Blanket ModalFrameCore implementation
// ============================================================================

impl<D, B> ModalFrameCore for ResolutionState<D, B>
where
    D: ResolutionData,
    B: ModalButtons<Context = D::ButtonCtx>,
{
    type Button = B;

    fn content_layout(&self) -> ContentLayout {
        self.data.content_layout()
    }

    fn list_title(&self) -> String {
        self.data.list_title()
    }

    fn empty_message(&self) -> &'static str {
        self.data.empty_message()
    }

    fn frame_state(&self) -> &FrameState<B> {
        &self.frame
    }

    fn frame_state_mut(&mut self) -> &mut FrameState<B> {
        &mut self.frame
    }

    fn cursor(&self) -> usize {
        self.list.cursor
    }

    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.list.cursor
    }

    fn list_len(&self) -> usize {
        self.data.list_len()
    }

    fn button_ctx(&self) -> D::ButtonCtx {
        self.data.button_ctx()
    }

    fn escape_action(&self) -> B::Action {
        let ctx = self.data.button_ctx();
        B::default().action(&ctx)
    }
}

// ============================================================================
// Debug impl
// ============================================================================

impl<D, B> std::fmt::Debug for ResolutionState<D, B>
where
    D: ResolutionData + std::fmt::Debug,
    B: ModalButtons<Context = D::ButtonCtx> + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolutionState")
            .field("cursor", &self.list.cursor)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal_buttons::ModalButtons;
    use ratatui::style::Color;
    use std::borrow::Cow;

    // -- Test data type --

    struct TestData {
        items: Vec<String>,
    }

    impl ResolutionData for TestData {
        type ButtonCtx = bool; // has_items

        fn list_len(&self) -> usize {
            self.items.len()
        }

        fn button_ctx(&self) -> bool {
            !self.items.is_empty()
        }

        fn selected_path(&self, cursor: usize) -> Option<&str> {
            self.items.get(cursor).map(|s| s.as_str())
        }

        fn content_layout(&self) -> ContentLayout {
            ContentLayout::FourSection {
                header_height: 3,
                detail_height: 3,
            }
        }

        fn list_title(&self) -> String {
            format!("Items ({})", self.items.len())
        }

        fn empty_message(&self) -> &'static str {
            "No items found"
        }
    }

    // -- Test button enum --

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum TestButton {
        Confirm,
        #[default]
        Cancel,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Confirmed,
        Cancelled,
    }

    impl ModalButtons for TestButton {
        type Context = bool;
        type Action = TestAction;

        fn all() -> &'static [Self] {
            &[Self::Confirm, Self::Cancel]
        }

        fn label(&self, _ctx: &bool) -> Cow<'static, str> {
            match self {
                Self::Confirm => "Confirm".into(),
                Self::Cancel => "Cancel".into(),
            }
        }

        fn color(&self, _ctx: &bool) -> Color {
            Color::White
        }

        fn enabled(&self, ctx: &bool) -> bool {
            match self {
                Self::Confirm => *ctx,
                Self::Cancel => true,
            }
        }

        fn action(&self, _ctx: &bool) -> TestAction {
            match self {
                Self::Confirm => TestAction::Confirmed,
                Self::Cancel => TestAction::Cancelled,
            }
        }
    }

    type TestState = ResolutionState<TestData, TestButton>;

    #[test]
    fn new_starts_at_cursor_zero() {
        let state = TestState::new(TestData {
            items: vec!["a".into(), "b".into()],
        });
        assert_eq!(state.list.cursor, 0);
    }

    #[test]
    fn selected_path_tracks_cursor() {
        let mut state = TestState::new(TestData {
            items: vec!["first".into(), "second".into()],
        });
        assert_eq!(state.selected_path(), Some("first"));
        state.list.cursor = 1;
        assert_eq!(state.selected_path(), Some("second"));
    }

    #[test]
    fn escape_returns_cancel_action() {
        let mut state = TestState::new(TestData {
            items: vec!["x".into()],
        });
        let result = state.handle_input(&InputAction::Cancel);
        assert_eq!(result, Some(TestAction::Cancelled));
    }

    #[test]
    fn nav_down_moves_cursor() {
        let mut state = TestState::new(TestData {
            items: vec!["a".into(), "b".into(), "c".into()],
        });
        assert_eq!(state.list.cursor, 0);
        let result = state.handle_input(&InputAction::NavDown);
        assert!(result.is_none());
        assert_eq!(state.list.cursor, 1);
    }

    #[test]
    fn nav_up_at_zero_stays() {
        let mut state = TestState::new(TestData {
            items: vec!["a".into(), "b".into()],
        });
        let result = state.handle_input(&InputAction::NavUp);
        assert!(result.is_none());
        assert_eq!(state.list.cursor, 0);
    }

    #[test]
    fn list_len_delegates_to_data() {
        let state = TestState::new(TestData {
            items: vec!["a".into(), "b".into(), "c".into()],
        });
        assert_eq!(ModalFrameCore::list_len(&state), 3);
    }

    #[test]
    fn empty_data_button_ctx() {
        let state = TestState::new(TestData { items: vec![] });
        assert_eq!(ModalFrameCore::button_ctx(&state), false);
    }
}
