//! Album Cluster View
//!
//! Two-pane layout for album canonicalization with EP/edition detection.
//! Includes quality resolution options for stashing lower-quality variants.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout, TwoPaneLayout};

use super::session::AlbumCanonSession;
use super::types::{AlbumClusterAction, AlbumDecision};
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};
use crate::ui::shared::{
    QualityAnalysis, QualityResolutionState,
};

/// Which pane is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Variants,
    Action,
    QualityOptions,
}

/// Focus within the Action pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionFocus {
    NameField,
    SquashButton,
}

/// State for the album cluster view.
#[derive(Debug, Clone)]
pub struct AlbumClusterState {
    session: AlbumCanonSession,
    focused_pane: Pane,
    selected_indices: HashSet<usize>,
    cursor_idx: usize,
    list_state: ListState,
    action_focus: ActionFocus,
    canonical_name: String,
    text_cursor: usize,
    squash_confirmed: bool,
    status_message: Option<String>,
    /// Quality analysis for current bucket
    quality_analysis: Option<QualityAnalysis>,
    /// Quality resolution options state
    quality_state: Option<QualityResolutionState>,
}

impl AlbumClusterState {
    /// Create a new cluster view state from a session.
    pub fn new(session: AlbumCanonSession) -> Self {
        let canonical_name = session
            .current_bucket()
            .and_then(|b| b.variants.first())
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let text_cursor = canonical_name.len();

        let mut state = Self {
            session,
            focused_pane: Pane::Variants,
            selected_indices: HashSet::new(),
            cursor_idx: 0,
            list_state: ListState::default(),
            action_focus: ActionFocus::NameField,
            canonical_name,
            text_cursor,
            squash_confirmed: false,
            status_message: None,
            quality_analysis: None,
            quality_state: None,
        };
        state.list_state.select(Some(0));
        state
    }

    /// Take ownership of the session.
    pub fn into_session(self) -> AlbumCanonSession {
        self.session
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> AlbumClusterAction {
        match key.code {
            KeyCode::Tab => return self.advance_to_next_bucket(),
            KeyCode::BackTab => return self.go_to_previous_bucket(),
            KeyCode::Esc => return AlbumClusterAction::ShowSessionReview,
            KeyCode::Right => {
                if matches!(self.focused_pane, Pane::Variants) {
                    self.focused_pane = Pane::Action;
                    return AlbumClusterAction::Continue;
                } else if matches!(self.action_focus, ActionFocus::NameField) {
                    if self.text_cursor < self.canonical_name.len() {
                        self.text_cursor += 1;
                        return AlbumClusterAction::Continue;
                    }
                }
            }
            KeyCode::Left => {
                if matches!(self.focused_pane, Pane::Action) {
                    if matches!(self.action_focus, ActionFocus::NameField) && self.text_cursor > 0 {
                        self.text_cursor -= 1;
                        return AlbumClusterAction::Continue;
                    } else {
                        self.focused_pane = Pane::Variants;
                        return AlbumClusterAction::Continue;
                    }
                } else if matches!(self.focused_pane, Pane::QualityOptions) {
                    self.focused_pane = Pane::Action;
                    return AlbumClusterAction::Continue;
                }
            }
            _ => {}
        }

        match self.focused_pane {
            Pane::Variants => self.handle_variants_key(key),
            Pane::Action => self.handle_action_key(key),
            Pane::QualityOptions => self.handle_quality_key(key),
        }
    }

    fn handle_variants_key(&mut self, key: KeyEvent) -> AlbumClusterAction {
        let bucket = match self.session.current_bucket() {
            Some(b) => b,
            None => return AlbumClusterAction::None,
        };

        match key.code {
            KeyCode::Up => {
                if self.cursor_idx > 0 {
                    self.cursor_idx -= 1;
                    self.list_state.select(Some(self.cursor_idx));
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Down => {
                if self.cursor_idx + 1 < bucket.variants.len() {
                    self.cursor_idx += 1;
                    self.list_state.select(Some(self.cursor_idx));
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Char(' ') => {
                // Toggle selection
                if self.selected_indices.contains(&self.cursor_idx) {
                    self.selected_indices.remove(&self.cursor_idx);
                } else {
                    self.selected_indices.insert(self.cursor_idx);
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Enter => {
                // Use selected variant as canonical name
                if let Some(variant) = bucket.variants.get(self.cursor_idx) {
                    self.canonical_name = variant.name.clone();
                    self.text_cursor = self.canonical_name.len();
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Char('a') => {
                // Select all
                self.selected_indices = (0..bucket.variants.len()).collect();
                AlbumClusterAction::Continue
            }
            _ => AlbumClusterAction::None,
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> AlbumClusterAction {
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                self.action_focus = match self.action_focus {
                    ActionFocus::NameField => ActionFocus::SquashButton,
                    ActionFocus::SquashButton => ActionFocus::NameField,
                };
                AlbumClusterAction::Continue
            }
            KeyCode::Enter => {
                match self.action_focus {
                    ActionFocus::NameField => {
                        self.action_focus = ActionFocus::SquashButton;
                    }
                    ActionFocus::SquashButton => {
                        self.toggle_squash_state();
                    }
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Char(c) => {
                if matches!(self.action_focus, ActionFocus::NameField) {
                    self.canonical_name.insert(self.text_cursor, c);
                    self.text_cursor += 1;
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Backspace => {
                if matches!(self.action_focus, ActionFocus::NameField) && self.text_cursor > 0 {
                    self.text_cursor -= 1;
                    self.canonical_name.remove(self.text_cursor);
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Delete => {
                if matches!(self.action_focus, ActionFocus::NameField)
                    && self.text_cursor < self.canonical_name.len()
                {
                    self.canonical_name.remove(self.text_cursor);
                }
                AlbumClusterAction::Continue
            }
            _ => AlbumClusterAction::None,
        }
    }

    fn handle_quality_key(&mut self, key: KeyEvent) -> AlbumClusterAction {
        match key.code {
            KeyCode::Up => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.move_up();
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Down => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.move_down();
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.toggle_current();
                }
                AlbumClusterAction::Continue
            }
            KeyCode::Left => {
                self.focused_pane = Pane::Action;
                AlbumClusterAction::Continue
            }
            _ => AlbumClusterAction::None,
        }
    }

    fn toggle_squash_state(&mut self) {
        if self.squash_confirmed {
            // Unconfirm
            self.squash_confirmed = false;
            self.quality_analysis = None;
            self.quality_state = None;
        } else {
            // Confirm and record decision
            self.squash_confirmed = true;
            self.record_current_decision();
        }
    }

    fn record_current_decision(&mut self) {
        let bucket = match self.session.current_bucket() {
            Some(b) => b.clone(),
            None => return,
        };

        // Determine which variants to rename
        let variants_to_rename: Vec<String> = if self.selected_indices.is_empty() {
            // All variants except the canonical one
            bucket
                .variants
                .iter()
                .filter(|v| v.name != self.canonical_name)
                .map(|v| v.name.clone())
                .collect()
        } else {
            // Only selected variants
            self.selected_indices
                .iter()
                .filter_map(|&idx| bucket.variants.get(idx))
                .filter(|v| v.name != self.canonical_name)
                .map(|v| v.name.clone())
                .collect()
        };

        // Generate pending changes
        let mut pending_changes = Vec::new();
        for variant_name in &variants_to_rename {
            pending_changes.push(PendingChange {
                id: None,
                session_id: self.session.session_id.clone(),
                change_type: ChangeType::TagEdit,
                source_path: format!("[album:{}]", variant_name),
                target_path: None,
                metadata_changes: Some(
                    serde_json::json!({
                        "operation": "album_canonicalization",
                        "old_album": variant_name,
                        "new_album": self.canonical_name,
                    })
                    .to_string(),
                ),
                created_at: None,
                status: ChangeStatus::Pending,
            });
        }

        let decision = AlbumDecision {
            bucket,
            canonical_name: self.canonical_name.clone(),
            variants_to_rename,
            pending_changes,
        };

        self.session.decisions.push(decision);

        let msg = format!("Recorded: {} variants → '{}'",
            self.session.decisions.last().map(|d| d.variants_to_rename.len()).unwrap_or(0),
            self.canonical_name
        );
        self.status_message = Some(msg);
    }

    fn advance_to_next_bucket(&mut self) -> AlbumClusterAction {
        self.session.advance();
        if self.session.is_complete() {
            return AlbumClusterAction::SessionComplete;
        }

        // Reset state for new bucket
        self.reset_for_current_bucket();
        AlbumClusterAction::Continue
    }

    fn go_to_previous_bucket(&mut self) -> AlbumClusterAction {
        if self.session.go_back() {
            self.reset_for_current_bucket();
        }
        AlbumClusterAction::Continue
    }

    fn reset_for_current_bucket(&mut self) {
        self.selected_indices.clear();
        self.cursor_idx = 0;
        self.list_state.select(Some(0));
        self.squash_confirmed = false;
        self.quality_analysis = None;
        self.quality_state = None;

        if let Some(bucket) = self.session.current_bucket() {
            self.canonical_name = bucket
                .variants
                .first()
                .map(|v| v.name.clone())
                .unwrap_or_default();
            self.text_cursor = self.canonical_name.len();
        }
    }

    /// Render the cluster view.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let has_quality = self.quality_state.as_ref().map(|q| q.has_options()).unwrap_or(false);

        if has_quality {
            let layout = ThreePaneLayout::horizontal()
                .left(PaneConfig::new("", 40))
                .middle(PaneConfig::new("", 35))
                .right(PaneConfig::new("", 25))
                .build(area);

            self.render_variants_pane(frame, layout.left.area);
            self.render_action_pane(frame, layout.middle.area);
            self.render_quality_pane(frame, layout.right.area);
        } else {
            let layout = TwoPaneLayout::horizontal()
                .left(PaneConfig::new("", 50))
                .right(PaneConfig::new("", 50))
                .build(area);

            self.render_variants_pane(frame, layout.left.area);
            self.render_action_pane(frame, layout.right.area);
        }
    }

    fn render_variants_pane(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, Pane::Variants);
        // Clone bucket data to avoid borrow issues with self.list_state
        let bucket = self.session.current_bucket().cloned();

        // Split area: half for album list, half for info panel
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Render album list in top half
        self.render_album_list(frame, chunks[0], focused, bucket.as_ref());

        // Render info panel in bottom half
        self.render_album_info_panel(frame, chunks[1], focused, bucket.as_ref());
    }

    fn render_album_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        bucket: Option<&super::types::AlbumBucket>,
    ) {
        let title = format!(
            " Albums ({}/{}) - {} ",
            self.session.current_index + 1,
            self.session.buckets.len(),
            bucket.map(|b| b.variant_description()).unwrap_or_default()
        );

        let items: Vec<ListItem> = bucket
            .map(|b| {
                b.variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let is_cursor = idx == self.cursor_idx;
                        let is_selected = self.selected_indices.contains(&idx);

                        let prefix = if is_selected { "[x] " } else { "[ ] " };

                        // Show format/edition info
                        let format_str = match variant.normalized.format_type {
                            crate::corpus::health::album_normalization::AlbumFormat::EP => " [EP]",
                            crate::corpus::health::album_normalization::AlbumFormat::LP => " [LP]",
                            crate::corpus::health::album_normalization::AlbumFormat::Standard => "",
                        };

                        let edition_str = variant
                            .normalized
                            .edition
                            .as_ref()
                            .map(|e| format!(" ({})", e))
                            .unwrap_or_default();

                        let base_style = if is_cursor && focused {
                            Style::default()
                                .bg(Color::Blue)
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                        } else if is_cursor {
                            Style::default().bg(Color::DarkGray)
                        } else if is_selected {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default()
                        };

                        // Single-line item (details shown in info panel below)
                        ListItem::new(Line::from(vec![
                            Span::styled(prefix.to_string(), base_style),
                            Span::styled(variant.name.clone(), base_style),
                            Span::styled(format_str.to_string(), base_style),
                            Span::styled(edition_str.clone(), base_style),
                            Span::styled(
                                format!(" ({} tracks)", variant.track_count),
                                base_style,
                            ),
                        ]))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_album_info_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        bucket: Option<&super::types::AlbumBucket>,
    ) {
        let block = Block::default()
            .title(" Album Details ")
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Get the currently hovered variant
        let variant = bucket.and_then(|b| b.variants.get(self.cursor_idx));

        let Some(variant) = variant else {
            let empty = Paragraph::new("No album selected")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, inner);
            return;
        };

        let label_style = Style::default().fg(Color::DarkGray);
        let value_style = Style::default().fg(Color::White);
        let highlight_style = Style::default().fg(Color::Yellow);

        let mut lines = Vec::new();

        // Album name
        lines.push(Line::from(vec![
            Span::styled("Album: ", label_style),
            Span::styled(&variant.name, value_style.add_modifier(Modifier::BOLD)),
        ]));

        // Format type
        let format_str = match variant.normalized.format_type {
            crate::corpus::health::album_normalization::AlbumFormat::EP => "EP",
            crate::corpus::health::album_normalization::AlbumFormat::LP => "LP",
            crate::corpus::health::album_normalization::AlbumFormat::Standard => "Standard",
        };
        lines.push(Line::from(vec![
            Span::styled("Format: ", label_style),
            Span::styled(format_str, value_style),
        ]));

        // Edition if present
        if let Some(edition) = &variant.normalized.edition {
            lines.push(Line::from(vec![
                Span::styled("Edition: ", label_style),
                Span::styled(edition.as_str(), highlight_style),
            ]));
        }

        // Track count
        lines.push(Line::from(vec![
            Span::styled("Tracks: ", label_style),
            Span::styled(variant.track_count.to_string(), value_style),
        ]));

        // Artists
        if !variant.artists.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Artists: ", label_style),
                Span::styled(variant.artists.join(", "), value_style),
            ]));
        }

        // File types
        if !variant.file_types.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("File Types: ", label_style),
                Span::styled(variant.file_types.join(", "), value_style),
            ]));
        }

        // Directories (show all, each on its own line if multiple)
        if !variant.directories.is_empty() {
            lines.push(Line::from(Span::styled("Directories:", label_style)));
            for dir in &variant.directories {
                lines.push(Line::from(vec![
                    Span::styled("  ", label_style),
                    Span::styled(dir.as_str(), Style::default().fg(Color::Cyan)),
                ]));
            }
        }

        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }

    fn render_action_pane(&self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, Pane::Action);

        let block = Block::default()
            .title(" Canonicalize To ")
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Label
                Constraint::Length(3), // Input field
                Constraint::Length(1), // Spacer
                Constraint::Length(3), // Squash button
                Constraint::Min(0),    // Status
            ])
            .split(inner);

        // Label
        let label = Paragraph::new("Album Name:");
        frame.render_widget(label, chunks[0]);

        // Text input
        let input_style = if focused && matches!(self.action_focus, ActionFocus::NameField) {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let display_text = if focused && matches!(self.action_focus, ActionFocus::NameField) {
            let before = &self.canonical_name[..self.text_cursor];
            let cursor = "|";
            let after = &self.canonical_name[self.text_cursor..];
            format!("{}{}{}", before, cursor, after)
        } else {
            self.canonical_name.clone()
        };

        let input_block = Block::default().borders(Borders::ALL).style(input_style);
        let input = Paragraph::new(display_text).block(input_block);
        frame.render_widget(input, chunks[1]);

        // Squash button
        let squash_focused = focused && matches!(self.action_focus, ActionFocus::SquashButton);
        let squash_style = if squash_focused {
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else if self.squash_confirmed {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let squash_text = if self.squash_confirmed {
            "[ Squash! (confirmed) ]"
        } else {
            "[ Squash! ]"
        };

        let squash_btn = Paragraph::new(squash_text)
            .style(squash_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(squash_btn, chunks[3]);

        // Status
        if let Some(msg) = &self.status_message {
            let status = Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Green));
            frame.render_widget(status, chunks[4]);
        }
    }

    fn render_quality_pane(&self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, Pane::QualityOptions);

        let block = Block::default()
            .title(" Quality Options ")
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(ref qs) = self.quality_state {
            let items: Vec<ListItem> = qs
                .options
                .iter()
                .enumerate()
                .map(|(idx, opt)| {
                    let is_cursor = idx == qs.cursor;
                    let is_selected = qs.is_selected(idx);

                    let prefix = if is_selected { "[x] " } else { "[ ] " };
                    let label = opt.short_label();
                    let content = format!("{}{}", prefix, label);

                    let style = if is_cursor && focused {
                        Style::default()
                            .bg(Color::Blue)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if is_cursor {
                        Style::default().bg(Color::DarkGray)
                    } else if is_selected {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };

                    ListItem::new(content).style(style)
                })
                .collect();

            let list = List::new(items);
            frame.render_widget(list, inner);
        }
    }
}
