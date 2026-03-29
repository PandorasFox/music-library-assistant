//! Artist Plural Normalization resolution — restructure multi-valued
//! ARTIST/ALBUMARTIST tags into singular (semicolon-joined) + plural (individual).
//!
//! Route: `/resolve/artist-plural`
//! Query: `GetArtistNeedsPluralData`
//! Data: `ArtistNeedsPluralModalData` (mm-meta)
//! Mutations: `ApplyTagOps` (deterministic, no operator choice beyond confirmation)

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::health_modals::ArtistNeedsPluralModalData;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData`.
pub struct ArtistPluralData(pub ArtistNeedsPluralModalData);

impl ResolutionData for ArtistPluralData {
    type ButtonCtx = ArtistPluralButtonCtx;

    fn list_len(&self) -> usize {
        self.0.files.len()
    }

    fn button_ctx(&self) -> ArtistPluralButtonCtx {
        ArtistPluralButtonCtx {
            has_files: self.0.has_files(),
            file_count: self.0.files.len(),
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.files.get(cursor).map(|f| f.corpus_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 5,
        }
    }

    fn list_title(&self) -> String {
        format!(" Artist Tags Need Pluralizing ({}) ", self.0.files.len())
    }

    fn empty_message(&self) -> &'static str {
        "No files need artist tag pluralization"
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for artist plural normalization modals.
pub type ArtistPluralState = ResolutionState<ArtistPluralData, ArtistPluralButton>;

// ============================================================================
// Action enum
// ============================================================================

/// Actions returned from the artist plural resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtistPluralAction {
    /// Confirm: semicolon-join singular tags, add plural tags for all files.
    ConfirmAll,
    /// Cancel and return.
    Cancel,
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for ArtistPluralState {
    type Action = ArtistPluralAction;

    fn dispatch(
        &self,
        action: ArtistPluralAction,
        _resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            ArtistPluralAction::ConfirmAll => {
                let mutations = self.data.0.pluralize_mutations();
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = ArtistPluralButton::Confirm
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: format!(
                        "Pluralize artist tags for {} files",
                        self.data.0.files.len()
                    ),
                    mutations,
                }
            }
            ArtistPluralAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Artist plural normalization cancelled"
    }
}

// ============================================================================
// Button context + enum
// ============================================================================

/// Lightweight context for button enablement/labels.
pub struct ArtistPluralButtonCtx {
    pub has_files: bool,
    pub file_count: usize,
}

/// Button choices for the artist plural resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtistPluralButton {
    Confirm,
    #[default]
    Cancel,
}

impl ModalButtons for ArtistPluralButton {
    type Context = ArtistPluralButtonCtx;
    type Action = ArtistPluralAction;

    fn all() -> &'static [Self] {
        &[Self::Confirm, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Confirm => format!("Pluralize All ({})", ctx.file_count).into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Confirm if ctx.has_files => Color::Green,
            Self::Confirm => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Confirm => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> ArtistPluralAction {
        match self {
            Self::Confirm => ArtistPluralAction::ConfirmAll,
            Self::Cancel => ArtistPluralAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Confirm => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ArtistPluralNormalization,
                label: "Pluralize artist tags".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
