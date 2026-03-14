//! ModalFrameCore: backend-agnostic trait for modal dialog logic.
//!
//! Each modal state describes its layout via `ContentLayout`, provides state
//! accessors, and gets input routing defaults. Rendering lives in mm-tui's
//! `ModalFrame` supertrait.

use crate::click_targets::ListClickTargets;
use crate::decision_field::DecisionField;
use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::{ButtonRowState, ModalButtons};

pub enum FrameInputResult<A> {
    Action(A),
    Consumed,
    Unhandled,
}

pub enum ContentLayout {
    ListAboveDetail { detail_height: u16 },
    DetailAboveList { detail_height: u16 },
    FourSection { header_height: u16, detail_height: u16 },
    HorizontalSplit { list_percent: u16, info_height: u16 },
    /// DecisionField above the list, detail below. For modals where the
    /// operator specifies a value (canonical tag, album name) via text input.
    FieldAboveList { field_height: u16, detail_height: u16 },
}

/// Common frame state fields shared by all ModalFrame implementors.
#[derive(Debug)]
pub struct FrameState<B: ModalButtons> {
    pub focus_pane: FocusPane,
    pub buttons: ButtonRowState<B>,
    pub click_targets: ListClickTargets,
}

impl<B: ModalButtons> Default for FrameState<B> {
    fn default() -> Self {
        Self {
            focus_pane: FocusPane::List,
            buttons: ButtonRowState::new(),
            click_targets: ListClickTargets::new(),
        }
    }
}

impl<B: ModalButtons> FrameState<B> {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Backend-agnostic modal frame logic. Rendering is in mm-tui's `ModalFrame`.
pub trait ModalFrameCore {
    type Button: ModalButtons;

    fn content_layout(&self) -> ContentLayout;
    fn list_title(&self) -> String;
    fn empty_message(&self) -> &'static str { "No items" }
    fn controls_height(&self) -> u16 { 3 }

    // State accessors
    fn frame_state(&self) -> &FrameState<Self::Button>;
    fn frame_state_mut(&mut self) -> &mut FrameState<Self::Button>;
    fn cursor(&self) -> usize;
    fn cursor_mut(&mut self) -> &mut usize;
    fn list_len(&self) -> usize;
    fn button_ctx(&self) -> <Self::Button as ModalButtons>::Context;
    fn escape_action(&self) -> <Self::Button as ModalButtons>::Action;

    /// Optional decision text field. Override to provide a labeled text input
    /// above the list pane. When present, `FocusPane::Field` becomes part of
    /// the focus cycle.
    fn decision_field(&self) -> Option<&DecisionField> { None }

    /// Mutable access to the decision field for input handling.
    fn decision_field_mut(&mut self) -> Option<&mut DecisionField> { None }

    fn handle_frame_input(
        &mut self, action: &InputAction,
    ) -> FrameInputResult<<Self::Button as ModalButtons>::Action> {
        let has_field = self.decision_field().is_some();

        // Focus pane cycling (Shift+arrow)
        match action {
            InputAction::FocusUp => {
                let fp = &mut self.frame_state_mut().focus_pane;
                *fp = fp.prev(has_field);
                return FrameInputResult::Consumed;
            }
            InputAction::FocusDown => {
                let fp = &mut self.frame_state_mut().focus_pane;
                *fp = fp.next(has_field);
                return FrameInputResult::Consumed;
            }
            _ => {}
        }

        // Decision field input (when focused)
        if self.frame_state().focus_pane == FocusPane::Field {
            match action {
                InputAction::Cancel => {
                    return FrameInputResult::Action(self.escape_action());
                }
                InputAction::Confirm => {
                    // Confirm from field fires the currently selected button,
                    // matching the tag canonicity UX where Enter from the text
                    // field confirms the whole modal.
                    let ctx = self.button_ctx();
                    return match self.frame_state_mut().buttons.confirm(&ctx) {
                        Some(a) => FrameInputResult::Action(a),
                        None => FrameInputResult::Consumed,
                    };
                }
                other => {
                    if let Some(field) = self.decision_field_mut() {
                        if field.handle_input(other) {
                            return FrameInputResult::Consumed;
                        }
                    }
                    return FrameInputResult::Unhandled;
                }
            }
        }

        // List and button pane input
        match action {
            InputAction::NavUp if self.frame_state().focus_pane == FocusPane::List => {
                *self.cursor_mut() = self.cursor().saturating_sub(1);
                FrameInputResult::Consumed
            }
            InputAction::NavDown if self.frame_state().focus_pane == FocusPane::List => {
                let max = self.list_len().saturating_sub(1);
                let c = self.cursor_mut();
                if *c < max { *c += 1; }
                FrameInputResult::Consumed
            }
            InputAction::PageUp => {
                *self.cursor_mut() = self.cursor().saturating_sub(10);
                FrameInputResult::Consumed
            }
            InputAction::PageDown => {
                let max = self.list_len().saturating_sub(1);
                *self.cursor_mut() = (self.cursor() + 10).min(max);
                FrameInputResult::Consumed
            }
            InputAction::NavLeft if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                self.frame_state_mut().buttons.nav_left(&ctx);
                FrameInputResult::Consumed
            }
            InputAction::NavRight if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                self.frame_state_mut().buttons.nav_right(&ctx);
                FrameInputResult::Consumed
            }
            InputAction::Confirm if self.frame_state().focus_pane == FocusPane::Buttons => {
                let ctx = self.button_ctx();
                match self.frame_state_mut().buttons.confirm(&ctx) {
                    Some(a) => FrameInputResult::Action(a),
                    None => FrameInputResult::Consumed,
                }
            }
            InputAction::Cancel => FrameInputResult::Action(self.escape_action()),
            _ => FrameInputResult::Unhandled,
        }
    }
}
