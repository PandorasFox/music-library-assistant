//! Types for OOB tag bucketed resolution modal.

use std::borrow::Cow;

use ratatui::style::Color;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;

use mm_meta::views::{BucketedOobFile, ConflictBucket};
use crate::bulk_selection::BulkSelectionState;
use crate::widgets::modal_buttons::ModalButtons;
use crate::widgets::{ButtonRowState, FocusPane, TextInputState};

// ============================================================================
// Resolution Button
// ============================================================================

/// Context for OobConflictButton enablement.
#[derive(Debug, Clone, Copy)]
pub struct OobConflictButtonCtx {
    pub is_resolvable: bool,
    pub is_acknowledgeable: bool,
    pub has_files: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OobConflictButton {
    /// Make disk match DB (write db_value to files)
    #[default]
    ApplyDb,
    /// Make DB match disk (assimilate disk_value into index)
    AssimilateDisk,
    /// Acknowledge mtime-only changes
    Acknowledge,
    /// Cancel
    Cancel,
}

impl ModalButtons for OobConflictButton {
    type Context = OobConflictButtonCtx;
    type Action = OobConflictAction;

    fn all() -> &'static [Self] {
        &[Self::ApplyDb, Self::AssimilateDisk, Self::Acknowledge, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::ApplyDb => "Apply DB -> Files".into(),
            Self::AssimilateDisk => "Assimilate Files -> DB".into(),
            Self::Acknowledge => "Acknowledge Mtime".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::ApplyDb => Color::Green,
            Self::AssimilateDisk => Color::Cyan,
            Self::Acknowledge => Color::Green,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::ApplyDb => ctx.is_resolvable && ctx.has_files,
            Self::AssimilateDisk => ctx.is_resolvable && ctx.has_files,
            Self::Acknowledge => ctx.is_acknowledgeable && ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> OobConflictAction {
        match self {
            Self::ApplyDb | Self::AssimilateDisk => OobConflictAction::Resolve,
            Self::Acknowledge => OobConflictAction::Acknowledge,
            Self::Cancel => OobConflictAction::Cancel,
        }
    }
}

// ============================================================================
// Per-Bucket File State
// ============================================================================

pub struct BucketFileState {
    pub files: Vec<BucketedOobFile>,
    pub cursor: usize,
    pub scroll: usize,
    /// Per-bucket selection state
    pub selection: BulkSelectionState,
    /// Inline text filter input
    pub filter_input: TextInputState,
    /// Whether the inline filter bar is actively accepting input
    pub filter_active: bool,
    /// Active filter text (applied on Enter)
    pub filter_text: Option<String>,
    /// Per-bucket filtered indices (cached)
    pub filtered_indices: Option<Vec<usize>>,
}

impl BucketFileState {
    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        let file_count = files.len();
        let mut selection = BulkSelectionState::new();
        selection.select_all(file_count);

        Self {
            files,
            cursor: 0,
            scroll: 0,
            selection,
            filter_input: TextInputState::new(),
            filter_active: false,
            filter_text: None,
            filtered_indices: None,
        }
    }

    pub fn current_file(&self) -> Option<&BucketedOobFile> {
        self.files.get(self.cursor)
    }

    /// Get indices of files that match the current filter.
    pub fn get_filtered_indices(&self) -> Vec<usize> {
        if let Some(ref indices) = self.filtered_indices {
            indices.clone()
        } else {
            (0..self.files.len()).collect()
        }
    }

    /// Apply the current filter input text as a path substring filter.
    pub fn apply_filter(&mut self) {
        let query = self.filter_input.value().trim().to_lowercase();
        if query.is_empty() {
            self.clear_filter();
        } else {
            let indices: Vec<usize> = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.to_lowercase().contains(&query))
                .map(|(idx, _)| idx)
                .collect();
            self.filtered_indices = Some(indices);
            self.filter_text = Some(query);
        }
        self.filter_active = false;
        self.filter_input.focused = false;
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter_text = None;
        self.filtered_indices = None;
        self.filter_active = false;
        self.filter_input.focused = false;
        self.filter_input.clear();
    }

    fn navigate_up(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    fn navigate_down(&mut self) -> bool {
        if self.cursor + 1 < self.files.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn page_up(&mut self) -> bool {
        let old = self.cursor;
        self.cursor = self.cursor.saturating_sub(20);
        self.cursor != old
    }

    fn page_down(&mut self) -> bool {
        let old = self.cursor;
        self.cursor = (self.cursor + 20).min(self.files.len().saturating_sub(1));
        self.cursor != old
    }
}

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobConflictAction {
    None,
    /// File or bucket selection changed — action handler should reload diff
    Navigate,
    /// Resolve the active bucket (bulk, buckets 1/2 only - tag sync/conflict)
    Resolve,
    /// Acknowledge mtime-only changes (bucket 0 only)
    Acknowledge,
    /// Close inspector and return to Insights
    Cancel,
}

// ============================================================================
// State
// ============================================================================

pub struct OobConflictState {
    /// Active bucket tab
    pub active_bucket: ConflictBucket,
    /// Per-bucket file lists (indexed by ConflictBucket::index())
    /// Each bucket has its own selection and filter state.
    pub buckets: [BucketFileState; 4],
    /// Counts per bucket (for tab labels)
    pub bucket_counts: [usize; 4],
    /// Button row state
    pub buttons: ButtonRowState<OobConflictButton>,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
}

impl OobConflictState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.path.as_str())
    }

    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        // Partition files into buckets
        let mut b0 = Vec::new();
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();
        let mut b3 = Vec::new();

        for file in files {
            match file.bucket {
                ConflictBucket::MtimeOnly => b0.push(file),
                ConflictBucket::DbOnly => b1.push(file),
                ConflictBucket::DiskOnly => b2.push(file),
                ConflictBucket::Conflict => b3.push(file),
            }
        }

        let bucket_counts = [b0.len(), b1.len(), b2.len(), b3.len()];

        // Start on first non-empty bucket
        let initial_bucket = ConflictBucket::ALL
            .iter()
            .find(|b| bucket_counts[b.index()] > 0)
            .copied()
            .unwrap_or(ConflictBucket::MtimeOnly);

        let buckets = [
            BucketFileState::new(b0),
            BucketFileState::new(b1),
            BucketFileState::new(b2),
            BucketFileState::new(b3),
        ];

        // Default button depends on initial bucket type
        let mut buttons = ButtonRowState::new();
        if initial_bucket.is_acknowledgeable() {
            buttons.selected = OobConflictButton::Acknowledge;
        }
        // else stays on ApplyDb (the Default), which is fine for resolvable buckets

        Self {
            active_bucket: initial_bucket,
            buckets,
            bucket_counts,
            buttons,
            focus_pane: FocusPane::List,
        }
    }

    pub fn active_bucket_state(&self) -> &BucketFileState {
        &self.buckets[self.active_bucket.index()]
    }

    fn active_bucket_state_mut(&mut self) -> &mut BucketFileState {
        &mut self.buckets[self.active_bucket.index()]
    }

    pub fn total_files(&self) -> usize {
        self.bucket_counts.iter().sum()
    }

    /// Build the button context from current state.
    pub fn button_ctx(&self) -> OobConflictButtonCtx {
        OobConflictButtonCtx {
            is_resolvable: self.active_bucket.is_resolvable(),
            is_acknowledgeable: self.active_bucket.is_acknowledgeable(),
            has_files: !self.active_bucket_state().files.is_empty(),
        }
    }

    /// Get tag mismatches for the currently selected file (from the signal data).
    pub fn current_mismatches(&self) -> &[mm_meta::views::TagMismatchEntry] {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.mismatches.as_slice())
            .unwrap_or(&[])
    }

    /// Handle a mouse click at (x, y). Returns an action if a button was clicked.
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<OobConflictAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.buttons.handle_click(x, y, &ctx) {
            self.focus_pane = FocusPane::Buttons;
            Some(action)
        } else {
            None
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobConflictAction {
        // Inline filter bar captures all input when active
        let bucket = self.active_bucket_state_mut();
        if bucket.filter_active {
            match action {
                InputAction::Confirm => {
                    bucket.apply_filter();
                    return OobConflictAction::None;
                }
                InputAction::Cancel => {
                    bucket.clear_filter();
                    return OobConflictAction::None;
                }
                other => {
                    bucket.filter_input.handle_input(other);
                    return OobConflictAction::None;
                }
            }
        }

        match action {
            // Shift+Up / Shift+Down: cycle focus pane
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                OobConflictAction::None
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                OobConflictAction::None
            }

            // Ctrl+A: toggle all selection in active bucket (respects filter)
            InputAction::TextHome => {
                let bucket = self.active_bucket_state_mut();
                let indices = bucket.get_filtered_indices();
                bucket.selection.toggle_all_filtered(&indices);
                OobConflictAction::None
            }

            // Ctrl+/: activate inline filter on active bucket
            InputAction::OpenFilter => {
                let bucket = self.active_bucket_state_mut();
                bucket.filter_active = true;
                bucket.filter_input.focused = true;
                OobConflictAction::None
            }

            // Space: toggle selection on current file in active bucket
            InputAction::Toggle => {
                let bucket = self.active_bucket_state_mut();
                if !bucket.files.is_empty() {
                    let cursor = bucket.cursor;
                    bucket.selection.toggle(cursor);
                }
                OobConflictAction::None
            }

            // Bucket tab navigation
            InputAction::CycleNext => {
                self.active_bucket = self.active_bucket.next();
                let ctx = self.button_ctx();
                self.buttons.clamp(&ctx);
                OobConflictAction::Navigate
            }
            InputAction::CyclePrev => {
                self.active_bucket = self.active_bucket.prev();
                let ctx = self.button_ctx();
                self.buttons.clamp(&ctx);
                OobConflictAction::Navigate
            }

            // File navigation within active bucket
            InputAction::NavUp => {
                if self.active_bucket_state_mut().navigate_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            InputAction::NavDown => {
                if self.active_bucket_state_mut().navigate_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            InputAction::PageUp => {
                if self.active_bucket_state_mut().page_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            InputAction::PageDown => {
                if self.active_bucket_state_mut().page_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }

            // Button navigation (when focused on buttons)
            InputAction::NavLeft => {
                if self.focus_pane == FocusPane::Buttons {
                    let ctx = self.button_ctx();
                    self.buttons.nav_left(&ctx);
                }
                OobConflictAction::None
            }
            InputAction::NavRight => {
                if self.focus_pane == FocusPane::Buttons {
                    let ctx = self.button_ctx();
                    self.buttons.nav_right(&ctx);
                }
                OobConflictAction::None
            }

            // Confirm resolution (when focused on buttons)
            InputAction::Confirm => {
                if self.focus_pane == FocusPane::Buttons {
                    let ctx = self.button_ctx();
                    self.buttons.confirm(&ctx)
                        .unwrap_or(OobConflictAction::None)
                } else {
                    OobConflictAction::None
                }
            }

            InputAction::Cancel => OobConflictAction::Cancel,

            _ => OobConflictAction::None,
        }
    }
}

