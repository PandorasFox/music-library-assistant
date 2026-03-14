//! Mutation builder utilities.
//!
//! Pure `data → Vec<Mutation>` functions shared by both TUI and web clients.

use std::path::PathBuf;

use crate::mutations::file_ops::StashFromZoneMutation;
use crate::mutations::indexing::DropFromIndexMutation;
use crate::mutations::Mutation;
use crate::paths::PathResolver;

/// Generate StashFromZone + DropFromIndex mutations for a single corpus file.
///
/// This is the base primitive used by corrupt file, subpar duplicate, and
/// inbox corpus match modals.
pub fn stash_file_mutations(
    corpus_path: &str,
    inode: i64,
    stash_name: &str,
    resolver: &PathResolver,
) -> Vec<Mutation> {
    let abs_path = resolver.resolve(std::path::Path::new(corpus_path));

    vec![
        Mutation::StashFromZone(StashFromZoneMutation {
            path: abs_path,
            stash_name: stash_name.to_string(),
        }),
        Mutation::DropFromIndex(DropFromIndexMutation {
            path: PathBuf::from(corpus_path),
            inode: Some(inode),
            zone: Some("corpus".to_string()),
        }),
    ]
}
