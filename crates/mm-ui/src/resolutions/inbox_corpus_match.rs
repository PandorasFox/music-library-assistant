//! Inbox Corpus Match resolution — inbox files matching existing corpus entries.
//!
//! Route: `/resolve/inbox-corpus-match`
//! Query: `GetInboxCorpusMatchData`
//! Data: `InboxCorpusMatchModalData` (mm-meta)
//! Mutations: `StashFromZone` + `DropFromIndex` per entry

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::review_match::InboxCorpusMatchModalData;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData`.
pub struct InboxCorpusMatchData(pub InboxCorpusMatchModalData);

impl ResolutionData for InboxCorpusMatchData {
    type ButtonCtx = InboxCorpusMatchModalData;

    fn list_len(&self) -> usize {
        self.0.entries.len()
    }

    fn button_ctx(&self) -> InboxCorpusMatchModalData {
        self.0.clone()
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.entries.get(cursor).map(|e| e.inbox_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::DetailAboveList { detail_height: 6 }
    }

    fn list_title(&self) -> String {
        format!(" Inbox Files ({}) ", self.0.entries.len())
    }

    fn empty_message(&self) -> &'static str {
        "No inbox corpus matches found"
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for inbox corpus match modals.
pub type InboxCorpusMatchState = ResolutionState<InboxCorpusMatchData, InboxMatchButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxCorpusMatchAction {
    /// Stash equivalent + subpar entries only
    ConfirmStash,
    /// Stash ALL inbox duplicates (including better-quality ones)
    ConfirmStashAll,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for InboxCorpusMatchState {
    type Action = InboxCorpusMatchAction;

    fn dispatch(
        &self,
        action: InboxCorpusMatchAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            InboxCorpusMatchAction::ConfirmStash => {
                let mutations = self.data.0.stash_and_drop_mutations(resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = InboxMatchButton::StashEquivalents
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Stash inbox corpus matches".into(),
                    mutations,
                }
            }
            InboxCorpusMatchAction::ConfirmStashAll => {
                let mutations = self.data.0.stash_all_mutations(resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = InboxMatchButton::StashAll
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Stash all inbox duplicates".into(),
                    mutations,
                }
            }
            InboxCorpusMatchAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Inbox corpus match resolution cancelled"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxMatchButton {
    StashEquivalents,
    StashAll,
    #[default]
    Cancel,
}

impl ModalButtons for InboxMatchButton {
    type Context = InboxCorpusMatchModalData;
    type Action = InboxCorpusMatchAction;

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

    fn action(&self, _ctx: &Self::Context) -> InboxCorpusMatchAction {
        match self {
            Self::StashEquivalents => InboxCorpusMatchAction::ConfirmStash,
            Self::StashAll => InboxCorpusMatchAction::ConfirmStashAll,
            Self::Cancel => InboxCorpusMatchAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::StashEquivalents => ProtocolBinding::Transaction {
                decision_key: DecisionKey::InboxCorpusMatch,
                label: "Stash inbox corpus matches".into(),
            },
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::InboxCorpusMatch,
                label: "Stash all inbox duplicates".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
