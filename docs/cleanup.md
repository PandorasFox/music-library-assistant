# Dead Code Cleanup Backlog

Generated from cargo check warnings audit (2026-01-30).

---

### Category 4: DB Query Methods - NEEDS REVIEW
These are query methods that may be vestigial or may need integration.

| Location | Methods | Notes |
|----------|---------|-------|
| `db/queries/mod.rs:468` | `get_track_tag_value`, `get_tag_mismatches_for_track` | Duplicate of metadata.rs? |
| `db/queries/deployment.rs:15` | `compute_deployment_stats` | Deployment feature incomplete? |
| `db/queries/health.rs:208` | Multiple methods | Health queries unused |
| `db/queries/metadata.rs:52` | `get_tag_mismatches_for_track`, `get_oob_conflict_files` | OOB flow incomplete? |
| `db/queries/tracks.rs:23` | `fingerprint_to_blob` | Duplicate of db_thread version |
| `db/queries/tracks.rs:47` | Multiple track methods | Track query layer unused |

---

### Category 5: db_thread SignalWriteOp - NEEDS INTEGRATION
These have complete send/receive paths but variants are never constructed.

| Location | Variants |
|----------|----------|
| `db_thread.rs:147` | `ClearFileSignalsInDirectory`, `UpdateTrackTag`, `DeleteTrackTag` |
| `db_thread.rs:528` | `clear_file_signals_in_directory`, `update_track_tag`, `delete_track_tag` (sender methods) |

---

### Category 6: Witch/Transaction System - PARTIAL IMPLEMENTATION
Looks like transaction management was built but not fully wired up.

| Location | Item | Notes |
|----------|------|-------|
| `witch/types.rs:78` | `Task::Migration` variant | Migration task type unused |
| `witch/types.rs:96` | `Migration.description` field | |
| `witch/types.rs:224,255` | `start_transaction`, `discard_decision` | DecisionScope methods |
| `witch/types.rs:289` | `TaskLabel::new` | |
| `witch/types.rs:338,351` | `CompletedSession.completed_at`, `should_display` | Session display logic |
| `witch/types.rs:424` | `PendingTransaction.started_at` | |
| `witch/types.rs:483` | `TransactionInfo` struct | |
| `witch/types.rs:507` | `DiscardSummary` fields | |
| `witch/transaction.rs:61` | `transaction_info`, `discard_decision` | |
| `witch/mod.rs:74` | `launch_time`, `path_resolver` fields | |
| `witch/mod.rs:261` | Multiple methods | |
| `witch/ui_read_cache.rs:340` | `want_deploy_modal_data` | |

---

### Category 7: UI Modal States - INCOMPLETE FEATURES
Tag editor modes and actions that were defined but never wired up.

| Location | Item |
|----------|------|
| `ui/mod.rs:64` | `OobSync`, `OobConflict` variants |
| `ui/mod.rs:133` | `is_first` method |
| `ui/mod.rs:509` | `gather_transaction_decisions`, `start_deployment_preview` |
| `ui/types.rs:44` | `AppState::DirBrowser` variant |
| `ui/types.rs:96` | `DirBrowserState::new` |
| `ui/tag_editor/types.rs:20` | `TagEditorContext::DuplicateResolution`, `DeployConflict` |
| `ui/tag_editor/types.rs:109` | `TagEditorAction::StageDecision`, `CommitTransaction`, `ShowModal` |
| `ui/tag_editor/types.rs:176` | `TagEditorNavigation::NextItem`, `PrevItem`, `NextSibling` |
| `ui/tag_editor/types.rs:266` | `TagSelectionState::Confirming`, `Editing` |

---

### Category 8: Tag Search System - INCOMPLETE
Advanced search conditions not implemented.

| Location | Item |
|----------|------|
| `ui/tag_search/types.rs:20` | `SearchFieldType::FileType`, `SampleRate`, `Bitrate`, `Duration` |
| `ui/tag_search/types.rs:31,235` | Various navigation methods |
| `ui/tag_search/types.rs:324` | Condition type check methods |
| `ui/tag_search/types.rs:366` | `TagSearchAction::EditAllTracks` |
| `ui/tag_search/types.rs:379` | `SEARCHABLE_TAGS` constant |
| `ui/tag_search/state.rs:82` | Multiple state methods |

---

### Category 9: Tree Browser - INCOMPLETE
Directory browser feature partially built.

| Location | Item |
|----------|------|
| `ui/tree_browser/mod.rs:97` | Multiple methods |
| `ui/tree_browser/config.rs:13` | `root_path` field |
| `ui/tree_browser/navigator.rs:439` | `current_path`, `filter` |
| `ui/tree_browser/variants/mod.rs:25` | `DirectorySelector` variant |
| `ui/tree_browser/variants/mod.rs:30` | `entry_filter`, `render_overlays` |
| `ui/tree_browser/variants/corpus.rs:83` | Multiple methods |
| `ui/tree_browser/variants/selector.rs:27` | `new`, `entry_filter` |

---

### Category 10: Widget Library - UNUSED BUILDERS
Widget infrastructure built but not used.

| Location | Item |
|----------|------|
| `ui/widgets/controls.rs:57,92,117` | `compact`, `prominent`, `title` field, various methods |
| `ui/widgets/layout.rs:31-279` | `TwoPaneLayout`, `TwoPaneLayoutBuilder`, various methods |
| `ui/widgets/modal.rs:80-512` | `error`, `success`, `ConfirmationModal`, `ScrollableModal` |
| `ui/widgets/resolution_layout.rs:107,168` | `with_list_percent`, `get` |
| `ui/widgets/text_input.rs:39-240` | `TextInput`, `TextInputStyle`, various methods |

---

### Category 11: Miscellaneous Dead Code

| Location | Item | Notes |
|----------|------|-------|
| `computations/mod.rs:94` | `ComputationResult.label` field | Result field unused |
| `computations/stats.rs:76` | `avg_db_read_us`, `avg_task_ms` | Stats getters |
| `corpus/deploy.rs:115` | `compute_deployment_path` | Superseded by `_with_tags` variant |
| `corpus/health/album_artist_detection.rs:35` | Analysis methods | |
| `corpus/health/collision.rs:25` | `canonical`, `confidence` fields | |
| `corpus/health/compound.rs:24` | `count` field | |
| `corpus/metadata.rs:19` | Multiple fields | |
| `corpus/mutations/types.rs:35` | `from_track_with_tags`, `from_track`, `get_tag` | |
| `corpus/mutations/types.rs:319` | `is_db_only`, `requires_serial`, etc. | |
| `corpus/mutations/types.rs:541` | `MutationResult` fields | |
| `corpus/mutations/migration.rs:711` | Fingerprint conversion functions | |
| `corpus/paths.rs:67,102` | `root`, `stash_dir`, `is_stash_path` | |
| `corpus/tags.rs:171-269` | `TagSet` methods, `TagSetDiff`, `DiffClassification` | |
| `corpus/db/types.rs` (various) | Many struct fields never read | |
| `ui/bulk_selection/state.rs:69` | `select_all`, `select_filtered`, `clear` | |
| `ui/compound_split/types.rs:24,237` | `separator` field, `jump_to` | |
| `ui/deploy_flow/types.rs:112` | `file_counts` | |
| `ui/eye.rs:110` | Animation methods | |
| `ui/filter_popup/state.rs` | Various methods | |
| `ui/insights_view/mod.rs:557` | `current_selection` | |
| `ui/missing_file_flow/types.rs:23` | `track_id`, `inode` fields | |
| `ui/progress_screen.rs:199` | Progress methods | |
| `ui/render.rs:27` | `config`, `throughput_samples` fields | |
| `ui/shit_format_flow/types.rs:138` | `has_files` | |
| `ui/startup/intake_confirmation.rs:49` | `directory_count` | |
| `ui/tag_canonicity/types.rs:52,296` | Constructor methods, navigation | |
| `ui/tag_editor/state.rs` | Various fields and methods | |
| `ui/wait_state.rs:62` | State methods | |
