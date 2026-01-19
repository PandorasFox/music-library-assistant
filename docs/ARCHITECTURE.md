# MLA System Architecture

This document describes the major subsystems of MLA and how they interact. For the conceptual foundations and design principles, see [PHILOSOPHY.md](PHILOSOPHY.md).

## Core Event Loop: ratatui

Ratatui's event loop should:
- tick the mla daemon subsystem
- tick the UI modal that is currently active
- render
- process inputs

(exact ordering on those last three might need to be re-evaluated).

General library, corpus, and operational state do not exist in UI code. UI code is for user-interaction-logic only.

## Task Daemon

The task daemon is where all operational logic flows through. To begin:

- it is responsible for accepting, queueing, and executing Tasks
- it is responsible for keeping the corpus+index in a read-only state until startup has been finished
- it is responsible for accumulating state (user Decisions with corpus Mutations) in a transactional format to simplify the UI modals - UI data state can be offloaded to Daemon once finalized, but before being committed

The types of tasks are:

- Mutations
  - These are tasks that can mutate the files in corpus or track/tag indices we maintain. They cannot be scheduled before the corpus health has finished computing. They require a DecisionWitness (only generable at an enter keypress handler callsite) to be entered into a transaction. Transactions require an additional DecisionWitnessed Decision to be committed, or discarded, giving us a two-stage review model for all destructive operations.
- Computations
  - Downstream computations. These emit "signals" to the health database, which can be altered while the corpus/index are in read-only states, because signals' only inputs are corpus state, index state, and other signals themselves.
  - mutations can emit computations as side effects. computations can emit other computations as side effects.
  - can be scheduled and executed at any time (and without user decision) as they are purely observational computations (that we record the results of). Ideally, whenever we start up, we basically just confirm that our corpus state on disk has not changed since our last run, and if it has, we generate some signals indicating that the user needs to acknowledge or resolve.
- Migrations
  - Special tasks that can only be executed in the very very initial stage of the Task Daemon, before we have even scheduled our first computation to start observing the corpus. They are purely and explicitly for kicking off startup DB migrations, and require a DecisionWitnessed Decision, as they might take some time & the user needs to approve that the timely migration might take place.

## Signals

Signals are the computed health state of the corpus (as well as deployed libraries, as deployment state is actually a secret fourth piece of state input to computations, oops. Maybe we should just genericize this to 'filesystem state' some time).

Anyways, signals can represent a variety of things:

- files that are present in corpus
- files that were present in corpus, but are still in index (missing now)
- files that are present in corpus, but have been modified since we last indexed them
- files with fingerprint duplicates
- files with identical overlaps
- files that can be deployed, but aren't
- files that are hard-linked into a library, but shouldn't be
- and more....

Basically, signals can also reference other signals. It is up to the UI logic (basically, human-driven dashboard queries) to present meaningful signals to the user that are actionable, with a handful of dynamic operational tools that take signal sets, clump together files based on common signals and tags/fingerprints/etc (other signals, potentially!), and then present the user with succinct Decisions.

Signals are purely informational. Signals are stateless and should be consistently recalculable; however, they are expensive to fully recalculate, so we should make efforts to trigger efficient recomputes when underlying metrics update.

## The Corpus and Index

The corpus is the files on disk. They are sacred; MLA shall not mutate them without a corresponding Enter keypress from the user. The Index is the Librarian (user)'s record of the corpus, and should also be treated as sacred. Some initial operations - such as indexing all files upon initial startup - might seem a bit superfluous, but it's good to set the example early: we always need the user to press enter to add a decision to a transaction, then enter again to finalize a transaction.

## The Mutation Engine

One of the concepts underlying all this is that Mutations are expressed as basic algebraic operations that we can accumulate, and 'sum' to calculate the expected end state after they all execute and perform their mutations.
This is what enables the transactions flow and the confirmation review processes.

We combine this with rust's type system (namely, Sealed Traits, to only allow for some interfaces to be invoked from certain callsites or modules) so that we have compile-time guarantees that an enter keypress has happened when we're adding a decision to a transaction/confirming a decision. We additionally have other sets of compile-time guarantees that the actual corpus/index mutating function implementations are only invoked from the Task execution callsite (in their new thread).

The combination of all these is what enables the Task Daemon to guarantee all of our operational sanity requirements.

## The corpus browser/search and tag editor

These are two fundamental tools for any Librarian, as one-off corpus introspection needs nice tools. Additionally, we need them for our own development cycle.

Because the tag editor also *must* use the Task Daemon for dispatching its edits etc, we both *get to leverage* the transaction UX for our UI state machine, and *have to validate* the mutations under a lens, as well. Thus, the tag editor and browsers/searchers are invaluable tools for validating functionality and correctness of mutations and computations in a small and verifiable context before applying them in bulk.

Additionally, all the UI components we make for the tag editor are necessary for our other UI modals, so the widgets and stuff there are just handy as well.

## Insights

Ultimately, all these systems exist to serve one thing: the Corpus Insights dashboard that presents signals to the user in a meaningful manner, to let them jump into precise guided Flows to resovle sets of signals in bulk.

This part will require planning for each individual flow on their own to make them each as powerful as possible while not duplicating unneccesary amounts of UI code. It is *the* reason everything else in MLA is so engineered.
