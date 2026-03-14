//! Inbox Corpus Match Resolution Modal Types

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::modal_frame::ContentLayout;
use mm_ui::protocol_binding::ProtocolBinding;
use mm_ui::resolution_state::{ResolutionData, ResolutionState};

pub use mm_meta::views::review_match::InboxCorpusMatchModalData;

// ============================================================================
// Data wrapper
// ============================================================================

/// Data payload for the inbox corpus match resolution modal.
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
// Concrete state type alias
// ============================================================================

pub type InboxCorpusMatchPreviewState = ResolutionState<InboxCorpusMatchData, InboxMatchButton>;

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxCorpusMatchPreviewAction {
    /// Stash equivalent + subpar entries only
    ConfirmStash,
    /// Stash ALL inbox duplicates (including better-quality ones)
    ConfirmStashAll,
    Cancel,
}

// ============================================================================
// Mutation builders
// ============================================================================

/// Extension methods for InboxCorpusMatchModalData that depend on server-only types.
// Mutation builder methods (stash_and_drop_mutations, stash_all_mutations)
// live on InboxCorpusMatchModalData in mm-meta.

// ============================================================================
// Button Definition
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxMatchButton {
    StashEquivalents,
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
            },
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::InboxCorpusMatch,
                label: "Stash all inbox duplicates".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
