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

## Pane Focus Indicators

- Focused pane should have highlighted border (typically yellow)
- Non-focused panes should have default border color
- Cursor/selection within a pane should be visually distinct

TODO: need to revisit all colors once we're more done on UI work.
