//! DecisionWitness - compile-time proof of operator confirmation gesture.
//!
//! This zero-sized token is created by the action dispatch layer when the
//! triggering event is a confirmation gesture (Enter, Space, y/Y keypress
//! or mouse click on a decision button). Staging helpers require a reference
//! to this token, making it a compile error to stage without one.
//!
//! The token is ephemeral: created on the stack in dispatch_action/handle_click,
//! passed by &reference to handlers, and dropped when the dispatch returns.
//!
//! Visibility: pub(in crate::ui::action_handlers) - invisible to ui/mod.rs,
//! tick.rs, and all other code outside action_handlers/.

/// Zero-sized proof that the current dispatch originated from an operator
/// confirmation gesture.
#[derive(Clone, Copy)]
pub(in crate::ui::action_handlers) struct DecisionWitness(());

impl DecisionWitness {
    pub(super) fn new() -> Self { Self(()) }
}

impl std::fmt::Debug for DecisionWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DecisionWitness")
    }
}
