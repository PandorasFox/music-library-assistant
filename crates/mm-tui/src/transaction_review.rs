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
use crate::widgets::modal_buttons::ModalButtons;
use crate::widgets::rich_text::{RichBlock, RichSpan};
use crate::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::widgets::wizard::{WizardItem, WizardOffer};
use crate::widgets::{centered_rect_fixed, ButtonRowState};
use mm_meta::witch_handle::WitchHandle;

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

/// Context for ReviewButton enablement (show_cancel flag).
#[derive(Debug, Clone, Copy)]
pub struct ReviewButtonCtx {
    pub show_cancel: bool,
}

/// Button choices for the transaction review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewButton {
    /// Cancel - return to source modal (safe default)
    #[default]
    Cancel,
    /// Discard all decisions, return to Insights
    Discard,
    /// Confirm and execute all decisions
    Confirm,
}

impl ModalButtons for ReviewButton {
    type Context = ReviewButtonCtx;
    type Action = TransactionReviewAction;

    fn all() -> &'static [Self] {
        &[Self::Cancel, Self::Discard, Self::Confirm]
    }

    fn label(&self, _ctx: &Self::Context) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Cancel => "Cancel".into(),
            Self::Discard => "Discard".into(),
            Self::Confirm => "Confirm".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> ratatui::style::Color {
        match self {
            Self::Cancel => Color::White,
            Self::Discard => Color::Red,
            Self::Confirm => Color::Green,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Cancel => ctx.show_cancel,
            Self::Discard => true,
            Self::Confirm => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> TransactionReviewAction {
        match self {
            Self::Cancel => TransactionReviewAction::Cancel,
            Self::Discard => TransactionReviewAction::Discard,
            Self::Confirm => TransactionReviewAction::Confirm,
        }
    }
}

/// Action returned from handling input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionReviewAction {
    None,
    /// Return to source modal (transaction remains active)
    Cancel,
    /// Discard transaction and return to Insights
    Discard,
    /// Commit transaction and proceed to Progress
    Confirm,
    /// User pressed Backspace/Delete on a decision — handler should set pending_removal
    RequestRemoval,
    /// User confirmed removal in the popup — handler should execute removal
    ConfirmRemoval(DecisionKey),
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
pub struct TransactionReviewState {
    /// Cached decisions from the Witch's active transaction.
    pub decisions: Vec<DecisionSummary>,
    /// StandardList state for decisions navigation + wizard.
    pub list: StandardListState,
    /// Whether button row is focused.
    pub buttons_focused: bool,
    pub buttons: ButtonRowState<ReviewButton>,
    pub post_commit_phase: PostCommitPhase,
    /// When set, a confirmation popup is shown for removing this decision.
    pub pending_removal: Option<DecisionKey>,
    /// Context for button enablement (carries show_cancel flag).
    button_ctx: ReviewButtonCtx,
}

impl TransactionReviewState {
    /// Create state for suspending mode (Cancel button available).
    pub fn new() -> Self {
        Self {
            decisions: Vec::new(),
            list: StandardListState::new(StandardListConfig::default()),
            buttons_focused: false,
            buttons: ButtonRowState::new(), // defaults to Cancel
            post_commit_phase: PostCommitPhase::SignalRefresh,
            pending_removal: None,
            button_ctx: ReviewButtonCtx { show_cancel: true },
        }
    }

    /// Create state for tabbed mode (no Cancel button).
    pub fn new_tabbed() -> Self {
        let mut buttons = ButtonRowState::new();
        buttons.selected = ReviewButton::Confirm;
        Self {
            decisions: Vec::new(),
            list: StandardListState::new(StandardListConfig::default()),
            buttons_focused: false,
            buttons,
            post_commit_phase: PostCommitPhase::SignalRefresh,
            pending_removal: None,
            button_ctx: ReviewButtonCtx { show_cancel: false },
        }
    }

    /// Get the button context for rendering.
    pub fn button_ctx(&self) -> &ReviewButtonCtx {
        &self.button_ctx
    }

    pub fn with_post_commit_phase(mut self, phase: PostCommitPhase) -> Self {
        self.post_commit_phase = phase;
        self
    }

    /// Refresh cached decisions from the Witch. Call after mutations or on tick.
    pub fn refresh_decisions(&mut self, witch: &WitchHandle) {
        self.decisions = fetch_decision_summaries(witch);
        self.list.clamp_cursor(&self.decisions);
    }

    /// Current cursor position (for action handler interop).
    pub fn cursor(&self) -> usize {
        self.list.cursor
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> TransactionReviewAction {
        // Confirmation popup mode — intercept all actions
        if let Some(ref key_to_remove) = self.pending_removal {
            return match action {
                InputAction::Confirm | InputAction::Toggle => {
                    let k = key_to_remove.clone();
                    self.pending_removal = None;
                    TransactionReviewAction::ConfirmRemoval(k)
                }
                InputAction::Cancel | InputAction::Backspace | InputAction::Delete => {
                    self.pending_removal = None;
                    TransactionReviewAction::None
                }
                _ => TransactionReviewAction::None,
            };
        }

        // Button focus mode
        if self.buttons_focused {
            return self.handle_buttons_input(action);
        }

        // Global shortcuts (work regardless of pane focus)
        match action {
            InputAction::Char('y') | InputAction::Char('Y') => {
                return TransactionReviewAction::Confirm;
            }
            InputAction::Shortcut('d') => return TransactionReviewAction::Discard,
            InputAction::Cancel => return TransactionReviewAction::Cancel,
            _ => {}
        }

        // Delegate to StandardList
        match self.list.handle_input(action, &self.decisions) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                TransactionReviewAction::None
            }
            ListInputResult::Confirm(_) => {
                // Enter on a decision — no action (read-only)
                TransactionReviewAction::None
            }
            ListInputResult::Unhandled => {
                // Handle remaining actions
                match action {
                    InputAction::Backspace | InputAction::Delete => {
                        TransactionReviewAction::RequestRemoval
                    }
                    InputAction::FocusDown => {
                        self.buttons_focused = true;
                        TransactionReviewAction::None
                    }
                    _ => TransactionReviewAction::None,
                }
            }
        }
    }

    fn handle_buttons_input(&mut self, action: &InputAction) -> TransactionReviewAction {
        match action {
            InputAction::NavLeft => {
                self.buttons.nav_left(&self.button_ctx);
                TransactionReviewAction::None
            }
            InputAction::NavRight => {
                self.buttons.nav_right(&self.button_ctx);
                TransactionReviewAction::None
            }
            InputAction::FocusUp => {
                self.buttons_focused = false;
                TransactionReviewAction::None
            }
            InputAction::Confirm | InputAction::Toggle => {
                self.buttons.confirm(&self.button_ctx)
                    .unwrap_or(TransactionReviewAction::None)
            }
            InputAction::Cancel => TransactionReviewAction::Cancel,
            InputAction::Char('y') | InputAction::Char('Y') => TransactionReviewAction::Confirm,
            InputAction::Shortcut('d') => TransactionReviewAction::Discard,
            _ => TransactionReviewAction::None,
        }
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
            | Mutation::ApplyBatchDirConfigEdits(_) => {}

            Mutation::InboxToCorpus(ref m) => {
                inodes.insert(m.inode);
            }

            Mutation::InboxDirToCorpus(ref m) => {
                inodes.extend(m.tracked_files.iter().map(|f| f.inode));
            }
        }
    }

    inodes.len()
}

/// Fetch decision summaries from the Witch's active transaction.
pub fn fetch_decision_summaries(witch: &WitchHandle) -> Vec<DecisionSummary> {
    witch
        .transaction_decision_details()
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            let diff_entries = d
                .mutations
                .iter()
                .flat_map(|m| m.diff_entries())
                .collect();

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

    let list_focused = !state.buttons_focused;

    state.list.render(
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
    let Some(ref key) = state.pending_removal else {
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
    let ctx = state.button_ctx;
    state.buttons.render(f, button_area, &ctx, state.buttons_focused);

    use crate::widgets::control_colors as cc;

    let mut hint_spans = vec![
        cc::nav("[Shift+↑↓]"),
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
