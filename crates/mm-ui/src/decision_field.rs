//! Decision Field: labeled text input for operator decisions in resolution modals.
//!
//! Used when the operator must specify a value as part of a resolution
//! (e.g., canonical tag value in tag canonicity, album name for missing album).
//! Composes with the modal skeleton via [`FocusPane::Field`].

use crate::input::InputAction;
use crate::text_input::TextInputState;

/// A labeled text input for specifying decision values in resolution modals.
///
/// Sits above the list pane in the modal layout. Focus reaches it via
/// `FocusPane::Field` (Shift+Up from List). Confirm from the field
/// fires the currently selected button, same as confirming from the
/// button pane.
pub struct DecisionField {
    /// The underlying text input state.
    pub input: TextInputState,
    /// Display label (e.g., "Squash to:", "Album artist:").
    pub label: String,
}

impl DecisionField {
    /// Create a new empty decision field with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            input: TextInputState::new(),
            label: label.into(),
        }
    }

    /// Create a field pre-filled with a value. Cursor moves to end.
    pub fn with_value(mut self, value: &str) -> Self {
        self.input.set_value(value);
        self
    }

    /// Current field value.
    pub fn value(&self) -> &str {
        self.input.value()
    }

    /// Whether the field is empty.
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Set value and move cursor to end.
    pub fn set_value(&mut self, value: &str) {
        self.input.set_value(value);
    }

    /// Clear the field.
    pub fn clear(&mut self) {
        self.input.clear();
    }

    /// Handle input when this field is focused. Returns true if consumed.
    pub fn handle_input(&mut self, action: &InputAction) -> bool {
        self.input.handle_input(action)
    }

    /// Split value into (before_cursor, cursor_char, after_cursor) for rendering.
    pub fn cursor_splits(&self) -> (&str, char, &str) {
        self.input.cursor_splits()
    }
}

impl std::fmt::Debug for DecisionField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionField")
            .field("label", &self.label)
            .field("value", &self.input.value())
            .finish()
    }
}

// ============================================================================
// WithDecisionField — generic render wrapper
// ============================================================================

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{FrameState, ModalFrameCore};

/// Composes a `ModalFrameCore` state with a `DecisionField` for rendering.
///
/// Created transiently during render/input calls when both the modal state
/// and the decision field need to act as one unit. Delegates all
/// `ModalFrameCore` methods to the inner state, and provides the field
/// via `decision_field()` / `decision_field_mut()`.
///
/// Both TUI (ratatui) and web (HTML) clients implement their render traits
/// on this wrapper to get DecisionField rendering for free.
pub struct WithDecisionField<'a, S: ModalFrameCore> {
    pub state: &'a mut S,
    pub field: &'a mut DecisionField,
}

impl<'a, S: ModalFrameCore> WithDecisionField<'a, S> {
    pub fn new(state: &'a mut S, field: &'a mut DecisionField) -> Self {
        Self { state, field }
    }
}

impl<S: ModalFrameCore> ModalFrameCore for WithDecisionField<'_, S> {
    type Button = S::Button;

    fn content_layout(&self) -> crate::modal_frame::ContentLayout {
        self.state.content_layout()
    }

    fn list_title(&self) -> String {
        self.state.list_title()
    }

    fn empty_message(&self) -> &'static str {
        self.state.empty_message()
    }

    fn controls_height(&self) -> u16 {
        self.state.controls_height()
    }

    fn frame_state(&self) -> &FrameState<S::Button> {
        self.state.frame_state()
    }

    fn frame_state_mut(&mut self) -> &mut FrameState<S::Button> {
        self.state.frame_state_mut()
    }

    fn cursor(&self) -> usize {
        self.state.cursor()
    }

    fn cursor_mut(&mut self) -> &mut usize {
        self.state.cursor_mut()
    }

    fn list_len(&self) -> usize {
        self.state.list_len()
    }

    fn button_ctx(&self) -> <S::Button as ModalButtons>::Context {
        self.state.button_ctx()
    }

    fn escape_action(&self) -> <S::Button as ModalButtons>::Action {
        self.state.escape_action()
    }

    fn decision_field(&self) -> Option<&DecisionField> {
        Some(self.field)
    }

    fn decision_field_mut(&mut self) -> Option<&mut DecisionField> {
        Some(self.field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_field_is_empty() {
        let field = DecisionField::new("Squash to:");
        assert!(field.is_empty());
        assert_eq!(field.value(), "");
        assert_eq!(field.label, "Squash to:");
    }

    #[test]
    fn with_value_prefills() {
        let field = DecisionField::new("Squash to:").with_value("DragonForce");
        assert_eq!(field.value(), "DragonForce");
        assert!(!field.is_empty());
    }

    #[test]
    fn set_value_replaces() {
        let mut field = DecisionField::new("Label").with_value("old");
        field.set_value("new");
        assert_eq!(field.value(), "new");
    }

    #[test]
    fn handles_char_input() {
        let mut field = DecisionField::new("Label");
        assert!(field.handle_input(&InputAction::Char('a')));
        assert!(field.handle_input(&InputAction::Char('b')));
        assert_eq!(field.value(), "ab");
    }

    #[test]
    fn does_not_handle_nav_up() {
        let mut field = DecisionField::new("Label");
        assert!(!field.handle_input(&InputAction::NavUp));
    }

    #[test]
    fn cursor_splits_work() {
        let field = DecisionField::new("Label").with_value("hello");
        let (before, ch, after) = field.cursor_splits();
        // Cursor at end after set_value
        assert_eq!(before, "hello");
        assert_eq!(ch, ' '); // past end
        assert_eq!(after, "");
    }
}
