# UI Architecture Refactoring Plan

This document outlines the plan to refactor MLA's UI module into a cleaner architecture with proper separation of concerns between View Context (routing) and View Modals (data/logic).

## Goals

1. **Clean tick separation**: Input → Update → Render lifecycle
2. **Decision Witness at keypress**: Witnesses created at user confirmation
3. **View Context / View Modal separation**: Routing vs data handling
4. **Signals → Decision Groups pipeline**: Transform signal collections into actionable groups
5. **Transaction-backed decisions**: Daemon holds pending decisions, UI drives

## Current Problems

1. **Giant App struct** (`ui/mod.rs`, 2000+ lines) - all state and logic in one place
2. **Scattered input handling** - each mode has its own key handler pattern
3. **Witness created far from keypress** - semantic guarantee is weak
4. **Dual tag editor implementations** - `TagEditorState` + `DirectoryTagEditorState`
5. **Views hold pending decisions** - should be in daemon transaction

---

## Architecture Overview

### Two-Layer UI Model

```
┌─────────────────────────────────────────────────────────────────────┐
│                         View Context                                 │
│  (Top-level routing: which modal, which signals, which group idx)   │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│   active_modal: ModalType     signals: Vec<Signal>     group_idx: N │
│                                                                      │
│         │                           │                        │       │
│         ▼                           ▼                        ▼       │
│   ┌──────────────────────────────────────────────────────────────┐  │
│   │                        View Modal                             │  │
│   │  (Data model: tag fields, selection state, edit buffers)      │  │
│   │                                                               │  │
│   │  - Receives group_idx from View Context                       │  │
│   │  - Fetches Decision from daemon transaction (if exists)       │  │
│   │  - Combines signals with logic → decision groups              │  │
│   │  - Drives mutation collection on operator decisions           │  │
│   └──────────────────────────────────────────────────────────────┘  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Data Flow

```
Signals (from DB)
       │
       ▼
┌──────────────────┐
│  View Context    │───▶ selects which modal, passes signals + group_idx
└──────────────────┘
       │
       ▼
┌──────────────────┐
│   View Modal     │───▶ transforms signals → decision groups
└──────────────────┘     manages edit state, collects mutations
       │
       │ on confirm (DecisionWitness)
       ▼
┌──────────────────┐
│  Daemon Txn      │───▶ holds pending decisions by group idx
└──────────────────┘
       │
       │ on commit (DecisionWitness)
       ▼
┌──────────────────┐
│  Mutation Queue  │───▶ executes mutations
└──────────────────┘
```

---

## Component Designs

### 1. View Context

The View Context is the top-level routing structure. It determines:
- Which modal is active (corpus browser, insights, tag editor, decision flow)
- Which signals are being prioritized
- Which decision group index is active

```rust
// In ui/context.rs

/// Top-level modal types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalType {
    Insights,
    CorpusBrowser,
    TagEditor,
    DeployPreview,
    DecisionFlow,
}

/// The View Context - routing state for the UI
#[derive(Debug)]
pub struct ViewContext {
    /// Which top-level modal is active
    pub active_modal: ModalType,

    /// Signals being prioritized in this context
    pub signals: Vec<Signal>,

    /// Current decision group index (for multi-group flows)
    pub group_idx: usize,

    /// Total groups (for progress display: "3 of 10")
    pub group_total: usize,
}

impl ViewContext {
    pub fn new(modal: ModalType) -> Self {
        Self {
            active_modal: modal,
            signals: Vec::new(),
            group_idx: 0,
            group_total: 0,
        }
    }

    /// Initialize with signals and compute group count
    pub fn with_signals(mut self, signals: Vec<Signal>) -> Self {
        self.group_total = compute_group_count(&signals);
        self.signals = signals;
        self
    }

    /// Advance to next group
    pub fn next_group(&mut self) -> bool {
        if self.group_idx + 1 < self.group_total {
            self.group_idx += 1;
            true
        } else {
            false
        }
    }

    /// Go to previous group
    pub fn prev_group(&mut self) -> bool {
        if self.group_idx > 0 {
            self.group_idx -= 1;
            true
        } else {
            false
        }
    }
}

/// Compute how many decision groups these signals produce
fn compute_group_count(signals: &[Signal]) -> usize {
    // Implementation depends on signal type
    // e.g., deploy conflicts → one group per conflict
    // e.g., duplicates → one group per duplicate set
    signals.len().max(1)
}
```

### 2. View Modal Trait Pattern

Each view modal handles its own data state and ticks independently. The core `app_run` determines which modal(s) are active and ticks them.

```rust
// In ui/modal.rs

/// Actions a view modal can return from input/update
pub enum ModalAction {
    /// No action
    None,

    /// Stage mutations to the daemon transaction
    StageDecision {
        group_idx: usize,
        mutations: Vec<Mutation>,
    },

    /// Request to advance to next group
    NextGroup,

    /// Request to go to previous group
    PrevGroup,

    /// Request to commit the transaction
    CommitTransaction { witness: DecisionWitness },

    /// Request to discard the transaction
    DiscardTransaction { witness: DecisionWitness },

    /// Show a nested modal (e.g., change preview)
    ShowOverlay(OverlayModal),

    /// Close this modal, return to previous
    Close,

    /// Status message to display
    Status(String),
}

/// Overlay modals (confirmation dialogs, previews)
pub enum OverlayModal {
    ChangePreview {
        changes: Vec<ChangeDescription>,
        scroll: usize,
    },
    Confirmation {
        message: String,
        on_confirm: Box<dyn FnOnce() -> ModalAction>,
    },
    UnsavedChanges {
        destination: ModalType,
    },
}
```

### 3. Tag Editor Modal (Example Implementation)

The tag editor modal receives its group_idx from the View Context, fetches any existing decision from the daemon, and manages edit state.

```rust
// In ui/tag_editor/modal.rs

pub struct TagEditorModal {
    // ═══════════════════════════════════════════════════════════════
    // From View Context
    // ═══════════════════════════════════════════════════════════════

    /// Current group index (from ViewContext)
    group_idx: usize,

    /// Tracks in this group (derived from signals + group_idx)
    tracks: Vec<Track>,

    // ═══════════════════════════════════════════════════════════════
    // Edit State (local to this modal)
    // ═══════════════════════════════════════════════════════════════

    /// Tag fields for each track
    tag_fields: Vec<Vec<TagField>>,

    /// Original values (for change detection)
    original_fields: Vec<Vec<TagField>>,

    /// Current edit buffer
    edit_buffer: String,

    // ═══════════════════════════════════════════════════════════════
    // Navigation State
    // ═══════════════════════════════════════════════════════════════

    current_track_idx: usize,
    current_field_idx: usize,
    focus: TagEditorFocus,

    // ═══════════════════════════════════════════════════════════════
    // Cached Signals (refreshed on update tick)
    // ═══════════════════════════════════════════════════════════════

    signals: Vec<HealthIssue>,
}

impl TagEditorModal {
    /// Create from View Context signals and group index.
    /// Fetches existing decision from daemon if present.
    pub fn from_context(
        signals: &[Signal],
        group_idx: usize,
        daemon: &TaskDaemon,
        db: &Database,
    ) -> Self {
        // Transform signals into tracks for this group
        let tracks = signals_to_tracks_for_group(signals, group_idx, db);

        // Check if daemon has a pending decision for this group
        let existing_decision = daemon.get_decision(group_idx);

        // Build initial tag fields
        let mut tag_fields = tracks_to_tag_fields(&tracks);

        // If there's an existing decision, apply its edits to show current state
        if let Some(decision) = existing_decision {
            apply_decision_to_fields(&mut tag_fields, decision);
        }

        let original_fields = tag_fields.clone();

        Self {
            group_idx,
            tracks,
            tag_fields,
            original_fields,
            edit_buffer: String::new(),
            current_track_idx: 0,
            current_field_idx: 0,
            focus: TagEditorFocus::TagFields,
            signals: Vec::new(),
        }
    }

    /// INPUT: Handle keyboard input
    pub fn handle_input(&mut self, key: KeyEvent) -> ModalAction {
        match self.focus {
            TagEditorFocus::TagFields => self.handle_field_input(key),
            TagEditorFocus::ActionPane => self.handle_action_input(key),
        }
    }

    /// Handle input on action pane (confirm button)
    fn handle_action_input(&mut self, key: KeyEvent) -> ModalAction {
        match key.code {
            KeyCode::Enter => {
                // THIS is the decision point - create witness HERE
                let witness = crate::daemon::confirm_decision();
                let mutations = self.collect_mutations();

                ModalAction::StageDecision {
                    group_idx: self.group_idx,
                    mutations,
                }
            }
            KeyCode::Tab => ModalAction::NextGroup,
            KeyCode::BackTab => ModalAction::PrevGroup,
            KeyCode::Esc | KeyCode::Left => {
                self.focus = TagEditorFocus::TagFields;
                ModalAction::None
            }
            _ => ModalAction::None,
        }
    }

    /// UPDATE: Poll for signal changes
    pub fn update(&mut self, ctx: &UpdateContext) -> Option<ModalAction> {
        if ctx.daemon_did_work {
            self.refresh_signals(ctx.db);
        }
        None
    }

    /// RENDER: Pure rendering
    pub fn render(&self, f: &mut Frame, area: Rect, ctx: &RenderContext) {
        // Render tag editor UI
    }

    /// Collect current edits into mutations
    fn collect_mutations(&self) -> Vec<Mutation> {
        let changes = compute_changes(&self.original_fields, &self.tag_fields);
        changes_to_mutations(&changes, &self.tracks)
    }
}
```

### 4. Input Category and Masking

Input categorization for routing decisions:

```rust
// In ui/input.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputCategory {
    Navigation,      // Up/Down/Left/Right
    Confirmation,    // Enter
    Cancellation,    // Esc
    LateralCycle,    // Tab/Shift-Tab
    TextEntry,       // Characters
    Scrolling,       // PageUp/PageDown
    Deletion,        // Backspace/Delete
    Shortcut,        // Ctrl+X etc
}

impl InputCategory {
    pub fn from_key(key: KeyEvent) -> Self {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Self::Shortcut;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => Self::Navigation,
            KeyCode::Enter => Self::Confirmation,
            KeyCode::Esc => Self::Cancellation,
            KeyCode::Tab | KeyCode::BackTab => Self::LateralCycle,
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => Self::Scrolling,
            KeyCode::Backspace | KeyCode::Delete => Self::Deletion,
            KeyCode::Char(_) => Self::TextEntry,
            _ => Self::Navigation,
        }
    }
}

/// Input mask - which categories an overlay captures
#[derive(Debug, Clone, Default)]
pub struct InputMask {
    masked: HashSet<InputCategory>,
}

impl InputMask {
    pub fn blocking() -> Self { /* masks everything */ }
    pub fn scrollable() -> Self { /* masks nav/confirm/cancel/scroll, not lateral */ }
    pub fn confirmation() -> Self { /* masks nav/confirm/cancel */ }

    pub fn masks(&self, cat: InputCategory) -> bool {
        self.masked.contains(&cat)
    }
}
```

### 5. Main Event Loop

The core `app_run` orchestrates ticks:

```rust
// In ui/mod.rs

fn run_app(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        // ═══════════════════════════════════════════════════════════════
        // UPDATE TICK
        // ═══════════════════════════════════════════════════════════════
        let daemon_status = app.daemon.tick();
        let update_ctx = UpdateContext {
            daemon: &app.daemon,
            db: &app.db,
            daemon_did_work: daemon_status.completed > 0,
        };

        // Tick the active modal
        if let Some(action) = app.tick_active_modal(&update_ctx) {
            app.handle_modal_action(action);
        }

        // ═══════════════════════════════════════════════════════════════
        // RENDER TICK
        // ═══════════════════════════════════════════════════════════════
        terminal.draw(|f| app.render(f))?;

        // ═══════════════════════════════════════════════════════════════
        // INPUT TICK
        // ═══════════════════════════════════════════════════════════════
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                app.handle_input(key);
            }
        }

        if app.should_quit { break; }
    }
    Ok(())
}

impl App {
    fn tick_active_modal(&mut self, ctx: &UpdateContext) -> Option<ModalAction> {
        match self.view_context.active_modal {
            ModalType::TagEditor => {
                self.tag_editor_modal.as_mut()?.update(ctx)
            }
            ModalType::Insights => {
                self.insights_modal.as_mut()?.update(ctx)
            }
            // ... other modals
        }
    }

    fn handle_input(&mut self, key: KeyEvent) {
        let category = InputCategory::from_key(key);

        // Check overlay modal first
        if let Some(ref mut overlay) = self.overlay_modal {
            if overlay.input_mask().masks(category) {
                if let Some(result) = overlay.handle_input(key) {
                    self.handle_overlay_result(result);
                }
                return;
            }
        }

        // Handle lateral cycling at app level
        if category == InputCategory::LateralCycle {
            self.handle_lateral_cycle(key);
            return;
        }

        // Dispatch to active modal
        let action = match self.view_context.active_modal {
            ModalType::TagEditor => {
                self.tag_editor_modal.as_mut()
                    .map(|m| m.handle_input(key))
                    .unwrap_or(ModalAction::None)
            }
            ModalType::Insights => {
                self.insights_modal.as_mut()
                    .map(|m| m.handle_input(key))
                    .unwrap_or(ModalAction::None)
            }
            // ... other modals
        };

        self.handle_modal_action(action);
    }

    fn handle_modal_action(&mut self, action: ModalAction) {
        match action {
            ModalAction::None => {}

            ModalAction::StageDecision { group_idx, mutations } => {
                // Add to daemon transaction
                self.daemon.stage_decision(group_idx, mutations);
            }

            ModalAction::NextGroup => {
                if self.view_context.next_group() {
                    self.reload_modal_for_current_group();
                } else {
                    // At end - show commit overlay
                    self.overlay_modal = Some(OverlayModal::commit_preview());
                }
            }

            ModalAction::PrevGroup => {
                if self.view_context.prev_group() {
                    self.reload_modal_for_current_group();
                }
            }

            ModalAction::CommitTransaction { witness } => {
                self.daemon.commit_transaction(&witness);
                self.view_context = ViewContext::new(ModalType::Insights);
            }

            ModalAction::DiscardTransaction { witness } => {
                self.daemon.discard_transaction(&witness);
                self.view_context = ViewContext::new(ModalType::Insights);
            }

            ModalAction::ShowOverlay(overlay) => {
                self.overlay_modal = Some(overlay);
            }

            ModalAction::Close => {
                self.view_context = ViewContext::new(ModalType::Insights);
            }

            ModalAction::Status(msg) => {
                self.status_message = Some(msg);
            }
        }
    }

    fn reload_modal_for_current_group(&mut self) {
        // Recreate the active modal with the new group_idx
        let signals = &self.view_context.signals;
        let idx = self.view_context.group_idx;

        match self.view_context.active_modal {
            ModalType::TagEditor => {
                self.tag_editor_modal = Some(TagEditorModal::from_context(
                    signals,
                    idx,
                    &self.daemon,
                    &self.db,
                ));
            }
            // ... other modals
        }
    }
}
```

### 6. Daemon Transaction Interface (Assumed)

The daemon provides a simple transaction interface. The UI drives, the daemon stores.

```rust
// In daemon.rs (interface only - implementation is straightforward)

impl TaskDaemon {
    /// Start a new transaction. Only one active at a time.
    pub fn begin_transaction(&mut self, label: &str) -> Result<(), TransactionError>;

    /// Stage a decision (mutations) for a group index.
    pub fn stage_decision(&mut self, group_idx: usize, mutations: Vec<Mutation>);

    /// Get staged decision for a group index (for UI restoration).
    pub fn get_decision(&self, group_idx: usize) -> Option<&Vec<Mutation>>;

    /// Commit the transaction - requires witness.
    pub fn commit_transaction(&mut self, witness: &DecisionWitness) -> Result<usize, CommitError>;

    /// Discard the transaction - requires witness (discard is also a decision).
    pub fn discard_transaction(&mut self, witness: &DecisionWitness);

    /// Check if transaction is active.
    pub fn has_transaction(&self) -> bool;

    /// Get transaction summary for UI display.
    pub fn transaction_summary(&self) -> Option<TransactionSummary>;
}
```

---

## Signals → Decision Groups Pipeline

The key transformation: signals (from health_issues table) become decision groups for the operator to process.

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Signal Pipeline                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. QUERY: Fetch signals from health_issues table                    │
│     SELECT * FROM health_issues WHERE type = ? AND resolved = 0      │
│                                                                      │
│  2. COLLATE: Group signals by common join                            │
│     - Deploy conflicts: group by target_path                         │
│     - Duplicates: group by fingerprint                               │
│     - Tag issues: group by track_id                                  │
│                                                                      │
│  3. HYDRATE: For each group, load related track metadata             │
│     - Fetch full Track records for display                           │
│     - Compute default actions (suggested mutations)                  │
│                                                                      │
│  4. PRESENT: Pass to View Context as Vec<Signal> + group count       │
│     - View Context routes to appropriate modal                       │
│     - Modal renders one group at a time                              │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

```rust
// In corpus/health/signals.rs or similar

/// A collated signal group ready for presentation
pub struct SignalGroup {
    /// Unique identifier for this group
    pub id: i64,

    /// Signal type
    pub signal_type: SignalType,

    /// Tracks involved in this group
    pub tracks: Vec<Track>,

    /// Suggested mutations (computed from signal + tracks)
    pub suggested_mutations: Vec<Mutation>,

    /// Display metadata
    pub label: String,
    pub severity: Severity,
}

/// Transform raw health issues into presentable signal groups
pub fn collate_signals(
    signals: Vec<HealthIssue>,
    db: &Database,
) -> Vec<SignalGroup> {
    // Group by common join (depends on signal type)
    // Hydrate with track data
    // Compute suggestions
}
```

---

## Migration Plan

### Phase 1: Foundation

1. **Create `ui/context.rs`** - ViewContext, ModalType, InputCategory, InputMask
2. **Create `ui/modal.rs`** - ModalAction enum, OverlayModal enum
3. **Add transaction interface to daemon** - begin/stage/get/commit/discard

### Phase 2: View Context Integration

1. **Refactor App** - add ViewContext field, remove UiMode enum
2. **Update main loop** - implement three-tick structure
3. **Add overlay modal handling** - input masking, result handling

### Phase 3: Tag Editor Modal

1. **Create unified `TagEditorModal`** - replaces TagEditorState + DirectoryTagEditorState
2. **Implement `from_context()`** - signals + group_idx → modal state
3. **Implement witness threading** - create witness at Enter keypress
4. **Wire up to ViewContext** - reload on group change

### Phase 4: Other Modals

1. **InsightsModal** - simple, already mostly clean
2. **CorpusBrowserModal** - navigation only, no decisions
3. **DeployPreviewModal** - similar pattern to tag editor

### Phase 5: Signal Pipeline

1. **Implement `collate_signals()`** - transform health_issues → SignalGroups
2. **Wire to ViewContext** - fetch signals on flow start
3. **Add signal refresh** - re-query on daemon work

### Phase 6: Cleanup

1. **Delete old files** - directory_state.rs, directory_input.rs, etc.
2. **Update CLAUDE.md** - document new architecture
3. **Remove legacy state from App**

---

## File Structure

```
ui/
  mod.rs              # Entry point, App, main loop
  context.rs          # ViewContext, ModalType
  input.rs            # InputCategory, InputMask
  modal.rs            # ModalAction, OverlayModal

  modals/
    mod.rs            # Trait patterns, shared utilities
    tag_editor.rs     # TagEditorModal
    insights.rs       # InsightsModal
    corpus_browser.rs # CorpusBrowserModal
    deploy_preview.rs # DeployPreviewModal

  widgets/            # (unchanged - rendering primitives)
    ...

  overlay/            # Overlay modal implementations
    change_preview.rs
    confirmation.rs
    unsaved_changes.rs
```

---

## Key Invariants

1. **DecisionWitness created at Enter keypress** - never elsewhere
2. **Only one transaction active** - daemon enforces
3. **View Context routes, View Modal handles data** - clear separation
4. **Signals are source of truth** - modals derive from signals + group_idx
5. **Daemon is passive storage** - UI drives all logic
