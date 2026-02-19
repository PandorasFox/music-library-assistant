//! Shared computation types.
//!
//! This module contains types shared across all computation phases:
//! - `ComputationWitness` - Proof of computation execution context
//!
//! Phase-specific computation enums and result types are in:
//! - `observation/mod.rs` - Observation phase (corpus observation)
//! - `derivation/mod.rs` - Derivation phase (first-level derivations)
//! - `analysis/mod.rs` - Analysis phase (full-corpus analysis)

// ============================================================================
// ComputationWitness - Proof of Computation Execution Context
// ============================================================================

/// Sealed module to prevent external construction of ComputationWitness.
mod sealed {
    /// Zero-sized proof that code is executing within a computation context.
    ///
    /// This witness is required by signal-altering database operations to ensure
    /// health signals are only modified through the computation system.
    ///
    /// Cannot be constructed outside of `execute_single` or the DB write thread.
    #[derive(Debug, Clone, Copy)]
    pub struct ComputationWitness(());

    impl ComputationWitness {
        /// Create a new witness. Only callable from within the computations module.
        pub(in crate::meta::computations) fn new() -> Self {
            Self(())
        }

        /// Create a witness for the DB write thread.
        ///
        /// The DB thread executes signal operations that were enqueued from legitimate
        /// computation contexts (which required a witness at send time). This constructor
        /// allows the DB thread to obtain a witness for the actual DB call.
        pub(crate) fn new_for_db_thread() -> Self {
            Self(())
        }
    }
}

pub use sealed::ComputationWitness;
