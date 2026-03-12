# Zone Generics, Inbox Collapse, and FS Watcher

Three interconnected architectural changes that pull on the same ball of yarn. Dependency order: zone generics → inbox collapse → FS watcher.

## Problem Statement

### Zone Duplication

Inbox and corpus are structurally identical — indexed audio files with tags and signals — but currently treated as separate worlds with duplicated logic:

| Corpus | Inbox | Difference |
|--------|-------|------------|
| `DeriveCorpusSignals` | `DeriveInboxSignals` | Corpus emits `MissingFile`; inbox silently drops |
| `DetectMissingTags` | `DetectInboxMissingTags` | Corpus has `ExpectedMissingTag` suppression |
| `DetectCompoundTagValues` | `DetectInboxCompoundTags` | Corpus uses dirty-inode orchestrator (scale) |
| `DetectTagCanonicalizations` | `DetectInboxTagCanonicity` | Inbox compares against corpus as reference |
| `gather()` | `gather_inbox()` | ~90% identical bodies, differ only in DB query + hardcoded zone |
| `get_distinct_tag_values()` | `get_distinct_inbox_tag_values()` | Fully duplicated SQL |
| `get_inodes_for_tag_values()` | `get_inbox_inodes_for_tag_values()` | Parallel functions |
| `get_audio_files_with_tag_presence()` | `get_inbox_audio_files_with_tag_presence()` | Already share private generic internally |
| `corpus_tags` table | `inbox_tags` table | Same schema, different table names |
| `FileInCorpusSignal` | `FileInInboxSignal` | Same shape, different signal types |
| `UnindexedFileSignal` | `InboxUnindexedSignal` | Same shape |
| `HealthyFileSignal` | `InboxHealthySignal` | Same shape |

Additionally, `check_mtime_differs()` hardcodes `Zone::Corpus` — a latent bug if inbox files ever need `VerifyTags`.

### Stateful Loaders (Closure Escape Hatch)

Three intake-related closures remain unconverted in the domain query migration:

- `IntakeConfirmationState::gather()` — corpus unindexed files
- `IntakeConfirmationState::gather_inbox()` — inbox unindexed files
- `IntakeConfirmationState::gather_startup()` — merges both

These are the same operation parameterized by zone. With zone generics, they collapse into a single `GetUnindexedFiles { zone }` domain query.

### Polling-Based FS Observation

MM is entirely polling-based. No inotify/fanotify infrastructure exists. The current architecture:

1. Startup: Witch queues `WalkCorpus` for corpus + inbox zones
2. `ScanCorpusDirectory` does recursive `read_dir()` + per-file `metadata()` calls
3. Compares disk state against DB via mtime batch queries
4. Spawns `VerifyMtime` → `VerifyTags` → `VerifyAudio` for changed/new files
5. Idle rescan: every 180s (configurable), re-walks corpus+inbox with mtime gating

This means:
- New files aren't detected until next poll (up to 3 minutes)
- Every poll re-walks the entire directory tree even if nothing changed
- The "initial FS state" is asserted by the same polling machinery, not a dedicated startup path

## Part 1: Zone Generics

### Design Goal

Encode zone at the type level so the compiler enforces which computations operate on which zones. Inbox and corpus share code paths where logic is identical, diverge where semantics differ, and the type system prevents accidental cross-contamination of tag reasoning.

### Trait Hierarchy

```rust
/// Marker trait: this zone has indexed audio files.
trait AudioZone: Zone {
    /// Signal type emitted when a file is first observed in this zone.
    type FilePresenceSignal: Signal;
    /// Signal type for unindexed files in this zone.
    type UnindexedSignal: Signal;
    /// Signal type for healthy (indexed + verified) files.
    type HealthySignal: Signal;
}

/// This zone has a tag table and supports tag queries.
trait TaggedZone: AudioZone {
    const TAG_TABLE: &'static str;
    /// How to handle files gone from disk.
    fn on_file_gone(inode: i64, sender: &SignalWriteSender);
}

/// This zone's tags are the source of truth for canonicalization.
/// Only Corpus implements this — inbox tags are compared *against* corpus,
/// never treated as canonical themselves.
trait CanonicalTagSource: TaggedZone {}

/// This zone's files participate in deploy path computation.
/// Only Corpus implements this.
trait Deployable: TaggedZone {}

/// This zone's files can be matched against external databases (AcoustID, MusicBrainz).
/// Both Corpus and Inbox could implement this — inbox for initial triage,
/// corpus for the full matching pipeline.
trait ExternallyMatchable: AudioZone {}
```

### Zone Implementations

```
Corpus: AudioZone + TaggedZone + CanonicalTagSource + Deployable + ExternallyMatchable
Inbox:  AudioZone + TaggedZone + ExternallyMatchable
Library: (none of the above — Library is a deployment target, not a source)
```

### What This Enables

**Unified computations:**
```rust
fn derive_zone_signals<Z: TaggedZone>(
    observed_inodes: &HashMap<i64, ObservedInode>,
    db: &ReadOnlyDb,
    sender: &SignalWriteSender,
) {
    // Shared: reconcile FilePresence signals, compute set operations
    // Zone-specific: Z::on_file_gone() handles the semantic difference
    // (corpus emits MissingFile, inbox silently drops)
}
```

**Unified tag queries:**
```rust
fn get_distinct_tag_values<Z: TaggedZone>(
    db: &Database,
    tag_name: &str,
) -> Result<Vec<String>> {
    // SQL parameterized by Z::TAG_TABLE
}
```

**Compile-time enforcement:**
```rust
fn detect_tag_canonicalizations<Z: CanonicalTagSource>(db: &ReadOnlyDb) {
    // Only callable for Corpus — inbox can't be a canonical source
}

fn detect_inbox_tag_canonicity(db: &ReadOnlyDb) {
    // Compares inbox tags against corpus canonical vocabulary
    // Uses TaggedZone for inbox reads, CanonicalTagSource for corpus reference
}
```

### Critical Constraint: Tag Reasoning Isolation

Corpus tag reasoning space must remain clean. The zone generic system must enforce:

1. **Tag canonicity signals are corpus-only.** `TagCanonicitySignal` and `InconsistentAlbumArtistSignal` are keyed by corpus tag state. Inbox tags are compared *against* this vocabulary but never contribute to it.

2. **Deploy path computation is corpus-only.** `DeriveCorpusDeployStatus` operates exclusively on corpus files. Inbox files have no deploy paths.

3. **Health signals use zone-scoped signal types.** `HealthyFileSignal` vs `InboxHealthySignal` remain distinct types even though the derive logic is shared. This ensures signal queries and GC are zone-isolated.

4. **Cross-zone queries are explicit.** `DetectInboxCorpusMatches` reads both zones — this is deliberate and should be encoded as a function taking `(TaggedZone, TaggedZone)` rather than being generic over a single zone.

### Signal Type Strategy

Two approaches for zone-scoped signals:

**Option A: Generic signal types.** `FilePresenceSignal<Z: AudioZone>` — single type parameterized by zone. Pros: maximum code sharing. Cons: may complicate signal table storage (need zone discriminant in signal key or separate tables per zone-signal combo).

**Option B: Associated types on zone traits.** Each zone declares its own signal types (current approach, formalized). Pros: signal tables remain simple, GC is straightforward. Cons: signal type declarations are duplicated (but the derive/emit logic is shared).

**Recommendation: Option B.** Signal storage is a DB concern and benefits from simple, predictable table names. The duplication is in type declarations (trivial), not in logic.

### Migration Path

1. Define the zone traits in `src/db/zones.rs` (or `src/zones.rs` — cross-cutting concern)
2. Implement for `Corpus`, `Inbox`, `Library`
3. Convert tag query functions to be zone-generic (starting with the already-internally-generic `get_audio_files_with_tag_presence_for_zone`)
4. Convert `DeriveCorpusSignals`/`DeriveInboxSignals` to `derive_zone_signals<Z: TaggedZone>`
5. Convert detection computations one at a time (missing tags, compound tags)
6. Collapse intake gatherers into zone-generic domain query

## Part 2: Inbox Collapse

With zone generics in place, the inbox-specific stateful loaders dissolve:

### `gather()` / `gather_inbox()` → `GetUnindexedFiles { zone }`

Both functions:
1. Query unindexed file signals for their zone
2. Check file existence on disk (this is the FS-touching part)
3. Group by directory, count bytes
4. Build `IntakeConfirmationState`

With zone generics, step 1 becomes `db.get_unindexed_signals::<Z>()`. Steps 2-4 are zone-agnostic.

The FS existence check (step 2) is the only part that doesn't fit the pure-DB domain query model. Two options:

- **Accept it:** The domain query does the stat calls inside `execute()`. This is pragmatic — the query runs on the cache thread, which is already allowed to do blocking work.
- **Split it:** Domain query returns unindexed signal data; a separate step (UI-side or Witch-side) does FS checks. Cleaner separation but adds a round-trip.

Recommendation: accept it for now. The FS watcher (Part 3) will eventually eliminate the need for these stat calls entirely — the watcher will already know what's on disk.

### `gather_startup()` → just call the zone-generic query twice

`gather_startup()` currently calls `gather()` then `gather_inbox()` and merges. With a zone-generic query, the startup path calls it for each configured zone and merges the results.

### Signal Consolidation

Currently, `UnindexedFileSignal` and `InboxUnindexedSignal` are separate signal types stored in separate tables. With zone generics, they could share a derive path while remaining distinct signal types (Option B from above). The intake UI consumes them identically.

## Part 3: FS Watcher Thread

### Architecture

Replace polling-based `WalkCorpus` → `ScanCorpusDirectory` with a persistent filesystem watcher thread.

```
                    ┌─────────────────────┐
                    │   FS Watcher Thread  │
                    │                      │
                    │  inotify/fanotify    │
                    │  per zone root       │
                    │                      │
                    │  Arc<RwLock<         │
                    │    ZoneState {       │
                    │      dirty: HashSet, │
                    │      initial_done,   │
                    │    }                 │
                    │  >>                  │
                    └──────────┬───────────┘
                               │ (writes)
                               ▼
                    ┌──────────────────────┐
          (reads)   │  Shared State Handle │
     ┌──────────────┤                      │
     │              │  per-zone RwLock'd   │
     │              │  observed inode sets  │
     │              └──────────────────────┘
     ▼
┌────────────┐
│   Witch    │
│            │──── weaves watcher events into
│            │     computation pipeline
└────────────┘
```

### Startup Sequence (New)

1. DB migrations, config load, basic Witch setup
2. Spawn FS watcher thread, give it zone roots (corpus, inbox, library dirs)
3. Watcher does **initial enumeration** — walks each zone root, builds complete inode→path map, reports as initial state assertion
4. Witch receives initial state, runs `DeriveZoneSignals` against it (same as today's post-walk derivation)
5. Watcher enters **steady state** — kernel events only, no polling
6. Witch reads dirty sets from watcher on each `tick()`, feeds into computation pipeline

### Watcher Thread State

```rust
pub struct FsWatcherHandle {
    /// Per-zone observed state. The watcher writes; the Witch reads.
    corpus: Arc<RwLock<ZoneWatchState>>,
    inbox: Arc<RwLock<ZoneWatchState>>,
    libraries: Arc<RwLock<ZoneWatchState>>,
    /// Control channel for shutdown, zone root changes, etc.
    control_tx: Sender<WatcherControl>,
}

struct ZoneWatchState {
    /// Inodes that have changed since last drain.
    /// The Witch drains this set on each tick.
    dirty_inodes: HashSet<i64>,
    /// Whether initial enumeration is complete for this zone.
    initial_scan_complete: bool,
    /// Full inode→path map (authoritative FS state).
    /// Updated by watcher in real-time.
    known_inodes: HashMap<i64, PathBuf>,
}
```

### Integration with Witch

The Witch's `tick()` currently drains `result_rx` for completed rayon tasks. With the FS watcher, it additionally:

```rust
fn tick(&mut self) {
    // ... existing rayon result drain ...

    // Drain FS watcher dirty sets
    if let Some(ref watcher) = self.fs_watcher {
        let corpus_dirty = watcher.drain_dirty(Zone::Corpus);
        let inbox_dirty = watcher.drain_dirty(Zone::Inbox);

        if !corpus_dirty.is_empty() {
            self.queue_verify_for_inodes(Zone::Corpus, corpus_dirty);
        }
        if !inbox_dirty.is_empty() {
            self.queue_verify_for_inodes(Zone::Inbox, inbox_dirty);
        }
    }
}
```

This replaces the idle rescan timer entirely. Changes are detected in real-time via kernel events.

### What the Watcher Replaces

| Current | Watcher-Based |
|---------|---------------|
| `WalkCorpus` (full directory tree walk) | Initial enumeration (one-time), then kernel events |
| `ScanCorpusDirectory` (per-dir stat + read_dir) | No longer needed — watcher reports changes |
| `maybe_start_idle_rescan()` (180s timer) | Eliminated — changes are immediate |
| `VerifyMtime` as gating step | Watcher already knows what changed; skip mtime comparison |
| `gather()`/`gather_inbox()` stat checks | Watcher's `known_inodes` map is the truth |

### Library Considerations

Library zone is simpler — library files are deployment targets, not source material. The watcher monitors library roots for leftover detection (files appearing/disappearing that weren't placed by MM). This replaces `execute_walk_library()`.

### Fallback: Periodic Full Scan

Even with a watcher, a periodic full-scan backstop is wise (inotify can miss events under heavy load, after suspend/resume, etc.). This becomes a low-frequency (hourly?) consistency check rather than the primary observation mechanism.

### Platform Considerations

- **Linux:** `inotify` (per-directory watches) or `fanotify` (per-mount, requires `CAP_SYS_ADMIN`). inotify is simpler but requires a watch per directory; fanotify is more efficient for large trees.
- **Crate:** `notify` crate provides cross-platform abstraction. Reasonable starting point.
- **Watch limits:** `/proc/sys/fs/inotify/max_user_watches` may need increasing for large corpus trees. The watcher should detect `ENOSPC` and fall back to polling with a warning.

## Dependency Order

```
1. Zone Generics (trait hierarchy + zone impls)
   │
   ├── Unified tag queries
   ├── Unified derive computations
   ├── Unified detection computations
   │
   ▼
2. Inbox Collapse
   │
   ├── gather/gather_inbox → zone-generic domain query
   ├── Startup intake → zone-parameterized
   ├── Remaining stateful loader closures dissolve
   │
   ▼
3. FS Watcher Thread
   │
   ├── Watcher thread + shared state
   ├── Witch integration (tick drain)
   ├── Startup sequence rewrite
   ├── Idle rescan elimination
   └── gather() stat checks eliminated (watcher knows FS state)
```

Each step is independently useful and leaves the system fully functional. Zone generics reduce duplication immediately. Inbox collapse simplifies the closure migration. FS watcher improves responsiveness and eliminates polling overhead.
