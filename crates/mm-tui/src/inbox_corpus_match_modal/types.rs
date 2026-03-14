//! Inbox Corpus Match Resolution Modal Types
//!
//! Data structures for the inbox corpus match resolution modal, including
//! file entries, quality classification, and button state.

use std::borrow::Cow;
use std::path::PathBuf;

use ratatui::style::Color;

use mm_meta::mutations::file_ops::StashFromZoneMutation;
use mm_meta::mutations::indexing::DropFromIndexMutation;
use mm_meta::mutations::Mutation;
use mm_meta::views::MatchClassification;

use mm_meta::decisions::DecisionKey;
use mm_ui::protocol_binding::ProtocolBinding;
use crate::widgets::modal_buttons::ModalButtons;

use super::preview::InboxCorpusMatchPreviewAction;

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

/// Button choices for the inbox corpus match resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxMatchButton {
    /// Stash only equivalent + subpar entries
    StashEquivalents,
    /// Stash ALL inbox duplicates (including better-quality ones)
    StashAll,
    #[default]
    Cancel,
}

impl ModalButtons for InboxMatchButton {
    type Context = InboxCorpusMatchModalData;
    type Action = InboxCorpusMatchPreviewAction;

    fn all() -> &'static [Self] {
        &[Self::StashEquivalents, Self::StashAll, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::StashEquivalents => {
                format!("Stash {} equiv+subpar", ctx.stashable_count()).into()
            }
            Self::StashAll => {
                format!("Stash all {}", ctx.total_count()).into()
            }
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::StashEquivalents if ctx.stashable_count() > 0 => Color::Cyan,
            Self::StashEquivalents => Color::DarkGray,
            Self::StashAll if ctx.total_count() > 0 => Color::Yellow,
            Self::StashAll => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::StashEquivalents => ctx.stashable_count() > 0,
            Self::StashAll => !ctx.entries.is_empty(),
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> InboxCorpusMatchPreviewAction {
        match self {
            Self::StashEquivalents => InboxCorpusMatchPreviewAction::ConfirmStash,
            Self::StashAll => InboxCorpusMatchPreviewAction::ConfirmStashAll,
            Self::Cancel => InboxCorpusMatchPreviewAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::StashEquivalents => ProtocolBinding::Transaction {
                decision_key: DecisionKey::InboxCorpusMatch,
                label: "Stash inbox corpus matches".into(),
                data_query: None,
            },
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::InboxCorpusMatch,
                label: "Stash all inbox duplicates".into(),
                data_query: None,
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
