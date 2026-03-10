# Bulk File Optimization Findings

Analyzed 17 files totaling ~22k LoC. Findings ordered by impact.

## Tier 1: Massive Mechanical Wins (>200 lines each)

### `queries/mod.rs` (1239 lines) — `delegate_read!` macro: -450 to -550 lines
- `ReadOnlyDb` is ~110 methods of pure `self.db.method(args)` forwarding
- A `delegate_read!` macro generates all delegation from method signatures
- Single highest-value refactor across the entire codebase

### `write_thread.rs` (3076 lines) — Generic closure variant: -360 to -440 lines
- Every DB write op is tripled: enum variant → sender method → match arm
- ~22 simple ops collapse into `GenericOp { f: Box<dyn FnOnce(&Database)>, name, ctx }`
- Sender gets private `send_op()` helper; executor gets one match arm
- Also: collapse 3 MB cache variants to 1 (-30), merge two `typed_clear_*` fns (-18), hoist `use rusqlite::params;` to module scope (-20)

## Tier 2: High-Value Structural Wins (100-200 lines each)

### `action_handlers/mod.rs` (1809 lines) — Split + macro-ify: -1400 moved + -50 reduced
- Split into 5 submodules (startup, tag_editor, corpus_browser, multi_step_resolutions, transaction)
- Macro-ify click handler — Pattern A (gesture) and B (no-action) identical across ~20 arms
- Group-wizard dedup — `handle_missing_album_single_action` / `handle_disc_extraction_action` ~130 lines each, structurally identical
- Type-encode `InsightAction` launch payloads to eliminate fragile string matching

### `mutations/indexing.rs` (1718 lines) — `MutationResult::from_result()`: -120 lines
- 11 `execute()` impls repeat 15 lines of identical ceremony
- `get_sender()` helper (-26), import path shortening (-26)

### `derivation/executors.rs` (1128 lines) — 5 helpers: -117 lines
- `require_sender!` macro (6 sites, -42)
- `FileInCorpus`/`FileInInbox` reconciliation helper (-30)
- `inode_has_oob_signal` + `reconcile_healthy_file_signal` (-20)
- `gc_signals!` macro for 19-call GC list (-13)
- `to_relative_str()` helper (-12)

### `deploy.rs` + `duplicates.rs` (1444 + 1342) — Shared helpers: -180 lines
- `require_signal_sender()` (9 sites, -72)
- `tags_to_map()` (7 sites, -35)
- `require_config()` (4 sites, -16)
- `reconcile_empty_and_return()` (-24)
- `ReconcileStats` struct replacing raw 4-tuples (-15)

## Tier 3: Medium Wins (50-120 lines each)

### `corpus/tags.rs` (1595 lines) — -120 lines
- `pick_picture_info()` (-28), Opus/OGG write macro (-20), `path_ext()` (-12)
- `open_buffered()` (-15), top-level lofty imports (-18), `assert_round_trip()` (-27)

### `release_packing/components.rs` + `stage_scoring.rs` (1226 + 1218) — -140 lines
- Extract `emit_proposal_signals` (-80-100)
- Extract `compute_elimination_score` (-40-50)
- `Default` for `PackingScoreBreakdown` (-14)

### `queries/health.rs` (1475 lines) — -100 lines
- `query_signal_paths()` (-25), use existing `query_signal_key_blobs` in 3 sites (-40)
- `count_by_category()` (-20)

### `witch/mod.rs` (1695 lines) — -200 lines
- `queue_walk_computations()` unification (-100, also fixes divergence risk)
- `From<FetchResultData> for FetchOutcome` (-25)
- `enqueue_single_task()` (-18), `PostSessionAction` enum (-20)
- `read_config()` (-15), merge awakening functions (-25)

## Tier 4: Polish Wins (20-50 lines each)

### `signals/data.rs` + `store.rs` (1303 + 1141)
- `impl_library_keyed!` macro (-16), `impl_as_str!` macro (-18)

### `corpus.rs` + `insights_view/mod.rs` (1267 + 1093)
- `WizardContext` struct (-25), `Snapshot<T>` wrapper (-30)
- `push_if` closure (-18), eliminate `InsightsModal` enum (-15)

### `domain.rs` (1090) — Fold 3 manual impls into macro (-30), `LoadableModal` trait (-28)

### `config_editor/state.rs` (1025) — `try_parse_into!` macro (-12), collection cursor helpers (-20)

### `external_fetch.rs` (1075) — `SchedulerState` struct (-35), `RateLimit` trait (-10)

### `ui/mod.rs` (1050) — `dispatch_handle_input!` macro (-25)

### `queries/files.rs` (1039) — `query_batch_by_zone_inodes` (-35), standardize `.collect()` (-15)

## Cross-Cutting Patterns

| Pattern | Sites | Fix |
|---------|-------|-----|
| `signal_sender()` guard block | ~20 across 6 files | `require_sender!` macro in shared helpers |
| `tags_to_map()` | 7+ sites | Free function in computation helpers |
| `MutationResult` ceremony | 15+ sites | `MutationResult::from_result()` constructor |
| `ReadOnlyDb` delegation | ~110 methods | `delegate_read!` macro |
| Reconcile-then-log tail | ~10 sites | `ReconcileStats` struct |

## Status

- [ ] Tier 1: `delegate_read!` macro
- [ ] Tier 1: `write_thread.rs` GenericOp
- [ ] Tier 2: action_handlers split
- [ ] Tier 2: `MutationResult::from_result()`
- [ ] Tier 2: derivation/executors helpers
- [ ] Tier 2: deploy+duplicates shared helpers
- [ ] Tier 3+: remaining items
