//! Transaction Review — shared core for reviewing staged decisions before commit.
//!
//! Two wrappers use this core:
//! - **SuspendingTransactionReview** (this module's `render` fn + `ActiveView::TransactionReview`):
//!   Modal that suspends the parent view. Has Cancel button, Esc pops view stack.
//! - **TabbedTransactionReview** (`tabbed_transaction_review` module):
//!   Persistent lateral tab. No Cancel, Tab/BackTab for cycling.

use std::collections::BTreeSet;

use crate::input::InputAction;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::{DiffEntry, Mutation};
use crate::widgets::rich_text::{RichBlock, RichSpan};
use crate::widgets::standard_list::{render_standard_list, ListEntry};
use crate::widgets::wizard::{WizardItem, WizardOffer};
use crate::widgets::centered_rect_fixed;

// Re-export interaction types from mm-ui.
pub use mm_ui::view_state::overlay::transaction_review::{
    ReviewButton, ReviewButtonCtx, TransactionInteraction, TransactionReviewAction,
};

// ============================================================================
// Types
// ============================================================================

/// Summary of a single decision for display.
#[derive(Debug, Clone)]
pub struct DecisionSummary {
    pub key: DecisionKey,
    pub label: String,
    pub mutation_count: usize,
    pub track_count: usize,
    pub diff_entries: Vec<DiffEntry>,
}

impl WizardItem for DecisionSummary {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        if self.diff_entries.is_empty() {
            let content = vec![RichBlock::Paragraph(vec![RichSpan::new(
                "No mutations to display",
                Style::default().fg(Color::DarkGray),
            )])];
            return Some(WizardOffer::Pane {
                title: self.label.clone(),
                content,
            });
        }

        let headers = vec![
            RichSpan::new(
                "Mutation",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            RichSpan::new(
                "Before",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            RichSpan::new(
                "After",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        let rows: Vec<Vec<Vec<RichSpan>>> = self
            .diff_entries
            .iter()
            .map(|e| {
                vec![
                    vec![RichSpan::new(&e.label, Style::default().fg(Color::White))],
                    vec![RichSpan::new(&e.old_value, Style::default().fg(Color::Red))],
                    vec![RichSpan::new(
                        &e.new_value,
                        Style::default().fg(Color::Green),
                    )],
                ]
            })
            .collect();

        let content = vec![RichBlock::Table {
            headers,
            rows,
            col_ratio: vec![30, 35, 35],
        }];

        Some(WizardOffer::Pane {
            title: self.label.clone(),
            content,
        })
    }
}

impl ListEntry for DecisionSummary {
    type Action = DecisionKey;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<DecisionKey> {
        None // Enter doesn't act on decisions directly
    }
}

// ============================================================================
// State
// ============================================================================

/// Progress phase to use after transaction commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostCommitPhase {
    /// Signal refresh (most common - file operations, tag edits)
    #[default]
    SignalRefresh,
    /// Content analysis (for intake indexing)
    ContentAnalysis,
}

/// Core state for transaction review, shared by both suspending and tabbed modes.
///
/// Composes data (decisions) with the backend-agnostic interaction state from mm-ui.
pub struct TransactionReviewState {
    /// Cached decisions from the Witch's active transaction.
    pub decisions: Vec<DecisionSummary>,
    /// Backend-agnostic interaction (list, buttons, removal popup).
    pub interaction: TransactionInteraction,
    pub post_commit_phase: PostCommitPhase,
}

impl Default for TransactionReviewState {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionReviewState {
    /// Create state for suspending mode (Cancel button available).
    pub fn new() -> Self {
        Self {
            decisions: Vec::new(),
            interaction: TransactionInteraction::new(),
            post_commit_phase: PostCommitPhase::SignalRefresh,
        }
    }

    /// Create state for tabbed mode (no Cancel button).
    pub fn new_tabbed() -> Self {
        Self {
            decisions: Vec::new(),
            interaction: TransactionInteraction::new_tabbed(),
            post_commit_phase: PostCommitPhase::SignalRefresh,
        }
    }

    pub fn with_post_commit_phase(mut self, phase: PostCommitPhase) -> Self {
        self.post_commit_phase = phase;
        self
    }

    /// Refresh cached decisions from pre-fetched decision details.
    pub fn refresh_decisions_from_details(
        &mut self,
        details: Vec<mm_meta::protocol::DecisionDetail>,
    ) {
        self.decisions = decision_details_to_summaries(details);
        self.interaction.clamp_to_data(&self.decisions);
    }

    /// Set decisions directly and clamp cursor.
    pub fn set_decisions(&mut self, decisions: Vec<DecisionSummary>) {
        self.decisions = decisions;
        self.interaction.clamp_to_data(&self.decisions);
    }

    /// Current cursor position (for action handler interop).
    pub fn cursor(&self) -> usize {
        self.interaction.cursor()
    }

    /// Handle input action, delegating to the interaction state.
    pub fn handle_input(&mut self, action: &InputAction) -> TransactionReviewAction {
        self.interaction.handle_input_with(action, &self.decisions)
    }

    // Convenience accessors for rendering code.

    pub fn button_ctx(&self) -> &ReviewButtonCtx {
        self.interaction.button_ctx()
    }

    pub fn buttons_focused(&self) -> bool {
        self.interaction.buttons_focused
    }

    pub fn pending_removal(&self) -> Option<&DecisionKey> {
        self.interaction.pending_removal.as_ref()
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Count unique files affected by a set of mutations.
fn count_unique_files(mutations: &[Mutation]) -> usize {
    let mut inodes = std::collections::HashSet::new();

    for m in mutations {
        match m {
            // ApplyTagOps - count unique inodes from all ops
            Mutation::ApplyTagOps(ref m) => {
                inodes.extend(m.ops.iter().map(|op| op.inode));
            }

            // Mutations with single inode
            Mutation::ApplyDbTagsToDisk(ref m) => {
                inodes.insert(m.inode);
            }
            Mutation::FlushTagsToDisk(ref m) => {
                inodes.insert(m.inode);
            }
            Mutation::AssimilateDiskTagsToDb(ref m) => {
                inodes.insert(m.inode);
            }

            Mutation::Transcode(ref m) => {
                inodes.insert(m.inode);
            }

            // DropFromIndex - count via inode if available
            Mutation::DropFromIndex(ref m) => {
                if let Some(i) = m.inode {
                    inodes.insert(i);
                }
            }

            // OOB resolution mutations with multiple (inode, path) pairs
            Mutation::AcknowledgeMtimeOnly(ref m) => {
                inodes.extend(m.tracks.iter().map(|(id, _)| *id));
            }

            // Mutations without inodes
            Mutation::StashFromZone(_)
            | Mutation::StashLeftovers(_)
            | Mutation::Move(_)
            | Mutation::IndexFileFromPath(_)
            | Mutation::HardLink(_)
            | Mutation::LibraryMove(_)
            | Mutation::UpdateFilePath(_)
            | Mutation::DropDirectoryFromIndex(_)
            | Mutation::EmitCanonicalTag(_)
            | Mutation::EmitExpectedOverlap(_)
            | Mutation::EmitExpectedDuplicate(_)
            | Mutation::EmitExpectedMissingTag(_)
            | Mutation::DropExternalMatch(_)
            | Mutation::ApplyConfigEdits(_)
            | Mutation::ApplyDirConfigEdit(_)
            | Mutation::ApplyBatchDirConfigEdits(_)
            | Mutation::ExportEditHistory(_)
            | Mutation::ClearEditHistory(_) => {}
        }
    }

    inodes.len()
}

/// Fetch decision summaries from the active transaction via the App.
pub(crate) fn fetch_decision_summaries(app: &mut crate::App) -> Vec<DecisionSummary> {
    let details = app.transaction_decision_details().unwrap_or_default();
    decision_details_to_summaries(details)
}

/// Convert pre-fetched decision details into display summaries.
fn decision_details_to_summaries(
    details: Vec<mm_meta::protocol::DecisionDetail>,
) -> Vec<DecisionSummary> {
    details
        .into_iter()
        .map(|d| {
            let diff_entries = mm_meta::mutations::coalesce_diff_entries(
                d.mutations
                    .iter()
                    .flat_map(|m| m.diff_entries())
                    .collect(),
            );

            DecisionSummary {
                key: d.key,
                label: d.label,
                mutation_count: d.mutations.len(),
                track_count: count_unique_files(&d.mutations),
                diff_entries,
            }
        })
        .collect()
}

// ============================================================================
// Shared Rendering (used by both suspending and tabbed modes)
// ============================================================================

/// Render the content area: decisions list (StandardList + wizard pane) + buttons + hints.
///
/// Used directly by the tabbed wrapper, and inside the outer frame by
/// `render_fullscreen`.
pub(crate) fn render_content(f: &mut Frame, area: Rect, state: &mut TransactionReviewState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Decisions (StandardList + wizard pane)
            Constraint::Length(1), // Buttons
            Constraint::Length(1), // Hints
        ])
        .split(area);

    let list_focused = !state.interaction.buttons_focused;

    render_standard_list(
        &mut state.interaction.list,
        f,
        chunks[0],
        &state.decisions,
        |idx, is_cursor, _is_selected, _width| {
            render_decision_row(&state.decisions, idx, is_cursor)
        },
        "Decisions",
        list_focused,
    );

    render_buttons_and_hints(f, chunks[1], chunks[2], state);
}

fn render_decision_row(
    decisions: &[DecisionSummary],
    idx: usize,
    is_cursor: bool,
) -> Line<'static> {
    let Some(decision) = decisions.get(idx) else {
        return Line::raw("");
    };

    let prefix = if is_cursor { "\u{25b8} " } else { "  " };
    let base_style = if is_cursor {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let edit_s = if decision.mutation_count == 1 {
        ""
    } else {
        "s"
    };
    let track_s = if decision.track_count == 1 { "" } else { "s" };

    Line::from(vec![
        Span::styled(prefix, base_style),
        Span::styled(decision.label.clone(), base_style),
        Span::styled(
            format!(
                "  ({} edit{}, {} track{})",
                decision.mutation_count, edit_s, decision.track_count, track_s,
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Render the removal confirmation popup overlay.
pub(crate) fn render_removal_popup(f: &mut Frame, area: Rect, state: &TransactionReviewState) {
    let Some(ref key) = state.interaction.pending_removal else {
        return;
    };

    let label = state
        .decisions
        .iter()
        .find(|d| d.key == *key)
        .map(|d| d.label.as_str())
        .unwrap_or("this decision");

    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_area = centered_rect_fixed(popup_width, 7, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Remove Decision ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Message
            Constraint::Length(1), // Hint
        ])
        .split(inner);

    let msg = Paragraph::new(vec![Line::from(vec![
        Span::styled("Remove ", Style::default().fg(Color::White)),
        Span::styled(
            crate::helpers::truncate_right(label, 30),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("?", Style::default().fg(Color::White)),
    ])])
    .alignment(Alignment::Center);
    f.render_widget(msg, chunks[0]);

    use crate::widgets::control_colors as cc;
    let hint = Paragraph::new(Line::from(vec![
        cc::confirm("[Enter]"),
        cc::text(" remove  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[1]);
}

fn render_buttons_and_hints(
    f: &mut Frame,
    button_area: Rect,
    hint_area: Rect,
    state: &mut TransactionReviewState,
) {
    let ctx = *state.interaction.button_ctx();
    crate::widgets::modal_buttons::render_buttons(
        &mut state.interaction.buttons,
        f,
        button_area,
        &ctx,
        state.interaction.buttons_focused,
    );

    use crate::widgets::control_colors as cc;

    let mut hint_spans = vec![
        cc::nav("[Shift+\u{2191}\u{2193}]"),
        cc::text(" pane  "),
        cc::nav("[↑↓/<>]"),
        cc::text(" navigate  "),
        cc::toggle("[Z]"),
        cc::text(" mutations  "),
        cc::confirm("[Enter]"),
        cc::text(" activate  "),
        cc::action("[Y]"),
        cc::text(" confirm  "),
        cc::cancel("[Ctrl+D]"),
        cc::text(" discard  "),
        cc::cancel("[Bksp]"),
        cc::text(" remove"),
    ];
    if ctx.show_cancel {
        hint_spans.push(cc::text("  "));
        hint_spans.push(cc::cancel("[Esc]"));
        hint_spans.push(cc::text(" cancel"));
    }
    let hint = Paragraph::new(Line::from(hint_spans)).alignment(Alignment::Center);
    f.render_widget(hint, hint_area);
}

// ============================================================================
// Suspending Mode Rendering (modal / fullscreen with outer frame)
// ============================================================================

/// Render the suspending transaction review (modal or fullscreen).
pub fn render(f: &mut Frame, area: Rect, state: &mut TransactionReviewState) {
    let has_diffs = state.decisions.iter().any(|d| !d.diff_entries.is_empty());

    if has_diffs {
        render_fullscreen(f, area, state);
    } else {
        render_modal(f, area, state);
    }

    render_removal_popup(f, area, state);
}

/// Standard centered modal for decisions without diffs (tag edits, etc.).
fn render_modal(f: &mut Frame, area: Rect, state: &mut TransactionReviewState) {
    let total_mutations: usize = state.decisions.iter().map(|d| d.mutation_count).sum();

    let modal_width = 74.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(2));
    let modal_area = centered_rect_fixed(modal_width, modal_height, area);

    f.render_widget(Clear, modal_area);

    let title = format!(
        " Review: {} decision{}, {} mutation{} ",
        state.decisions.len(),
        if state.decisions.len() == 1 { "" } else { "s" },
        total_mutations,
        if total_mutations == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    render_content(f, inner, state);
}

/// Full-screen layout with outer frame, delegates content to `render_content`.
fn render_fullscreen(f: &mut Frame, area: Rect, state: &mut TransactionReviewState) {
    let total_mutations: usize = state.decisions.iter().map(|d| d.mutation_count).sum();

    let title = format!(
        " Review: {} decision{}, {} mutation{} ",
        state.decisions.len(),
        if state.decisions.len() == 1 { "" } else { "s" },
        total_mutations,
        if total_mutations == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    render_content(f, inner, state);
}
