# MM System Architecture

This document describes the major subsystems of MM and how they interact. For the conceptual foundations and design principles, see [PHILOSOPHY.md](PHILOSOPHY.md).

## Module Organization

```
src/
├── main.rs                         # Entry point
├── config.rs                       # Configuration
├── db_thread.rs                    # DB write thread
├── logging.rs                      # Logging
├── corpus/                         # Corpus filesystem, DB, codecs, tags, deploy
│   ├── db/                         # Database schema, queries, types
│   ├── health/                     # Health assessment logic
│   └── ...                         # codecs, metadata, paths, tags, transcode
├── meta/                           # Signals, Mutations, Computations
│   ├── signals/
│   │   ├── mod.rs
│   │   └── types.rs                # SignalType, Signal, AggregateSignal, CorpusSummary
│   ├── mutations/
│   │   ├── mod.rs                  # Mutation enum, MutationToken sealed module
│   │   ├── types.rs                # TagOp, PendingSignal, MutationResult, etc.
│   │   ├── tag_edit.rs             # ApplyTagOps executor
│   │   ├── indexing.rs             # Indexing executors
│   │   ├── file_ops.rs             # File operation executors
│   │   ├── transcode.rs            # Transcoding executor
│   │   └── migration.rs            # MigrationRegistry
│   └── computations/
│       ├── mod.rs                  # Computation enum, execute_single()
│       ├── types.rs                # ComputationWitness sealed module
│       ├── helpers.rs              # Signal emission/clearing helpers
│       ├── stats.rs                # Thread-local stats + read-only DB connections
│       ├── observation/             # Filesystem observation (no inference)
│       ├── derivation/             # First-level derivations
│       └── analysis/               # Full-corpus analysis
│           ├── schedule.rs         # ScheduleContentAnalysis
│           ├── duplicates.rs       # Fingerprint overlaps, metadata dupes, cross-source
│           ├── tags.rs             # Missing tags, canonicalization, compounds
│           ├── deploy.rs           # Deploy conflicts, health signals
│           └── formats.rs          # Shit format detection
├── witch/                          # The Witch orchestrator
└── ui/                             # Terminal UI layer
```

## The Witch

The core orchestrator of all the subsystems that make up MM. All Data Flows Through Her.

The Witch is named such because She enforces orderliness in her domain, and provides all guarantees for data which flows properly through Her. She is where all operational logic flows through. To begin:

- She is responsible for accepting, queueing, and executing Tasks
- She is responsible for keeping the corpus+index in a read-only state until startup has been finished
- She is responsible for accumulating state (user Decisions with corpus Mutations) in a transactional format to simplify the UI modals - UI data state can be offloaded to Her once finalized, but before being committed

The types of tasks are:

- Mutations
  - These are tasks that can mutate the files in corpus or track/tag indices we maintain. They cannot be scheduled before the corpus health has finished computing. They require a DecisionWitness (only generable at an enter keypress handler callsite) to be entered into a transaction. Transactions require an additional DecisionWitnessed Decision to be committed, or discarded, giving us a two-stage review model for all destructive operations.
- Computations
  - Downstream computations. These emit "signals" to the health database, which can be altered while the corpus/index are in read-only states, because signals' only inputs are corpus state, index state, and other signals themselves.
  - mutations can emit computations as side effects. computations can emit other computations as side effects.
  - can be scheduled and executed at any time (and without user decision) as they are purely observational computations (that we record the results of). Ideally, whenever we start up, we basically just confirm that our corpus state on disk has not changed since our last run, and if it has, we generate some signals indicating that the user needs to acknowledge or resolve.
- Migrations
  - Special tasks that can only be executed in the very very initial stage of the Witch's lifecycle, before we have even scheduled our first computation to start observing the corpus. They are purely and explicitly for kicking off startup DB migrations, and require a DecisionWitnessed Decision, as they might take some time & the user needs to approve that the timely migration might take place.

## Signals

Signals are the computed health state of the corpus (as well as deployed libraries, as deployment state is actually a secret fourth piece of state input to computations, oops. Maybe we should just genericize this to 'filesystem state' some time).

Anyways, signals can represent a variety of things:

- inodes that are present in corpus
- inodes that were present in corpus, but are still in index (missing now)
- inodes that are present in corpus, but have been modified since we last indexed them
- inodes with fingerprint duplicates
- inodes with identical overlaps
- inodes that can be deployed, but aren't
- inodes that are hard-linked into a library, but shouldn't be
- and more....

Basically, signals can also reference other signals. It is up to the UI logic (basically, human-driven dashboard queries) to present meaningful signals to the user that are actionable, with a handful of dynamic operational tools that take signal sets, clump together files based on common signals and tags/fingerprints/etc (other signals, potentially!), and then present the user with succinct Decisions.

Signals are purely informational. Signals are stateless and should be consistently recalculable; however, they are expensive to fully recalculate, so we should make efforts to trigger efficient recomputes when underlying metrics update.

## The Corpus and Index

The corpus is the files on disk. They are sacred; MM shall not mutate them without a corresponding Enter keypress from the user. The Index is the Librarian (user)'s record of the corpus, and should also be treated as sacred. Some initial operations - such as indexing all files upon initial startup - might seem a bit superfluous, but it's good to set the example early: we always need the user to press enter to add a decision to a transaction, then enter again to finalize a transaction.

## The Mutation Engine

All mutations are modelled as instantiable/preparable objects that we can accumulate, and then schedule for execution in bulk.

For example, edits are roughly `edit(inode, tag_name, old_value, new_value)` - we can generate lots of those, hang onto them, and then send em off for execution, easily aggregated by inode!

## The UI Layer

Everything else is fundamentally at the UI layer - all resolution modals, the corpus browser, the search interface, the tag editor.... they all just use the underlying architecture we've built up.
