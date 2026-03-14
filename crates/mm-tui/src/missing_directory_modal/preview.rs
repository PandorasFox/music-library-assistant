//! Missing Directory Resolution Preview UI
//!
//! Uses the generic `ResolutionState` — only the data wrapper, button enum,
//! and rendering are modal-specific.

use std::borrow::Cow;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use mm_meta::decisions::DecisionKey;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::modal_frame::ContentLayout;
use mm_ui::protocol_binding::ProtocolBinding;
use mm_ui::resolution_state::{ResolutionData, ResolutionState};

use super::MissingDirectoryModalData;
use crate::helpers::truncate_left;
use crate::widgets::modal_frame::ModalFrame;

// ============================================================================
// Data wrapper
// ============================================================================

/// Data payload for the missing directory resolution modal.
pub struct MissingDirectoryData(pub MissingDirectoryModalData);

impl ResolutionData for MissingDirectoryData {
    type ButtonCtx = MissingDirectoryButtonCtx;

    fn list_len(&self) -> usize {
        self.0.count()
    }

    fn button_ctx(&self) -> MissingDirectoryButtonCtx {
        MissingDirectoryButtonCtx {
            has_directories: self.0.count() > 0,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.directories.get(cursor).map(|s| s.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 2,
        }
    }

    fn list_title(&self) -> String {
        format!(" Deleted Directories ({}) ", self.0.count())
    }

    fn empty_message(&self) -> &'static str {
        "No missing directories"
    }
}

// ============================================================================
// Concrete state type alias
// ============================================================================

pub type MissingDirectoryPreviewState = ResolutionState<MissingDirectoryData, MissingDirectoryButton>;

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingDirectoryPreviewAction {
    /// User confirmed drop action.
    ConfirmDrop,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// Button Definition
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingDirectoryButton {
    Drop,
    #[default]
    Cancel,
}

pub struct MissingDirectoryButtonCtx {
    pub has_directories: bool,
}

impl ModalButtons for MissingDirectoryButton {
    type Context = MissingDirectoryButtonCtx;
    type Action = MissingDirectoryPreviewAction;

    fn all() -> &'static [Self] {
        &[Self::Drop, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Drop => "Drop All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Drop if ctx.has_directories => Color::Yellow,
            Self::Drop => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Drop => ctx.has_directories,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingDirectoryPreviewAction {
        match self {
            Self::Drop => MissingDirectoryPreviewAction::ConfirmDrop,
            Self::Cancel => MissingDirectoryPreviewAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Drop => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingDirectory,
                label: "Drop missing directories".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// ModalFrame Rendering
// ============================================================================

impl ModalFrame for MissingDirectoryPreviewState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let count = self.data.0.count();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing Directory Acknowledgment ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} directories)", count),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let dir = &self.data.0.directories[idx];
        let path = truncate_left(dir, width.saturating_sub(2) as usize);
        let style = if is_cursor {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let desc = Paragraph::new(
            "These directories were deleted externally. \
             Dropping will remove them and their files from the index.",
        )
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, area);
    }
}
