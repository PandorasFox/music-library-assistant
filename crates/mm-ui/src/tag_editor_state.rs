//! Tag Editor State: composed editor using TagSet + FieldForm + ButtonRow.
//!
//! This is the shared state machine for the tag editor, consumed by both
//! TUI and web clients. Platform-specific rendering and action handling
//! remain in mm-tui / mm-web.

use std::borrow::Cow;
use std::collections::BTreeSet;

use ratatui::style::Color;

use mm_meta::db_types::Zone;
use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
use mm_meta::mutations::{Mutation, TagOp};

use crate::field_form::FieldFormState;
use crate::modal_buttons::{ButtonRowState, ModalButtons};
use crate::tag_set::{AggregatedTagSet, TagSet};

// ============================================================================
// AudioFileInfo — lightweight file identity for the tag editor
// ============================================================================

/// Minimal file info the tag editor needs.
///
/// The full `AudioFile` from mm-meta carries audio metadata (duration, bitrate,
/// sample_rate) that only the TUI render layer uses. The state machine only
/// needs identity (inode, zone, path).
#[derive(Debug, Clone)]
pub struct AudioFileInfo {
    pub inode: i64,
    pub zone: Zone,
    pub path: String,
}

// ============================================================================
// TagEditorMode
// ============================================================================

/// Whether the editor shows per-file or aggregated views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagEditorMode {
    /// Edit files one at a time (Tab navigates between them).
    #[default]
    Individual,
    /// Unified view — changes apply to all files at once.
    Aggregated,
}

// ============================================================================
// FocusPane
// ============================================================================

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    /// The tag form / file list content area.
    #[default]
    Content,
    /// The button row at the bottom.
    Buttons,
}

// ============================================================================
// TagEditorButton
// ============================================================================

/// Action buttons for the tag editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagEditorButton {
    /// Stage current changes and open transaction review.
    ReviewAll,
    /// Revert current file to original tags.
    Revert,
    /// Cancel / close the editor.
    #[default]
    Cancel,
}

/// Context for button enablement and labels.
pub struct TagEditorButtonCtx {
    pub has_changes: bool,
    pub staged_count: usize,
}

/// Actions produced by button presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEditorButtonAction {
    ReviewAll,
    Revert,
    Cancel,
}

impl ModalButtons for TagEditorButton {
    type Context = TagEditorButtonCtx;
    type Action = TagEditorButtonAction;

    fn all() -> &'static [Self] {
        &[Self::ReviewAll, Self::Revert, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::ReviewAll => "Review All".into(),
            Self::Revert => "Revert".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::ReviewAll => {
                if ctx.has_changes || ctx.staged_count > 0 {
                    Color::Green
                } else {
                    Color::DarkGray
                }
            }
            Self::Revert => {
                if ctx.has_changes {
                    Color::Red
                } else {
                    Color::DarkGray
                }
            }
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::ReviewAll => ctx.has_changes || ctx.staged_count > 0,
            Self::Revert => ctx.has_changes,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> TagEditorButtonAction {
        match self {
            Self::ReviewAll => TagEditorButtonAction::ReviewAll,
            Self::Revert => TagEditorButtonAction::Revert,
            Self::Cancel => TagEditorButtonAction::Cancel,
        }
    }
}

// ============================================================================
// TagEditorLaunchMode
// ============================================================================

/// Whether the editor owns its transaction or is embedded in a parent.
#[derive(Debug, Clone)]
pub enum TagEditorLaunchMode {
    /// Normal standalone mode — editor owns the transaction lifecycle.
    Standalone,
    /// Embedded within a parent modal — parent owns the transaction.
    Embedded {
        decision_key: mm_meta::decisions::DecisionKey,
        decision_label: String,
    },
}

// ============================================================================
// TagEditorState
// ============================================================================

/// Composed tag editor state machine.
///
/// Holds per-file TagSets, a FieldForm for editing, optional aggregated view,
/// and a ButtonRow. Platform-specific rendering and action handling are
/// implemented in mm-tui / mm-web.
pub struct TagEditorState {
    /// Per-file tag data (current state, mutated by FieldForm).
    pub tag_sets: Vec<TagSet>,
    /// Per-file original tag data (preserved for diffing).
    pub original_tag_sets: Vec<TagSet>,
    /// File identity info.
    pub files: Vec<AudioFileInfo>,

    /// Navigation.
    pub current_file: usize,
    pub mode: TagEditorMode,

    /// The form widget state (operates on tag_sets[current_file]).
    pub form: FieldFormState,

    /// Aggregated view (only when mode == Aggregated).
    pub aggregated: Option<AggregatedTagSet>,

    /// Buttons + focus.
    pub buttons: ButtonRowState<TagEditorButton>,
    pub focus: FocusPane,

    /// Transaction tracking.
    pub staged_decision_count: usize,
    pub launch_mode: TagEditorLaunchMode,
}

impl TagEditorState {
    /// Create a new tag editor state.
    pub fn new(
        mode: TagEditorMode,
        files: Vec<AudioFileInfo>,
        tag_sets: Vec<TagSet>,
    ) -> Self {
        debug_assert_eq!(files.len(), tag_sets.len());

        let original_tag_sets = tag_sets.clone();

        let aggregated = if mode == TagEditorMode::Aggregated {
            Some(AggregatedTagSet::from_tag_sets(&tag_sets))
        } else {
            None
        };

        Self {
            tag_sets,
            original_tag_sets,
            files,
            current_file: 0,
            mode,
            form: FieldFormState::new(),
            aggregated,
            buttons: ButtonRowState::new(),
            focus: FocusPane::Content,
            staged_decision_count: 0,
            launch_mode: TagEditorLaunchMode::Standalone,
        }
    }

    /// Builder: set embedded mode.
    pub fn with_embedded_mode(
        mut self,
        decision_key: mm_meta::decisions::DecisionKey,
        decision_label: String,
    ) -> Self {
        self.launch_mode = TagEditorLaunchMode::Embedded {
            decision_key,
            decision_label,
        };
        self
    }

    /// Whether this editor is embedded (parent owns transaction).
    pub fn is_embedded(&self) -> bool {
        matches!(self.launch_mode, TagEditorLaunchMode::Embedded { .. })
    }

    // ========================================================================
    // Mutation generation
    // ========================================================================

    /// Diff current vs original for the current file → mutations.
    pub fn mutations_for_current(&self) -> Vec<Mutation> {
        if self.mode == TagEditorMode::Aggregated {
            return self.mutations_for_all();
        }

        let Some(file) = self.files.get(self.current_file) else {
            return Vec::new();
        };
        let Some(current) = self.tag_sets.get(self.current_file) else {
            return Vec::new();
        };
        let Some(original) = self.original_tag_sets.get(self.current_file) else {
            return Vec::new();
        };

        let ops = current.diff(original, file.inode);
        Self::ops_to_mutations(ops, file.zone)
    }

    /// Diff current vs original for ALL files → mutations.
    pub fn mutations_for_all(&self) -> Vec<Mutation> {
        if self.mode == TagEditorMode::Aggregated {
            if let Some(ref agg) = self.aggregated {
                let inodes: Vec<i64> = self.files.iter().map(|f| f.inode).collect();
                let ops = agg.diff_all(&self.original_tag_sets, &inodes);
                let zone = self
                    .files
                    .first()
                    .map(|f| f.zone)
                    .unwrap_or(Zone::Corpus);
                return Self::ops_to_mutations(ops, zone);
            }
            return Vec::new();
        }

        let mut all_ops = Vec::new();
        for (idx, (current, original)) in self
            .tag_sets
            .iter()
            .zip(self.original_tag_sets.iter())
            .enumerate()
        {
            if let Some(file) = self.files.get(idx) {
                all_ops.extend(current.diff(original, file.inode));
            }
        }

        let zone = self
            .files
            .first()
            .map(|f| f.zone)
            .unwrap_or(Zone::Corpus);
        Self::ops_to_mutations(all_ops, zone)
    }

    /// Whether the current file (or aggregated view) has uncommitted edits.
    pub fn has_changes(&self) -> bool {
        if self.mode == TagEditorMode::Aggregated {
            return self.has_aggregated_changes();
        }
        match (
            self.tag_sets.get(self.current_file),
            self.original_tag_sets.get(self.current_file),
        ) {
            (Some(current), Some(original)) => current != original,
            _ => false,
        }
    }

    /// Whether any file has uncommitted edits.
    pub fn has_changes_any(&self) -> bool {
        if self.mode == TagEditorMode::Aggregated {
            return self.has_aggregated_changes();
        }
        self.tag_sets
            .iter()
            .zip(self.original_tag_sets.iter())
            .any(|(current, original)| current != original)
    }

    /// Whether the current file's specific index has changes.
    pub fn has_changes_for_file(&self, idx: usize) -> bool {
        match (self.tag_sets.get(idx), self.original_tag_sets.get(idx)) {
            (Some(current), Some(original)) => current != original,
            _ => false,
        }
    }

    // ========================================================================
    // File navigation
    // ========================================================================

    /// Navigate to the next file (Individual mode).
    pub fn next_file(&mut self) {
        if self.current_file + 1 < self.files.len() {
            self.current_file += 1;
            self.form.reset();
        }
    }

    /// Navigate to the previous file (Individual mode).
    pub fn prev_file(&mut self) {
        if self.current_file > 0 {
            self.current_file -= 1;
            self.form.reset();
        }
    }

    /// Revert the current file to its original tags.
    pub fn revert_current(&mut self) {
        if let Some(original) = self.original_tag_sets.get(self.current_file) {
            if let Some(current) = self.tag_sets.get_mut(self.current_file) {
                *current = original.clone();
            }
        }
        self.form.reset();
    }

    /// Build a stable decision key item from mutations.
    pub fn decision_key_item(&self, mutations: &[Mutation]) -> String {
        let mut inodes = BTreeSet::new();
        let mut tag_names = BTreeSet::new();

        for m in mutations {
            if let Mutation::ApplyTagOps(ref atm) = m {
                for op in &atm.ops {
                    inodes.insert(op.inode);
                    tag_names.insert(op.tag_name.to_uppercase());
                }
            }
        }

        let inode_part: String = inodes
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let tags_part: String = tag_names.into_iter().collect::<Vec<_>>().join(",");

        if tags_part.is_empty() {
            inode_part
        } else {
            format!("{}:{}", inode_part, tags_part)
        }
    }

    /// The button context for the current state.
    pub fn button_ctx(&self) -> TagEditorButtonCtx {
        TagEditorButtonCtx {
            has_changes: self.has_changes(),
            staged_count: self.staged_decision_count,
        }
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    fn ops_to_mutations(ops: Vec<TagOp>, zone: Zone) -> Vec<Mutation> {
        let ops: Vec<TagOp> = ops.into_iter().filter(|op| !op.is_nop()).collect();
        if ops.is_empty() {
            Vec::new()
        } else {
            vec![Mutation::ApplyTagOps(ApplyTagOpsMutation { ops, zone })]
        }
    }

    fn has_aggregated_changes(&self) -> bool {
        self.aggregated
            .as_ref()
            .map(|agg| {
                agg.entries().iter().any(|e| {
                    matches!(e.state, crate::tag_set::AggregatedState::Edited(_))
                })
            })
            .unwrap_or(false)
    }
}

impl std::fmt::Debug for TagEditorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TagEditorState")
            .field("mode", &self.mode)
            .field("current_file", &self.current_file)
            .field("files_count", &self.files.len())
            .field("focus", &self.focus)
            .field("staged_decision_count", &self.staged_decision_count)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_files(count: usize) -> Vec<AudioFileInfo> {
        (0..count)
            .map(|i| AudioFileInfo {
                inode: 100 + i as i64,
                zone: Zone::Corpus,
                path: format!("track{}.flac", i + 1),
            })
            .collect()
    }

    fn make_tag_sets(count: usize) -> Vec<TagSet> {
        (0..count)
            .map(|i| {
                TagSet::from_pairs(vec![
                    ("ARTIST".into(), "Bach".into()),
                    ("TITLE".into(), format!("Track {}", i + 1)),
                    ("ALBUM".into(), "WTC".into()),
                ])
            })
            .collect()
    }

    #[test]
    fn new_individual_mode() {
        let state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(3),
            make_tag_sets(3),
        );
        assert_eq!(state.files.len(), 3);
        assert_eq!(state.current_file, 0);
        assert!(!state.has_changes());
        assert!(state.aggregated.is_none());
    }

    #[test]
    fn new_aggregated_mode() {
        let state = TagEditorState::new(
            TagEditorMode::Aggregated,
            make_files(3),
            make_tag_sets(3),
        );
        assert!(state.aggregated.is_some());
    }

    #[test]
    fn mutations_for_current_no_changes() {
        let state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(1),
            make_tag_sets(1),
        );
        assert!(state.mutations_for_current().is_empty());
    }

    #[test]
    fn mutations_for_current_with_changes() {
        let mut state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(1),
            make_tag_sets(1),
        );

        // Modify a tag
        state.tag_sets[0].set_value(0, 0, "Handel".into());

        let mutations = state.mutations_for_current();
        assert_eq!(mutations.len(), 1);
        if let Mutation::ApplyTagOps(ref atm) = mutations[0] {
            assert_eq!(atm.zone, Zone::Corpus);
            assert!(!atm.ops.is_empty());
        } else {
            panic!("expected ApplyTagOps");
        }
    }

    #[test]
    fn has_changes_detects_edits() {
        let mut state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(2),
            make_tag_sets(2),
        );
        assert!(!state.has_changes());
        assert!(!state.has_changes_any());

        state.tag_sets[0].set_value(0, 0, "Handel".into());
        assert!(state.has_changes()); // current_file=0 has changes
        assert!(state.has_changes_any());

        state.next_file();
        assert!(!state.has_changes()); // current_file=1 has no changes
        assert!(state.has_changes_any()); // but file 0 still does
    }

    #[test]
    fn file_navigation() {
        let mut state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(3),
            make_tag_sets(3),
        );
        assert_eq!(state.current_file, 0);

        state.next_file();
        assert_eq!(state.current_file, 1);

        state.next_file();
        assert_eq!(state.current_file, 2);

        // Can't go past end
        state.next_file();
        assert_eq!(state.current_file, 2);

        state.prev_file();
        assert_eq!(state.current_file, 1);

        state.prev_file();
        assert_eq!(state.current_file, 0);

        // Can't go before start
        state.prev_file();
        assert_eq!(state.current_file, 0);
    }

    #[test]
    fn revert_current() {
        let mut state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(1),
            make_tag_sets(1),
        );

        state.tag_sets[0].set_value(0, 0, "Handel".into());
        assert!(state.has_changes());

        state.revert_current();
        assert!(!state.has_changes());
    }

    #[test]
    fn decision_key_item_stable() {
        let state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(1),
            make_tag_sets(1),
        );

        let mut modified = state.tag_sets.clone();
        modified[0].set_value(0, 0, "Handel".into());

        // Build a fake mutation set to test key generation
        let ops = vec![
            TagOp::replace_tag(100, "ARTIST", "Bach", "Handel"),
        ];
        let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops,
            zone: Zone::Corpus,
        })];

        let key = state.decision_key_item(&mutations);
        assert_eq!(key, "100:ARTIST");
    }

    #[test]
    fn button_ctx_reflects_state() {
        let state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(1),
            make_tag_sets(1),
        );

        let ctx = state.button_ctx();
        assert!(!ctx.has_changes);
        assert_eq!(ctx.staged_count, 0);

        // ReviewAll and Revert disabled when no changes
        assert!(!TagEditorButton::ReviewAll.enabled(&ctx));
        assert!(!TagEditorButton::Revert.enabled(&ctx));
        assert!(TagEditorButton::Cancel.enabled(&ctx));
    }

    #[test]
    fn mutations_for_all_individual() {
        let mut state = TagEditorState::new(
            TagEditorMode::Individual,
            make_files(2),
            make_tag_sets(2),
        );

        // Modify both files
        state.tag_sets[0].set_value(0, 0, "Handel".into());
        state.tag_sets[1].set_value(0, 0, "Vivaldi".into());

        let mutations = state.mutations_for_all();
        assert_eq!(mutations.len(), 1);
        if let Mutation::ApplyTagOps(ref atm) = mutations[0] {
            // Should have ops for both files
            let inodes: BTreeSet<i64> = atm.ops.iter().map(|op| op.inode).collect();
            assert!(inodes.contains(&100));
            assert!(inodes.contains(&101));
        }
    }
}
