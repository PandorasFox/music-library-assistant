# MM Design Philosophy

This document captures the conceptual foundations of MM - the principles that guide architectural decisions and user experience design. When in doubt, refer here.

There are several guiding metaphors for different facets of MM. The most important metaphor is that of the Librarian (the user) and their Library (their music library).

DRAGON'S NOTE: I need to touch up the language in this / align a few of the metaphors a bit

## The Librarian Metaphor

The tool-user is the Librarian, and MM is the librarian's assisstant. MM's purpose is to be the librarian's primary interface for surfacing information from their music library,
and when it surfaces health issues with the library corpus, it should proactively ask the user for their Opinion on how to resolve a given health issue. More on MM's role as the assistant later.

The Librarian must execute on all of the following processes to properly maintain their corpus:

- gathering insight on their library corpus
- categorizing out-of-corpus items against the existing corpus catalog
- organizing out-of-corpus items into the corpus after they are categorized
- marking corpus files as 'damaged' or mis-tagged, and preventing them from deploying into libraries
- repairing the corpus with a wide variety of basic tools

These processes all largely mirror traditional, physical book library processes - items are checked out, checked in, and must go through some health checks and validation before being re-entered for circulation.

## Operator-Driven Decisions

**MM must never make Decisions or Mutations on its own.** All corpus Mutations must be attributable to explicit Operator Decisions. MM's role is to:

1. **Surface information**: Identify health issues, duplicates, conflicts
2. **Propose options**: Present resolution choices to the operator
3. **Accumulate intent**: Gather Decisions during UI modals
4. **Execute faithfully**: Apply only what the operator explicitly approved

This is a hard constraint, not a preference. MM may compute, analyze, and recommend - but the final Decision always belongs to the Librarian. Even "obvious" fixes (like removing exact duplicates) require operator confirmation.

**Decisions** are accumulated during operator modals and represent user intent. **Mutations** are the filesystem/database changes that realize those Decisions. The relationship is:

```
Operator Interaction → Decisions (accumulated) → Mutations (executed in batch)
```

Every Mutation must trace back to a Decision. If we can't attribute a Mutation to an operator Decision, that Mutation should not happen.

## Algebraic Changes

Changes to the librarian's corpus should not be applied lightly. MM earns this privilege by modelling all corpus changes as algebraic changes that we can accumulate, preview, adjust, and then commit or discard.

Corpus mutations are modeled as composable, reversible functions. This is not just an implementation detail - it fundamentally shapes how users interact with the system. We want firm fundamentals and guarantees in this problem space.

*Non-Exhaustive* examples of changes that should be modelled algebraically:
- atomically moving files (e.g. from intake to corpus)
- editing tags of a file
- editing tags of a section of the corpus (all files under a directory)

More details on the algebraic bits at the end.

## The MM Metaphor

MM itself is inspired by the MM (Milton Library Assistant) from the Talos Principle. It is a conversational dialogue interface for accessing library functions, and it operates by presenting the user with series of choices, keeping them engaged.

I think that the almost-conversational back and forth between the Librarian and their Assistant is an ideal pattern for empowering users to make organizational decisions about large music libraries. To elaborate further:

- the assistant is responsible for identifying which *simple* decisions will have the *biggest* impact towards the goal of the current process (e.g. duplicate reduction).
- The assisstant does not make decisions - the assistant solely presents a decision to the librarian, then dispatches workers behind the scenes to prepare the algebraic changes and prepare them
- the librarian has many responsibilities, and should be able to disengage from an MM dialogue while preserving the proposed changes.

Broadly, the purpose of MM is to enact the Librarian's coherent organization and tagging preferences across a large library. It is still likely that the Librarian will need to handle hundreds of metadata corrections by hand - but MM is intended to cut down on the *thousands* of duplicates or slightly-mistagged files that plague some digital libraries.

Lastly, MM itself is a largely machine-written toolkit for librarians. It itself must be cleanly organized and compartmentalized:

- core database/algebraic change logic should exist as its own core library leveraged across MM components
- low-level corpus operations should be organized into a corpus library
- corpus operations (scanning, dedup/tagging, deploying) should have their logic organized in individual operations modules


### Other tooling

I've started also having Claude hand me some scripts that spit out src/ tree analytics, to help with my own at-a-glance health-checks of MM's source tree itself -> drive refactorings and cleanup.
