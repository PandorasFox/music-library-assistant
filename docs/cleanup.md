# Dead Code Cleanup Backlog

Fresh audit 2026-01-30. **50 warnings remaining** (started at 115).

---

## Quick Fixes - RESOLVED

- `corpus/db/mod.rs:24` - removed unused `types::Signal` re-export

---

## Category 1: Corpus Core - Computation/Mutation Infrastructure

### Previously Resolved

- `ComputationResult.label` - removed
- `ThreadStats::avg_db_read_us()`, `avg_task_ms()` - removed
- `corpus/deploy.rs` - removed `compute_deployment_path()`
- `corpus/paths.rs` - marked test-only: `root()`, `stash_dir()`, `is_stash_path()`
- `corpus/tags.rs` - marked test-only: `get()`, `len()`, `is_empty()`, `as_slice()`, `diff()`, `TagSetDiff`, `DiffClassification`

### Remaining (7 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `corpus/metadata.rs:19` | `AudioMetadata` | Multiple fields never read |
| `mutations/types.rs:68` | `ExtractedMetadata` | `from_track_with_tags`, `from_track`, `get_tag` |
| `mutations/types.rs:328` | `Mutation` | `is_db_only`, `requires_serial`, `affected_track_id`, `affected_directories` |
| `mutations/types.rs:782` | `MutationResult` | `mutation`, `duration_ms` fields |
| `mutations/migration.rs:711` | Migration helpers | `fingerprint_blob_to_text`, `fingerprint_text_to_blob`, `apply_all_pending` |

---

## Category 2: Health Analysis System (3 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `health/album_artist_detection.rs:35` | Methods | `most_common_artist`, `most_common_album_artist`, `mostly_missing_album_artist` |
| `health/collision.rs:25` | `TagCollision` | `canonical`, `confidence` fields |
| `health/compound.rs:24` | `CompoundTagValue` | `count` field |

---

## Category 3: Database Layer (14 warnings)

### Previously Resolved

- `db/queries/health.rs` - removed `get_signals_since`, `get_aggregate_signal_tracks`
- `db/queries/metadata.rs` - removed `get_tag_mismatches_for_track`, `get_oob_conflict_files`
- `db/types.rs` - removed `TagResolutionBucket`, `TagResolutionEntry` aliases, `OobSignalFile` struct

### Type Definitions (Fields Never Read)

| Location | Type | Unused Fields |
|----------|------|---------------|
| `db/types.rs:27` | `Track` | `needs_disk_flush` - NOTE: should be preserved; should be integrated across all db->file tag writes |
| `db/types.rs:43` | `ScanStateEntry` | `source`, `path`, `file_size` |
| `db/types.rs:316` | `CorpusFileSignalType` | Variants: `CorpusFileModifiedOutOfBand`, `MovedFile`, `TagParseError`, `WaveformReadError` |
| `db/types.rs:360,435,481` | Signal types | 3× `from_str` methods never used |
| `db/types.rs:674` | `CorpusSummary` | `track_count`, `deployment_stats`, `pending_changes`, `last_scan` |
| `db/types.rs:746` | `CorpusFilesBucket` | `directory_breakdown` |
| `db/types.rs:768` | `TagSquashEntry` | `total_tracks` |
| `db/types.rs:806` | `DirectoryBreakdown` | `entries` |
| `db/types.rs:812` | `DirectoryBreakdownEntry` | `directory`, `count` |
| `db/types.rs:828` | `DeploySignalFile` | `track_id` |
| `db/types.rs:839` | `StaleSignalFile` | `corpus_path`, `track_id` |
| `db/types.rs:886` | `TagMismatchEntry` | `db_values`, `disk_values` |

---

## Category 4: DB Thread (1 warning)

### Previously Resolved

- `DbThreadHandle::timing_enabled()` - removed

### Remaining

| Location | Item | Notes |
|----------|------|-------|
| `db_thread.rs:375` | `DbThreadStats` | Fields: `signal_writes`, `index_writes`, `queue_empty` |

---

## Category 5: Witch/Transaction System - RESOLVED

All dead API surface removed in earlier cleanup.

---

## Category 6: UI Core (4 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/mod.rs:204` | `App::new()` | Constructor never called |
| `ui/types.rs:95` | `ExitConfirmModalState::new()` | Constructor never called |
| `ui/render.rs:27` | `RenderContext` | `config`, `throughput_samples` fields |

---

## Category 7: Tag Editor (2 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_editor/state.rs:220` | Methods | `is_directory_edit`, `is_individual_mode` |
| `ui/tag_editor/types.rs:47` | `TagEditContext::BulkEdit` | `group_context` field |

---

## Category 8: Tag Canonicity (2 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_canonicity/types.rs:52` | Constructors | `from_collision`, `from_album_artist_issue` |
| `ui/tag_canonicity/types.rs:296` | Methods | `is_on_text_field`, `is_on_first_item`, `is_on_last_item`, `canonical_value` |

---

## Category 9: Tag Search (2 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/tag_search/types.rs:53` | `ConditionType::prev()` | Navigation method |
| `ui/tag_search/types.rs:235` | `LogicalOperator::prev()` | Navigation method |

---

## Category 10: Tree Browser (3 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/tree_browser/navigator.rs:439` | `TreeNavigator` | `current_path`, `filter` |
| `ui/tree_browser/variants/mod.rs:26` | `BrowserVariant` | `entry_filter` |
| `ui/tree_browser/variants/corpus.rs:83` | `CorpusBrowserVariant` | Multiple methods unused |

---

## Category 11: Compound Split / Deploy / Other Flows (6 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/compound_split/types.rs:24` | `CompoundSplitData` | `separator` field |
| `ui/compound_split/types.rs:234` | Method | `jump_to` |
| `ui/deploy_flow/types.rs:112` | Method | `file_counts` |
| `ui/missing_file_flow/types.rs:23` | `RestorableMissingFile` | `track_id`, `inode` fields |
| `ui/shit_format_flow/types.rs:138` | Method | `has_files` |
| `ui/startup/intake_confirmation.rs:49` | `IntakeConfirmationState` | `directory_count` field |

---

## Category 12: Eye Animation - RESOLVED

Removed: `animation_state`, `blink_just_completed`, `trigger_flutter`

---

## Category 13: Filter Popup (3 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/filter_popup/state.rs:41` | `FilterConditionType::label()` | Label method |
| `ui/filter_popup/state.rs:171` | `FilterCondition` | `describe`, `describe_range` |
| `ui/filter_popup/state.rs:353` | `FilterPopupState` | `with_condition`, `condition_type` |

---

## Category 14: Bulk Selection - RESOLVED

Removed: `select_all`, `select_filtered`, `clear`

---

## Category 15: Progress / Wait State (2 warnings)

| Location | Item | Notes |
|----------|------|-------|
| `ui/progress_screen.rs:199` | `ProgressScreen` | `progress`, `progress_detail`, `tick_count` |
| `ui/wait_state.rs:62` | `WaitState` | `reset`, `is_waiting`, `has_seen_working`, `tick_with_db_drain` |

---

## Category 16: Insights View - RESOLVED

Marked `#[cfg(test)]`: `current_selection()`

---

## Category 17: Widget Library - MOSTLY RESOLVED

### Previously at 24 warnings, now 2 warnings

### Resolved

- **Controls Widget**: Removed `compact()`, `prominent()`, `title` field, unused methods (`bindings()`, `style()`, `title()`, `render_string()`, `render_paragraph()`, `render_lines()`), `dir_browser()` preset
- **Layout Widget**: Removed entire `TwoPaneLayout`/`TwoPaneLayoutBuilder`, `PaneConfig::focused()`, `FocusablePane.block`/`focused` fields, `FocusablePane::inner()`, `ThreePaneLayoutBuilder::style()`, `ThreePaneLayout::vertical()`. Kept `ThreePaneLayout::horizontal()` (used by tag editor)
- **Modal Widget**: Removed `ConfirmationModal`, `ScrollableModal`, `ModalStyle::error()`, `ModalStyle::success()`, `ModalButton::style_when_selected()`, `ModalButton::style_when_unselected()`
- **Text Input Widget**: Removed `TextInput` struct, `TextInputStyle` struct, `TextInputState::with_value()`, `TextInputState::focused()`. Kept `TextInputState` (used)

### Remaining (2 warnings)

| Location | Item |
|----------|------|
| `widgets/resolution_layout.rs:107` | `with_list_percent()` |
| `widgets/resolution_layout.rs:168` | `ButtonRects::get()` |

---

## Summary by Severity

### High Priority (Blocking Features)
- ~~**Witch API** (Category 5)~~ **RESOLVED**
- ~~**DB Query Layer** (Category 3)~~ **PARTIALLY RESOLVED** - 14 type field warnings remain

### Medium Priority (Incomplete Features)
- **Tree Browser** (Category 10): 3 warnings
- **Tag Editor** (Category 7): 2 warnings
- **Tag Search** (Category 9): 2 warnings

### Low Priority (Vestigial/Forward-Looking)
- ~~**Widget Library** (Category 17)~~ **MOSTLY RESOLVED** - 2 warnings remain
- **Health Analysis** (Category 2): 3 warnings
- **DB Type Fields** (Category 3): 14 warnings - struct fields populated but never read

---

## Progress

| Date | Warnings | Change | Notes |
|------|----------|--------|-------|
| 2026-01-30 (initial) | 115 | - | Initial audit |
| 2026-01-30 (witch cleanup) | 112 | -3 | Removed 16 dead Witch API methods |
| 2026-01-30 (tracks.rs cleanup) | 109 | -3 | Removed 28 vestigial methods, kept 20 active queries |
| 2026-01-30 (low-priority sweep) | 74 | -35 | Removed stats methods, deploy helper, DB queries, type aliases, eye animation, bulk selection; marked test-only code |
| 2026-01-30 (widget cleanup) | 50 | -24 | Removed TwoPaneLayout, ConfirmationModal, ScrollableModal, TextInput widget, unused control/modal methods |
