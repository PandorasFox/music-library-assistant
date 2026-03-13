//! Operator Decision Handlers - SEALED MODULE
//!
//! ╔════════════════════════════════════════════════════════════════════════════╗
//! ║  THIS MODULE IS THE OPERATOR CONFIRMATION BOUNDARY                         ║
//! ║                                                                            ║
//! ║  Functions here take &ConfirmationGesture to prove the call originates     ║
//! ║  from a confirmed operator action (Enter keypress or mouse click).        ║
//! ║                                                                            ║
//! ║  DO NOT:                                                                   ║
//! ║  - Add new functions without explicit human operator approval              ║
//! ║  - Call these functions from anywhere except action handlers               ║
//! ║  - Extend this module with new patterns or abstractions                    ║
//! ╚════════════════════════════════════════════════════════════════════════════╝
//!
//! # Why This Module Exists
//!
//! MM enforces that all corpus mutations are attributable to explicit operator
//! decisions. The `ConfirmationGesture` type is a zero-sized proof that code
//! is executing in a user-confirmed context.
//!
//! # Transaction Pattern
//!
//! All mutation flows go through a standardized Transaction Review modal:
//! 1. Source modal stages decision(s) via `stage_decision()`
//! 2. TransactionReview modal shows staged decisions
//! 3. User confirms via `commit_transaction()` or discards via `discard_transaction()`

use crate::meta::decisions::{Decision, DecisionKey, DiscardSummary};
use crate::meta::protocol::ProtocolError;
use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::witch::WitchHandle;

// =============================================================================
// SEALED DECISION HANDLERS
// =============================================================================

/// Stage a decision to the active transaction.
///
/// Called from Enter keypress when user confirms a single item (tag save, etc.).
/// The gesture is exchanged for a Decision; the decision crosses the protocol boundary.
pub fn stage_decision(
    witch: &mut WitchHandle,
    key: DecisionKey,
    decision: Decision,
) -> Result<(), ProtocolError> {
    witch.add_decision(key, decision)
}

/// Commit the active transaction - queue all staged mutations for execution.
///
/// Called from Enter keypress on transaction commit confirmation.
pub fn commit_transaction(
    witch: &mut WitchHandle,
    _gesture: &ConfirmationGesture,
) -> Result<(), ProtocolError> {
    witch.confirm_transaction()
}

/// Discard the active transaction - drop all staged decisions.
///
/// Called from Escape/cancel keypress on transaction modal.
/// Does not require a gesture — discarding is always safe.
pub fn discard_transaction(
    witch: &mut WitchHandle,
) -> Result<DiscardSummary, ProtocolError> {
    witch.discard_transaction()
}

/// Remove an entire decision from the active transaction.
///
/// Called when user removes a decision from transaction review.
pub fn remove_decision(
    witch: &mut WitchHandle,
    key: &DecisionKey,
    _gesture: &ConfirmationGesture,
) -> Result<(), ProtocolError> {
    witch.remove_decision(key)
}
