//! Operator Decision Handlers - SEALED MODULE
//!
//! ╔════════════════════════════════════════════════════════════════════════════╗
//! ║  THIS MODULE IS THE OPERATOR CONFIRMATION BOUNDARY                         ║
//! ║                                                                            ║
//! ║  Functions here create DecisionWitnesses via with_operator_decision() -    ║
//! ║  the ONLY sanctioned call sites.                                           ║
//! ║                                                                            ║
//! ║  DO NOT:                                                                   ║
//! ║  - Export with_operator_decision or DecisionScope from here                ║
//! ║  - Add new functions without explicit human operator approval              ║
//! ║  - Call these functions from anywhere except action handlers               ║
//! ║  - Extend this module with new patterns or abstractions                    ║
//! ║                                                                            ║
//! ║  The witness cannot escape the callback scope - this is by design.         ║
//! ║  All decision authority flows through these narrow, auditable functions.   ║
//! ╚════════════════════════════════════════════════════════════════════════════╝
//!
//! # Why This Module Exists
//!
//! MM enforces that all corpus mutations are attributable to explicit operator
//! decisions. The `DecisionWitness` type is a zero-sized proof that code is
//! executing in a user-confirmed context.
//!
//! This module implements a sealed access pattern:
//! 1. `DecisionWitness` can only be created inside `Witch::with_operator_decision()`
//! 2. That method should only be called from functions in THIS module
//! 3. These functions are called from action handlers
//!
//! # Transaction Pattern
//!
//! All mutation flows now go through a standardized Transaction Review modal:
//! 1. Source modal stages decision(s) via `stage_decision()`
//! 2. TransactionReview modal shows staged decisions
//! 3. User confirms via `commit_transaction()` or discards via `discard_transaction()`
//!
//! The result: a clear, auditable boundary between "user confirmed the operation"
//! and "mutations were queued for execution".

use crate::meta::decisions::{DiscardSummary, TransactionError};
use crate::meta::mutations::Mutation;
use crate::witch::Witch;

// =============================================================================
// SEALED DECISION HANDLERS
// =============================================================================
// These functions are the ONLY sanctioned call sites for with_operator_decision().
// They are called from Enter keypress handlers in confirmation modals.
// =============================================================================

/// Stage a decision to the active transaction.
///
/// Called from Enter keypress when user confirms a single item (tag save, etc.).
/// The decision is added to the transaction but not yet committed.
///
/// # Call Sites
/// - Tag editor: when user saves edits for a track
/// - Tag canonicity: when user confirms a cluster resolution
pub fn stage_decision(
    witch: &mut Witch,
    index: usize,
    label: &str,
    mutations: Vec<Mutation>,
) -> Result<(), TransactionError> {
    witch.with_operator_decision(|scope| {
        scope.add_decision(index, label, mutations)
    })
}

/// Commit the active transaction - queue all staged mutations for execution.
///
/// Called from Enter keypress on transaction commit confirmation.
/// All accumulated decisions are committed and their mutations queued.
///
/// # Call Sites
/// - Tag editor: when user commits all staged edits
/// - Tag canonicity review: when user confirms all resolutions
pub fn commit_transaction(witch: &mut Witch) -> Result<(), TransactionError> {
    witch.with_operator_decision(|scope| {
        scope.confirm_transaction()
    })
}

/// Discard the active transaction - drop all staged decisions.
///
/// Called from Escape/cancel keypress on transaction modal.
/// All accumulated decisions are discarded without execution.
///
/// # Call Sites
/// - Transaction review: when user cancels/discards the transaction
/// - Source modals: when user cancels after returning from review
pub fn discard_transaction(witch: &mut Witch) -> Result<DiscardSummary, TransactionError> {
    witch.with_operator_decision(|scope| {
        scope.discard_transaction()
    })
}
