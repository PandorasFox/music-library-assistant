# MLA UX Patterns

Common UI patterns that should be observed across all MLA interfaces.

## Navigation Consistency

### Tab / Shift+Tab Pairing

Tab and Shift+Tab should always be consistently paired for forward/backward navigation:

- **Tab**: Advance to next group/item (skip current)
- **Shift+Tab**: Go back to previous group/item

These keys should never be used for unrelated actions. If Tab advances, Shift+Tab must go back.

## Operation Flow Pattern

All corpus-mutating operation flows should follow the **mutation-accumulating pattern**:

1. **Interactive Phase**: Present items/groups for operator decisions
2. **Mutation Accumulation**: Each decision generates pending mutations (PendingChange records) that are accumulated throughout the flow
3. **Review Screen**: After all decisions are made, show a summary of all accumulated mutations
4. **Commit/Discard**: Allow operator to commit all pending mutations at once, or discard the entire session

This pattern ensures:
- Operator can review all changes before any filesystem mutations occur
- All changes in a session can be atomically committed or discarded
- Session state is preserved when navigating between groups (Tab/Shift+Tab)

### Decision Persistence Rule

**Any time a modal/group is exited, active decisions MUST be persisted first.**

This applies to all navigation actions:
- Tab (advance to next)
- Shift+Tab (go back to previous)
- Esc (go to review)
- Any other action that leaves the current editing context

Decisions should never be discarded by navigation. The accumulation pattern means we always move forward toward the review stage, preserving all work done along the way. When revisiting a previous group, its saved decision should be reloaded into the UI. When leaving again (in any direction), any modifications should be persisted as an update-in-place.

### Symmetric Navigation

**Forward and backward navigation should be functionally identical**, with one exception:

- **Tab (Next)**: Persist current decision → advance index → load next group's state
- **Shift+Tab (Back)**: Persist current decision → decrement index → load previous group's state
- **Exception**: Advancing past the last group proceeds to the Review stage

This symmetry accommodates non-linear workflows. Librarians may need to revisit previous decisions, change their minds, skip around, and return. The system must reliably preserve all accumulated state regardless of navigation path.

**Key principle**: Librarians trust us to hold their state. Navigation should never lose work.

### Example: Artist Canonicalization Flow

1. Present artist name clusters one at a time
2. Operator selects variants to squash and chooses canonical name
3. Each "Squash!" decision generates TagEdit mutations for affected tracks
4. Tab advances to next cluster (preserving decision), Shift+Tab goes back
5. Esc opens review screen showing all decisions and affected track counts
6. Commit writes to database and starts background tag-flush operation

## Pane Focus Indicators

- Focused pane should have highlighted border (typically yellow)
- Non-focused panes should have default border color
- Cursor/selection within a pane should be visually distinct
