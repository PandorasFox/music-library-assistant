# Client-Server Architecture Roadmap

This document describes MM's planned evolution from a monolithic TUI application to a client-server architecture where the Witch operates as a Unix socket server with multiple client types.

## Motivation

- Web UI + API alongside TUI for self-hosted music management
- Auth subsystem with session-based access control
- Decoupling the Witch's event loop from the TUI frame clock
- Natural follow-on from the domain query system (45+ queries already `Serialize`-ready)

## Current Architecture

### System Topology

```
┌─────────────────────────────────────────────────────────┐
│  mm (single binary)                                     │
│                                                         │
│  ┌──────────┐   &mut Witch   ┌────────────────────┐    │
│  │   TUI    │ ◄────────────► │      Witch         │    │
│  │ (ratatui)│                │  (orchestrator)     │    │
│  └──────────┘                │                     │    │
│       │                      │  ┌──────────────┐   │    │
│       │ CacheHandle          │  │  db_thread    │   │    │
│       ▼                      │  │ (single writer)│  │    │
│  ┌──────────┐                │  └──────────────┘   │    │
│  │  Cache   │                │  ┌──────────────┐   │    │
│  │  Thread  │                │  │  fs_watcher   │   │    │
│  └──────────┘                │  └──────────────┘   │    │
│                              │  ┌──────────────┐   │    │
│                              │  │  rayon pool   │   │    │
│                              │  └──────────────┘   │    │
│                              │  ┌──────────────┐   │    │
│                              │  │  ext. fetch   │   │    │
│                              │  │  scheduler    │   │    │
│                              │  └──────────────┘   │    │
│                              └────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

### Thread Inventory

| Thread | Owner | Purpose | Communication |
|--------|-------|---------|---------------|
| Main (TUI) | `run_app()` | Event loop + rendering | `&mut Witch` direct calls |
| db_thread | Witch | Serialized DB writes | `mpsc::Sender<DbOp>` |
| cache_thread | Witch (via `CacheHandle`) | Read-only DB queries for UI | `CacheRequest` / `CacheReady` channels |
| fs_watcher | Witch | inotify + polling fallback | `mpsc::Sender<FsEvent>` |
| rayon pool | Witch | CPU-bound computation dispatch | `mpsc::Sender<TaskResult>` |

### Channel Map

```
TUI ──► Witch: direct &mut method calls (same thread via tick())
Witch ──► db_thread: mpsc::Sender<DbOp>
Witch ──► rayon pool: rayon::spawn() closures
rayon pool ──► Witch: mpsc::Sender<TaskResult>
fs_watcher ──► Witch: mpsc::Sender<FsEvent>
cache_thread ◄──► UI: CacheRequest / CacheReady channels
Witch ──► UI: mpsc::Receiver<WitchNotice> (status, mutations completed, errors)
```

### Witch Pub Interface Catalogue

**State inspection:**
- `reasoning_level() -> ReasoningLevel`
- `is_initial_scanning() -> bool`
- `needs_schema_update() -> bool`
- `pending_schema_descriptions() -> Vec<String>`

**Transaction API:**
- `start_transaction(label) -> Result<(), TransactionError>`
- `has_transaction() -> bool`
- `transaction_summary() -> Option<(&str, usize, usize)>`
- `add_decision(key, decision) -> Result<(), TransactionError>`
- `get_decision(key) -> Option<&WitnessedDecision>`
- `decision_keys() -> Vec<DecisionKey>`
- `remove_decision(key, gesture) -> Result<(), TransactionError>`
- `confirm_transaction(gesture) -> Result<(), TransactionError>`
- `discard_transaction() -> Result<DiscardSummary, TransactionError>`

**External fetch:**
- `request_external_fetch()`
- `is_external_fetch_active() -> bool`
- `external_fetch_progress() -> Option<&FetchProgress>`
- `has_acoustid_api_key() -> bool`
- `request_release_packing()`

**Maintenance:**
- `queue_schema_reconciliation(gesture)`
- `queue_vacuum(gesture)`
- `latch_read_only_for_safety(reason)`

**Notice stream (push):**
- `WitchNotice::StatusUpdate(WorkStatus)`
- `WitchNotice::MutationsCompleted`
- `WitchNotice::Error(String)`
- `WitchNotice::SafetyLatch(String)`
- `WitchNotice::ConfigUpdated`

---

## Target Architecture

```
                    Unix Socket
                        │
        ┌───────────────┤
        │               │
   TUI client    HTTP server
   (mm tui)      (mm serve)
                 (forked proc)
```

The Witch becomes a Unix socket server. ALL clients — TUI, HTTP server — connect through the same socket and authenticate the same way. No special-casing.

The HTTP server is itself a client of the Witch — a forked process that accepts HTTP connections and translates them into protocol messages over the socket. No HTTP code inside the Witch.

---

## Protocol Design

Protocol types live in `meta/protocol.rs` (extracted to `mm-protocol` crate at workspace split).

### Message Types

**`WitchQuery`** — read-only requests (~15 variants mapping to Witch read methods):

| Variant | Maps to | Response |
|---------|---------|----------|
| `Status` | `WitchNotice::StatusUpdate` stream | `WorkStatus` |
| `ReasoningLevel` | `reasoning_level()` | `ReasoningLevel` |
| `HasPending` | `has_transaction()` + decision count | `bool` |
| `IsInitialScanning` | `is_initial_scanning()` | `bool` |
| `DbQueueDepth` | internal queue len | `usize` |
| `HasTransaction` | `has_transaction()` | `bool` |
| `TransactionSummary` | `transaction_summary()` | `Option<TransactionSummaryData>` |
| `DecisionKeys` | `decision_keys()` | `Vec<DecisionKey>` |
| `GetDecision(DecisionKey)` | `get_decision(key)` | serialized decision data |
| `HandledDecisionKinds` | kind→handler mapping | `Vec<DecisionKeyKind>` |
| `IsExternalFetchActive` | `is_external_fetch_active()` | `bool` |
| `ExternalFetchProgress` | `external_fetch_progress()` | `Option<FetchProgress>` |
| `HasAcoustIdApiKey` | `has_acoustid_api_key()` | `bool` |
| `NeedsSchemaUpdate` | `needs_schema_update()` | `bool` |
| `PendingSchemaDescriptions` | `pending_schema_descriptions()` | `Vec<String>` |

DomainQuery dispatch stays as the existing trait-based system. Domain queries are dispatched through the cache thread's `domain_query()` mechanism, not folded into `WitchQuery`. The protocol will carry serialized domain queries as opaque payloads.

**`WitchCommand`** — write/action requests (~10 variants):

| Variant | Maps to | Auth level |
|---------|---------|------------|
| `StartTransaction { label }` | `start_transaction()` | AuthRequired |
| `AddDecision { key, label, mutations }` | `add_decision()` | AuthRequired |
| `RemoveDecision { key }` | `remove_decision()` | AuthRequired |
| `ConfirmTransaction` | `confirm_transaction()` | AuthRequired |
| `DiscardTransaction` | `discard_transaction()` | AuthRequired |
| `RequestExternalFetch` | `request_external_fetch()` | AuthRequired |
| `RequestReleasePacking` | `request_release_packing()` | AuthRequired |
| `QueueSchemaReconciliation` | `queue_schema_reconciliation()` | AuthRequired |
| `QueueVacuum` | `queue_vacuum()` | AuthRequired |
| `LatchReadOnlyForSafety { reason }` | `latch_read_only_for_safety()` | AuthRequired |

**`QueryResponse`** — typed response variants matching each query.

**`CommandResponse`** — `Ok` | `TransactionError(TransactionError)`.

**`ProtocolError`** — error conditions at the protocol level:
- `Transaction(TransactionError)` — transaction state violation
- `InvalidSession` — session token not recognized
- `Unauthorized` — insufficient authorization level
- `NotReady` — Witch not yet initialized
- `Internal(String)` — unexpected server error

### Wire Format

Messages are length-prefixed, serde-serialized (JSON for debugging, bincode for production — format selection deferred). Each request carries a `SessionId`. Each response carries a correlation ID matching the request.

### TransactionSummaryData

Wire-safe projection of transaction state:
```rust
struct TransactionSummaryData {
    label: String,
    decision_count: usize,
    mutation_count: usize,
}
```

---

## Authorization Model

ONE auth path. Every client authenticates the same way.

### Session Lifecycle

1. Client connects to Unix socket
2. Client sends `Login` with credentials
3. Witch validates, returns `SessionId`
4. Client includes `SessionId` in every subsequent request
5. Witch checks authorization level per command

### Authorization Levels

| Level | When | Allowed |
|-------|------|---------|
| `FirstTimeSetup` | No users exist | Setup commands only |
| `Unauthenticated` | Before login | Status queries (is system up? needs setup?) |
| `AuthRequired` | After login (default) | Everything |

### ConfirmationGesture Evolution

Currently: compile-time ZST proving an Enter keypress happened in an action handler.

It should remain as such: the TUI still has need of this for its own compile-time structural guarantees, and will remain - just as a boundary for some domain queries it executes.

The Witch's internal witnesses (`MutationExecutionWitness`, `ComputationWitness`, `MaintenanceWitness`) are server-side only and unchanged. They prove execution context, not client identity.

### Auth Infrastructure

Session/token/password management is designed in a separate plan doc. The protocol layer here is auth-aware but not auth-implementing.

Auth will live in a separate thread. There will be a sessions table that other threads/handlers can read and lookup against for authenticated methods. This will be a lightweight touch handled by trait or derives.

---

## Transaction Model

### Current Model

```rust
// Single transaction, single operator
pending_transaction: Option<PendingTransaction>
```

One operator, one staging area. `ConfirmationGesture` required at add/remove/confirm boundaries.

### Target Model

```rust
// Per-session staging
transactions: HashMap<SessionId, PendingTransaction>
```

- Each authenticated session gets its own transaction staging area
- Transactions require explicit `StartTransaction` (no always-open mode across sessions)
- Execution serialization: one transaction executes at a time (db_thread is single-writer)
- When a session confirms, its transaction is serialized through db_thread
- Other sessions' transactions remain staged and unaffected

### Conflict Detection (Deferred)

When two sessions stage mutations touching the same inodes or tags, conflicts are possible. The problem space:

- **Inode-level**: two sessions both editing tags on the same file
- **Tag-level**: two sessions making conflicting canonicity decisions
- **Structural**: one session deletes a file another session is editing

Detection and resolution deferred to a later phase. For now, first-to-commit wins; second commit will encounter stale state and the session will need to refresh.

---

## Tokio Migration Strategy

### Full Async Witch

The Witch's event loop migrates from `tick()` polling to tokio `select!`:

```rust
loop {
    tokio::select! {
        conn = socket_listener.accept() => { /* new client */ }
        result = hades_results.recv() => { /* rayon task completed */ }
        event = fs_events.recv() => { /* filesystem change */ }
        msg = scheduler_rx.recv() => { /* external fetch scheduling */ }
        _ = maintenance_timer.tick() => { /* periodic maintenance */ }
    }
}
```

### Thread → Task Migration

| Current Thread | Becomes |
|---------------|---------|
| db_thread | Async task with tokio mpsc channel |
| fs_watcher | Async inotify task (tokio-inotify) |
| cache_thread | Async task with tokio mpsc channel |
| external_fetch scheduler | Async task |
| rayon pool | (CPU-bound work), supervised by Hades task |

### Hades — Rayon Supervisor

A dedicated async task that owns rayon dispatch:

- Accepts computation/mutation tasks from the Witch
- Dispatches to rayon pool
- Drains results via tokio mpsc
- Chains spawned follow-up computations immediately (no Witch round-trip)
- Tracks in-flight counts
- Exposes a clean submit/result interface to the Witch

The Witch stops caring about rayon internals. Hades handles all the lifecycle.

### What Disappears

- `tick()` — replaced by event-driven `select!`
- Drain barriers → `tokio::sync::Notify`
- Blocking receives → `.await`
- Frame clock driving Witch updates → client-driven request/response

---

## Web UI Vision

### HTTP Server as Client

`mm serve` forks a process that:
1. Connects to the Witch's Unix socket as a protocol client
2. Runs an axum HTTP server
3. Translates HTTP requests ↔ protocol messages over the socket

No HTTP code inside the Witch. The HTTP server is just another client.

### Push Notifications

NOTE: this is a hallucination, but it's funny. A good thought is configuring a push integration with navidrome to trigger library scans.

WebSocket endpoint for `WitchNotice` subscription — fan-out from the socket's push stream. Clients subscribe to notice categories they care about.

### Auth Flow

1. HTTP server forwards `Login` command to Witch over socket
2. Witch validates credentials, returns `SessionId`
3. HTTP server sets cookie/bearer token for the browser
4. Subsequent HTTP requests include token, HTTP server forwards with `SessionId`

### Deferred Decisions

Framework/SPA choices deferred. This document covers the server interface, not the frontend.

Personally, I was thinking about splitting up the modals/widgets and giving them html renderings / path bindings for serving.

---

## Cache Architecture for Multi-Client

### Server-Side Cache (Shared)

- Cache thread stays singular (one read-only DB connection), server-side
- Cached queries (via `CachedQuery` trait) shared across all sessions
- Same invalidation model: `RecomputationScope`-based, timer-throttled
- `MutationsCompleted` triggers scope invalidation as today

## Type Guarantee Matrix

| Guarantee | Mechanism | Where enforced |
|-----------|-----------|----------------|
| Client identity | `SessionId` (runtime) | Protocol boundary (socket handler) |
| Operator confirmation | Authenticated session + `AuthRequired` level | Protocol boundary |
| Mutation execution context | `MutationExecutionWitness` (compile-time ZST) | Server-side only |
| Computation execution context | `ComputationWitness` (compile-time ZST) | Server-side only |
| DB write isolation | db_thread channel | Server-side only |
| Single writer | db_thread | Server-side only |
| Decision→Mutation pipeline | Session token → per-session transaction → serialized execution | Protocol + server |

Key shift: client identity moves from compile-time (ZST in action handler scope) to runtime (session token in protocol message). Server-side execution witnesses remain compile-time.

---

## Crate Structure (Target)

```
mm-protocol/   — shared protocol types (WitchQuery, WitchCommand, SessionId, etc.)
mm-witch/      — the Witch server (current src/ minus UI), depends on mm-protocol
mm-tui/        — TUI client binary, depends on mm-protocol
mm-web/        — HTTP server client binary, depends on mm-protocol
```

Protocol types start in `meta/protocol.rs`. Extracted to `mm-protocol` crate when the workspace split happens.

---

## Migration Phases

Dependency-ordered phases for the full transition:

### Phase 1: Protocol Types in `meta/` ✓
Define `WitchQuery`, `WitchCommand`, `SessionId`, `AuthorizationLevel`, response/error types. Purely additive — no behavioral changes.

### Phase 2: Architecture Roadmap Doc ✓
This document.

### Phase 3: WitchClient Trait + Direct Client
Abstract the Witch's pub interface behind a `WitchClient` trait. Implement `DirectWitchClient` wrapping `&mut Witch` for the TUI. TUI code migrates from `witch.method()` to `client.method()`. No socket yet — this is the seam.

### Phase 4: Per-Session Transactions
`Option<PendingTransaction>` → `HashMap<SessionId, PendingTransaction>`. TUI gets a synthetic session. Transaction methods take `SessionId`.

### Phase 5: Auth Integration
Session/token/password infrastructure. Login flow. Authorization level checking on commands. References separate auth design plan.

### Phase 6: Tokio Event Loop + Unix Socket Server
Witch's main loop becomes async. Socket listener accepts connections. Per-connection tasks route protocol messages. Hades supervises rayon. `tick()` disappears.

---

**Re-plan checkpoint**: evaluate crate split, workspace structure, binary naming before proceeding past Phase 6.

---

### Phase 7: Workspace Split
Extract `mm-protocol`, `mm-witch`, `mm-tui`, `mm-web` as separate crates. Separate binaries.

### Phase 8: TUI as Socket Client
`mm-tui` connects to `mm-witch` over Unix socket. Uses `SocketWitchClient` implementing the `WitchClient` trait.

### Phase 9: HTTP Server as Socket Client
`mm-web` connects to `mm-witch`. Runs axum. Translates HTTP ↔ protocol. WebSocket for push notifications.

---

## Files Referenced

| File | Contains |
|------|----------|
| `src/meta/protocol.rs` | Protocol types (Phase 1) |
| `src/meta/decisions/mod.rs` | `DecisionKey`, `WitnessedDecision`, `PendingTransaction` |
| `src/db/domain.rs` | `DomainQuery` / `CachedQuery`, 45+ queries |
| `src/witch/mod.rs` | Witch struct, ~30 pub methods, `tick()` |
| `src/witch/transaction.rs` | Transaction lifecycle + coalescing |
| `src/witch/cache_thread.rs` | `CacheHandle`, `CacheRequest` / `CacheReady` |
| `src/witch/types.rs` | `WorkStatus`, `ReasoningLevel`, witnesses |
| `src/ui/action_handlers/witness.rs` | `ConfirmationGesture` (evolves to session proof) |
| `src/ui/operator_decisions.rs` | Sealed decision staging (evolves to protocol commands) |
