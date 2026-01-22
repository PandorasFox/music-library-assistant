//! Operator Decision Handlers - SEALED MODULE
//!
//! ╔════════════════════════════════════════════════════════════════════════════╗
//! ║  THIS MODULE IS THE OPERATOR CONFIRMATION BOUNDARY                         ║
//! ║                                                                            ║
//! ║  Functions here are called ONLY from Enter keypress handlers in            ║
//! ║  confirmation modals. They create DecisionWitnesses via                    ║
//! ║  with_operator_decision() - the ONLY sanctioned call site.                 ║
//! ║                                                                            ║
//! ║  DO NOT:                                                                   ║
//! ║  - Export with_operator_decision or DecisionScope from here                ║
//! ║  - Add new functions without explicit human operator approval              ║
//! ║  - Call these functions from anywhere except Enter keypress handlers       ║
//! ║  - Extend this module with new patterns or abstractions                    ║
//! ║                                                                            ║
//! ║  The witness cannot escape the callback scope - this is by design.         ║
//! ║  All decision authority flows through these narrow, auditable functions.   ║
//! ╚════════════════════════════════════════════════════════════════════════════╝
//!
//! # Why This Module Exists
//!
//! MLA enforces that all corpus mutations are attributable to explicit operator
//! decisions. The `DecisionWitness` type is a zero-sized proof that code is
//! executing in a user-confirmed context.
//!
//! Previously, `confirm_decision()` was a public function that could be called
//! from anywhere, making it easy to accidentally (or intentionally) bypass the
//! operator confirmation requirement.
//!
//! This module implements a sealed access pattern:
//! 1. `DecisionWitness` can only be created inside `Witch::with_operator_decision()`
//! 2. That method should only be called from functions in THIS module
//! 3. These functions are only called from Enter keypress handlers
//!
//! The result: a clear, auditable boundary between "user pressed Enter to confirm"
//! and "mutations were queued for execution".

use crate::corpus::mutations::Mutation;
use crate::witch::{CommitSummary, DiscardSummary, TransactionError, Witch};

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
pub fn commit_transaction(witch: &mut Witch) -> Result<CommitSummary, TransactionError> {
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
/// - Tag editor: when user cancels/escapes without committing
/// - Tag canonicity: when user cancels the resolution flow
pub fn discard_transaction(witch: &mut Witch) -> Result<DiscardSummary, TransactionError> {
    witch.with_operator_decision(|scope| {
        scope.discard_transaction()
    })
}

/// Execute a complete single-decision transaction (start, add, commit).
///
/// Called from Enter keypress for simple flows where there's exactly one
/// decision with no review step. This is a convenience for flows that don't
/// need multi-decision staging.
///
/// # Call Sites
/// - Intake confirmation: when user confirms indexing unindexed files
/// - Deploy confirmation: when user confirms deployment operations
/// - Missing file resolution: when user confirms file operations
pub fn execute_single_decision(
    witch: &mut Witch,
    transaction_label: &str,
    decision_label: &str,
    mutations: Vec<Mutation>,
) -> Result<CommitSummary, TransactionError> {
    witch.with_operator_decision(|scope| {
        scope.start_transaction(transaction_label)?;
        scope.add_decision(0, decision_label, mutations)?;
        scope.confirm_transaction()
    })
}
