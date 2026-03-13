//! ConfirmationGesture - compile-time proof of operator confirmation gesture.
//!
//! This zero-sized token is created by the action dispatch layer when the
//! triggering event is a confirmation gesture (Enter keypress or mouse click
//! on a decision button). The gesture is the toll; a `Decision` is the ticket.
//!
//! The token is ephemeral: created on the stack in dispatch_action/handle_click,
//! passed by &reference to handlers, and dropped when the dispatch returns.
//!
//! Visibility: pub(in crate::action_handlers) constructor - the type itself
//! is pub(crate) so it can appear in function signatures across ui/ modules,
//! but only action_handlers/ code can mint new instances.

use mm_meta::decisions::Decision;
use mm_meta::mutations::Mutation;

/// Zero-sized proof that the current dispatch originated from an operator
/// confirmation gesture.
#[derive(Clone, Copy)]
pub(crate) struct ConfirmationGesture(());

impl ConfirmationGesture {
    pub(in crate::action_handlers) fn new() -> Self {
        Self(())
    }

    /// Exchange a gesture for a Decision. The gesture proves operator intent;
    /// the Decision is the serializable artifact that crosses the protocol boundary.
    pub fn decide(&self, label: impl Into<String>, mutations: Vec<Mutation>) -> Decision {
        Decision {
            label: label.into(),
            mutations,
        }
    }
}

impl std::fmt::Debug for ConfirmationGesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfirmationGesture")
    }
}
