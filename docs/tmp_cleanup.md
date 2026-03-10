# Consolidation Audit — Cleanup Targets

## Tier 1: High-yield, mechanical consolidation

### 1. Signal `query_all`/`query_by_key`/`query_by_inode` in `store.rs` — 13 hand-written methods

All follow identical shapes differing only in table name, column list, and deserialized type. The `bincode::deserialize` + error-mapping block is copy-pasted 13 times with an inconsistency (`FromSqlConversionFailure` vs `ToSqlConversionFailure`).

Could be optional arms on `impl_corpus_signal!`/`impl_aggregate_signal!`, or at minimum a shared `deserialize_blob<T>()` helper.

**Sub-patterns:**
- `query_all` for simple corpus signals (inode+path, no blob): `UnindexedFileSignal`, `HealthyFileSignal` — store.rs L391-424
- `query_all` for aggregate blob signals (key+data): `FingerprintOverlapSignal`, `MissingTagSignal`, `MissingAlbumSingleSignal`, `CrossSourceOverlapSignal`, `ReleaseOverlapSignal`, `DiscExtractionSignal` — store.rs L828-1175
- `query_by_inode` for corpus blob signals: `CompoundTagSignal`, `InboxCompoundTagSignal` — store.rs L644-734
- `query_by_key` for aggregate blob signals: `TagCanonicitySignal`, `InconsistentAlbumArtistSignal`, `InboxTagCanonicitySignal` — store.rs L976-1135

### 2. Focused-border block pattern — 17 sites across 8 files

```rust
let border_color = if is_focused { Color::Yellow } else { Color::DarkGray };
let block = Block::default().title("...").borders(Borders::ALL)
    .border_style(Style::default().fg(border_color));
```

One `fn focused_block(title: &str, is_focused: bool) -> Block` collapses all. `StandardList` uses `Color::White` for focused — inconsistency to resolve.

**Locations:**
- `release_packing_browser/render.rs` L101-112, L229-239, L364-375
- `oob_sync_modal/render.rs` L81-99, L239-250
- `oob_conflict_modal/render.rs` L114-138, L246-257
- `compound_split_v2/render.rs` L104-119, L191-207
- `tag_canonicity_v2/render.rs` L88-105, L173-188
- `missing_album_modal.rs` L328-340, L420-431
- `disc_extraction_modal.rs` L393-405, L482-493
- `moved_file_modal/render.rs` L51-64, L159-171
- `widgets/wizard_pane.rs` L80

### 3. Click-target population loop — ~14 sites across 7 files

Identical `click_targets.clear()` / `set_list_area` / `for (vis_idx, entry_idx)` loop, all reinventing what `StandardList` encapsulates.

**Locations:**
- `tag_canonicity_v2/render.rs` L107-133, L207-215
- `compound_split_v2/render.rs` L122-146, L225-234
- `history_view/render.rs` L77-86, L197-205, L345-352
- `tree_browser/render.rs` L400-407
- `corrupt_file_modal/preview.rs` L232-241
- `tag_editor/render.rs` L301-317, L479-489
- `moved_file_modal/render.rs` L67-76

### 4. Manual scroll computation — ~10 sites across 5 files

Same clamp logic reinvented per-pane, including duplicated within the same file.

**Locations:**
- `tag_canonicity_v2/render.rs` L116-122, L198-205
- `compound_split_v2/render.rs` L130-136, L217-223
- `history_view/render.rs` L74, L193, L342 (local helper, still duplicated from StandardList)
- `release_packing_browser/render.rs` L136-140, L344-349
- `tag_editor/render.rs` L213-217

### 5. Format helpers missing from `ui/helpers.rs`

| Missing helper | Duplicated sites |
|---|---|
| `format_duration_ms(ms) -> String` | 5 sites / 4 files: manual_review_modal/preview.rs:442, directory_cluster_modal/preview.rs:834, tag_editor/render.rs:81, external_match_modal/types.rs:190+234 |
| `format_bytes(bytes) -> String` | 4 divergent implementations: manual_review_modal/preview.rs:473 (2-tier), tag_editor/render.rs:86 (3-tier), intake_confirmation.rs:420 (4-tier), vacuum.rs:114 (ad-hoc) |
| `format_sample_rate(sr) -> String` | 2 identical (manual_review_modal:459, directory_cluster_modal:851) + 1 inconsistent (tag_editor:98) |
| `format_kbps(br) -> String` | 4 sites |

---

## Tier 2: Medium-yield structural consolidation

### 6. `compound_split_v2` and `tag_canonicity_v2` are structural twins

Identical three-pane layout (25/35/40), identical `render_tags_pane` (pending edits block is verbatim copy), identical scroll/click-target boilerplate in both panes.

### 7. Audio metadata display block — 3 files

`manual_review_modal/preview.rs`, `directory_cluster_modal/preview.rs`, and `tag_editor/render.rs` all render format/duration/bitrate/sample-rate/size/album-art with the same structure. Album art Yes/No block is byte-for-byte identical between the first two.

### 8. `label_style`/`value_style` and semantic style constants

`Style::default().fg(Color::DarkGray)` appears 349 times across 53 files. Named constants like `LABEL_STYLE`, `VALUE_STYLE`, `SECTION_HEADING_STYLE` would reduce noise. The `kv_line()` helper in `release_packing_browser` (used 14 times locally) could be promoted to shared.

### 9. `get_compound_signal_groups` vs `get_inbox_compound_signal_groups` — ~70% identical

`health.rs` L66-229. Same accumulator, group_order tracking, final filter_map. Differs only in table name and safety/tag filtering.

### 10. Score bar rendering — 2 implementations

`release_packing_browser/render.rs:999` and `knot_browser/render.rs:228`. Same algorithm, different return types.

---

## Tier 3: Low-yield cleanup

### 11. Duplicate `centered_rect_fixed`
Two implementations (`helpers.rs` and `widgets/modal.rs`). The `widgets/modal.rs` version is canonical; `helpers.rs` version is stale.

### 12. `truncate_for_width` in release_packing_browser
Reimplements `truncate_right` from helpers with one extra edge case guard.

### 13. `external_match_view/render.rs` — 4 near-identical render helpers
All define same `marker = if is_cursor { "▸ " } else { "  " }` + style block independently.

### 14. Enum mirror conversion
`CorpusMatchQuality` → `MatchClassification` is 1:1 match, should be a `From` impl.

---

## Vestigial/Dead Code

| Item | Location | Status |
|---|---|---|
| `Eye::blink_completed` field | `eye.rs:87-88` | Written, never read |
| Commented-out import with stale TODO | `tag_search/mod.rs:27` | Dead comment |
| hjkl in docstrings (code is correct) | `missing_album_modal.rs:9-10`, `inbox_organize/mod.rs:10-11` | Stale docs |
| `_v2` suffix with no v1 | `compound_split_v2/`, `tag_canonicity_v2/` | Stale naming |
| `centered_rect` percent variant | `widgets/modal.rs:16-34` | Unused externally |
| Raw `List`/`ListItem` in older modals | `missing_album_modal.rs`, `disc_extraction_modal.rs` | Pre-widget code |
| All config descriptions are `"TODO"` | `config_editor/build.rs:213-389` | Scaffolding |
