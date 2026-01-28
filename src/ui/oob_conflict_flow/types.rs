//! Types for OOB tag bucketed resolution flow.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::corpus::db::types::{BucketedOobFile, ConflictBucket, TagMismatchEntry};
use crate::ui::bulk_selection::BulkSelectionState;
use crate::ui::filter_popup::FilterCondition;
use crate::ui::widgets::{ButtonRects, FocusPane};

// ============================================================================
// Resolution Button
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionButton {
    /// Make disk match DB (write db_value to files)
    ApplyDb,
    /// Make DB match disk (assimilate disk_value into index)
    AssimilateDisk,
}

impl ResolutionButton {
    pub fn toggle(self) -> Self {
        match self {
            Self::ApplyDb => Self::AssimilateDisk,
            Self::AssimilateDisk => Self::ApplyDb,
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
}

impl BucketFileState {
    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        Self { files, cursor: 0, scroll: 0 }
    }

    pub fn current_file(&self) -> Option<&BucketedOobFile> {
        self.files.get(self.cursor)
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
    /// Resolve the active bucket (bulk, buckets 1/2 only)
    Resolve,
    /// Close inspector and return to Insights
    Cancel,
    /// Open filter popup (Ctrl+F)
    OpenFilter,
}

// ============================================================================
// State
// ============================================================================

pub struct OobConflictState {
    /// Active bucket tab
    pub active_bucket: ConflictBucket,
    /// Per-bucket file lists (indexed by ConflictBucket::index())
    pub buckets: [BucketFileState; 4],
    /// Counts per bucket (for tab labels)
    pub bucket_counts: [usize; 4],
    /// Resolution button selection (for resolvable buckets)
    pub selected_button: ResolutionButton,
    /// On-demand tag diff for the currently selected file
    pub current_diff: Vec<TagMismatchEntry>,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
    /// Button rectangles for click detection (set during render)
    pub button_rects: ButtonRects,
    /// Bulk selection state for multi-file operations
    pub selection: BulkSelectionState,
    /// Active filter condition (from Ctrl+F popup)
    pub filter: Option<FilterCondition>,
    /// Filtered indices for active bucket (cached, updated when filter changes)
    pub filtered_indices: Option<Vec<usize>>,
}

impl OobConflictState {
    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        // Partition files into buckets
        let mut b0 = Vec::new();
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();
        let mut b3 = Vec::new();

        for file in files {
            match file.bucket {
                ConflictBucket::NoChanges => b0.push(file),
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
            .unwrap_or(ConflictBucket::NoChanges);

        Self {
            active_bucket: initial_bucket,
            buckets: [
                BucketFileState::new(b0),
                BucketFileState::new(b1),
                BucketFileState::new(b2),
                BucketFileState::new(b3),
            ],
            bucket_counts,
            selected_button: ResolutionButton::ApplyDb,
            current_diff: Vec::new(),
            focus_pane: FocusPane::List,
            button_rects: ButtonRects::new(),
            selection: BulkSelectionState::new(),
            filter: None,
            filtered_indices: None,
        }
    }

    /// Get indices of files in active bucket that match the current filter.
    pub fn get_filtered_indices(&self) -> Vec<usize> {
        if let Some(ref indices) = self.filtered_indices {
            indices.clone()
        } else {
            (0..self.active_bucket_state().files.len()).collect()
        }
    }

    /// Apply a filter condition and compute filtered indices for active bucket.
    pub fn apply_filter(&mut self, condition: FilterCondition) {
        if condition.is_active() {
            // For now, accept all (bucket files don't have full metadata)
            // TODO: Add full metadata filtering when track info is available
            let indices: Vec<usize> = (0..self.active_bucket_state().files.len()).collect();
            self.filtered_indices = Some(indices);
            self.filter = Some(condition);
        } else {
            self.filter = None;
            self.filtered_indices = None;
        }
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter = None;
        self.filtered_indices = None;
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

    pub fn handle_key(&mut self, key: KeyEvent) -> OobConflictAction {
        // Ctrl+Tab / Ctrl+Shift+Tab: cycle focus pane
        if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.focus_pane = self.focus_pane.prev();
            } else {
                self.focus_pane = self.focus_pane.next();
            }
            return OobConflictAction::None;
        }

        // Ctrl+A: toggle all selection in active bucket (respects filter)
        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let indices = self.get_filtered_indices();
            self.selection.toggle_all_filtered(&indices);
            return OobConflictAction::None;
        }

        // Ctrl+F: open filter popup
        if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return OobConflictAction::OpenFilter;
        }

        match key.code {
            // Space: toggle selection on current file in active bucket
            KeyCode::Char(' ') => {
                let bucket_state = self.active_bucket_state();
                if !bucket_state.files.is_empty() {
                    self.selection.toggle(bucket_state.cursor);
                }
                OobConflictAction::None
            }

            // Bucket tab navigation (regular Tab, not Ctrl+Tab)
            KeyCode::Tab => {
                self.active_bucket = self.active_bucket.next();
                OobConflictAction::Navigate
            }
            KeyCode::BackTab => {
                self.active_bucket = self.active_bucket.prev();
                OobConflictAction::Navigate
            }

            // File navigation within active bucket
            KeyCode::Up | KeyCode::Char('k') => {
                if self.active_bucket_state_mut().navigate_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.active_bucket_state_mut().navigate_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::PageUp => {
                if self.active_bucket_state_mut().page_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::PageDown => {
                if self.active_bucket_state_mut().page_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }

            // Resolution button toggle (resolvable buckets only, when focused on buttons)
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                if self.focus_pane == FocusPane::Buttons && self.active_bucket.is_resolvable() {
                    self.selected_button = self.selected_button.toggle();
                }
                OobConflictAction::None
            }

            // Confirm resolution (when focused on buttons)
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons
                    && self.active_bucket.is_resolvable()
                    && !self.active_bucket_state().files.is_empty()
                {
                    OobConflictAction::Resolve
                } else {
                    OobConflictAction::None
                }
            }

            KeyCode::Esc => OobConflictAction::Cancel,

            _ => OobConflictAction::None,
        }
    }
}

// ============================================================================
// On-demand diff computation
// ============================================================================

/// Compute the tag diff between DB and disk for a single track.
///
/// Reads DB tags from the database and disk tags from the filesystem,
/// then returns entries for all fields where the values differ.
pub fn compute_tag_diff(
    db: &crate::corpus::db::Database,
    track_id: i64,
    abs_path: &std::path::Path,
) -> Vec<TagMismatchEntry> {
    use std::collections::{HashMap, HashSet};

    // Get DB tags
    let db_tags = match db.get_track_tags(track_id) {
        Ok(tags) => tags,
        Err(_) => return Vec::new(),
    };
    let db_map: HashMap<String, String> = db_tags
        .into_iter()
        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
        .collect();

    // Read disk tags
    let disk_tags = match crate::corpus::metadata::read_all_tags(abs_path) {
        Ok(tags) => tags,
        Err(_) => return Vec::new(),
    };
    let disk_map: HashMap<String, String> = disk_tags
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();

    // Find all differing fields
    let mut all_tags: HashSet<String> = db_map.keys().cloned().collect();
    all_tags.extend(disk_map.keys().cloned());

    let mut mismatches = Vec::new();
    for tag_name in all_tags {
        let db_value = db_map.get(&tag_name).filter(|s| !s.is_empty()).cloned();
        let disk_value = disk_map.get(&tag_name).filter(|s| !s.is_empty()).cloned();

        if db_value != disk_value {
            mismatches.push(TagMismatchEntry {
                field: tag_name,
                db_value,
                disk_value,
            });
        }
    }

    mismatches.sort_by(|a, b| a.field.cmp(&b.field));
    mismatches
}
