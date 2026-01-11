# MLA Design Philosophy

This document captures the conceptual foundations of MLA - the principles that guide architectural decisions and user experience design. When in doubt, refer here.

## The Librarian Metaphor

MLA models itself on the work of a librarian, not a file manager. A librarian:

- **Surveys** the collection before acting
- **Preserves** materials, never casually discards
- **Organizes** according to systematic principles
- **Serves** patrons by making materials discoverable
- **Repairs** damaged items or acquires replacements

This metaphor shapes everything from menu organization to how we handle duplicates.

## The Librarian Cycle

Operations flow through a repeating cycle:

```
    ┌─────────────────────────────────────────┐
    │                                         │
    ▼                                         │
┌────────┐    ┌────────┐    ┌──────────────┐  │
│ Insight│───▶│ Intake │───▶│ Organization │  │
└────────┘    └────────┘    └──────────────┘  │
    ▲              │               │          │
    │              │               ▼          │
    │              │        ┌────────────┐    │
    │              │        │ Deployment │    │
    │              │        └────────────┘    │
    │              │               │          │
    │              ▼               │          │
    │         ┌─────────┐         │          │
    └─────────│ Repair  │◀────────┘          │
              └─────────┘                     │
                   │                          │
                   └──────────────────────────┘
```

### 1. Insight

Before any action, understand the current state. Reports answer questions like:

- What's in the corpus? (track counts, format distribution)
- What quality issues exist? (missing tags, low bitrates)
- What duplicates exist? (fingerprint clusters, metadata conflicts)
- What's deployed? (library coverage, stale links)

Insight operations are always read-only. They inform decisions but never alter the corpus.

### 2. Intake

New material enters the corpus through controlled gates:

- Source validation (format, quality thresholds)
- Metadata enrichment (external lookups, inference)
- Duplicate detection (is this already in the corpus?)
- Placement decision (where does this belong?)

Intake is the only entry point. No files spontaneously appear in the corpus.

### 3. Organization

The corpus evolves through explicit, trackable mutations:

- **Deduplication**: Resolving fingerprint duplicates
- **Tag repair**: Fixing metadata inconsistencies
- **Path normalization**: Moving files to canonical locations
- **Quality upgrades**: Replacing files with better versions

All organization operations produce reversible change records.

### 4. Deployment

The corpus is the source of truth. Libraries are views into the corpus:

- Libraries contain hard links, not copies
- Deployment rules determine what's visible where
- Changes to corpus automatically reflect in libraries
- Stale deployments (dangling links) are detected and cleaned

### 5. Repair

Some files cannot be fixed in place:

- Corrupted audio requiring re-acquisition
- Missing metadata requiring manual research
- Duplicates requiring human judgment

The `lost-files` workspace holds these temporarily. Once repaired or replaced, they re-enter through Intake.

## Algebraic Change Model

Corpus mutations are modeled as composable, reversible functions. This is not just an implementation detail - it fundamentally shapes how users interact with the system.

### Why Algebraic?

Traditional file managers apply changes immediately and irreversibly. MLA instead:

1. **Records intent**: "Move file A to B" becomes a data structure
2. **Accumulates changes**: Multiple operations queue up
3. **Previews effects**: Show what would happen before committing
4. **Applies atomically**: All-or-nothing execution
5. **Supports reversal**: Committed changes can be undone

### Change Types

```rust
enum ChangeType {
    Move,       // Relocate within corpus
    Delete,     // Remove from corpus (to lost-files)
    TagEdit,    // Modify embedded metadata
    Deploy,     // Create library hard link
    Undeploy,   // Remove library hard link
}
```

### Change Composition

Changes compose naturally:

- `Move(A→B) + Move(B→C) = Move(A→C)`
- `Delete(A) + Create(A) = NoOp`
- `TagEdit(field=X) + TagEdit(field=Y) = TagEdit(field=Y)`

This allows optimization and conflict detection before execution.

### Change Sessions

Related changes group into sessions:

```
Session: "Deduplicate Artist X discography"
├── Delete: album1/track1.flac (keeping album2/track1.flac)
├── Delete: album1/track2.flac (keeping album3/track2.flac)
├── TagEdit: album2/track1.flac (inherit genre from deleted)
└── Deploy: album2/* → music library
```

Sessions can be committed, rolled back, or exported as a script.

## Well-Tested Atomic Operations

The philosophy: **design operations carefully, test them thoroughly, then apply them in bulk without fear**.

### The Atomic Operation Library

Each operation type has:

1. **Precondition checks**: What must be true before execution?
2. **Execution logic**: The actual filesystem/database changes
3. **Postcondition verification**: Did it work correctly?
4. **Reversal logic**: How to undo this specific change
5. **Test suite**: Covering edge cases and failure modes

### Bulk Application

Once an operation is trusted, it can be applied to thousands of files:

```
Applying "Remove duplicate lower-bitrate versions"
├── 847 files identified
├── Preconditions: ✓ all passed
├── Dry run: ✓ no conflicts
├── Execute: [████████████████████] 847/847
└── Verification: ✓ all postconditions satisfied
```

Users review patterns, not individual files. The system handles the tedious enumeration.

### Failure Handling

Atomic operations fail completely rather than partially:

- If file 400 of 847 fails, files 1-399 are rolled back
- Failures are logged with full context for diagnosis
- The user can fix the issue and retry the entire batch

## Dialogue-Based Operator Interface

MLA prefers structured conversation over dense dashboards. The operator is a colleague, not a data entry clerk.

### The Decision Flow

Complex operations proceed as a dialogue:

```
┌─────────────────────────────────────────────────────┐
│ Decision 3/12: Duplicate Detection                   │
├─────────────────────────────────────────────────────┤
│                                                      │
│ These files appear to be duplicates:                 │
│                                                      │
│   A: web/releases/bandcamp/artist/track.flac        │
│      320kbps, 2019-03-15, tags: complete            │
│                                                      │
│   B: web/releases/itunes/artist/track.m4a           │
│      256kbps AAC, 2020-01-01, tags: partial         │
│                                                      │
│ Recommendation: Keep A (higher quality, better tags) │
│                                                      │
├─────────────────────────────────────────────────────┤
│  > Accept recommendation                             │
│    Reject (keep both)                                │
│    Defer to later                                    │
│    Show audio comparison                             │
└─────────────────────────────────────────────────────┘
```

### Principles of Dialogue Design

1. **One decision at a time**: Don't overwhelm with options
2. **Context first**: Show relevant information before asking
3. **Recommendations offered**: The system has opinions, clearly stated
4. **Escape hatches available**: Defer, skip, or get more details
5. **Progress visible**: "3/12" - the operator knows where they are

### Pattern Learning

Over time, the dialogue learns operator preferences:

- "You've accepted 'keep higher bitrate' 47 times. Auto-apply?"
- "You always defer FLAC vs ALAC decisions. Create a rule?"
- "Bandcamp releases consistently preferred over iTunes. Note this?"

This isn't hidden ML magic. The system explicitly proposes rules based on observed patterns, and the operator explicitly accepts or rejects them.

### Keyboard-Driven Navigation

The dialogue uses arrow keys, not hotkeys:

- `↑/↓` - Navigate between options
- `Enter` - Select highlighted option
- `Esc` - Exit flow (shows summary)

This reserves alphanumeric keys for potential data entry (e.g., custom keep-reason, manual tag values).

## Design Principles Summary

1. **Read-only by default**: Observation never mutates
2. **Explicit confirmation**: No silent changes to corpus
3. **Reversible operations**: Every change can be undone
4. **Batch over individual**: Design patterns, apply in bulk
5. **Conversation over dashboard**: One question at a time
6. **Hard links over copies**: Single source of truth
7. **Lost-files over deletion**: Preserve until explicitly discarded

## Anti-Patterns to Avoid

- **Immediate execution**: Don't apply changes without preview
- **Silent failures**: Don't swallow errors, surface them clearly
- **Modal complexity**: Don't nest modes deeply, keep paths flat
- **Clever inference**: Don't guess operator intent, ask
- **Hidden state**: Don't accumulate changes invisibly
- **Format worship**: Don't assume FLAC > MP3 without context
