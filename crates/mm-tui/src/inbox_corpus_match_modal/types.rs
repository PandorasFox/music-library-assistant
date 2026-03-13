//! Inbox Corpus Match Resolution Modal Types
//!
//! Data structures for the inbox corpus match resolution modal, including
//! file entries, quality classification, and button state.

use std::path::PathBuf;

use mm_meta::mutations::file_ops::StashFromZoneMutation;
use mm_meta::mutations::indexing::DropFromIndexMutation;
use mm_meta::mutations::Mutation;
use mm_meta::views::MatchClassification;

pub use mm_meta::views::review_match::InboxCorpusMatchModalData;

/// Extension methods for InboxCorpusMatchModalData that depend on server-only types.
pub trait InboxCorpusMatchModalDataExt {
    /// Generate StashFromZone + DropFromIndex mutations for stashable entries.
    fn stash_and_drop_mutations(
        &self,
        resolver: &mm_meta::paths::PathResolver,
    ) -> Vec<Mutation>;

    /// Generate StashFromZone + DropFromIndex mutations for ALL entries.
    fn stash_all_mutations(
        &self,
        resolver: &mm_meta::paths::PathResolver,
    ) -> Vec<Mutation>;
}

impl InboxCorpusMatchModalDataExt for InboxCorpusMatchModalData {
    /// Generate StashFromZone + DropFromIndex mutations for stashable entries.
    ///
    /// Only Equivalent and Subpar entries are stashed. Better entries
    /// (inbox is higher quality) are left alone.
    fn stash_and_drop_mutations(
        &self,
        resolver: &mm_meta::paths::PathResolver,
    ) -> Vec<Mutation> {
        stash_mutations_for(&self.entries, resolver, |c| {
            matches!(
                c,
                MatchClassification::Equivalent | MatchClassification::Subpar
            )
        })
    }

    /// Generate StashFromZone + DropFromIndex mutations for ALL entries,
    /// including those classified as Better.
    fn stash_all_mutations(
        &self,
        resolver: &mm_meta::paths::PathResolver,
    ) -> Vec<Mutation> {
        stash_mutations_for(&self.entries, resolver, |_| true)
    }
}

fn stash_mutations_for(
    entries: &[mm_meta::views::InboxCorpusMatchEntry],
    resolver: &mm_meta::paths::PathResolver,
    predicate: impl Fn(MatchClassification) -> bool,
) -> Vec<Mutation> {
    let mut mutations = Vec::new();

    for entry in entries {
        if !predicate(entry.classification) {
            continue;
        }

        let abs_path = resolver.resolve(std::path::Path::new(&entry.inbox_path));

        mutations.push(Mutation::StashFromZone(StashFromZoneMutation {
            path: abs_path,
            stash_name: "inbox_duplicate".to_string(),
        }));

        mutations.push(Mutation::DropFromIndex(DropFromIndexMutation {
            path: PathBuf::from(&entry.inbox_path),
            inode: Some(entry.inbox_inode),
            zone: Some("inbox".to_string()),
        }));
    }

    mutations
}

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    /// Stash only equivalent + subpar entries
    StashEquivalents,
    /// Stash ALL inbox duplicates (including better-quality ones)
    StashAll,
    #[default]
    Cancel,
}

impl SelectedButton {
    pub fn left(&mut self) {
        *self = match *self {
            Self::Cancel => Self::StashAll,
            Self::StashAll => Self::StashEquivalents,
            Self::StashEquivalents => Self::StashEquivalents,
        };
    }

    pub fn right(&mut self) {
        *self = match *self {
            Self::StashEquivalents => Self::StashAll,
            Self::StashAll => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
    }
}
