# MLA Design Philosophy

This document captures the conceptual foundations of MLA - the principles that guide architectural decisions and user experience design. When in doubt, refer here.

There are several guiding metaphors for different facets of MLA. The most important metaphor is that of the Librarian (the user) and their Library (their music library).

## The Librarian Metaphor

The tool-user is the Librarian, and MLA is the librarian's assisstant. MLA's purpose is to be the librarian's primary interface for surfacing information from their music library,
and when it surfaces health issues with the library corpus, it should proactively ask the user for their Opinion on how to resolve a given health issue. More on MLA's role as the assistant later.

The Librarian must execute on all of the following processes to properly maintain their corpus:

- gathering insight on their library corpus
- categorizing out-of-corpus items against the existing corpus catalog
- organizing out-of-corpus items into the corpus after they are categorized
- marking corpus files as 'damaged' or mis-tagged, and preventing them from deploying into libraries
- repairing the corpus with a wide variety of basic tools

These processes all largely mirror traditional, physical book library processes - items are checked out, checked in, and must go through some health checks and validation before being re-entered for circulation.

## Operator-Driven Decisions

**MLA must never make Decisions or Mutations on its own.** All corpus Mutations must be attributable to explicit Operator Decisions. MLA's role is to:

1. **Surface information**: Identify health issues, duplicates, conflicts
2. **Propose options**: Present resolution choices to the operator
3. **Accumulate intent**: Gather Decisions during UI flows
4. **Execute faithfully**: Apply only what the operator explicitly approved

This is a hard constraint, not a preference. MLA may compute, analyze, and recommend - but the final Decision always belongs to the Librarian. Even "obvious" fixes (like removing exact duplicates) require operator confirmation.

**Decisions** are accumulated during operator flows and represent user intent. **Mutations** are the filesystem/database changes that realize those Decisions. The relationship is:

```
Operator Flow → Decisions (accumulated) → Mutations (executed in batch)
```

Every Mutation must trace back to a Decision. If we can't attribute a Mutation to an operator Decision, that Mutation should not happen.

## Algebraic Changes

Changes to the librarian's corpus should not be applied lightly. MLA earns this privilege by modelling all corpus changes as algebraic changes that we can accumulate, preview, adjust, and then commit or discard.

Corpus mutations are modeled as composable, reversible functions. This is not just an implementation detail - it fundamentally shapes how users interact with the system. We want firm fundamentals and guarantees in this problem space.

*Non-Exhaustive* examples of changes that should be modelled algebraically:
- atomically moving files (e.g. from intake to corpus)
- editing tags of a file
- editing tags of a section of the corpus (all files under a directory)

More details on the algebraic bits at the end.

## The MLA Metaphor

MLA itself is inspired by the MLA (Milton Library Assistant) from the Talos Principle. It is a conversational dialogue interface for accessing library functions, and it operates by presenting the user with series of choices, keeping them engaged.

I think that the almost-conversational back and forth between the Librarian and their Assistant is an ideal flow for empowering users to make organizational decisions about large music libraries. To elaborate further:

- the assistant is responsible for identifying which *simple* decisions will have the *biggest* impact towards the goal of the current process (e.g. duplicate reduction).
- The assisstant does not make decisions - the assistant solely presents a decision to the librarian, then dispatches workers behind the scenes to prepare the algebraic changes and prepare them
- the librarian has many responsibilities, and should be able to disengage from an MLA dialogue while preserving the proposed changes.

Broadly, the purpose of MLA is to enact the Librarian's coherent organization and tagging preferences across a large library. It is still likely that the Librarian will need to handle hundreds of metadata corrections by hand - but MLA is intended to cut down on the *thousands* of duplicates or slightly-mistagged files that plague some digital libraries.

Lastly, MLA itself is a largely machine-written toolkit for librarians. It itself must be cleanly organized and compartmentalized:

- core database/algebraic change logic should exist as its own core library leveraged across MLA components
- low-level corpus operations should be organized into a corpus library
- corpus operation flows (scanning, dedup/tagging flows, deploying) should have their logic organized in individual operations modules

### Misc notes
- insights and reports should be able to be computed during scans, and have the latest report summaries displayed in the menu for them, rather than needing generation
- librarian should be nudged into the next steps of different flows. E.g. upon scan completion, reports should be generated/finalized and summaries should be given, with prompts to enter relevant flows for resolving corpus health issues.
- I, specifically the architect, need to enumerate more concepts about library health that we care about.

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


## Design Principles Summary

1. **Operator-driven**: MLA never mutates autonomously; all Mutations trace to operator Decisions
2. **Read-only by default**: Observation never mutates
3. **Explicit confirmation**: No silent changes to corpus
4. **Consistency, Safety, Predictablity, Observability**: We value consistent use of our handful of systems highly.

## Anti-Patterns to Avoid

- **Autonomous mutation**: Never mutate corpus without an attributable operator Decision
- **Immediate execution**: Don't apply changes without preview
- **Silent failures**: Don't swallow errors, surface them clearly
- **Modal complexity**: Don't nest modes deeply, keep paths flat
- **Clever inference**: Don't guess operator intent, ask
- **Hidden state**: Don't accumulate changes invisibly
