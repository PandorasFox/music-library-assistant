//! Transaction review interaction state.
//!
//! Backend-agnostic input handling for reviewing staged decisions before commit.
//! Two consumers:
//! - **SuspendingTransactionReview** (push/pop overlay, show_cancel=true)
//! - **TabbedTransactionReview** (lateral tab, show_cancel=false)
//!
//! Rendering lives in mm-tui (`transaction_review` module).

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;

use crate::input::InputAction;
use crate::modal_buttons::{ButtonRowState, ModalButtons};
use crate::route::{FocusTarget, TransactionReviewRoute};
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::WizardItem;

// ============================================================================
// Action
// ============================================================================

/// Action returned from handling input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionReviewAction {
    None,
    /// Return to source modal (transaction remains active)
    Cancel,
    /// Discard transaction and return to Insights
    Discard,
    /// Commit transaction and proceed to Progress
    Confirm,
    /// User pressed Backspace/Delete on a decision — handler should set pending_removal
    RequestRemoval,
    /// User confirmed removal in the popup — handler should execute removal
    ConfirmRemoval(DecisionKey),
}

// ============================================================================
// ReviewButton
// ============================================================================

/// Context for ReviewButton enablement (show_cancel flag).
#[derive(Debug, Clone, Copy)]
pub struct ReviewButtonCtx {
    pub show_cancel: bool,
}

/// Button choices for the transaction review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewButton {
    /// Cancel - return to source modal (safe default)
    #[default]
    Cancel,
    /// Discard all decisions, return to Insights
    Discard,
    /// Confirm and execute all decisions
    Confirm,
}

impl ModalButtons for ReviewButton {
    type Context = ReviewButtonCtx;
    type Action = TransactionReviewAction;

    fn all() -> &'static [Self] {
        &[Self::Cancel, Self::Discard, Self::Confirm]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Cancel => "Cancel".into(),
            Self::Discard => "Discard".into(),
            Self::Confirm => "Confirm".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::Cancel => Color::White,
            Self::Discard => Color::Red,
            Self::Confirm => Color::Green,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Cancel => ctx.show_cancel,
            Self::Discard => true,
            Self::Confirm => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> TransactionReviewAction {
        match self {
            Self::Cancel => TransactionReviewAction::Cancel,
            Self::Discard => TransactionReviewAction::Discard,
            Self::Confirm => TransactionReviewAction::Confirm,
        }
    }

    // protocol_binding: default (Navigation) — transaction lifecycle
    // confirm/discard are handled by dedicated /tx/confirm and /tx/discard
    // endpoints in the web UI, not via ProtocolBinding dispatch.
}

// ============================================================================
// TransactionInteraction
// ============================================================================

/// Backend-agnostic interaction state for transaction review.
///
/// Owns list navigation, button focus, and removal popup state.
/// Does NOT own the decisions data — that's passed in via `handle_input_with`.
pub struct TransactionInteraction {
    /// StandardList state for decisions navigation + wizard.
    pub list: StandardListState,
    /// Whether button row is focused.
    pub buttons_focused: bool,
    pub buttons: ButtonRowState<ReviewButton>,
    /// When set, a confirmation popup is shown for removing this decision.
    pub pending_removal: Option<DecisionKey>,
    /// Context for button enablement (carries show_cancel flag).
    button_ctx: ReviewButtonCtx,
}

impl TransactionInteraction {
    /// Create state for suspending mode (Cancel button available).
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
            buttons_focused: false,
            buttons: ButtonRowState::new(), // defaults to Cancel
            pending_removal: None,
            button_ctx: ReviewButtonCtx { show_cancel: true },
        }
    }

    /// Create state for tabbed mode (no Cancel button).
    pub fn new_tabbed() -> Self {
        let mut buttons = ButtonRowState::new();
        buttons.selected = ReviewButton::Confirm;
        Self {
            list: StandardListState::new(StandardListConfig::default()),
            buttons_focused: false,
            buttons,
            pending_removal: None,
            button_ctx: ReviewButtonCtx { show_cancel: false },
        }
    }

    /// Get the button context for rendering.
    pub fn button_ctx(&self) -> &ReviewButtonCtx {
        &self.button_ctx
    }

    /// Current cursor position (for action handler interop).
    pub fn cursor(&self) -> usize {
        self.list.cursor
    }

    /// Handle input with typed decision items.
    ///
    /// `items` is the decisions list — used for StandardList navigation
    /// and wizard integration.
    pub fn handle_input_with<T: WizardItem + ListEntry>(
        &mut self,
        action: &InputAction,
        items: &[T],
    ) -> TransactionReviewAction {
        // Confirmation popup mode — intercept all actions
        if let Some(ref key_to_remove) = self.pending_removal {
            return match action {
                InputAction::Confirm | InputAction::Toggle => {
                    let k = key_to_remove.clone();
                    self.pending_removal = None;
                    TransactionReviewAction::ConfirmRemoval(k)
                }
                InputAction::Cancel | InputAction::Backspace | InputAction::Delete => {
                    self.pending_removal = None;
                    TransactionReviewAction::None
                }
                _ => TransactionReviewAction::None,
            };
        }

        // Button focus mode
        if self.buttons_focused {
            return self.handle_buttons_input(action);
        }

        // Global shortcuts (work regardless of pane focus)
        match action {
            InputAction::Char('y') | InputAction::Char('Y') => {
                return TransactionReviewAction::Confirm;
            }
            InputAction::Shortcut('d') => return TransactionReviewAction::Discard,
            InputAction::Cancel => return TransactionReviewAction::Cancel,
            _ => {}
        }

        // Delegate to StandardList
        match self.list.handle_input(action, items) {
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled => TransactionReviewAction::None,
            ListInputResult::Confirm(_) => {
                // Enter on a decision — no action (read-only)
                TransactionReviewAction::None
            }
            ListInputResult::Unhandled => {
                // Handle remaining actions
                match action {
                    InputAction::Backspace | InputAction::Delete => {
                        TransactionReviewAction::RequestRemoval
                    }
                    InputAction::FocusDown => {
                        self.buttons_focused = true;
                        TransactionReviewAction::None
                    }
                    _ => TransactionReviewAction::None,
                }
            }
        }
    }

    fn handle_buttons_input(&mut self, action: &InputAction) -> TransactionReviewAction {
        match action {
            InputAction::NavLeft => {
                self.buttons.nav_left(&self.button_ctx);
                TransactionReviewAction::None
            }
            InputAction::NavRight => {
                self.buttons.nav_right(&self.button_ctx);
                TransactionReviewAction::None
            }
            InputAction::FocusUp => {
                self.buttons_focused = false;
                TransactionReviewAction::None
            }
            InputAction::Confirm | InputAction::Toggle => self
                .buttons
                .confirm(&self.button_ctx)
                .unwrap_or(TransactionReviewAction::None),
            InputAction::Cancel => TransactionReviewAction::Cancel,
            InputAction::Char('y') | InputAction::Char('Y') => {
                TransactionReviewAction::Confirm
            }
            InputAction::Shortcut('d') => TransactionReviewAction::Discard,
            _ => TransactionReviewAction::None,
        }
    }

    /// Clamp list cursor to valid range after decisions change.
    pub fn clamp_to_data<T: ListEntry>(&mut self, items: &[T]) {
        self.list.clamp_cursor(items);
    }
}

impl Default for TransactionInteraction {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// ViewCore
// ============================================================================

impl ViewCore for TransactionInteraction {
    type Route = TransactionReviewRoute;
    type Action = TransactionReviewAction;
    type Data = (); // uses handle_input_with for data-dependent input handling

    fn from_route(route: &TransactionReviewRoute) -> Self {
        let mut interaction = Self::new();
        if let Some(cursor) = route.cursor {
            interaction.list.cursor = cursor;
        }
        if route.focus == Some(FocusTarget::Buttons) {
            interaction.buttons_focused = true;
        }
        interaction
    }

    fn to_route(&self) -> TransactionReviewRoute {
        TransactionReviewRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
            focus: if self.buttons_focused {
                Some(FocusTarget::Buttons)
            } else {
                None
            },
        }
    }

    fn handle_input(
        &mut self,
        _action: &InputAction,
        _data: &(),
    ) -> Option<TransactionReviewAction> {
        // Use handle_input_with for data-dependent input handling.
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::wizard::WizardOffer;

    /// Minimal item for testing list interaction.
    struct TestDecision {
        #[allow(dead_code)] // used to construct realistic test items
        key: DecisionKey,
    }

    impl WizardItem for TestDecision {
        fn wizard(&self, _width: u16) -> Option<WizardOffer> {
            None
        }
    }

    impl ListEntry for TestDecision {
        type Action = DecisionKey;
        fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<DecisionKey> {
            None // Enter doesn't act on decisions directly
        }
    }

    fn make_decisions(n: usize) -> Vec<TestDecision> {
        (0..n)
            .map(|_| TestDecision {
                key: DecisionKey::IntakeIndex,
            })
            .collect()
    }

    // -- ViewCore round-trip --

    #[test]
    fn from_route_default() {
        let interaction = TransactionInteraction::from_route(&TransactionReviewRoute::default());
        assert_eq!(interaction.list.cursor, 0);
        assert!(!interaction.buttons_focused);
    }

    #[test]
    fn from_route_with_cursor_and_focus() {
        let interaction = TransactionInteraction::from_route(&TransactionReviewRoute {
            cursor: Some(3),
            focus: Some(FocusTarget::Buttons),
        });
        assert_eq!(interaction.list.cursor, 3);
        assert!(interaction.buttons_focused);
    }

    #[test]
    fn to_route_zero_cursor_is_none() {
        let interaction = TransactionInteraction::from_route(&TransactionReviewRoute::default());
        let route = interaction.to_route();
        assert_eq!(route.cursor, None);
        assert_eq!(route.focus, None);
    }

    #[test]
    fn to_route_nonzero_cursor() {
        let mut interaction =
            TransactionInteraction::from_route(&TransactionReviewRoute::default());
        interaction.list.cursor = 5;
        let route = interaction.to_route();
        assert_eq!(route.cursor, Some(5));
    }

    #[test]
    fn to_route_buttons_focused() {
        let mut interaction =
            TransactionInteraction::from_route(&TransactionReviewRoute::default());
        interaction.buttons_focused = true;
        let route = interaction.to_route();
        assert_eq!(route.focus, Some(FocusTarget::Buttons));
    }

    #[test]
    fn round_trip_via_route() {
        let mut interaction = TransactionInteraction::from_route(&TransactionReviewRoute {
            cursor: Some(7),
            focus: Some(FocusTarget::Buttons),
        });
        interaction.list.cursor = 7;
        interaction.buttons_focused = true;
        let route = interaction.to_route();
        let restored = TransactionInteraction::from_route(&route);
        assert_eq!(restored.list.cursor, 7);
        assert!(restored.buttons_focused);
    }

    // -- Input handling --

    #[test]
    fn confirm_shortcut() {
        let mut interaction = TransactionInteraction::new();
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Char('y'), &items);
        assert_eq!(action, TransactionReviewAction::Confirm);
    }

    #[test]
    fn discard_shortcut() {
        let mut interaction = TransactionInteraction::new();
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Shortcut('d'), &items);
        assert_eq!(action, TransactionReviewAction::Discard);
    }

    #[test]
    fn cancel_shortcut() {
        let mut interaction = TransactionInteraction::new();
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Cancel, &items);
        assert_eq!(action, TransactionReviewAction::Cancel);
    }

    #[test]
    fn backspace_requests_removal() {
        let mut interaction = TransactionInteraction::new();
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Backspace, &items);
        assert_eq!(action, TransactionReviewAction::RequestRemoval);
    }

    #[test]
    fn removal_popup_confirm() {
        let mut interaction = TransactionInteraction::new();
        interaction.pending_removal = Some(DecisionKey::IntakeIndex);
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Confirm, &items);
        assert_eq!(
            action,
            TransactionReviewAction::ConfirmRemoval(DecisionKey::IntakeIndex)
        );
        assert!(interaction.pending_removal.is_none());
    }

    #[test]
    fn removal_popup_cancel() {
        let mut interaction = TransactionInteraction::new();
        interaction.pending_removal = Some(DecisionKey::IntakeIndex);
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::Cancel, &items);
        assert_eq!(action, TransactionReviewAction::None);
        assert!(interaction.pending_removal.is_none());
    }

    #[test]
    fn focus_down_enters_buttons() {
        let mut interaction = TransactionInteraction::new();
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::FocusDown, &items);
        assert_eq!(action, TransactionReviewAction::None);
        assert!(interaction.buttons_focused);
    }

    #[test]
    fn focus_up_exits_buttons() {
        let mut interaction = TransactionInteraction::new();
        interaction.buttons_focused = true;
        let items = make_decisions(3);
        let action = interaction.handle_input_with(&InputAction::FocusUp, &items);
        assert_eq!(action, TransactionReviewAction::None);
        assert!(!interaction.buttons_focused);
    }

    #[test]
    fn tabbed_mode_no_cancel() {
        let interaction = TransactionInteraction::new_tabbed();
        assert!(!interaction.button_ctx.show_cancel);
        assert_eq!(interaction.buttons.selected, ReviewButton::Confirm);
    }
}
