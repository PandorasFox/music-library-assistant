# Tag Editor Refactoring Plan

## Goal
Refactor the tag editor into a unified, reusable modal component that:
- Works in both single-file and bulk-edit contexts
- Surfaces health signals for the file being edited
- Models changes as mutations dispatched immediately (with future support for resolution flow accumulation)
- Provides clear action buttons: Confirm, Drop Changes, Fill from Disk

## Current State
- Two separate implementations: `TagEditorState` (track-based) and `DirectoryTagEditorState`
- Significant code duplication between the two
- No signal surfacing in the tag editor
- Changes converted to `Mutation::TagEditAndFlush` and queued to daemon

## Implementation Phases

### Phase 1: Define Unified Types (`types.rs`)

Add new types while preserving existing ones:

```rust
/// Group context for returning to parent flows
pub struct GroupContext {
    pub source_flow: String,      // "deploy_conflict", "duplicate_resolution", etc.
	// HUMAN NOTE: ^ source_flow STRING???? enum that. Enum that now. TODO check for other instances of this bad pattern to correct elsewhere.
    pub group_index: usize,
    pub total_groups: usize,
    pub group_id: Option<i64>,
}

/// Context for tag editing - determines mode and behavior
pub enum TagEditContext {
    /// Single file editing with full signal display
    SingleFile {
        track: Track,
        parent_group: Option<GroupContext>,
    },
    /// Bulk editing with aggregated values
    BulkEdit {
        files: Vec<Track>,
        aggregated_fields: Vec<AggregatedTagField>,
        parent_group: Option<GroupContext>,
    },
}

/// Action buttons in the actions pane
pub enum TagEditorButton {
    ConfirmChanges,
    DropChanges,
    FillFromDisk,
}

/// Unified focus enum
pub enum TagEditorPaneFocus {
    FileList,    // BulkEdit only
    TagFields,   // Default
    Actions,
    Signals,     // SingleFile only
}

/// Unified action result
pub enum UnifiedTagEditorAction {
    None,
    ShowModal(TagEditorModal),
    DispatchMutations(Vec<Mutation>),
    Exit,
    StatusMessage(String),
    NextGroup,
    PrevGroup,
}

/// Snapshot for change detection
pub enum FieldSnapshot {
    Single(Vec<TagField>),
    Aggregated(Vec<AggregatedTagField>),
}
```

Keep existing: `TagField`, `AggregatedTagField`, `AggregatedValue`, `FieldEditState`, `TagChange`, `GroupedChange`, `GatheringMessage`, `GatheringState`

### Phase 2: Unified State (`state.rs`)

Replace `TagEditorState` + `DirectoryTagEditorState` with single `TagEditorModal`:

```rust
pub struct TagEditorModal {
    // Context
    pub context: TagEditContext,

    // Navigation
    pub current_field_idx: usize,
    pub current_track_idx: usize,
    pub field_scroll_offset: usize,
    pub track_scroll_offset: usize,

    // Editing
    pub field_edit_state: FieldEditState,
    pub value_buffer: String,
    pub various_confirm_state: Option<VariousConfirmState>,

    // Change tracking
    pub original_fields: FieldSnapshot,
    pub current_fields: FieldSnapshot,

    // UI state
    pub focus: TagEditorPaneFocus,
    pub selected_button: TagEditorButton,

    // Signals (SingleFile mode)
    pub signals: Vec<HealthIssue>,

    // Async gathering (BulkEdit from directory)
    pub gathering_state: Option<GatheringState>,
}
```

**Constructors:**
- `TagEditorModal::single_file(track, parent_group)` - for single file editing
- `TagEditorModal::bulk_from_directory(path, parent_group)` - starts async gathering (HUMAN NOTE: async? we still sync on that before rendering UI, correct? We maybe want to just fetch from DB and collate, while dispatching Tasks to check tags in-file to check signals freshness before we potentially do writes)
- `TagEditorModal::bulk_from_tracks(tracks, parent_group)` - from pre-loaded tracks

**Key Methods:**
- `load_signals(&mut self, db: &Database)` - fetch `HealthIssue` records for track
- `has_changes() -> bool` - compare original vs current
- `generate_mutations() -> Vec<Mutation>` - create `TagEditAndFlush` mutations
- `drop_changes()` - revert to original
- `fill_from_disk()` - reload from disk, update original

### Phase 3: Unified Input (`input.rs`)

Single `handle_key()` that dispatches based on focus:
- `TagFields` - field navigation, editing (existing logic)
- `Actions` - button navigation and execution
- `ContextList` - list of other contexts to tab/shift-tab through (can be other directories to switch to in bulk edit context)

Button actions:
- **ConfirmChanges** → `generate_mutations()` → `UnifiedTagEditorAction::DispatchMutations(mutations)`
- **DropChanges** → `drop_changes()` → status message
- **FillFromDisk** → `fill_from_disk()` → status message (generates db update mutations)

### Phase 4: Unified Rendering (`render.rs`)

Context-aware three-pane layout:

**SingleFile mode:**
```
┌─────────────┬───────────────────────┬──────────┐
│  Contexts   │     Tag Fields        │ Actions  │
│  (30%)      │     (55%)             │ (15%)    │
│             │                       │          │
│             │ artist: [value]       │ Confirm  │
│             │ album: [value]        │ Drop     │
│             │ ...                   │ Fill     │
└─────────────┴───────────────────────┴──────────┘
```

**BulkEdit mode:**
```
┌─────────────┬───────────────────────┬──────────┐
│  Contexts   │  Aggregated Tags      │ Actions  │
│  (30%)      │  (55%)                │ (15%)    │
│             │                       │          │
│ track1.flac │ artist: (various)     │ Confirm  │
│ track2.flac │ album: [value]        │ Drop     │
│ ...         │ ...                   │ Fill     │
└─────────────┴───────────────────────┴──────────┘
```

Signals pane shows filtered `HealthIssue` types:
- `OutOfBandTagChange` - "!" yellow
- `MissingTag` - "?" red
- `TagCanonical` - "~" cyan

There's currently one generic info pane above the tag editor. It should be split in three, matching the panes under it in sizing (30-55-15):
- group stats (root dir, # files (or dirs, in bulk edit contexts), filetype counts if applicable)
- file info: keep this as it is, just shorten the path to be the part past the 'root dir' in group stats. lots of good info here!
- signals pane: where we surface the signals. bulkedit context will be TODO for what insights we pull in later.

### Phase 5: Integration (`ui/mod.rs`)

Replace dual state fields:
```rust
// Before:
tag_editor: Option<TagEditorState>,
directory_tag_editor: Option<DirectoryTagEditorState>,

// After:
tag_editor_modal: Option<TagEditorModal>,
```

Update action handling:
```rust
fn handle_tag_editor_modal_action(&mut self, action: UnifiedTagEditorAction) {
    match action {
        UnifiedTagEditorAction::DispatchMutations(mutations) => {
            self.daemon().queue_all(mutations);
            // Handle group navigation if in workflow
            // Otherwise exit to Insights
        }
        // ... other actions
    }
}
```

Note: the daemon queueing interface is not yet finalized; these tasks will need a label to go with them by implementation time - these mutations should be labelled as "tag edit decisions".

### Phase 6: Cleanup

**Delete:**
- `src/ui/tag_editor/directory_state.rs`
- `src/ui/tag_editor/directory_input.rs`
- `src/ui/tag_editor/directory_render.rs`

**Update mod.rs exports:**
```rust
pub use state::TagEditorModal;
pub use types::{
    TagEditContext, GroupContext, TagEditorButton,
    UnifiedTagEditorAction, TagField, AggregatedTagField,
    // ...
};
```

## Files to Modify

| File | Changes |
|------|---------|
| `src/ui/tag_editor/types.rs` | Add `TagEditContext`, `GroupContext`, `TagEditorButton`, `TagEditorPaneFocus`, `UnifiedTagEditorAction`, `FieldSnapshot` |
| `src/ui/tag_editor/state.rs` | Replace with unified `TagEditorModal` struct and methods |
| `src/ui/tag_editor/input.rs` | Unified input handling with button actions |
| `src/ui/tag_editor/render.rs` | Context-aware rendering (signals vs file list pane) |
| `src/ui/tag_editor/mod.rs` | Update exports, remove directory_* |
| `src/ui/mod.rs` | Replace dual state with single modal, update action handling |

## Files to Delete

- `src/ui/tag_editor/directory_state.rs`
- `src/ui/tag_editor/directory_input.rs`
- `src/ui/tag_editor/directory_render.rs`

## Verification

1. **Build**: `cargo build` - ensure compilation
2. **Single file edit**: Navigate to file in corpus browser → open tag editor → verify signals pane shows relevant issues
3. **Bulk edit**: Navigate to directory → open tag editor → verify aggregated values display
4. **Confirm changes**: Edit tags → Confirm → verify mutations queued to daemon
5. **Drop changes**: Edit tags → Drop → verify reverted to original
6. **Fill from disk**: (if OOB signal present) → Fill → verify db updated from disk

## Future Considerations

- Resolution flow contexts for decision accumulation (not immediate dispatch)
- Signal aggregation in BulkEdit mode
- Canonicalization suggestions inline with tag editing
