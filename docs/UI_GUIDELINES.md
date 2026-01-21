# UI Development Guidelines

This document codifies patterns and rules for maintaining a clean, consistent UI codebase.

## Module Organization Rules

### What Belongs in `src/ui/mod.rs`

- `App` struct and core fields
- `UiMode` enum
- `handle_key()` dispatch (thin - delegates to modules)
- Mode transition orchestration (`start_*` methods, kept minimal)
- Entry points (`run_menu`, `run_app`)

### What Does NOT Belong in `src/ui/mod.rs`

- Complex action handling logic (belongs in respective modules)
- Rendering code (belongs in `render.rs` or module-specific renderers)
- State structs for specific modes (belong in mode modules)
- Helper functions for specific features (belong in feature modules)

### Extraction Thresholds

- Extract when UiMode-specific code exceeds ~100 lines
- State structs for modes live in their handling module, not mod.rs
- All rendering goes through `render.rs` -> sub-module renderers

## Database Access Rules

**Never call `Database::open()` in UI code.**

- Always use `witch.read_only_db()` for queries (or the `App::db()` helper)
- Only the Witch's worker threads create write connections
- Pre-App startup code (migrations, first-time setup) is the exception

## Widget Usage Requirements

Before creating new rendering code, check `ui/widgets/`:

| Widget | Purpose |
|--------|---------|
| `SelectableList` | Navigable lists with selection highlighting |
| `HealthStatus` | Consistent status coloring (Healthy, Info, Warning, Critical) |
| `StatusIndicator` | Single-item status display with icon/label |
| `ThreePaneLayout` | Standard three-pane layouts |
| `Modal`, `ConfirmationModal` | Popup dialogs |
| `ControlsHint` | Context-sensitive keyboard hints |
| `TextInputState` | Text input with cursor management |

### Modal Dialogs

- Modal dialogs MUST use `widgets::Modal` or `widgets::ConfirmationModal`
- Use `control_presets::for_mode()` for keyboard hints

### Confirmation Dialog Safety

**Cancel/No must be the default selection** - protects against stuck Enter key.

- Destructive actions (delete, overwrite, exit with pending work) require explicit navigation to confirm
- Use `ModalButton::new("Cancel", "").selected()` pattern

## Action Pattern

Modes should use an action-based pattern for clean separation:

```rust
// In mode module (e.g., tag_editor/input.rs)
pub fn handle_key(state: &mut State, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Enter => Action::Confirm,
        KeyCode::Esc => Action::Cancel,
        _ => Action::None,
    }
}

// In mod.rs
fn handle_mode_action(&mut self, action: Action) {
    match action {
        Action::Confirm => { /* orchestrate result */ }
        Action::Cancel => { /* transition mode */ }
        Action::None => {}
    }
}
```

Benefits:
- State mutation happens in state module
- Mode transitions happen in mod.rs
- Testing is easier (actions can be unit tested)

## Anti-Patterns to Avoid

1. **Vestigial code with `todo!()` blockers** - delete it entirely
2. **Mode-specific state structs in mod.rs** - move to mode module
3. **Hand-rolled modal rendering** - use widgets
4. **Duplicate match arms for labels/messages** - add helper methods
5. **Default-selecting "Confirm" in modals** - always default to Cancel
6. **Using `#[allow(dead_code)]`** - remove dead code instead (see CLAUDE.md)

## Module Structure Reference

```
src/ui/
├── mod.rs              # App, UiMode, handle_key dispatch, start_* orchestration
├── render.rs           # render() dispatch, RenderContext, footer/sidebar
├── app.rs              # EyeAnimation, splash screen state
├── helpers.rs          # String formatting, truncation helpers
├── widgets/            # Reusable UI primitives
│   ├── controls.rs     # ControlsHint, presets
│   ├── layout.rs       # PaneConfig, ThreePaneLayout
│   ├── list.rs         # SelectableList
│   ├── modal.rs        # Modal, ConfirmationModal
│   ├── status.rs       # HealthStatus, StatusIndicator
│   └── text_input.rs   # TextInputState
├── startup/            # Pre-main-loop modals
│   ├── first_time_setup.rs
│   ├── intake_confirmation.rs
│   └── migrations.rs
├── insights_view/      # Main entry point lateral view
├── tag_editor/         # Unified tag editor module
├── tree_browser/       # Unified tree browser (corpus, directory selector)
├── deploy_flow/        # Deployment preview and execution
├── tag_search/         # Tag search lateral view
└── drop_flow.rs        # Drop missing files flow
```

## Testing Checklist

When making UI changes:

1. `cargo build` - verify compilation
2. `cargo test` - verify tests pass
3. Launch app: `cargo run`
4. Navigate lateral views (Insights -> Deploy -> TagSearch -> CorpusBrowser)
5. Verify control hints in footer for each mode
6. Test the specific flow you modified
