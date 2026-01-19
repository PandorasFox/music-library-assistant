# Tag Editor Refactoring Plan (Synthesized)

## Implementation Progress

| Phase | Status | Notes |
|-------|--------|-------|
| Phase 0 | COMPLETE | Direct queueing removed from daemon.rs |
| Phase 1 | COMPLETE | Unified types added to types.rs |
| Phase 2 | COMPLETE | `UnifiedTagEditorState` created in state.rs |
| Phase 3 | COMPLETE | Input handling with transaction actions |
| Phase 4 | STUB | Rendering helpers added, full impl pending |
| Phase 5 | PENDING | UI integration with transaction lifecycle |
| Phase 6 | PENDING | Cleanup of legacy files |

---

This plan reconciles the original `tag_editor_plan.md` and `UI_ARCHITECTURE_PLAN.md` with the current codebase state after major cleanup efforts.

## Current State Assessment

### What's Already Implemented

1. **TaskDaemon with DecisionWitness pattern** (`src/daemon.rs:108-172`)
   - `confirm_decision()` creates sealed `DecisionWitness` tokens
   - Mutations require witness to be queued
   - `MutationExecutionWitness` for actual execution context

2. **Two mutation queueing interfaces exist** (PROBLEM):

   **Direct queueing** (`src/daemon.rs:758-828`) - SHOULD BE REMOVED:
   - `queue()`, `queue_with_label()`, `queue_all()`, `queue_all_with_label()`
   - Immediately queues mutations with witness
   - Bypasses transaction accumulation
   - Promotes bad pattern of witness-per-action

   **Transaction interface** (`src/daemon.rs:952-1099`) - CORRECT PATTERN:
   - `start_transaction(label)` - begin accumulating decisions
   - `add_decision(idx, witness, label, mutations)` - stage decision to transaction
   - `confirm_transaction(witness)` - commit all at once
   - `discard_transaction(witness)` - abort all

3. **Dual tag editor implementations** (still separate)
   - `TagEditorState` (569 lines) - single-file/duplicate-workflow editing
   - `DirectoryTagEditorState` (628 lines) - bulk directory editing
   - ~1,300 lines of duplicated logic between them

4. **Broken integration point** (`src/ui/mod.rs:1039-1044`)
   - `todo!("Reconnect: Tag editor save needs DecisionWitness")`
   - Mutations are computed but not queued

### What the Original Plans Proposed

| Plan | Focus | Scope |
|------|-------|-------|
| `tag_editor_plan.md` | Unify dual implementations into `TagEditorModal` | Tag editor only |
| `UI_ARCHITECTURE_PLAN.md` | Full View Context/View Modal separation | Entire UI layer |

The UI architecture plan is more ambitious and should be deferred. This plan focuses on **tag editor only**.

---

## Key Architectural Decision: Transaction-Based Flow

**All tag editor saves must use the transaction interface, not direct queueing.**

Flow for multi-file/multi-group editing:
```
Enter tag editor → daemon.start_transaction("Tag edits")
   ↓
Edit file 1, confirm → daemon.add_decision(0, witness, "Track A", mutations)
   ↓
Tab to file 2, edit, confirm → daemon.add_decision(1, witness, "Track B", mutations)
   ↓
... repeat for N files/groups ...
   ↓
Final review screen → show all staged decisions
   ↓
Commit → daemon.confirm_transaction(witness) → all mutations execute
   OR
Discard → daemon.discard_transaction(witness) → nothing happens
```

**Witness is created at each `add_decision()` call (user pressed Enter to confirm that edit) AND at final `confirm_transaction()` (user committed the batch).**

---

## Implementation Phases

### Phase 0: Remove Direct Mutation Queueing Interface

**Goal**: Force all mutations through transactions. Remove temptation to bypass.

**Files**:
- `src/daemon.rs`

**Changes**:
1. Make direct queueing methods `pub(crate)` or remove entirely:
   - `queue()` → remove or make internal
   - `queue_with_label()` → remove or make internal
   - `queue_all()` → remove or make internal
   - `queue_all_with_label()` → remove or make internal
2. `queue_mutations_internal()` stays (used by `confirm_transaction()`)
3. Update any callers to use transaction interface

**Verification**: `cargo build` - any external callers will fail to compile

---

### Phase 1: Define Unified Types (`types.rs`)

Add new types alongside existing ones:

```rust
/// Source flow that spawned the tag editor (for navigation context)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagEditorSource {
    CorpusBrowser,           // Single file from browser
    DirectoryEdit,           // Bulk edit from directory selection
    DuplicateResolution,     // From duplicate detection flow
    DeployConflict,          // From deploy conflict resolution
}

/// Context for tag editing - determines mode and available features
pub enum TagEditContext {
    /// Single file editing with signals display
    SingleFile {
        track: Track,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    },
    /// Bulk editing with aggregated values
    BulkEdit {
        tracks: Vec<Track>,
        source: TagEditorSource,
        group_context: Option<GroupContext>,
    },
}

/// Group context for multi-step workflows
pub struct GroupContext {
    pub source: TagEditorSource,
    pub group_index: usize,
    pub total_groups: usize,
}

/// Unified focus enum
pub enum TagEditorPaneFocus {
    ContextList,  // Left pane (tracks or files)
    TagFields,    // Center pane (default)
    Actions,      // Right pane
}

/// Action buttons
pub enum TagEditorButton {
    Confirm,
    DropChanges,
    FillFromDisk,
}

/// Unified action result - transaction-based
pub enum UnifiedTagEditorAction {
    None,
    /// Stage mutations for current item to the active transaction
    StageDecision { index: usize, mutations: Vec<Mutation> },
    /// Commit all staged decisions and exit
    CommitTransaction,
    /// Discard all staged decisions and exit
    DiscardTransaction,
    /// Navigate to next item (within current transaction)
    NextItem,
    /// Navigate to previous item
    PrevItem,
    /// Show an overlay modal (change preview, unsaved warning, etc.)
    ShowModal(TagEditorModalType),
    /// Display a status message
    StatusMessage(String),
}

/// Modal dialogs
pub enum TagEditorModalType {
    /// Preview changes for current item before staging
    ChangePreview { changes: Vec<GroupedChange>, scroll: usize },
    /// Warn about unsaved changes when navigating away
    UnsavedChanges { destination: TagEditorDestination },
    /// Review all staged decisions before final commit
    TransactionReview { decisions: Vec<(usize, String, usize)>, scroll: usize },
    // (index, label, mutation_count)
}
```

**Keep existing**: `TagField`, `AggregatedTagField`, `AggregatedValue`, `FieldEditState`, `TagChange`, `GroupedChange`, `GatheringState`, `GatheringMessage`

---

### Phase 2: Unified State (`state.rs`)

Create `TagEditorModal` struct that handles both contexts:

```rust
pub struct TagEditorModal {
    // Context
    pub context: TagEditContext,

    // Navigation
    pub current_field_idx: usize,
    pub current_track_idx: usize,  // BulkEdit: which file; SingleFile: which track in group
    pub field_scroll_offset: usize,
    pub track_scroll_offset: usize,

    // Editing state
    pub field_edit_state: FieldEditState,
    pub name_buffer: String,
    pub value_buffer: String,
    pub various_confirm_state: Option<VariousConfirmState>,  // BulkEdit only

    // Tag data (per-track)
    pub tag_fields: Vec<Vec<TagField>>,       // Current state
    pub original_fields: Vec<Vec<TagField>>,  // For change detection

    // UI state
    pub focus: TagEditorPaneFocus,
    pub selected_button: TagEditorButton,
    pub visible_height: usize,

    // Signals (SingleFile context)
    pub signals: Vec<HealthIssue>,

    // Async gathering (BulkEdit from directory)
    pub gathering_state: Option<GatheringState>,

    // Active modal dialog
    pub modal: Option<TagEditorModalType>,
}
```

**Constructors**:
- `TagEditorModal::single_file(track, source, group_context)` - from corpus browser or duplicate flow
- `TagEditorModal::bulk_from_directory(path, source)` - starts async gathering
- `TagEditorModal::bulk_from_tracks(tracks, source, group_context)` - pre-loaded tracks

**Key Methods**:
- `fn has_changes(&self) -> bool` - compare original vs current
- `fn compute_changes(&self) -> Vec<GroupedChange>` - for preview
- `fn generate_mutations(&self) -> Vec<Mutation>` - create `TagEditAndFlush` mutations
- `fn drop_changes(&mut self)` - revert to original
- `fn fill_from_disk(&mut self)` - reload metadata from files
- `fn load_signals(&mut self, db: &Database)` - fetch health issues for track(s)

---

### Phase 3: Unified Input (`input.rs`)

Single `handle_key()` that dispatches based on focus:

```rust
impl TagEditorModal {
    pub fn handle_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        // Handle modal first if active
        if let Some(ref mut modal) = self.modal {
            return self.handle_modal_key(key);
        }

        match self.focus {
            TagEditorPaneFocus::ContextList => self.handle_context_list_key(key),
            TagEditorPaneFocus::TagFields => self.handle_tag_fields_key(key),
            TagEditorPaneFocus::Actions => self.handle_actions_key(key),
        }
    }

    fn handle_actions_key(&mut self, key: KeyEvent) -> UnifiedTagEditorAction {
        match key.code {
            KeyCode::Enter => match self.selected_button {
                TagEditorButton::Confirm => {
                    // Show change preview first
                    let changes = self.compute_changes();
                    if changes.is_empty() {
                        UnifiedTagEditorAction::StatusMessage("No changes to save".into())
                    } else {
                        self.modal = Some(TagEditorModalType::ChangePreview {
                            changes, scroll: 0
                        });
                        UnifiedTagEditorAction::None
                    }
                }
                TagEditorButton::DropChanges => {
                    self.drop_changes();
                    UnifiedTagEditorAction::StatusMessage("Changes dropped".into())
                }
                TagEditorButton::FillFromDisk => {
                    self.fill_from_disk();
                    UnifiedTagEditorAction::StatusMessage("Reloaded from disk".into())
                }
            }
            // ... navigation
        }
    }
}
```

**Key bindings** (preserve existing patterns):
- Up/Down: Field navigation
- Left/Right: Focus toggle (name/value in edit mode, pane switch otherwise)
- Tab/Shift+Tab: Item navigation within transaction (next/prev file or group)
- Enter: On Actions pane - execute selected button
- Enter: On Confirm button - show ChangePreview, then stage to transaction
- Esc: Cancel current edit OR show UnsavedChanges warning OR exit (with DiscardTransaction if staged decisions exist)
- Ctrl+U: Clear field
- Ctrl+R: Show TransactionReview (staged decisions summary)

**Action button behavior**:
- **Confirm**: Show change preview → if user confirms → `StageDecision` → auto-advance to next item
- **Drop Changes**: Revert current item to original (no mutation staged)
- **Fill from Disk**: Reload tags from file, stage update mutations - HUMAN NOTE: this should be dynamic based on the "out of band tag changes detected" signal for the relevant track,
and should have an accompanying "fill from db" button that fills tag fields with the db values. 

---

### Phase 4: Unified Rendering (`render.rs`)

Three-pane layout with context-aware content:

```
┌─────────────────────────────────────────────────────────────────┐
│ [Context Info] Root dir | File info | Signals                   │
├──────────────┬────────────────────────────────┬─────────────────┤
│  Context     │     Tag Fields                 │  Actions        │
│  List        │                                │                 │
│  (30%)       │  artist: [value]    ✎         │  [Confirm]      │
│              │  album: [value]               │  Drop Changes   │
│  track1.flac │  title: [value]               │  Fill from Disk │
│> track2.flac │  ...                          │                 │
│  track3.flac │                                │                 │
├──────────────┴────────────────────────────────┴─────────────────┤
│ [Controls: ↑↓ Navigate  ←→ Focus  Enter Confirm  Esc Exit]      │
└─────────────────────────────────────────────────────────────────┘
```

**Info pane** (top, three columns matching pane widths):
- Column 1 (30%): Group stats - root dir, file count, group progress (e.g., "2 of 5")
- Column 2 (55%): File info - path (relative to root), duration, bitrate, format
- Column 3 (15%): Signals summary - count by severity

**Context-specific rendering**:
- SingleFile: Context list shows tracks in duplicate group (or single track)
- BulkEdit: Context list shows files in directory

**Signals pane** (SingleFile only):
- Surface `HealthIssue` records for current track
- Icon + color by type: `OutOfBandTagChange` (yellow !), `MissingTag` (red ?), `TagCanonical` (cyan ~)

---

### Phase 5: Integration (`src/ui/mod.rs`)

Replace dual state fields:

```rust
// Before:
tag_editor: Option<tag_editor::TagEditorState>,
tag_editor_modal: Option<tag_editor::TagEditorModal>,  // note: this was the dialog modal
directory_tag_editor: Option<tag_editor::DirectoryTagEditorState>,
directory_tag_editor_modal: Option<tag_editor::types::DirectoryTagEditorModal>,

// After:
tag_editor: Option<tag_editor::TagEditorModal>,  // unified modal state
```

**Transaction lifecycle in UI**:

```rust
// On entering tag editor (any entry point)
fn open_tag_editor(&mut self, context: TagEditContext) {
    // Start transaction for this editing session
    if let Some(daemon) = self.task_daemon.as_mut() {
        let label = match &context {
            TagEditContext::SingleFile { .. } => "Tag edits",
            TagEditContext::BulkEdit { .. } => "Bulk tag edits",
        };
        let _ = daemon.start_transaction(label);
    }

    self.tag_editor = Some(TagEditorModal::new(context));
    self.mode = UiMode::TagEditor;
}
```

**Action handling with transaction flow**:

```rust
fn handle_tag_editor_action(&mut self, action: UnifiedTagEditorAction) {
    match action {
        UnifiedTagEditorAction::StageDecision { index, mutations } => {
            // User confirmed changes for THIS item - stage to transaction
            let witness = crate::daemon::confirm_decision();
            if let Some(daemon) = self.task_daemon.as_mut() {
                let label = self.tag_editor.as_ref()
                    .map(|e| e.current_item_label())
                    .unwrap_or_else(|| "Tag edit".to_string());
                let _ = daemon.add_decision(index, &witness, label, mutations);
            }
            // Stay in editor, advance to next item
            self.tag_editor_next_item();
        }

        UnifiedTagEditorAction::CommitTransaction => {
            // User is done editing all items - show review, then commit
            let witness = crate::daemon::confirm_decision();
            if let Some(daemon) = self.task_daemon.as_mut() {
                match daemon.confirm_transaction(&witness) {
                    Ok(summary) => {
                        self.status_message = Some(format!(
                            "Committed {} decisions ({} mutations)",
                            summary.decision_count,
                            summary.mutation_count
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Commit failed: {}", e));
                    }
                }
            }
            self.tag_editor = None;
            self.mode = UiMode::Insights;
        }

        UnifiedTagEditorAction::DiscardTransaction => {
            // User aborted - discard all staged decisions
            let witness = crate::daemon::confirm_decision();
            if let Some(daemon) = self.task_daemon.as_mut() {
                let _ = daemon.discard_transaction(&witness);
            }
            self.tag_editor = None;
            self.mode = UiMode::Insights;
            self.status_message = Some("Edits discarded".to_string());
        }

        UnifiedTagEditorAction::NextItem => self.tag_editor_next_item(),
        UnifiedTagEditorAction::PrevItem => self.tag_editor_prev_item(),
        UnifiedTagEditorAction::ShowModal(modal) => {
            if let Some(ref mut editor) = self.tag_editor {
                editor.modal = Some(modal);
            }
        }
        UnifiedTagEditorAction::StatusMessage(msg) => {
            self.status_message = Some(msg);
        }
        UnifiedTagEditorAction::None => {}
    }
}
```

**Review screen before commit**:

When user reaches the end of items (or presses a "Review" hotkey), show a summary modal:
- List of all staged decisions with item labels
- Per-decision mutation count
- Buttons: [Commit All] [Discard All] [Back to Editing]

Entry points updated:
- Corpus browser "Edit Tags" → `open_tag_editor(TagEditContext::SingleFile { ... })`
- Directory selector "Edit Tags" → `open_tag_editor(TagEditContext::BulkEdit { ... })`
- Duplicate resolution flow → `open_tag_editor(TagEditContext::SingleFile { ..., group_context })`

---

### Phase 6: Cleanup

**Delete**:
- `src/ui/tag_editor/directory_state.rs` (628 lines)
- `src/ui/tag_editor/directory_input.rs` (132 lines)
- `src/ui/tag_editor/directory_render.rs` (506 lines)

**Update**:
- `src/ui/tag_editor/mod.rs` - remove directory_* exports, export unified `TagEditorModal`
- `docs/FUTURE_FEATURES.md` - remove tag editor items, update status
- `CLAUDE.md` - update module responsibilities

---

## Files to Modify

| File | Action | Notes |
|------|--------|-------|
| `src/daemon.rs` | Modify | Phase 0 (remove direct queueing interface) |
| `src/ui/mod.rs` | Modify | Phase 5 (transaction-based integration) |
| `src/ui/tag_editor/types.rs` | Modify | Phase 1 (add unified types) |
| `src/ui/tag_editor/state.rs` | Rewrite | Phase 2 (unified `TagEditorModal`) |
| `src/ui/tag_editor/input.rs` | Rewrite | Phase 3 (unified input) |
| `src/ui/tag_editor/render.rs` | Rewrite | Phase 4 (unified render) |
| `src/ui/tag_editor/mod.rs` | Modify | Phase 6 (update exports) |

## Files to Delete

| File | Lines | Phase |
|------|-------|-------|
| `src/ui/tag_editor/directory_state.rs` | 628 | Phase 6 |
| `src/ui/tag_editor/directory_input.rs` | 132 | Phase 6 |
| `src/ui/tag_editor/directory_render.rs` | 506 | Phase 6 |

**Total deletion**: 1,266 lines

---

## Verification Plan

1. **Build**: `cargo build` after each phase
2. **Phase 0 test**: Verify direct queue methods are inaccessible externally, `cargo build` catches any callers
3. **Phase 2 test**: Construct `TagEditorModal` in both contexts, verify state initialization
4. **Phase 4 test**: Visual verification of unified rendering in both contexts
5. **Phase 5 test**:
   - Enter tag editor (single file or bulk)
   - Edit multiple items, confirm each (verify decisions staged to transaction)
   - Press Ctrl+R to see transaction review
   - Commit → verify all mutations execute
   - Test discard path → verify nothing executes
6. **Phase 6 test**: Verify no regressions after cleanup

---

## Deferred Work

The following items from `UI_ARCHITECTURE_PLAN.md` are **not** included in this plan:

1. **View Context / View Modal separation** - broader architectural change affecting all UI modes
2. **Signal → Decision Groups pipeline** - requires collation logic and broader signal infrastructure
3. **Input category masking** - nice-to-have for overlay modal input handling

These can be addressed in a future refactoring pass after the tag editor unification is complete.

**Note**: Transaction flow with multi-item review IS included in this plan (see Phase 5).
