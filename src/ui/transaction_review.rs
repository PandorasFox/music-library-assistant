//! Transaction Review — shared core for reviewing staged decisions before commit.
//!
//! Two wrappers use this core:
//! - **SuspendingTransactionReview** (this module's `render` fn + `ActiveView::TransactionReview`):
//!   Modal that suspends the parent view. Has Cancel button, Esc pops view stack.
//! - **TabbedTransactionReview** (`tabbed_transaction_review` module):
//!   Persistent lateral tab. No Cancel, Tab/BackTab for cycling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::meta::decisions::DecisionKey;
use crate::meta::mutations::{DiffEntry, Mutation};
use crate::ui::widgets::{centered_rect_fixed, ConfirmationButton, FocusPane, render_button_row};
use crate::witch::Witch;

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

/// Which button is focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewButtonFocus {
    /// Cancel - return to source modal (safe default)
    #[default]
    Cancel,
    /// Discard all decisions, return to Insights
    Discard,
    /// Confirm and execute all decisions
    Confirm,
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
///
/// Decision data is fetched from the Witch's active transaction at render time,
/// NOT stored in this state. The Witch is the source of truth.
pub struct TransactionReviewState {
    pub cursor: usize,
    pub scroll: usize,
    pub focus_pane: FocusPane,
    pub button_focus: ReviewButtonFocus,
    pub post_commit_phase: PostCommitPhase,
    /// When set, a confirmation popup is shown for removing this decision.
    pub pending_removal: Option<DecisionKey>,
    /// Whether the Cancel button is available (false in tabbed mode).
    show_cancel: bool,
}

impl TransactionReviewState {
    /// Create state for suspending mode (Cancel button available).
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            focus_pane: FocusPane::List,
            button_focus: ReviewButtonFocus::Cancel,
            post_commit_phase: PostCommitPhase::SignalRefresh,
            pending_removal: None,
            show_cancel: true,
        }
    }

    /// Create state for tabbed mode (no Cancel button).
    pub fn new_tabbed() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            focus_pane: FocusPane::List,
            button_focus: ReviewButtonFocus::Confirm,
            post_commit_phase: PostCommitPhase::SignalRefresh,
            pending_removal: None,
            show_cancel: false,
        }
    }

    pub fn with_post_commit_phase(mut self, phase: PostCommitPhase) -> Self {
        self.post_commit_phase = phase;
        self
    }

    /// Move button focus left (Cancel <- Discard <- Confirm).
    pub fn focus_left(&mut self) {
        self.button_focus = match self.button_focus {
            ReviewButtonFocus::Confirm => ReviewButtonFocus::Discard,
            ReviewButtonFocus::Discard if self.show_cancel => ReviewButtonFocus::Cancel,
            ReviewButtonFocus::Discard => ReviewButtonFocus::Discard,
            ReviewButtonFocus::Cancel => ReviewButtonFocus::Cancel,
        };
    }

    /// Move button focus right (Cancel -> Discard -> Confirm).
    pub fn focus_right(&mut self) {
        self.button_focus = match self.button_focus {
            ReviewButtonFocus::Cancel => ReviewButtonFocus::Discard,
            ReviewButtonFocus::Discard => ReviewButtonFocus::Confirm,
            ReviewButtonFocus::Confirm => ReviewButtonFocus::Confirm,
        };
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> TransactionReviewAction {
        // Confirmation popup mode — intercept all keys
        if let Some(ref key_to_remove) = self.pending_removal {
            return match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let k = key_to_remove.clone();
                    self.pending_removal = None;
                    TransactionReviewAction::ConfirmRemoval(k)
                }
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Delete => {
                    self.pending_removal = None;
                    TransactionReviewAction::None
                }
                _ => TransactionReviewAction::None,
            };
        }

        // Shift+Up/Down: switch focus between decisions list and buttons
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            return match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    TransactionReviewAction::None
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    TransactionReviewAction::None
                }
                _ => TransactionReviewAction::None,
            };
        }

        // Normal mode — arrow keys scoped to focused pane
        match key.code {
            // List navigation (only when list is focused)
            KeyCode::Up if self.focus_pane == FocusPane::List => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TransactionReviewAction::None
            }
            KeyCode::Down if self.focus_pane == FocusPane::List => {
                // Cursor bounds checked at render time against actual decision count
                self.cursor = self.cursor.saturating_add(1);
                TransactionReviewAction::None
            }
            // Button navigation (only when buttons are focused)
            KeyCode::Left if self.focus_pane == FocusPane::Buttons => {
                self.focus_left();
                TransactionReviewAction::None
            }
            KeyCode::Right if self.focus_pane == FocusPane::Buttons => {
                self.focus_right();
                TransactionReviewAction::None
            }
            // Remove decision (only when list is focused)
            KeyCode::Backspace | KeyCode::Delete if self.focus_pane == FocusPane::List => {
                TransactionReviewAction::RequestRemoval
            }
            // Activate button (only when buttons are focused)
            KeyCode::Enter | KeyCode::Char(' ') if self.focus_pane == FocusPane::Buttons => {
                match self.button_focus {
                    ReviewButtonFocus::Cancel => TransactionReviewAction::Cancel,
                    ReviewButtonFocus::Discard => TransactionReviewAction::Discard,
                    ReviewButtonFocus::Confirm => TransactionReviewAction::Confirm,
                }
            }
            // Global shortcuts (work regardless of pane focus)
            KeyCode::Esc => TransactionReviewAction::Cancel,
            KeyCode::Char('y') | KeyCode::Char('Y') => TransactionReviewAction::Confirm,
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    TransactionReviewAction::Discard
                } else {
                    TransactionReviewAction::None
                }
            }
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
            Mutation::MoveToStash(_)
            | Mutation::Move(_)
            | Mutation::IndexFileFromPath(_)
            | Mutation::HardLink(_)
            | Mutation::LibraryMove(_)
            | Mutation::DbMigration { .. }
            | Mutation::UpdateFilePath(_)
            | Mutation::DropDirectoryFromIndex(_)
            | Mutation::EmitCanonicalTag(_)
            | Mutation::EmitExpectedOverlap(_)
            | Mutation::EmitExpectedDuplicate(_)
            | Mutation::EmitExpectedMissingTag(_)
            | Mutation::ApplyConfigEdits(_)
            | Mutation::ApplyDirConfigEdit(_) => {}

            Mutation::EmbedAlbumArt(ref m) => {
                inodes.insert(m.inode);
            }

            Mutation::InboxToCorpus(ref m) => {
                inodes.insert(m.inode);
            }
        }
    }

    inodes.len()
}

/// Fetch decision summaries from the Witch's active transaction.
pub fn fetch_decision_summaries(witch: &Witch) -> Vec<DecisionSummary> {
    witch
        .decision_keys()
        .iter()
        .filter_map(|key| {
            witch.get_decision(key).map(|d| {
                let diff_entries = d.mutations.iter()
                    .flat_map(|m| match m.as_executor() {
                        Some(e) => e.diff_entries(),
                        None => Vec::new(),
                    })
                    .collect();

                DecisionSummary {
                    key: key.clone(),
                    label: d.label.clone(),
                    mutation_count: d.mutations.len(),
                    track_count: count_unique_files(&d.mutations),
                    diff_entries,
                }
            })
        })
        .collect()
}

// ============================================================================
// Shared Rendering (used by both suspending and tabbed modes)
// ============================================================================

/// Render the content area: decisions pane + mutations pane + buttons + hints.
///
/// Used directly by the tabbed wrapper, and inside the outer frame by
/// `render_fullscreen`.
pub(crate) fn render_content(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    let max_cursor = decisions.len().saturating_sub(1);
    let cursor = state.cursor.min(max_cursor);

    // Decide how much space the decision list gets vs the diff area.
    // +2 accounts for the block border.
    let list_rows = if decisions.len() <= 1 { 1 } else { decisions.len().min(5) as u16 };
    let list_block_height = list_rows + 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(list_block_height), // Decisions block (bordered)
            Constraint::Min(5),                    // Mutations block (bordered)
            Constraint::Length(1),                 // Buttons
            Constraint::Length(1),                 // Hints
        ])
        .split(area);

    // Decisions pane (blue border)
    let decisions_block = Block::default()
        .title(" Decisions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let decisions_inner = decisions_block.inner(chunks[0]);
    f.render_widget(decisions_block, chunks[0]);
    render_decision_list(f, decisions_inner, decisions, cursor, state.scroll);

    // Mutations pane (purple border)
    let mutations_block = Block::default()
        .title(" Mutations ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let mutations_inner = mutations_block.inner(chunks[1]);
    f.render_widget(mutations_block, chunks[1]);
    if let Some(decision) = decisions.get(cursor) {
        render_diff_entries(f, mutations_inner, &decision.diff_entries);
    }

    render_buttons_and_hints(f, chunks[2], chunks[3], state);
}

/// Render the removal confirmation popup overlay.
pub(crate) fn render_removal_popup(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    let Some(ref key) = state.pending_removal else { return };

    let label = decisions.iter()
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

    let msg = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Remove ", Style::default().fg(Color::White)),
            Span::styled(
                crate::ui::helpers::truncate_right(label, 30),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("?", Style::default().fg(Color::White)),
        ]),
    ])
    .alignment(Alignment::Center);
    f.render_widget(msg, chunks[0]);

    use crate::ui::widgets::control_colors as cc;
    let hint = Paragraph::new(Line::from(vec![
        cc::confirm("[Enter]"),
        cc::text(" remove  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[1]);
}

/// Render diff entries with red (old) → green (new) coloring.
pub(crate) fn render_diff_entries(f: &mut Frame, area: Rect, entries: &[DiffEntry]) {
    if entries.is_empty() {
        let empty = Paragraph::new("No changes")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, area);
        return;
    }

    let visible_height = area.height as usize;

    let items: Vec<ListItem> = entries
        .iter()
        .take(visible_height)
        .map(|entry| {
            let line = Line::from(vec![
                Span::styled(
                    format!("  {:<36} ", entry.label),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    &entry.old_value,
                    Style::default().fg(Color::Red),
                ),
                Span::styled(
                    " → ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    &entry.new_value,
                    Style::default().fg(Color::Green),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, area);

    // Scroll indicator
    if entries.len() > visible_height {
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(3),
            y: area.y,
            width: 2,
            height: 1,
        };
        let indicator = Paragraph::new("v").style(Style::default().fg(Color::DarkGray));
        f.render_widget(indicator, indicator_area);
    }
}

fn render_buttons_and_hints(f: &mut Frame, button_area: Rect, hint_area: Rect, state: &TransactionReviewState) {
    let bf = state.focus_pane == FocusPane::Buttons;

    let mut buttons = Vec::new();
    if state.show_cancel {
        buttons.push(
            ConfirmationButton::new("Cancel", Color::White)
                .selected(bf && state.button_focus == ReviewButtonFocus::Cancel),
        );
    }
    buttons.push(
        ConfirmationButton::new("Discard", Color::Red)
            .selected(bf && state.button_focus == ReviewButtonFocus::Discard),
    );
    buttons.push(
        ConfirmationButton::new("Confirm", Color::Green)
            .selected(bf && state.button_focus == ReviewButtonFocus::Confirm),
    );
    render_button_row(f, button_area, &buttons);

    use crate::ui::widgets::control_colors as cc;

    let mut hint_spans = vec![
        cc::nav("[Shift+↑↓]"),
        cc::text(" pane  "),
        cc::nav("[↑↓/<>]"),
        cc::text(" navigate  "),
        cc::confirm("[Enter]"),
        cc::text(" activate  "),
        cc::action("[Y]"),
        cc::text(" confirm  "),
        cc::cancel("[Ctrl+D]"),
        cc::text(" discard  "),
        cc::cancel("[Bksp]"),
        cc::text(" remove"),
    ];
    if state.show_cancel {
        hint_spans.push(cc::text("  "));
        hint_spans.push(cc::cancel("[Esc]"));
        hint_spans.push(cc::text(" cancel"));
    }
    let hint = Paragraph::new(Line::from(hint_spans)).alignment(Alignment::Center);
    f.render_widget(hint, hint_area);
}

fn render_decision_list(
    f: &mut Frame,
    area: Rect,
    decisions: &[DecisionSummary],
    cursor: usize,
    scroll: usize,
) {
    let visible_height = area.height as usize;

    // Calculate scroll offset to keep cursor visible
    let scroll_offset = if cursor >= scroll + visible_height {
        cursor - visible_height + 1
    } else if cursor < scroll {
        cursor
    } else {
        scroll
    };

    let items: Vec<ListItem> = decisions
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, decision)| {
            let is_selected = idx == cursor;

            let prefix = if is_selected { "> " } else { "  " };
            let text = format!(
                "{}{} ({} edit{}, {} track{})",
                prefix,
                decision.label,
                decision.mutation_count,
                if decision.mutation_count == 1 { "" } else { "s" },
                decision.track_count,
                if decision.track_count == 1 { "" } else { "s" },
            );

            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, area);

    // Show scroll indicator if needed
    if decisions.len() > visible_height {
        let indicator = if scroll_offset > 0 && scroll_offset + visible_height < decisions.len() {
            "^v"
        } else if scroll_offset > 0 {
            "^"
        } else {
            "v"
        };

        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(3),
            y: area.y,
            width: 2,
            height: 1,
        };

        let indicator_widget =
            Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
        f.render_widget(indicator_widget, indicator_area);
    }
}

// ============================================================================
// Suspending Mode Rendering (modal / fullscreen with outer frame)
// ============================================================================

/// Render the suspending transaction review (modal or fullscreen).
pub fn render(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    let has_diffs = decisions.iter().any(|d| !d.diff_entries.is_empty());

    if has_diffs {
        render_fullscreen(f, area, state, decisions);
    } else {
        render_modal(f, area, state, decisions);
    }

    render_removal_popup(f, area, state, decisions);
}

/// Standard centered modal for decisions without diffs (tag edits, etc.).
fn render_modal(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    let max_cursor = decisions.len().saturating_sub(1);
    let cursor = state.cursor.min(max_cursor);

    let total_mutations: usize = decisions.iter().map(|d| d.mutation_count).sum();

    let modal_width = 74.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(2));
    let modal_area = centered_rect_fixed(modal_width, modal_height, area);

    f.render_widget(Clear, modal_area);

    let title = format!(
        " Review: {} decision{}, {} mutation{} ",
        decisions.len(),
        if decisions.len() == 1 { "" } else { "s" },
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

    // Layout: decisions block + buttons + hints
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Decision list (bordered)
            Constraint::Length(1), // Buttons
            Constraint::Length(1), // Hints
        ])
        .split(inner);

    // Decisions pane (blue border)
    let decisions_block = Block::default()
        .title(" Decisions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let decisions_inner = decisions_block.inner(chunks[0]);
    f.render_widget(decisions_block, chunks[0]);
    render_decision_list(f, decisions_inner, decisions, cursor, state.scroll);

    render_buttons_and_hints(f, chunks[1], chunks[2], state);
}

/// Full-screen layout with outer frame, delegates content to `render_content`.
fn render_fullscreen(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    let total_mutations: usize = decisions.iter().map(|d| d.mutation_count).sum();

    let title = format!(
        " Review: {} decision{}, {} mutation{} ",
        decisions.len(),
        if decisions.len() == 1 { "" } else { "s" },
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

    render_content(f, inner, state, decisions);
}
