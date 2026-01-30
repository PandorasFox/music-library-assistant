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
//! UI Decision (with DecisionWitness)
//!     │
//!     ▼
//! Witch.queue_mutations()
//!     │
//!     ▼
//! Parallel Worker executes:
//!     ├── tag_edit::execute_single()
//!     ├── indexing::execute_single()
//!     ├── file_ops::execute_single()
//!     └── transcode::execute_single()
//! ```
//!
mod types;
mod migration;
pub mod tag_edit;
pub mod indexing;
pub mod file_ops;
pub mod transcode;

pub use types::*;
pub use migration::MigrationRegistry;

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
        /// This is only callable from within the `corpus::mutations` module hierarchy.
        pub(in crate::corpus::mutations) fn new() -> Self {
            Self(())
        }
    }
}

pub use sealed::MutationToken;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_create_bulk_tag_mutations() {
        // Test the DB-first tag mutation creation structure with spawn chaining.
        // UI only generates SetTrackTagsDb mutations - disk sync is spawned during execution.
        let files = vec![
            (1i64, PathBuf::from("/test/a.flac")),
            (2i64, PathBuf::from("/test/b.flac")),
        ];

        // Create mutations using the single-mutation pattern (spawns disk sync)
        let mutations: Vec<Mutation> = files
            .iter()
            .map(|(track_id, _path)| {
                Mutation::SetTrackTagsDb {
                    track_id: *track_id,
                    tags: vec![("album_artist".to_string(), "Various Artists".to_string())],
                }
            })
            .collect();

        // 2 files × 1 mutation each = 2 mutations (disk sync is spawned)
        assert_eq!(mutations.len(), 2);

        // Verify SetTrackTagsDb mutations
        assert!(matches!(&mutations[0], Mutation::SetTrackTagsDb { track_id: 1, .. }));
        assert_eq!(mutations[0].category(), MutationCategory::Indexing); // DB-only
        assert!(mutations[0].is_db_only());

        assert!(matches!(&mutations[1], Mutation::SetTrackTagsDb { track_id: 2, .. }));
    }
}
