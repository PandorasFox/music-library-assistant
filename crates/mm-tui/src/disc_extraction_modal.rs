//! Disc Extraction Resolution Modal
//!
//! Shows groups where a disc value can be extracted from ALBUM or TRACKNUMBER
//! tags. For ALBUM: "Album, Disc 2" → ALBUM="Album" + DISCNUMBER="2".
//! For TRACKNUMBER: "A01" → TRACKNUMBER="01" + DISCNUMBER="A".
//!
//! ## Controls
//!
//! - Shift+Up/Down: Cycle focus between file list and resolution buttons
//! - Up/Down: Navigate file list (List focus)
//! - Left/Right: Cycle resolution option (Buttons focus)
//! - Enter: Confirm selected resolution (Buttons focus)
//! - Tab/Shift-Tab: Navigate between groups
//! - t: Edit selected track in tag editor
//! - T (Shift+T): Bulk-edit all tracks in group in tag editor
//! - Ctrl+R: Show transaction review
//! - Escape: Cancel

use std::collections::BTreeSet;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::helpers::{render_pane, truncate_right};
use crate::input::InputAction;
use crate::widgets::{
    control_colors as cc, render_file_path_list, ConfirmationButton, FocusPane, ListClickTargets,
    PathEntry, ResolutionLayout,
};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use mm_meta::domain_queries::{DiscExtractionGroup, DiscExtractionModalData};
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::standard_list::ListEntry;
use mm_ui::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Resolution Option
// ============================================================================

/// Resolution option for a disc extraction group.
#[deprecated(note = "use mm_ui::resolutions::disc_extraction::DiscExtractionState")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscResolution {
    /// Extract disc value and clean source tag.
    Apply,
    /// Leave tags as-is (skip this group).
    Skip,
}

impl DiscResolution {
    fn next(self) -> Self {
        match self {
            Self::Apply => Self::Skip,
            Self::Skip => Self::Apply,
        }
    }

    fn prev(self) -> Self {
        self.next()
    }
}

// ============================================================================
// Action Enum
// ============================================================================

/// Actions returned from the disc extraction modal.
#[deprecated(note = "use mm_ui::resolutions::disc_extraction::DiscExtractionAction")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscExtractionAction {
    /// No action needed.
    None,
    /// Cancel and return to Insights view.
    Cancel,
    /// Confirm the selected resolution for the current group.
    Confirm(DiscResolution),
    /// Navigate to next/prev group.
    NavigateGroup(bool),
    /// Show transaction review.
    ShowReview,
    /// Open tag editor for all tracks in group (individual mode).
    EditTracks,
    /// Open tag editor for all tracks in group (aggregated mode).
    EditTracksAggregated,
}

// ============================================================================
// Data Types (re-exported from mm-meta)
// ============================================================================

pub use mm_meta::domain_queries::{DiscExtractionGroup as DiscExtractionGroupReexport, DiscExtractionModalData as DiscExtractionModalDataReexport};

/// Loaded data for the modal — alias for the wire type from mm-meta.
// DEPRECATED: use mm_ui::resolutions::disc_extraction types instead
pub type DiscExtractionData = DiscExtractionModalData;

// Re-export from mm-meta where the canonical implementation lives.

// ============================================================================
// State
// ============================================================================

/// State for the disc extraction resolution modal.
#[deprecated(note = "use mm_ui::resolutions::disc_extraction::DiscExtractionState")]
#[derive(Debug)]
pub struct DiscExtractionState {
    /// Loaded data (immutable — groups are never removed).
    pub data: DiscExtractionData,
    /// Current group index.
    pub current_group: usize,
    /// File cursor within current group.
    pub file_cursor: usize,
    /// File scroll offset within current group.
    pub file_scroll: usize,
    /// Current focus pane (List or Buttons).
    pub focus_pane: FocusPane,
    /// Currently selected resolution option.
    pub selected_resolution: DiscResolution,
    /// Disc tag name from config (e.g., "DISCNUMBER").
    pub disc_tag_name: String,
    /// Click targets for file list items (set during render)
    pub click_targets: ListClickTargets,
}

impl DiscExtractionState {
    pub fn new(data: DiscExtractionData, disc_tag_name: String) -> Self {
        Self {
            data,
            current_group: 0,
            file_cursor: 0,
            file_scroll: 0,
            focus_pane: FocusPane::List,
            selected_resolution: DiscResolution::Apply,
            disc_tag_name,
            click_targets: ListClickTargets::new(),
        }
    }

    /// Get the current group, if any.
    pub fn current_group_data(&self) -> Option<&DiscExtractionGroup> {
        self.data.groups.get(self.current_group)
    }

    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.current_group_data()
            .and_then(|g| g.files.get(self.file_cursor))
            .map(|f| f.path.as_str())
    }

    /// All inodes in the current group (for tag editor).
    pub fn current_group_inodes(&self) -> Vec<i64> {
        self.current_group_data()
            .map(|g| g.files.iter().map(|f| f.inode).collect())
            .unwrap_or_default()
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<DiscExtractionAction> {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                let file_count = self.current_group_data().map_or(0, |g| g.files.len());
                if idx < file_count {
                    self.focus_pane = FocusPane::List;
                    self.file_cursor = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> DiscExtractionAction {
        // FocusUp/FocusDown: cycle focus pane
        match action {
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev(false);
                return DiscExtractionAction::None;
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next(false);
                return DiscExtractionAction::None;
            }
            _ => {}
        }

        // Ctrl+R: show transaction review
        if matches!(action, InputAction::Shortcut('r')) {
            return DiscExtractionAction::ShowReview;
        }

        match action {
            InputAction::Cancel => DiscExtractionAction::Cancel,

            // Group navigation
            InputAction::CycleNext => DiscExtractionAction::NavigateGroup(true),
            InputAction::CyclePrev => DiscExtractionAction::NavigateGroup(false),

            // File list navigation (List focus)
            InputAction::NavUp if self.focus_pane == FocusPane::List => {
                if self.file_cursor > 0 {
                    self.file_cursor -= 1;
                }
                DiscExtractionAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::List => {
                if let Some(group) = self.current_group_data() {
                    if self.file_cursor + 1 < group.files.len() {
                        self.file_cursor += 1;
                    }
                }
                DiscExtractionAction::None
            }

            // Resolution button cycling (Buttons focus)
            InputAction::NavLeft if self.focus_pane == FocusPane::Buttons => {
                self.selected_resolution = self.selected_resolution.prev();
                DiscExtractionAction::None
            }
            InputAction::NavRight if self.focus_pane == FocusPane::Buttons => {
                self.selected_resolution = self.selected_resolution.next();
                DiscExtractionAction::None
            }

            // Confirm resolution (Buttons focus)
            InputAction::Confirm if self.focus_pane == FocusPane::Buttons => {
                if self.current_group_data().is_some() {
                    DiscExtractionAction::Confirm(self.selected_resolution)
                } else {
                    DiscExtractionAction::None
                }
            }

            // Tag editor shortcuts (available regardless of focus)
            InputAction::Char('t') => DiscExtractionAction::EditTracks,
            InputAction::Char('T') => DiscExtractionAction::EditTracksAggregated,

            _ => DiscExtractionAction::None,
        }
    }

    /// Render the modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let padded = ResolutionLayout::padded(area);
        f.render_widget(Clear, padded);

        let layout = ResolutionLayout::new(padded, 3, 3, 50);

        let total_groups = self.data.groups.len();

        // Info bar: description, group counter
        let header_text = if let Some(g) = self.data.groups.get(self.current_group) {
            format!(
                " {} ({} files) [group {}/{}] ",
                g.description,
                g.files.len(),
                self.current_group + 1,
                total_groups,
            )
        } else {
            " No disc extraction groups ".to_string()
        };

        let header_block = Block::default()
            .borders(Borders::ALL)
            .title(header_text)
            .border_style(Style::default().fg(Color::Yellow));
        render_pane(f, layout.info_bar, header_block);

        // Content panes
        if self.current_group < self.data.groups.len() {
            self.render_file_list(f, layout.list_pane);
            self.render_preview(f, layout.details_pane);
        } else {
            let empty = Paragraph::new("All groups resolved.")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(empty, layout.list_pane);
        }

        // Buttons + hint bar
        self.render_buttons(f, layout.buttons);
    }

    fn render_file_list(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::List;

        let block = crate::helpers::focused_block(" Files ", is_focused);
        let inner = render_pane(f, area, block);

        let group = &self.data.groups[self.current_group];

        self.click_targets.populate(inner, self.file_scroll, group.files.len());

        let entries: Vec<PathEntry> = group
            .files
            .iter()
            .map(|file| PathEntry {
                path: &file.path,
                prefix: vec![Span::styled(
                    format!("{}: {} — ", file.source_tag, file.original_value),
                    Style::default().fg(Color::White),
                )],
                suffix: vec![],
            })
            .collect();

        render_file_path_list(f, inner, &entries, self.file_cursor, self.file_scroll);
    }

    fn render_preview(&self, f: &mut Frame, area: Rect) {
        let title = match self.selected_resolution {
            DiscResolution::Apply => " Preview: Apply ",
            DiscResolution::Skip => " Preview: Skip ",
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = render_pane(f, area, block);

        let group = &self.data.groups[self.current_group];
        let lines: Vec<ListItem> = match self.selected_resolution {
            DiscResolution::Apply => group
                .files
                .iter()
                .flat_map(|file| {
                    vec![
                        ListItem::new(Line::from(Span::styled(
                            format!(
                                " {} = \"{}\" \u{2192} \"{}\"",
                                file.source_tag, file.original_value, file.cleaned_value
                            ),
                            Style::default().fg(Color::Cyan),
                        ))),
                        ListItem::new(Line::from(Span::styled(
                            format!(" + {} = \"{}\"", self.disc_tag_name, group.disc_value),
                            Style::default().fg(Color::Green),
                        ))),
                    ]
                })
                .collect(),
            DiscResolution::Skip => {
                vec![ListItem::new(Line::from(Span::styled(
                    " No changes (skip this group)",
                    Style::default().fg(Color::DarkGray),
                )))]
            }
        };

        let list = List::new(lines);
        f.render_widget(list, inner);
    }

    fn render_buttons(&self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Buttons;
        let border_color = if is_focused {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(border_color));
        let inner = render_pane(f, area, block);

        if inner.height == 0 {
            return;
        }

        // Resolution buttons (top line of inner area)
        let button_area = Rect { height: 1, ..inner };

        let buttons = [
            ConfirmationButton::new("Apply", Color::Cyan)
                .selected(is_focused && self.selected_resolution == DiscResolution::Apply),
            ConfirmationButton::new("Skip", Color::Yellow)
                .selected(is_focused && self.selected_resolution == DiscResolution::Skip),
        ];

        crate::widgets::render_button_row(f, button_area, &buttons);

        // Hint line (bottom line of inner area)
        if inner.height >= 2 {
            let hint_area = Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            };

            let hints = Line::from(vec![
                cc::nav("Shift+\u{2191}\u{2193}"),
                cc::text(" focus  "),
                cc::nav("\u{2190}\u{2192}"),
                cc::text(" option  "),
                cc::confirm("[Enter]"),
                cc::text(" confirm  "),
                cc::nav("[Tab]"),
                cc::text(" group  "),
                cc::edit("[t]"),
                cc::text(" edit  "),
                cc::edit("[T]"),
                cc::text(" aggregate  "),
                cc::review("[^R]"),
                cc::text(" review  "),
                cc::cancel("[Esc]"),
                cc::text(" cancel"),
            ]);

            let hint_para = Paragraph::new(hints).alignment(Alignment::Center);
            f.render_widget(hint_para, hint_area);
        }
    }
}

// ============================================================================
// V3: WizardItem wrapper for StandardList
// ============================================================================

/// Display wrapper for files in a Disc Extraction group (V3 StandardList).
pub struct DiscFileListItem {
    pub path: String,
    pub source_tag: String,
    pub original_value: String,
    pub cleaned_value: String,
    pub inode: i64,
}

impl WizardItem for DiscFileListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content = vec![
            RichBlock::Paragraph(vec![RichSpan::new(
                &format!("{}: \"{}\" \u{2192} \"{}\"", self.source_tag, self.original_value, self.cleaned_value),
                Style::default().fg(Color::Cyan),
            )]),
            RichBlock::Paragraph(vec![RichSpan::new(
                &self.path,
                Style::default().fg(Color::White),
            )]),
        ];
        Some(WizardOffer::Pane {
            title: format!("File: {}", self.source_tag),
            content,
        })
    }
}

impl ListEntry for DiscFileListItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build items vec from a disc extraction group's files.
pub fn build_file_items(group: &DiscExtractionGroup) -> Vec<DiscFileListItem> {
    group.files.iter().map(|f| DiscFileListItem {
        path: f.path.clone(),
        source_tag: f.source_tag.clone(),
        original_value: f.original_value.clone(),
        cleaned_value: f.cleaned_value.clone(),
        inode: f.inode,
    }).collect()
}

// ============================================================================
// V3: Render function
// ============================================================================

/// Render the Disc Extraction V3 resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(
    f: &mut Frame,
    area: Rect,
    data: &DiscExtractionModalData,
    current_group: usize,
    list: &mut mm_ui::standard_list::StandardListState,
    buttons: &mut mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::disc_extraction::DiscExtractionButton>,
    focus: mm_ui::geometry::FocusPane,
    disc_tag_name: &str,
) {
    let padded = mm_ui::geometry::padded_rect(area);
    f.render_widget(Clear, padded);

    let vertical = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(3), // Title bar
            ratatui::layout::Constraint::Min(5),    // StandardList
            ratatui::layout::Constraint::Length(3),  // Buttons
        ])
        .split(padded);

    // --- Title bar ---
    render_disc_extraction_title(f, vertical[0], data, current_group, disc_tag_name);

    // --- StandardList ---
    let group = data.groups.get(current_group);
    let items: Vec<DiscFileListItem> = group
        .map(build_file_items)
        .unwrap_or_default();

    let list_focused = focus == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_group + 1;
        let total = data.groups.len();
        let file_count = items.len();
        let desc = group.map(|g| g.description.as_str()).unwrap_or("?");
        format!("{} ({} files) [group {}/{}] \u{2014} [Z] details", desc, file_count, current, total)
    };

    crate::widgets::standard_list::render_standard_list(
        list,
        f,
        vertical[1],
        &items,
        |idx, is_cursor, _is_selected, width| render_file_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = mm_ui::resolutions::disc_extraction::DiscExtractionButtonCtx {
        has_files: !items.is_empty(),
        group_index: current_group,
    };
    let button_focused = focus == mm_ui::geometry::FocusPane::Buttons;
    crate::widgets::modal_buttons::render_buttons(buttons, f, vertical[2], &ctx, button_focused);
}

fn render_disc_extraction_title(
    f: &mut Frame,
    area: Rect,
    data: &DiscExtractionModalData,
    current_group: usize,
    disc_tag_name: &str,
) {
    let group = data.groups.get(current_group);
    let current = current_group + 1;
    let total = data.groups.len();
    let file_count = group.map_or(0, |g| g.files.len());
    let disc_value = group.map(|g| g.disc_value.as_str()).unwrap_or("?");

    let title = format!(
        " Disc Extraction ({}/{}) \u{2014} {} files, {} = \"{}\" ",
        current, total, file_count, disc_tag_name, disc_value,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, area);
}

fn render_file_item(
    items: &[DiscFileListItem],
    idx: usize,
    is_cursor: bool,
    width: u16,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    let marker = if is_cursor { "\u{25b8} " } else { "  " };
    let label_style = if is_cursor {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let max_width = width.saturating_sub(2) as usize;
    let transform = format!("{}: \"{}\" \u{2192} \"{}\"", item.source_tag, item.original_value, item.cleaned_value);
    let transform_max = max_width.saturating_sub(5);
    let display = truncate_right(&transform, transform_max);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(display.to_string(), label_style),
        Span::styled(
            format!(" \u{2014} {}", truncate_right(&item.path, max_width.saturating_sub(transform_max + 5))),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
