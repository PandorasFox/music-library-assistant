//! Corpus Mutations Module
//!
//! See [`docs/MUTATION_REFERENCE.md`](../../../docs/MUTATION_REFERENCE.md) for the
//! canonical reference of mutation types, spawned computations, and signal effects.
//!
//! **Any changes to mutation behavior must be reflected in that document.**
//!
//! ## Overview
//!
//! Provides a standardized interface for all corpus-mutating operations.
//! All mutations are resolved to single file/track level.
//!
//! ## Architecture
//!
//! Mutations are executed through the Witch:
//!
//! ```text
//! UI Decision (with ConfirmationGesture)
//!     │
//!     ▼
//! Witch.queue_mutations()
//!     │
//!     ▼
//! Parallel Worker:
//!     mutation.as_executor().execute(&MutationContext { ... })
//!     └── Each struct's MutationExecutor::execute() impl
//! ```
//!
pub mod config_edit;
pub mod dir_config_edit;
pub mod file_ops;
pub mod indexing;
pub mod tag_edit;
pub mod traits;
pub mod transcode;
mod types;

pub use types::*;

/// Access control for corpus-mutating operations.
///
/// # Design Pattern: Witness Token
///
/// `MutationToken` is a zero-sized proof that code is executing within a mutation
/// executor. Functions that mutate the corpus (write tags to files, move files, etc.)
/// require this token as a parameter.
///
/// The token can ONLY be constructed within the `mutations` module (via `pub(in ...)`).
/// External code (ui/, flows/) cannot create a token, so they cannot call protected
/// functions directly - they MUST go through the mutation system.
///
/// This transforms the runtime invariant "only call corpus-mutating functions from
/// mutation executors" into a compile-time guarantee that is impossible to violate.
///
/// ## Benefits
///
/// 1. **Impossible to bypass** - External code cannot compile if it tries to call protected functions
/// 2. **Self-documenting** - The token parameter clearly indicates authorization is required
/// 3. **Zero runtime cost** - ZST compiles away completely
/// 4. **Auditable** - Easy to grep for which functions require the token
pub mod sealed {
    /// A zero-sized token proving code is executing within a mutation executor.
    ///
    /// Cannot be constructed outside the `mutations` module.
    #[derive(Clone, Copy)]
    pub struct MutationToken(());

    impl MutationToken {
        /// Create a new mutation token.
        ///
        /// This is only callable from within the `meta::mutations` module hierarchy.
        pub(in crate::meta::mutations) fn new() -> Self {
            Self(())
        }
    }
}

pub use sealed::MutationToken;

#[cfg(test)]
mod tests {
    use super::{types::MutationDispatch, *};

    #[test]
    fn test_create_bulk_tag_mutations() {
        // Test the incremental tag mutation creation structure with spawn chaining.
        // UI generates ApplyTagOps mutations - disk sync is spawned during execution.
        let ops = vec![
            TagOp::add_tag(1, "album_artist", "Various Artists"),
            TagOp::add_tag(2, "album_artist", "Various Artists"),
        ];

        // Create a single ApplyTagOps mutation containing all ops
        let mutation = Mutation::ApplyTagOps(tag_edit::ApplyTagOpsMutation {
            ops,
            zone: crate::db::types::Zone::Corpus,
        });

        // Verify ApplyTagOps mutation
        assert_eq!(mutation.label(), "Tag edit");
        assert!(mutation.is_db_only());
    }
}
