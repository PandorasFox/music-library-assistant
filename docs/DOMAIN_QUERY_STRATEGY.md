# Domain Query Layer Strategy

This document defines the strategy for MM's read path: a formal domain query vocabulary where any client (TUI, web API, future tooling) communicates with the Witch through the same typed protocol. The Witch is the sole authority; clients are interchangeable consumers of Her services.

## Current State

The read path is fully protocol-driven. All client→DB reads flow through typed `DomainQuery` structs:

- **Summary queries** — periodic, throttled, scope-invalidated via `CachedQuery` trait
- **Detail queries** — one-shot, triggered by user action via `CacheHandle::domain_query()`

38 domain query types are defined. Zero closure callsites remain. The closure escape hatch (`cache.query(|db| ...)`) is dead — `query()` is private, only reachable through `domain_query::<Q>()`.

## The Witch Owns All Read Access

The TUI is a child thread the Witch spawns. A web server would be another child. Both are equal clients — neither "has a handle to the Witch" in any ownership sense. They have a channel to Her, nothing more.

Read access must flow through the Witch's channel (or a read-specific channel She provides), not through direct `ReadOnlyDb` handles leaked to client code. This ensures:

- The Witch can enforce access patterns, throttling, and caching centrally
- New clients get read access by speaking the protocol, not by receiving a DB handle
- The query surface is auditable and versionable as a typed enum

## Query Shape: Summary vs Detail

Every domain query falls into one of two categories. Respecting this distinction is critical for keeping the TUI responsive and the web API efficient.

### Summary Queries

Cheap aggregate data for populating lists, dashboards, and overview screens. No inode resolution. Returns counts, group keys, and lightweight metadata. These refresh on intervals via the generic cache. A web client might poll these or subscribe via SSE.

Currently 6 cached summary queries, each defined with scope-based invalidation:

| Query | Scope | Throttle | Urgent |
|-------|-------|----------|--------|
| `GetInsights` | `TAGS \| FILES` | 30s | — |
| `GetInboxOverview` | `FILES \| INBOX` | 15s | — |
| `GetDeployStatus` | `DEPLOY \| FILES` | 15s | — |
| `GetEditHistory` | `TAGS` | 30s | — |
| `GetExternalMatches` | `EXTERNAL` | 15s | 5s |
| `GetPackingDirs` | `EXTERNAL \| FILES` | 30s | — |

### Detail Queries

Expensive, fully-enriched data for a specific item the operator has selected. Triggered by user action (opening a modal, drilling into a group), not by periodic refresh.

**The key rule: detail queries are self-contained.** If a query requires inode resolution to be useful to the caller, the resolution happens *inside* the query. Callers must never need a follow-up `get_audio_files_by_inodes` round-trip. The inode→file join is an implementation detail, not a domain concept.

### Why Not Just Join Everything?

Eager joining kills responsiveness. A modal showing 50 signal groups should open instantly with counts, then resolve file details only for the group the operator selects. The summary/detail split maps directly to this UX pattern:

1. Open modal → summary query (instant)
2. Select item → detail query (heavier, but scoped)

Web API consumers benefit identically: list endpoints are fast, detail endpoints do the work.

## Generic Cache Infrastructure

The cache thread manages a `TypeId`-keyed `GenericCache` where each slot is a `RegisteredSlot` carrying:
- Execute closure (reconstructs the query via `Q::default()` and runs it)
- Throttle duration and optional urgent throttle
- `RecomputationScope` — which mutation domains invalidate this entry
- Demand flags (`wanted`, `urgent`)

### Adding a Cached Query

Three touch points:

1. **Define** with scope in `src/db/domain.rs`:
   ```rust
   define_domain_query! {
       GetFoo => FooData, cached(30, TAGS | FILES), db.get_foo_data()
   }
   ```

2. **Signal demand** in the event loop:
   ```rust
   app.cache.want::<GetFoo>();
   ```

3. **Read** in view code:
   ```rust
   app.cached.get::<GetFoo>()
   ```

### Scope-Based Invalidation

When mutations complete, the Witch sends `invalidate_scope(session_recomputation_scope)` — only cache slots whose `SCOPE` overlaps the mutation scope are invalidated. A tag edit doesn't re-query deploy status; a library deploy doesn't re-query edit history.

### Demand-Not-Timing

Clients express **demand** ("I want this data"), not **timing** ("refresh if stale"). The cache thread decides whether to serve cached data or refresh:

- **TUI**: `want::<Q>()` per frame — cache thread throttles
- **Web API**: each request is implicit demand; the service decides freshness
- **SSE/WebSocket**: the service pushes when cached data refreshes

## Trait-Driven Design (Macro-Forward)

The query vocabulary is designed with proc-macro extraction as an explicit goal. **Deliberate uniformity** — every query fits the same trait shape, no exceptions.

### Core Traits

```rust
trait DomainQuery: Send + 'static {
    type Response: Serialize + Send + 'static;
    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response;
}

trait CachedQuery: DomainQuery + Default {
    const THROTTLE: Duration;
    const SCOPE: RecomputationScope;
    const URGENT_THROTTLE: Option<Duration> = None;
}
```

### Uniformity Rules

These rules exist so that a proc-macro can eventually generate the plumbing for any query from a struct definition + attribute annotation alone:

1. **All inputs are fields on the query struct.** If a query needs config, the caller puts it in the struct. If it needs a path prefix, that's a field. No reaching into ambient state.

2. **All outputs are in the `Response` type.** No side-channel `Sender<T>`, no writing to shared state, no logging-as-output. The response struct is the complete result.

3. **`execute` takes `self` by value and `&ReadOnlyDb` — nothing else.** This is the rigid function signature the macro targets. If you need something beyond the DB (e.g., config for a deploy query), it goes in the query struct as a field populated by the caller.

4. **Response types are `Serialize`.** Every response must be serializable from day one, even before the web API exists. This catches "I stuck a `PathBuf` with platform-specific semantics in here" problems early and ensures web-readiness is never a retrofit.

5. **No trait objects or dynamic dispatch in query/response types.** Enum variants, concrete structs, `Vec<T>` — things a derive macro and serde can reason about statically.

6. **Identical error handling.** Queries return their `Response` type, not `Result`. The `execute` implementation handles errors internally (defaulting, logging, returning empty collections). The caller gets a usable value, always. This keeps the dispatch layer trivial — no per-query error-handling logic.

### The `define_domain_query!` Macro

The macro is implemented as `macro_rules!` in `src/db/domain.rs`. Supported forms:

```rust
// Simple cached with scope: unit struct, single db method, unwrap_or_default
define_domain_query! {
    /// Doc comment
    GetFoo => FooData, cached(15, TAGS | FILES), db.get_foo_data()
}

// Simple cached with urgent throttle:
define_domain_query! {
    /// Doc comment
    GetFoo => FooData, cached(15, EXTERNAL, urgent(5)), db.get_foo_data()
}

// Body cached: custom execute logic with db in scope
define_domain_query! {
    /// Doc comment
    GetBar => BarData, cached(30, TAGS), |db| {
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

// Modal load shorthand: unit struct, Response::load(db).ok().unwrap_or_default()
define_domain_query! {
    /// Doc comment
    GetModal => ModalData, uncached, modal_load
}

// Parameterized: struct with fields, custom execute body
define_domain_query! {
    /// Doc comment
    GetQuux { field1: Type1, field2: Type2 } => QuuxData, uncached, |s, db| {
        db.some_query(&s.field1, s.field2).unwrap_or_default()
    }
}
```

Each invocation generates:
- The query struct with `#[derive(Serialize, Deserialize)]` (and `Default` for cached)
- The `DomainQuery` impl with the specified response type and execute body
- The `CachedQuery` impl (if `cached`) with throttle, scope, and optional urgent throttle

**When adding new queries, always use the macro.** If a query doesn't fit the macro's forms, that's a signal to reshape the query (or extend the macro with a new form), not to hand-write the impls.

## Migration History

All phases complete:

1. ~~**Establish the `DomainQuery` trait and dispatch infrastructure.**~~ Traits defined in `src/db/domain.rs`. Cache thread refresh path routes through `DomainQuery::execute()`.

2. ~~**Convert `CacheReady` variants to trait-implementing summary query structs.**~~ All 6 variants converted. `Serialize` added to all response types and their transitive dependencies.

3. ~~**Extract `define_domain_query!` macro.**~~ `macro_rules!` macro handles all forms. All 6 summary queries use the macro.

4. ~~**Convert closure callsites to detail query structs.**~~ All closures converted across 4 waves. 38 domain query types defined. `CacheHandle::domain_query()` is the sole entry point. `Serialize` added to `FileEntry`, `AudioInfo`, `AudioFile`, and ~30 modal/view types.

5. ~~**Eliminate `get_audio_files_by_inodes` as a client-facing query.**~~ Folded into self-contained detail queries. Remains as an internal DB helper only.

6. ~~**Remove the closure escape hatch.**~~ Zero `cache.query(|db| ...)` callsites remain. `query()` is private, only reachable through `domain_query::<Q>()`. The `CacheRequest::Query` variant still exists as internal plumbing for `domain_query()` but is not exposed to clients.

7. ~~**Generic cache with scope-based invalidation.**~~ Hardcoded `ThrottleState` replaced with `TypeId`-keyed `GenericCache`. `CachedQuery` trait carries `SCOPE` and `URGENT_THROTTLE`. Invalidation is targeted by `RecomputationScope`, not blanket. Adding a cached query is 3 touch points instead of 8.

## Future Work

- **Web API client**: can be introduced at any time — all response types are `Serialize`, all queries are protocol-driven. The cache infrastructure is client-agnostic.
- **Proc-macro extraction**: the `macro_rules!` macro can be promoted to a proc-macro for richer compile-time validation and code generation when the query surface stabilizes.
- **Typed one-shot protocol**: `CacheRequest::Query` still carries `Box<dyn FnOnce>` internally. Could be replaced with a typed enum dispatch, but the untyped form is sealed behind `domain_query::<Q>()` so this is polish, not architecture.

## Non-Goals

- **Replacing the Witch's internal read patterns.** Computations running on rayon threads use their own thread-local `ReadOnlyDb` connections. That is internal to the Witch's domain, not client-facing, and stays as-is.
- **Real-time streaming for all data.** SSE/WebSocket is a future optimization for lively data. The initial web API can be stateless request/response.
- **Multi-user access control.** MM is a single-operator system. Auth is "prove you're the operator" (bearer token or session), not role-based access.
