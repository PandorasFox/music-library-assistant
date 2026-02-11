//! Standardized Transaction Confirmation Review Modal
//!
//! All mutation flows pass through this modal before execution:
//! - Tag Editor
//! - Tag Canonicity Resolution
//! - Deploy Preview
//! - Missing File Resolution
//! - Intake Confirmation
//!
//! The modal shows a summary of all staged decisions and provides
//! Cancel/Discard/Confirm actions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::meta::mutations::Mutation;
use crate::ui::widgets::{centered_rect_fixed, ConfirmationButton, render_button_row};
use crate::witch::Witch;

// ============================================================================
// Types
// ============================================================================

/// Summary of a single decision for display.
#[derive(Debug, Clone)]
pub struct DecisionSummary {
    pub label: String,
    pub mutation_count: usize,
    pub track_count: usize,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionReviewAction {
    None,
    /// Return to source modal (transaction remains active)
    Cancel,
    /// Discard transaction and return to Insights
    Discard,
    /// Commit transaction and proceed to Progress
    Confirm,
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

/// State for the standardized transaction review modal.
///
/// Decision data is fetched from the Witch's active transaction at render time,
/// NOT stored in this state. The Witch is the source of truth.
///
/// Cancel navigation is handled by the `SuspendedView` stored alongside this
/// state in `ActiveView::TransactionReview` — no source tracking needed here.
pub struct TransactionReviewState {
    pub cursor: usize,
    pub scroll: usize,
    pub button_focus: ReviewButtonFocus,
    pub post_commit_phase: PostCommitPhase,
}

impl TransactionReviewState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            button_focus: ReviewButtonFocus::Cancel, // Safe default
            post_commit_phase: PostCommitPhase::SignalRefresh,
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
            ReviewButtonFocus::Discard => ReviewButtonFocus::Cancel,
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
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus_left();
                TransactionReviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.focus_right();
                TransactionReviewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TransactionReviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Cursor bounds checked at render time against actual decision count
                self.cursor = self.cursor.saturating_add(1);
                TransactionReviewAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.button_focus {
                    ReviewButtonFocus::Cancel => TransactionReviewAction::Cancel,
                    ReviewButtonFocus::Discard => TransactionReviewAction::Discard,
                    ReviewButtonFocus::Confirm => TransactionReviewAction::Confirm,
                }
            }
            KeyCode::Esc => TransactionReviewAction::Cancel,
            KeyCode::Char('y') | KeyCode::Char('Y') => TransactionReviewAction::Confirm,
            KeyCode::Char('d') | KeyCode::Char('D') => {
                // Require Ctrl+D for discard to avoid accidental discards
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
            | Mutation::EmitCanonicalTag(_) => {}
        }
    }

    inodes.len()
}

/// Fetch decision summaries from the Witch's active transaction.
pub fn fetch_decision_summaries(witch: &Witch) -> Vec<DecisionSummary> {
    witch
        .decision_indices()
        .iter()
        .filter_map(|&idx| {
            witch.get_decision(idx).map(|d| DecisionSummary {
                label: d.label.clone(),
                mutation_count: d.mutations.len(),
                track_count: count_unique_files(&d.mutations),
            })
        })
        .collect()
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the transaction review modal.
pub fn render(f: &mut Frame, area: Rect, state: &TransactionReviewState, decisions: &[DecisionSummary]) {
    // Clamp cursor to valid range
    let max_cursor = decisions.len().saturating_sub(1);
    let cursor = state.cursor.min(max_cursor);

    // Calculate totals
    let total_mutations: usize = decisions.iter().map(|d| d.mutation_count).sum();

    // Modal dimensions
    let modal_width = 74.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(2));
    let modal_area = centered_rect_fixed(modal_width, modal_height, area);

    // Clear background
    f.render_widget(Clear, modal_area);

    // Title with summary
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

    // Layout: list + spacer + buttons + hints
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Decision list
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Buttons
            Constraint::Length(1), // Hints
        ])
        .split(inner);

    // Render decision list
    render_decision_list(f, chunks[0], decisions, cursor, state.scroll);

    // Render buttons
    render_button_row(f, chunks[2], &[
        ConfirmationButton::new("Cancel", Color::White)
            .selected(state.button_focus == ReviewButtonFocus::Cancel),
        ConfirmationButton::new("Discard", Color::Red)
            .selected(state.button_focus == ReviewButtonFocus::Discard),
        ConfirmationButton::new("Confirm", Color::Green)
            .selected(state.button_focus == ReviewButtonFocus::Confirm),
    ]);

    // Render hints
    use crate::ui::widgets::control_colors as cc;

    let hints = Line::from(vec![
        cc::nav("[<>]"),
        cc::text(" select  "),
        cc::confirm("[Enter]"),
        cc::text(" activate  "),
        cc::action("[Y]"),
        cc::text(" confirm  "),
        cc::cancel("[Ctrl+D]"),
        cc::text(" discard  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]);
    let hint = Paragraph::new(hints).alignment(Alignment::Center);
    f.render_widget(hint, chunks[3]);
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

