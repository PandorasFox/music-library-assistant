# Domain Query Layer Strategy

This document defines the long-term strategy for transforming MM's read path into a formal domain query vocabulary. The goal: any client (TUI, web API, future tooling) communicates with the Witch through the same typed protocol. The Witch is the sole authority; clients are interchangeable consumers of Her services.

## Architectural Context

Today the TUI communicates with the Witch's cache thread via two mechanisms:

1. **Demand-flag polling** (`want_insights()` → `CacheReady::Insights(...)`) — periodic, throttled refreshes of view-shaped data blobs
2. **Closure escape hatch** (`cache.query(|db| ...)`) — ad-hoc one-shot queries using raw `ReadOnlyDb` access

Both are TUI-specific. The demand-flag protocol assumes a single consumer polling in a render loop. The closure escape hatch passes raw DB handles through an untyped channel, making it impossible to serve the same queries over HTTP or any other transport.

The target architecture replaces both with a **typed domain query vocabulary** that any client can speak.

## The Witch Owns All Read Access

The TUI is a child thread the Witch spawns. A web server would be another child. Both are equal clients — neither "has a handle to the Witch" in any ownership sense. They have a channel to Her, nothing more.

Read access must flow through the Witch's channel (or a read-specific channel She provides), not through direct `ReadOnlyDb` handles leaked to client code. This ensures:

- The Witch can enforce access patterns, throttling, and caching centrally
- New clients get read access by speaking the protocol, not by receiving a DB handle
- The query surface is auditable and versionable as a typed enum

## Query Shape: Summary vs Detail

Every domain query falls into one of two categories. Respecting this distinction is critical for keeping the TUI responsive and the web API efficient.

### Summary Queries

Cheap aggregate data for populating lists, dashboards, and overview screens. No inode resolution. Returns counts, group keys, and lightweight metadata.

```
CorpusSummary        → file counts, index state, zone stats
InboxOverview        → inbox file counts by category
InsightsSummary      → signal group counts by type
DeployStatus         → library health, stale/leftover counts
ExternalMatchBuckets → match counts by confidence tier
PackingDirectories   → directory list with packing categories
EditSessions         → session list with timestamps and edit counts
MissingTagGroups     → list of (tag_name, affected_count)
CompoundTagGroups    → list of (tag_name, group_count, safe_count)
```

These are the queries that refresh on intervals (the current `CacheReady` variants). A web client might poll these or subscribe via SSE.

### Detail Queries

Expensive, fully-enriched data for a specific item the operator has selected. Triggered by user action (opening a modal, drilling into a group), not by periodic refresh.

```
MissingTagDetail     { tag_name }     → Vec<{ inode, path, existing_tags }>
CompoundSplitDetail  { group }        → full split preview with file metadata
SessionEdits         { session_id }   → edits with inode→path resolution
OobConflictDetail    { inode }        → tag diff (db vs disk)
ExternalMatchDetail  { recording_ids} → recording cache + release summaries
AudioFilesByInodes   { inodes, zone } → resolved file metadata
TagEditFiles         { directory }    → files with tags for editing
```

**The key rule: detail queries are self-contained.** If a query requires inode resolution to be useful to the caller, the resolution happens *inside* the query. Callers must never need a follow-up `get_audio_files_by_inodes` round-trip. The inode→file join is an implementation detail, not a domain concept.

### Why Not Just Join Everything?

Eager joining kills responsiveness. A modal showing 50 signal groups should open instantly with counts, then resolve file details only for the group the operator selects. The summary/detail split maps directly to this UX pattern:

1. Open modal → summary query (instant)
2. Select item → detail query (heavier, but scoped)

Web API consumers benefit identically: list endpoints are fast, detail endpoints do the work.

## Consolidation: The `get_audio_files_by_inodes` Pattern

The single most common ad-hoc query (~10 callsites) is fetching signal data, then immediately resolving the embedded inodes to file metadata in a second round-trip. This two-step pattern should not leak into the domain vocabulary.

**Current pattern (eliminate):**
```rust
let signals = cache.query(|db| db.get_missing_tag_signals()).recv();
let inodes: Vec<i64> = signals.iter().flat_map(|s| &s.inodes).collect();
let files = cache.query(|db| db.get_audio_files_by_inodes(&inodes, Zone::Corpus)).recv();
// manually zip signals + files
```

**Target pattern:**
```rust
// Single domain query, returns enriched data
let detail = witch.query(MissingTagDetail { tag_name }).await;
// detail.entries: Vec<{ inode, path, tags, signal_data }>
```

`get_audio_files_by_inodes` may still exist as an internal helper within query implementations, but it is not a domain query that clients should ever need to call directly.

## The Closure Escape Hatch

`cache.query(|db| ...)` currently accepts arbitrary closures over `ReadOnlyDb`. This is the primary migration target — every callsite needs to become a named `DomainQuery` variant.

The closure form has served well for rapid iteration, but it is:

- **Untyped** — the channel carries `Box<dyn FnOnce>`, invisible to any protocol layer
- **Non-serializable** — cannot be sent over HTTP, logged, or traced
- **Unreviewable** — the actual query is buried in a closure at the callsite

Each closure callsite should be examined and converted to either a summary or detail query variant. Some callsites are composite (fetch signals + resolve inodes + compute derived state) — these become single self-contained detail queries.

## Modal Init Loaders

Several modals already have composite loader functions (`DeployModalData::load(db)`, `ManualReviewData::load(db, kind)`, `IntakeConfirmationState::gather(db, ...)`). These are the right shape — named, typed, self-contained. They just receive a raw `&ReadOnlyDb` instead of being invoked through the query protocol.

Migration path: wrap each loader as a `DomainQuery` variant. The implementation calls the same loader function internally. The external interface becomes protocol-driven.

## Cache and Throttle Layer

The cache thread's throttle logic (per-query `_last_refreshed` timestamps, configurable intervals) remains valuable but becomes an implementation detail of the query service, not something clients interact with.

Clients express **demand** ("I want corpus summary data"), not **timing** ("refresh if stale"). The query service decides whether to serve cached data or refresh. This maps to:

- **TUI**: demand flags per frame, same as today, but over the domain protocol
- **Web API**: each request is implicit demand; the service decides freshness
- **SSE/WebSocket**: the service pushes when cached data refreshes

## Trait-Driven Design (Macro-Forward)

The query vocabulary must be designed with proc-macro extraction as an explicit goal. This means **deliberate uniformity** — every query must fit the same trait shape, no exceptions. Resist the temptation to add one-off convenience methods or special-case certain queries. If a query doesn't fit the trait, reshape the query, not the trait.

### Core Traits

```rust
/// Every domain query implements this. The trait is the contract
/// that a future proc-macro will generate against.
trait DomainQuery: Serialize + Send + 'static {
    type Response: Serialize + Send + 'static;

    /// All inputs come from `self`. All outputs go in `Response`.
    /// No side channels, no &mut, no extra context parameters.
    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response;
}

/// Summary queries that benefit from throttled caching.
/// Detail queries implement only DomainQuery.
trait CachedQuery: DomainQuery {
    const THROTTLE: Duration;
}
```

### Uniformity Rules

These rules exist so that a proc-macro can eventually generate the plumbing for any query from a struct definition + attribute annotation alone:

1. **All inputs are fields on the query struct.** If a query needs config, the caller puts it in the struct. If it needs a path prefix, that's a field. No reaching into ambient state.

2. **All outputs are in the `Response` type.** No side-channel `Sender<T>`, no writing to shared state, no logging-as-output. The response struct is the complete result.

3. **`execute` takes `self` by value and `&ReadOnlyDb` — nothing else.** This is the rigid function signature the macro will target. If you need something beyond the DB (e.g., config for a deploy query), it goes in the query struct as a field populated by the caller.

4. **Response types are `Serialize`.** Every response must be serializable from day one, even before the web API exists. This catches "I stuck a `PathBuf` with platform-specific semantics in here" problems early and ensures web-readiness is never a retrofit.

5. **No trait objects or dynamic dispatch in query/response types.** Enum variants, concrete structs, `Vec<T>` — things a derive macro and serde can reason about statically.

6. **Identical error handling.** Queries return their `Response` type, not `Result`. The `execute` implementation handles errors internally (defaulting, logging, returning empty collections). The caller gets a usable value, always. This keeps the dispatch layer trivial — no per-query error-handling logic.

### The `define_domain_query!` Macro

The macro is implemented as `macro_rules!` in `src/db/domain.rs`. It supports four forms:

```rust
// Simple cached: unit struct, single db method, unwrap_or_default
define_domain_query! {
    /// Doc comment
    GetFoo => FooData, cached(15), db.get_foo_data()
}

// Body cached: custom execute logic with db in scope
define_domain_query! {
    /// Doc comment
    GetBar => BarData, cached(30), |db| {
        let x = db.get_x().unwrap_or_default();
        let y = db.get_y().unwrap_or_default();
        BarData { x, y }
    }
}

// Simple uncached (detail query):
define_domain_query! {
    /// Doc comment
    GetBaz => BazData, uncached, db.get_baz_data()
}

// Body uncached (detail query):
define_domain_query! {
    /// Doc comment
    GetQux => QuxData, uncached, |db| { ... }
}
```

Each invocation generates:
- The query struct with `#[derive(Serialize, Deserialize)]`
- The `DomainQuery` impl with the specified response type and execute body
- The `CachedQuery` impl (if `cached`) with the throttle duration

**When adding new queries, always use the macro.** If a query doesn't fit the macro's forms, that's a signal to reshape the query (or extend the macro with a new form), not to hand-write the impls.

## Migration Strategy

This is incremental work. The TUI continues functioning throughout. The trait-driven design is not a later phase — it applies from the first query converted.

1. ~~**Establish the `DomainQuery` trait and dispatch infrastructure.**~~ **DONE.** Traits defined in `src/db/domain.rs`. Cache thread refresh path routes through `DomainQuery::execute()`.

2. ~~**Convert `CacheReady` variants to trait-implementing summary query structs.**~~ **DONE.** All 6 variants (Insights, InboxOverview, DeployStatus, EditHistory, ExternalMatches, PackingDirs) converted. `Serialize` added to all response types and their transitive dependencies.

3. ~~**Extract `define_domain_query!` macro.**~~ **DONE.** `macro_rules!` macro in `src/db/domain.rs` handles simple/body × cached/uncached forms. All 6 summary queries use the macro.

4. **Convert closure callsites to detail query structs.** **MOSTLY DONE.** Each `cache.query(|db| ...)` becomes a named struct with fields for its parameters. `CacheHandle::domain_query()` method added as the typed entry point. 38 domain query types defined, 36 tests passing. Callsite conversions complete:
   - **Wave 1** (13 callsites): OOB sync/bucketed/moved files, packing knots/paths, compound splits, tag canonicity keys, missing album singles, edit history export
   - **Wave 2** (signal key queries): InconsistentAlbumArtist keys, TagCanonicity keys, InboxTagCanonicity keys, DiscExtraction with paths
   - **Wave 3** (modal init loaders — 14 callsites): `start_resolution!` macro (5 uses), ShitFormat, ReleaseOverlap, InboxCorpusMatch, Deploy, ManualReview, CorpusTags, PackingBrowser, UnsolvedPacking
   - **Wave 4** (composite queries — 5 callsites): MissingTagAudioFiles, AudioFilesByInodes (2 uses), SessionEditDetail, CurrentTagValues, AllAudioFilesWithTags
   - `Serialize` added to `FileEntry`, `AudioInfo`, `AudioFile`, and ~30 modal/view types

   **Remaining ~17 closures** — all require design work beyond mechanical conversion:
   - **Stateful loaders** (6): `IntakeConfirmationState::gather_*` (3), `InboxOrganizeState::load_*` (1), `CompoundSplitDataV2::from_compound_group` (2 — filesystem I/O: read tags from disk)
   - **Disk-touching queries** (2): `compute_tag_diff` (compares DB tags vs on-disk tags)
   - **Complex composites** (4): tag editor ops (2, file resolution + tag loading), MusicBrainz recording detail (1, JSON parsing), tag search file matching (1, full corpus scan + tag resolution)
   - **Tag canonicity signal dispatch** (1): dispatches to 3 different signal kinds each building modal data with secondary DB reads
   - **Startup/tick** (2): startup intake gather, tick-based polling

5. **Eliminate `get_audio_files_by_inodes` as a client-facing query.** It remains as an internal helper within `execute` implementations, but no client should ever call it directly.

6. **Remove the closure escape hatch** once all callsites are converted.

At any point after step 2, a web server can be introduced as a second client speaking the same protocol. The `Serialize` bound on all response types (enforced from step 1) guarantees web-readiness without retrofit.

## Non-Goals

- **Replacing the Witch's internal read patterns.** Computations running on rayon threads use their own thread-local `ReadOnlyDb` connections. That is internal to the Witch's domain, not client-facing, and stays as-is.
- **Real-time streaming for all data.** SSE/WebSocket is a future optimization for lively data. The initial web API can be stateless request/response.
- **Multi-user access control.** MM is a single-operator system. Auth is "prove you're the operator" (bearer token or session), not role-based access.
