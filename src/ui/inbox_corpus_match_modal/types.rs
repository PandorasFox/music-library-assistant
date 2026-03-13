//! Inbox Corpus Match Resolution Modal Types
//!
//! Data structures for the inbox corpus match resolution modal, including
//! file entries, quality classification, and button state.

use std::path::PathBuf;

use crate::meta::mutations::file_ops::StashFromZoneMutation;
use crate::meta::mutations::indexing::DropFromIndexMutation;
use crate::meta::mutations::Mutation;
use crate::meta::views::{InboxCorpusMatchEntry, MatchClassification};

/// Cached data for the inbox corpus match resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
/// Entries sorted: Equivalent first, then Subpar, then Better.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InboxCorpusMatchModalData {
    pub entries: Vec<InboxCorpusMatchEntry>,
}

impl InboxCorpusMatchModalData {
    /// Total number of entries.
    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    /// Count of entries safe to stash (Equivalent + Subpar).
    pub fn stashable_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                matches!(
                    e.classification,
                    MatchClassification::Equivalent | MatchClassification::Subpar
                )
            })
            .count()
    }

    /// Count by classification: (better, equivalent, subpar).
    pub fn count_by_class(&self) -> (usize, usize, usize) {
        let mut better = 0;
        let mut equivalent = 0;
        let mut subpar = 0;
        for e in &self.entries {
            match e.classification {
                MatchClassification::Better => better += 1,
                MatchClassification::Equivalent => equivalent += 1,
                MatchClassification::Subpar => subpar += 1,
            }
        }
        (better, equivalent, subpar)
    }

    /// Generate StashFromZone + DropFromIndex mutations for stashable entries.
    ///
    /// Only Equivalent and Subpar entries are stashed. Better entries
    /// (inbox is higher quality) are left alone.
    pub fn stash_and_drop_mutations(
        &self,
        resolver: &crate::corpus::paths::PathResolver,
    ) -> Vec<Mutation> {
        self.stash_mutations_for(resolver, |c| {
            matches!(
                c,
                MatchClassification::Equivalent | MatchClassification::Subpar
            )
        })
    }

    /// Generate StashFromZone + DropFromIndex mutations for ALL entries,
    /// including those classified as Better.
    pub fn stash_all_mutations(
        &self,
        resolver: &crate::corpus::paths::PathResolver,
    ) -> Vec<Mutation> {
        self.stash_mutations_for(resolver, |_| true)
    }

    fn stash_mutations_for(
        &self,
        resolver: &crate::corpus::paths::PathResolver,
        predicate: impl Fn(MatchClassification) -> bool,
    ) -> Vec<Mutation> {
        let mut mutations = Vec::new();

        for entry in &self.entries {
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
