# Dead Code Cleanup Backlog

Generated from cargo check warnings audit (2026-01-30). **~85 warnings remaining** (started at 115).

---

## Unused Variables - RESOLVED

**Resolved 2026-01-30:** Prefixed with underscore.

- `ui/compound_split/types.rs:124` - `path` → `_path`
- `ui/tag_canonicity/types.rs:364` - `path` → `_path`

---

## Category 1: Corpus Core - Computation/Mutation Infrastructure

### Resolved Items

**Resolved 2026-01-30:**
- `ComputationResult.label` - removed (field was set but never read)
- `computations/stats.rs` - removed `avg_db_read_us()`, `avg_task_ms()` methods
- `corpus/deploy.rs` - removed `compute_deployment_path()` (superseded by `_with_tags` variant)
- `corpus/paths.rs` - marked `root()`, `stash_dir()`, `is_stash_path()` as `#[cfg(test)]`
- `corpus/tags.rs` - marked `get()`, `len()`, `is_empty()`, `as_slice()`, `diff()`, `TagSetDiff`, `DiffClassification` as `#[cfg(test)]`
- Phase Result structs (`asleep/awakening/awake`) - renamed `computation` → `_computation` (72 call sites, field stored but unused after label removal)

### Remaining

| Location | Item | Notes |
|----------|------|-------|
| `mutations/types.rs:35` | `ExtractedMetadata::from_track_with_tags`, `from_track`, `get_tag` | Construction helpers unused |
| `mutations/types.rs:295` | `Mutation::is_db_only`, `requires_serial`, `affected_track_id`, `affected_directories` | Mutation introspection methods |
| `mutations/types.rs:516` | `MutationResult.mutation`, `duration_ms` | Result fields unused |
| `mutations/migration.rs:711` | `fingerprint_blob_to_text`, `fingerprint_text_to_blob`, `apply_all_pending` | Migration helpers |
| `corpus/deploy.rs:115` | `compute_deployment_path` | Superseded by `_with_tags` variant |
| `corpus/metadata.rs:19` | `AudioMetadata` fields: `artist`, `album`, `album_artist`, `title`, `track_number`, `genre`, `isrc` | Struct populated but fields unused |
| `corpus/paths.rs:67,102` | `PathResolver::root`, `stash_dir`, `is_stash_path` | Path utilities unused |
| `corpus/tags.rs:170-269` | `TagSet::get`, `len`, `is_empty`, `as_slice`, `diff`; `TagSetDiff`; `DiffClassification` | Tag diffing system unused |

---

## Category 2: Health Analysis System

| Location | Item | Notes |
|----------|------|-------|
| `health/album_artist_detection.rs:35` | `most_common_artist`, `most_common_album_artist`, `mostly_missing_album_artist` | Album artist analysis |
| `health/collision.rs:25` | `TagCollision.canonical`, `confidence` | Collision resolution fields |
| `health/compound.rs:24` | `CompoundTagValue.count` | Count field unused |

---

## Category 3: Database Layer

### tracks.rs - RESOLVED

**Resolved 2026-01-30:** Vestigial direct-mutation API removed.

The old tracks.rs had ~1160 lines with 28+ dead methods - a mix of:
- **Write methods** (insert/update/delete) - vestigial from before `db_thread::SignalWriteSender` migration
- **Unused query methods** - forward-looking API that was never connected

**Architectural context:** All writes now go through `SignalWriteSender` which enforces the operator-driven invariant via typed witness tokens. The old direct Database mutation methods were the pre-migration API.

**After cleanup:** 500 lines, 20 query methods - all actively used:
- Core track queries: `get_all_tracks`, `get_track_by_id`, `get_track_by_path`, `get_track_count`, `get_tracks_by_ids`, `get_tracks_by_corpus_path_prefix`, `get_tracks_for_tag_editing`, `get_all_track_inodes`
- Tag queries: `get_track_tags`, `get_all_tracks_with_tags`, `get_track_ids_for_tag_values`
- Format queries: `get_track_counts_by_file_type`, `get_tracks_by_file_types`
- Aggregate queries for computations: `get_duplicate_fingerprint_groups`, `get_duplicate_inode_groups`, `get_tracks_with_tag_presence`, `get_all_track_tags_ordered`, `get_album_artist_data`
- Helpers: `fingerprint_to_text`, `blob_to_fingerprint`, `row_to_track`

UI code properly uses `ReadOnlyDb` wrapper (no `.inner()` calls needed). Computation executors use `&Database` directly in worker threads.

### Query Methods - RESOLVED

**Resolved 2026-01-30:** Removed unused query methods.

- `db/queries/health.rs` - removed `get_signals_since`, `get_aggregate_signal_tracks`
- `db/queries/metadata.rs` - removed `get_tag_mismatches_for_track`, `get_oob_conflict_files`

### Type Definitions (Fields Never Read)

| Location | Type | Unused Fields |
|----------|------|---------------|
| `db/types.rs:27` | `Track` | `needs_disk_flush` |
| `db/types.rs:43` | `ScanStateEntry` | `source`, `path`, `file_size` |
| `db/types.rs:310` | `CorpusFileSignalType` | Variants: `CorpusFileModifiedOutOfBand`, `MovedFile`, `TagParseError`, `WaveformReadError` |
| `db/types.rs:351,373` | `CorpusFileSignalType` | `from_str`, `to_signal_type` |
| `db/types.rs:424,434` | `LibraryFileSignalType` | `from_str`, `to_signal_type` |
| `db/types.rs:470` | `FileSignalType` | `from_str` |
| `db/types.rs:513` | `AggregateSignal` | `track_ids()` |
| `db/types.rs:652` | `CorpusSummary` | `track_count`, `deployment_stats`, `pending_changes`, `last_scan` |
| `db/types.rs:724` | `CorpusFilesBucket` | `directory_breakdown` |
| `db/types.rs:746` | `TagSquashEntry` | `total_tracks` |
| `db/types.rs:751-752` | Type aliases | ~~`TagResolutionBucket`, `TagResolutionEntry`~~ REMOVED |
| `db/types.rs:786` | `DirectoryBreakdown` | `entries` |
| `db/types.rs:792` | `DirectoryBreakdownEntry` | `directory`, `count` |
| `db/types.rs:808` | `DeploySignalFile` | `track_id` |
| `db/types.rs:819` | `StaleSignalFile` | `corpus_path`, `track_id` |
| `db/types.rs:866` | `TagMismatchEntry` | `db_values`, `disk_values` |
| `db/types.rs:886` | `OobSignalFile` | ~~Never constructed~~ REMOVED |

---

## Category 4: DB Thread

| Location | Item | Notes |
|----------|------|-------|
| `db_thread.rs:375` | `DbThreadStats` | Fields: `signal_writes`, `index_writes`, `queue_empty` |
| `db_thread.rs:408` | `timing_enabled()` | ~~Method never called~~ REMOVED |

---

## Category 5: Witch/Transaction System - RESOLVED

**Resolved 2026-01-30:** Dead API surface cleanup.

**Removed (dead methods never called):**
- `Witch::state()` - replaced by `eye_state()`
- `Witch::observation_state()` - internal state not needed UI-side
- `Witch::is_db_thread_ready()` - never called
- `Witch::is_accepting_mutations()` - field accessed directly
- `Witch::is_read_only()` - field accessed directly
- `Witch::has_completed_session()` - `status().completed_session` used instead
- `Witch::cancel()` - UX has no cancel-all flow
- `Witch::queue_mutation_internal()` (singular) - only plural version used
- `Witch::queue_computation()` - wrapper never called externally
- `Witch::queue_computations()` - never called
- `Witch::queue_computations_with_label()` - never called
- `Witch::queue_migration_with_label()` - never called
- `Witch::queue_migrations()` - never called
- `Witch::queue_migrations_internal()` - never called
- `DecisionScope::start_transaction()` - transactions started via Witch
- `SpawnedMutation::mutation()` - accessor never used

**Kept (internal use):**
- `queue_computation_with_label()` - now private, called internally
- `queue_mutations_internal()` (plural) - called from transaction confirm
- `queue_migration()` - called from DecisionScope

---

## Category 6: UI Core

| Location | Item | Notes |
|----------|------|-------|
| `ui/mod.rs:204` | `App::new()` | Constructor never called |
| `ui/types.rs:95` | `ExitConfirmModalState::new()` | Constructor never called |
| `ui/render.rs:27` | `RenderContext.config`, `throughput_samples` | Render context fields |

---

## Category 7: Tag Editor

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_editor/state.rs:72-114` | `UnifiedTagEditorState` fields | `various_confirm_state`, `original_aggregated_fields`, `signals` |
| `ui/tag_editor/state.rs:333-529` | Methods | `has_changes`, `get_changes_for_preview`, `drop_changes`, `load_signals` |
| `ui/tag_editor/state.rs:1228-1261` | Methods | `get_values_for_tag`, `is_first_occurrence` |
| `ui/tag_editor/state.rs:1324` | `has_modal()` | Modal check |
| `ui/tag_editor/types.rs:47` | `TagEditContext::BulkEdit.group_context` | Field in variant |
| `ui/tag_editor/types.rs:54` | `GroupContext.source` | Field unused |
| `ui/tag_editor/types.rs:153-157` | `UnsavedChangesDestination` | Variants: `NextItem`, `PrevItem`, `NextSibling` |
| `ui/tag_editor/types.rs:195` | `TagField.is_unique_per_track` | Field unused |
| `ui/tag_editor/types.rs:208-210` | `TagChange` | `old_name`, `deleted` fields |
| `ui/tag_editor/types.rs:243-245` | `VariousConfirmState` | Variants: `Confirming`, `Editing` |
| `ui/tag_editor/types.rs:254` | `AggregatedTagField.editable` | Field unused |

---

## Category 8: Tag Canonicity

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_canonicity/types.rs:52,78` | Constructors | `from_collision`, `from_album_artist_issue` |
| `ui/tag_canonicity/types.rs:296-328` | Methods | `is_on_text_field`, `is_on_first_item`, `is_on_last_item`, `canonical_value` |

---

## Category 9: Tag Search

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_search/state.rs:82,315` | Methods | `active_field_focus`, `apply_tag_name_suggestion` |
| `ui/tag_search/types.rs:53` | `ConditionType::prev()` | Navigation method |
| `ui/tag_search/types.rs:235` | `LogicalOperator::prev()` | Navigation method |
| `ui/tag_search/types.rs:324-334` | `SearchCondition` | `is_tag_condition`, `is_file_type_condition`, `is_range_condition` |
| `ui/tag_search/types.rs:377` | `SEARCHABLE_TAGS` | Constant never used |

---

## Category 10: Tree Browser

| Location | Item | Notes |
|----------|------|-------|
| `ui/tree_browser/mod.rs:97-247` | `TreeBrowserState` | `directory_selector`, `config`, `navigator`, `navigator_mut`, `variant`, `variant_mut`, `current_entry`, `current_path`, `has_filter`, `filtered_file_count` |
| `ui/tree_browser/config.rs:13` | `TreeBrowserConfig.root_path` | Field unused |
| `ui/tree_browser/navigator.rs:439,459` | `TreeNavigator` | `current_path`, `filter` |
| `ui/tree_browser/variants/mod.rs:25` | `BrowserVariant::DirectorySelector` | Variant never constructed |
| `ui/tree_browser/variants/mod.rs:30,71` | `BrowserVariant` | `entry_filter`, `render_overlays` |
| `ui/tree_browser/variants/corpus.rs:83-483` | `CorpusBrowserVariant` | `entry_filter`, `start_search`, `focus`, `search_input`, `search`, `is_match_selection_mode`, `match_selection_idx`, `render_search_bar` |
| `ui/tree_browser/variants/selector.rs:27,35` | `DirectorySelectorVariant` | `new`, `entry_filter` |

---

## Category 11: Compound Split / Deploy / Other Flows

| Location | Item | Notes |
|----------|------|-------|
| `ui/compound_split/types.rs:24` | `CompoundSplitData.separator` | Field unused |
| `ui/compound_split/types.rs:234` | `jump_to()` | Navigation method |
| `ui/deploy_flow/types.rs:112` | `file_counts()` | Stats method |
| `ui/missing_file_flow/types.rs:23` | `RestorableMissingFile` | `track_id`, `inode` fields |
| `ui/shit_format_flow/types.rs:138` | `has_files()` | Check method |
| `ui/startup/intake_confirmation.rs:49` | `IntakeConfirmationState.directory_count` | Field unused |

---

## Category 12: Eye Animation - RESOLVED

**Resolved 2026-01-30:** Removed unused animation trigger methods.

- `animation_state()` - removed
- `blink_just_completed()` - removed
- `trigger_flutter()` - removed

---

## Category 13: Filter Popup

| Location | Item | Notes |
|----------|------|-------|
| `ui/filter_popup/state.rs:41` | `FilterConditionType::label()` | Label method |
| `ui/filter_popup/state.rs:171,194` | `FilterCondition` | `describe`, `describe_range` |
| `ui/filter_popup/state.rs:353,547` | `FilterPopupState` | `with_condition`, `condition_type` |

---

## Category 14: Bulk Selection - RESOLVED

**Resolved 2026-01-30:** Removed unused selection methods (only used in tests, removed with tests).

- `select_all()` - removed
- `select_filtered()` - removed
- `clear()` - removed

---

## Category 15: Progress / Wait State

| Location | Item | Notes |
|----------|------|-------|
| `ui/progress_screen.rs:199-233` | `ProgressScreen` | `progress`, `progress_detail`, `tick_count` |
| `ui/wait_state.rs:62-116` | `WaitState` | `reset`, `is_waiting`, `has_seen_working`, `tick_with_db_drain` |

---

## Category 16: Insights View - RESOLVED

**Resolved 2026-01-30:** Marked test-only method.

- `current_selection()` - marked `#[cfg(test)]`

---

## Category 17: Widget Library

### Controls Widget

| Location | Item |
|----------|------|
| `widgets/controls.rs:57,67` | `ControlsStyle::compact()`, `prominent()` |
| `widgets/controls.rs:92` | `ControlsHint.title` field |
| `widgets/controls.rs:117-185` | `bindings()`, `style()`, `title()`, `render_string()`, `render_paragraph()`, `render_lines()` |
| `widgets/controls.rs:213` | `dir_browser()` |

### Layout Widgets

| Location | Item |
|----------|------|
| `widgets/layout.rs:31` | `PaneConfig::focused()` |
| `widgets/layout.rs:42-44` | `FocusablePane.block`, `focused` fields |
| `widgets/layout.rs:49` | `FocusablePane::inner()` |
| `widgets/layout.rs:88-181` | `TwoPaneLayout`, `TwoPaneLayoutBuilder` - entire struct + methods |
| `widgets/layout.rs:241` | `ThreePaneLayoutBuilder::style()` |
| `widgets/layout.rs:279` | `ThreePaneLayout::vertical()` |

### Modal Widgets

| Location | Item |
|----------|------|
| `widgets/modal.rs:80,88` | `ModalStyle::error()`, `success()` |
| `widgets/modal.rs:171,218` | `Modal::size()`, `compute_area()` |
| `widgets/modal.rs:258,263` | `ModalButton::style_when_selected()`, `style_when_unselected()` |
| `widgets/modal.rs:326-379` | `ConfirmationModal` - entire struct + all methods |
| `widgets/modal.rs:441-510` | `ScrollableModal` - entire struct + all methods |

### Resolution Layout

| Location | Item |
|----------|------|
| `widgets/resolution_layout.rs:107` | `with_list_percent()` |
| `widgets/resolution_layout.rs:168` | `ButtonRects::get()` |

### Text Input Widget

| Location | Item |
|----------|------|
| `widgets/text_input.rs:39,45` | `TextInputState::with_value()`, `focused()` |
| `widgets/text_input.rs:194-224` | `TextInputStyle` struct + `search()` |
| `widgets/text_input.rs:233-323` | `TextInput` - entire struct + all methods |

---

## Summary by Severity

### High Priority (Blocking Features)
- ~~**Witch API** (Category 5): 14+ core methods unused~~ **RESOLVED** - dead API surface removed
- ~~**DB Query Layer** (Category 3): 28+ track query methods never called~~ **RESOLVED** - vestigial mutation API removed, remaining 4 methods in health.rs/metadata.rs

### Medium Priority (Incomplete Features)
- **Tree Browser** (Category 10): Directory browser partially built
- **Tag Editor** (Category 7): Many state management methods unused
- **Tag Search** (Category 9): Search condition system incomplete

### Low Priority (Vestigial/Forward-Looking)
- **Widget Library** (Category 17): Builder patterns and widgets built but not yet used
- **Health Analysis** (Category 2): Analysis methods unused
- **Eye Animation** (Category 12): Animation effects unused

---

## Progress

| Date | Warnings | Change | Notes |
|------|----------|--------|-------|
| 2026-01-30 (initial) | 115 | - | Initial audit |
| 2026-01-30 (witch cleanup) | 112 | -3 | Removed 16 dead Witch API methods |
| 2026-01-30 (tracks.rs cleanup) | 109 | -3 | Removed 28 vestigial methods, kept 20 active queries |
| 2026-01-30 (low-priority sweep) | ~85 | -24 | Removed unused vars, stats methods, deploy helper, DB queries, type aliases, eye animation, bulk selection; marked test-only code with #[cfg(test)] |
