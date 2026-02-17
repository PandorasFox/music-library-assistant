# MM UX Patterns

Common UI patterns that should be observed across all MM interfaces.

## Navigation Consistency

### Tab / Shift+Tab Pairing

Tab and Shift+Tab should always be consistently paired for forward/backward navigation:

- **Tab**: Advance to next group/item (skip current)
- **Shift+Tab**: Go back to previous group/item

These keys should never be used for unrelated actions. If Tab advances, Shift+Tab must go back.

### Symmetric Navigation

**Forward and backward navigation should be functionally identical**, with one exception:

- **Tab (Next)**: Persist current decision → advance index → load next group's state
- **Shift+Tab (Back)**: Persist current decision → decrement index → load previous group's state
- **Exception**: Advancing past the last group proceeds to the Review stage

This symmetry accommodates non-linear workflows. Librarians may need to revisit previous decisions, change their minds, skip around, and return. The system must reliably preserve all accumulated state regardless of navigation path.

**Key principle**: Librarians trust us to hold their state. Navigation should never lose work.

### Arrow Keys Only — No Vim-Style Navigation

**Navigation must use arrow keys exclusively.** Do not bind `h`/`j`/`k`/`l` or any other alphanumeric keys as navigation alternatives. Alphanumeric keys are reserved for data input and shortcuts, never for directional movement.

- **Up/Down/Left/Right arrows**: The only valid navigation keys
- **hjkl**: Not valid navigation inputs — do not add these as alternatives
- **Alphanumeric keys**: Reserved for data input (typing into fields) or action shortcuts (e.g., `x` to remove, `n` to create)

This is a hard rule. Vim-style navigation keeps getting introduced and must be actively rejected.

## Pane Focus Indicators

- Focused pane should have highlighted border (typically yellow)
- Non-focused panes should have default border color
- Cursor/selection within a pane should be visually distinct

TODO: need to revisit all colors once we're more done on UI work.
