# Dead Code Cleanup Backlog

Generated from cargo check warnings audit (2026-01-30). **112 warnings remaining** (started at 115).

---

## Unused Variables (2 warnings)

Quick fixes - prefix with underscore or use the value.

| Location | Variable | Context |
|----------|----------|---------|
| `ui/compound_split/types.rs:124` | `path` | Destructured but unused in track_info lookup |
| `ui/tag_canonicity/types.rs:364` | `path` | Destructured but unused in track_info lookup |

---

## Category 1: Corpus Core - Computation/Mutation Infrastructure

| Location | Item | Notes |
|----------|------|-------|
| `computations/mod.rs:94` | `ComputationResult.label` | Field set but never read |
| `computations/stats.rs:76` | `avg_db_read_us()`, `avg_task_ms()` | Stats methods never called |
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

### Query Methods (Never Called)

| Location | Methods |
|----------|---------|
| `db/queries/health.rs:572` | `get_signals_since`, `get_aggregate_signal_tracks` |
| `db/queries/metadata.rs:52,232` | `get_tag_mismatches_for_track`, `get_oob_conflict_files` |
| `db/queries/tracks.rs` | **28 methods**: `fingerprint_to_blob`, `insert_track`, `insert_track_with_tags`, `insert_track_with_tags_inner`, `clear_source`, `delete_track_by_path`, `delete_tracks_by_paths`, `get_all_tracks_for_source`, `log_scan`, `get_sources`, `get_tracks_by_fingerprint`, `get_tracks_by_inode`, `get_tracks_by_ids`, `get_tracks_by_metadata`, `get_library_tracks_by_source`, `get_tracks_by_paths`, `get_tracks_by_exact_paths`, `get_tracks_in_directory_with_fingerprint`, `get_duplicate_inodes_in_corpus`, `search_tracks_by_tag`, `set_track_tags`, `update_track_tag`, `delete_track_tag`, `get_track_tag_value`, `update_track_inode`, `update_track_path`, `delete_track`, `update_track_metadata`, `update_track_metadata_with_tags`, `get_tracks_needing_disk_flush` |

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
| `db/types.rs:751-752` | Type aliases | `TagResolutionBucket`, `TagResolutionEntry` |
| `db/types.rs:786` | `DirectoryBreakdown` | `entries` |
| `db/types.rs:792` | `DirectoryBreakdownEntry` | `directory`, `count` |
| `db/types.rs:808` | `DeploySignalFile` | `track_id` |
| `db/types.rs:819` | `StaleSignalFile` | `corpus_path`, `track_id` |
| `db/types.rs:866` | `TagMismatchEntry` | `db_values`, `disk_values` |
| `db/types.rs:886` | `OobSignalFile` | Never constructed |

---

## Category 4: DB Thread

| Location | Item | Notes |
|----------|------|-------|
| `db_thread.rs:375` | `DbThreadStats` | Fields: `signal_writes`, `index_writes`, `queue_empty` |
| `db_thread.rs:408` | `timing_enabled()` | Method never called |

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

## Category 12: Eye Animation

| Location | Item | Notes |
|----------|------|-------|
| `ui/eye.rs:110-254` | `Eye` methods | `animation_state`, `blink_just_completed`, `trigger_flutter` |

---

## Category 13: Filter Popup

| Location | Item | Notes |
|----------|------|-------|
| `ui/filter_popup/state.rs:41` | `FilterConditionType::label()` | Label method |
| `ui/filter_popup/state.rs:171,194` | `FilterCondition` | `describe`, `describe_range` |
| `ui/filter_popup/state.rs:353,547` | `FilterPopupState` | `with_condition`, `condition_type` |

---

## Category 14: Bulk Selection

| Location | Item | Notes |
|----------|------|-------|
| `ui/bulk_selection/state.rs:69-85` | `BulkSelectionState` | `select_all`, `select_filtered`, `clear` |

---

## Category 15: Progress / Wait State

| Location | Item | Notes |
|----------|------|-------|
| `ui/progress_screen.rs:199-233` | `ProgressScreen` | `progress`, `progress_detail`, `tick_count` |
| `ui/wait_state.rs:62-116` | `WaitState` | `reset`, `is_waiting`, `has_seen_working`, `tick_with_db_drain` |

---

## Category 16: Insights View

| Location | Item | Notes |
|----------|------|-------|
| `ui/insights_view/mod.rs:557` | `current_selection()` | Selection accessor |

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
- **DB Query Layer** (Category 3): 28+ track query methods never called - entire track query API unused

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
