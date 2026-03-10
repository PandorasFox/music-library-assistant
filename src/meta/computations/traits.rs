//! Execution traits for computations.
//!
//! Each computation struct implements a phase-specific executor trait,
//! carrying its own execution logic and label. This replaces the
//! centralized match dispatch in `mod.rs::execute_single()`.
//!
//! Per-phase traits preserve the compile-time phase boundary enforcement:
//! each trait returns its phase's Result type, which can only spawn
//! computations from the same phase.

use super::types::ComputationWitness;
use crate::db::ReadOnlyDb;

/// Context provided to computation executors at execution time.
pub struct ComputationContext<'a> {
    pub read_db: &'a ReadOnlyDb<'a>,
    pub witness: &'a ComputationWitness,
}
