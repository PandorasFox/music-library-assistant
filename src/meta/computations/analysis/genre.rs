//! Genre-vocabulary computations.
//!
//! - `ImportGenresFromTags` (Phase 1): scans every corpus inode's `GENRE`
//!   tag values, resolves through `genre_aliases`, writes ledger rows with
//!   `source = FileTagImport`, queues unresolved strings.
//! - `DeriveDiscogsGenreLedger` (Phase 5): walks `discogs_release_cache`,
//!   maps each Discogs release to its packed inodes via the linkage table +
//!   release-packing signals, extracts Discogs Genres + Styles, resolves
//!   through `genre_aliases`, and writes ledger rows with
//!   `source = Discogs` (kind=Genre for Genres, kind=Style for Styles).
//!
//! Both computations are full-corpus passes for now: cheap one-shot scans
//! that batch-write through the db_thread. The ledger upserts on the
//! `(inode, genre_id, source, kind)` PK so re-running is safe and idempotent.

use std::collections::{HashMap, HashSet};

use mm_meta::external::genre_resolve::{
    build_alias_map, resolve_genre, AliasMap, ResolveOutcome,
};
use mm_meta::external::genre_source::{GenreKind, GenreSource};

use crate::db::write_thread::{InodeGenreRow, UnresolvedGenreObservation};
use crate::logging::log_general;
use crate::meta::computations::traits::ComputationContext;

use super::{Computation, Result};

/// Execute `ImportGenresFromTags`.
///
/// Dirty-inode-aware: reads `dirty_inodes` for `import_genres_from_tags`,
/// re-derives the `FileTagImport` ledger rows for just those inodes, and
/// clears their dirty markers. Empty dirty queue → no-op (this used to be a
/// full-corpus scan on every TAGS-scope cycle).
///
/// Bootstrap fallback: if the ledger has zero `FileTagImport` rows AND the
/// dirty queue is empty, do a one-shot full-corpus scan so a fresh install
/// gets seeded without needing every inode to first churn through a mutation.
///
/// Per-inode path: wipes the inode's `FileTagImport` rows before re-inserting
/// so removed tag values don't linger as ghost ledger entries.
pub fn execute_import_genres_from_tags(ctx: &ComputationContext<'_>) -> Result {
    use crate::meta::computations::IMPORT_GENRES_FROM_TAGS_COMPUTATION;

    let computation = Computation::ImportGenresFromTags;
    let sender = require_sender!(computation);
    let read_db = ctx.read_db;
    let witness = ctx.witness;

    let dirty_inodes = match read_db.get_dirty_inodes(IMPORT_GENRES_FROM_TAGS_COMPUTATION) {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(computation, format!("query dirty inodes: {e}"));
        }
    };

    let source_key = GenreSource::FileTagImport.to_key();
    let kind_key = GenreKind::Genre.to_key();

    if dirty_inodes.is_empty() {
        // Bootstrap detection: if no FileTagImport ledger rows exist yet, do
        // a one-shot full scan. Otherwise this is steady-state with no
        // pending genre-tag changes — skip the work entirely.
        let needs_bootstrap = match read_db.count_inode_genres_by_source() {
            Ok(rows) => !rows.iter().any(|(src, _)| *src == source_key),
            Err(_) => false,
        };
        if !needs_bootstrap {
            log_general("[COMPUTE] ImportGenresFromTags: no dirty inodes, skipping");
            return Result::success(computation, Vec::new());
        }
        log_general(
            "[COMPUTE] ImportGenresFromTags: FileTagImport ledger empty, running bootstrap scan",
        );
        return run_full_corpus_scan(ctx, source_key, kind_key, sender, witness, computation);
    }

    let alias_rows = match read_db.get_genre_alias_pairs() {
        Ok(rows) => rows,
        Err(e) => return Result::failure(computation, format!("load alias map: {e}")),
    };
    let aliases = build_alias_map(alias_rows);

    let mut ledger_rows: Vec<InodeGenreRow> = Vec::new();
    let mut unresolved_seen: HashMap<String, ()> = HashMap::new();
    let mut unresolved_batch: Vec<UnresolvedGenreObservation> = Vec::new();
    let mut resolved_count: usize = 0;
    let mut unresolved_count: usize = 0;
    let mut tag_rows_seen: usize = 0;

    for &inode in &dirty_inodes {
        // Wipe before re-inserting so a removed GENRE tag drops its
        // ledger row rather than lingering forever.
        sender.clear_inode_genres_for_source(inode, source_key, witness);

        let raws = match read_db.get_corpus_genre_tag_values_for_inode(inode) {
            Ok(rs) => rs,
            Err(e) => {
                log_general(format!(
                    "[COMPUTE] ImportGenresFromTags: load GENRE values for inode {inode} failed: {e}",
                ));
                continue;
            }
        };

        for raw in &raws {
            tag_rows_seen += 1;
            match resolve_genre(raw, &aliases) {
                ResolveOutcome::Resolved { genre_ids, raw } => {
                    resolved_count += genre_ids.len();
                    for gid in genre_ids {
                        ledger_rows.push(InodeGenreRow {
                            inode,
                            genre_id: gid,
                            source: source_key,
                            kind: kind_key,
                            source_ref: None,
                            raw_value: Some(raw.clone()),
                            confidence: None,
                        });
                    }
                }
                ResolveOutcome::Unresolved { raw } => {
                    unresolved_count += 1;
                    if unresolved_seen.insert(raw.clone(), ()).is_none() {
                        unresolved_batch.push(UnresolvedGenreObservation {
                            raw_value: raw,
                            source: source_key,
                        });
                    }
                }
            }
        }

        sender.clear_dirty_inode(inode, IMPORT_GENRES_FROM_TAGS_COMPUTATION, witness);
    }

    if !ledger_rows.is_empty() {
        sender.write_inode_genres(ledger_rows, witness);
    }
    if !unresolved_batch.is_empty() {
        sender.bump_unresolved_genre_observations(unresolved_batch, witness);
    }

    log_general(format!(
        "[COMPUTE] ImportGenresFromTags: {} dirty inodes, {} resolved, {} unresolved across {} tag rows",
        dirty_inodes.len(),
        resolved_count,
        unresolved_count,
        tag_rows_seen,
    ));

    Result::success(computation, Vec::new())
}

/// One-shot full-corpus scan used only for the initial bootstrap (empty
/// ledger). After this runs once, subsequent passes go through the
/// dirty-inode path.
fn run_full_corpus_scan(
    ctx: &ComputationContext<'_>,
    source_key: i64,
    kind_key: i64,
    sender: crate::db::write_thread::SignalWriteSender,
    witness: &impl crate::db::write_thread::SignalWitness,
    computation: Computation,
) -> Result {
    let read_db = ctx.read_db;

    let alias_rows = match read_db.get_genre_alias_pairs() {
        Ok(rows) => rows,
        Err(e) => return Result::failure(computation, format!("load alias map: {e}")),
    };
    let aliases = build_alias_map(alias_rows);

    let tag_rows = match read_db.get_corpus_genre_tag_values() {
        Ok(rows) => rows,
        Err(e) => return Result::failure(computation, format!("load corpus GENRE values: {e}")),
    };

    let mut ledger_rows: Vec<InodeGenreRow> = Vec::with_capacity(tag_rows.len());
    let mut unresolved_seen: HashMap<String, ()> = HashMap::new();
    let mut unresolved_batch: Vec<UnresolvedGenreObservation> = Vec::new();
    let mut resolved_count: usize = 0;
    let mut unresolved_count: usize = 0;

    for (inode, raw) in &tag_rows {
        match resolve_genre(raw, &aliases) {
            ResolveOutcome::Resolved { genre_ids, raw } => {
                resolved_count += genre_ids.len();
                for gid in genre_ids {
                    ledger_rows.push(InodeGenreRow {
                        inode: *inode,
                        genre_id: gid,
                        source: source_key,
                        kind: kind_key,
                        source_ref: None,
                        raw_value: Some(raw.clone()),
                        confidence: None,
                    });
                }
            }
            ResolveOutcome::Unresolved { raw } => {
                unresolved_count += 1;
                if unresolved_seen.insert(raw.clone(), ()).is_none() {
                    unresolved_batch.push(UnresolvedGenreObservation {
                        raw_value: raw,
                        source: source_key,
                    });
                }
            }
        }
    }

    if !ledger_rows.is_empty() {
        sender.write_inode_genres(ledger_rows, witness);
    }
    if !unresolved_batch.is_empty() {
        sender.bump_unresolved_genre_observations(unresolved_batch, witness);
    }

    log_general(format!(
        "[COMPUTE] ImportGenresFromTags (bootstrap): {} resolved, {} unresolved across {} tag rows",
        resolved_count,
        unresolved_count,
        tag_rows.len(),
    ));

    Result::success(computation, Vec::new())
}

/// Watermark key in `app_metadata`: largest `discogs_release_cache.fetched_at`
/// processed by the last successful `DeriveDiscogsGenreLedger` run. Used to
/// skip releases whose cached JSON hasn't been refreshed since.
const DISCOGS_LEDGER_WATERMARK_KEY: &str = "discogs_genre_ledger_watermark";

/// Execute `DeriveDiscogsGenreLedger`.
///
/// Watermark-gated: only re-processes Discogs releases whose cached JSON was
/// `fetched_at` greater than the stored watermark. The ledger uses
/// `(inode, genre_id, source, kind)` as its PK with UPSERT, so re-running
/// untouched cache rows would produce no net change — the watermark lets us
/// skip the parse+resolve cost entirely.
///
/// Scheduled only on startup (`scope = None`) or when `EXTERNAL` scope is
/// flagged (new Discogs JSON landed). Release-packing changes alone do NOT
/// trigger this — accepted trade-off for cycle-time predictability.
pub fn execute_derive_discogs_genre_ledger(ctx: &ComputationContext<'_>) -> Result {
    let computation = Computation::DeriveDiscogsGenreLedger;
    let sender = require_sender!(computation);
    let read_db = ctx.read_db;
    let witness = ctx.witness;

    // Read the watermark. Missing/unparseable → 0 (treat as first run).
    let watermark: i64 = read_db
        .get_app_metadata(DISCOGS_LEDGER_WATERMARK_KEY)
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    let alias_rows = match read_db.get_genre_alias_pairs() {
        Ok(rows) => rows,
        Err(e) => return Result::failure(computation, format!("load alias map: {e}")),
    };
    let aliases: AliasMap = build_alias_map(alias_rows);

    // Pull (mb_release_id, discogs_release_id) linkage rows whose cached JSON
    // is newer than the watermark. We don't iterate `discogs_release_cache`
    // first because the cache may contain orphan rows whose MB linkage has
    // been dropped — those are skipped (no inodes to attribute to).
    let linkage = match read_db.get_discogs_linkage_pairs_since(watermark) {
        Ok(rows) => rows,
        Err(e) => return Result::failure(computation, format!("load discogs linkage: {e}")),
    };

    if linkage.is_empty() {
        log_general(format!(
            "[COMPUTE] DeriveDiscogsGenreLedger: nothing newer than watermark {watermark} \
             (no MB→Discogs links with refreshed cache); skipping",
        ));
        return Result::success(computation, Vec::new());
    }

    // Group MB releases by Discogs release so we load each cached JSON once.
    let mut by_discogs: HashMap<String, Vec<String>> = HashMap::new();
    for (mb_id, dg_id) in linkage {
        by_discogs.entry(dg_id).or_default().push(mb_id);
    }

    let source_key = GenreSource::Discogs.to_key();
    let genre_kind = GenreKind::Genre.to_key();
    let style_kind = GenreKind::Style.to_key();

    let mut ledger_rows: Vec<InodeGenreRow> = Vec::new();
    let mut unresolved_seen: HashSet<String> = HashSet::new();
    let mut unresolved_batch: Vec<UnresolvedGenreObservation> = Vec::new();

    let mut releases_processed: usize = 0;
    let mut releases_no_inodes: usize = 0;
    let mut releases_no_cache: usize = 0;
    let mut releases_parse_fail: usize = 0;
    let mut resolved_count: usize = 0;
    let mut unresolved_count: usize = 0;

    for (discogs_id, mb_ids) in by_discogs {
        // Cached Discogs JSON
        let json = match read_db.get_discogs_release_cache(&discogs_id) {
            Ok(Some((blob, _))) => blob,
            _ => {
                releases_no_cache += 1;
                continue;
            }
        };
        let release = match crate::external::discogs::parse_release(&json) {
            Ok(r) => r,
            Err(_) => {
                releases_parse_fail += 1;
                continue;
            }
        };

        // Inodes packed to ANY of the MB releases this Discogs id is linked
        // to (rare: usually 1, but compilations/regional pressings can fan out).
        let mut inodes: Vec<i64> = Vec::new();
        for mb_id in &mb_ids {
            if let Ok(rows) = read_db.get_inodes_for_packed_release(mb_id) {
                inodes.extend(rows);
            }
        }
        inodes.sort_unstable();
        inodes.dedup();

        if inodes.is_empty() {
            releases_no_inodes += 1;
            log_general(format!(
                "[COMPUTE] Discogs release #{} ({:?}) has no packed inodes; skipping",
                release.id, release.title,
            ));
            continue;
        }

        releases_processed += 1;

        // Resolve each Discogs Genre/Style once per release, then expand to
        // per-inode ledger rows. Each raw can resolve to multiple canonicals
        // (e.g. a compound alias), so flatten via `extend`.
        let mut resolved_genres: Vec<i64> = Vec::new();
        for raw in &release.genres {
            match resolve_genre(raw, &aliases) {
                ResolveOutcome::Resolved { genre_ids, .. } => {
                    resolved_count += genre_ids.len();
                    resolved_genres.extend(genre_ids);
                }
                ResolveOutcome::Unresolved { raw } => {
                    unresolved_count += 1;
                    if unresolved_seen.insert(raw.clone()) {
                        unresolved_batch.push(UnresolvedGenreObservation {
                            raw_value: raw,
                            source: source_key,
                        });
                    }
                }
            }
        }
        let mut resolved_styles: Vec<i64> = Vec::new();
        for raw in &release.styles {
            match resolve_genre(raw, &aliases) {
                ResolveOutcome::Resolved { genre_ids, .. } => {
                    resolved_count += genre_ids.len();
                    resolved_styles.extend(genre_ids);
                }
                ResolveOutcome::Unresolved { raw } => {
                    unresolved_count += 1;
                    if unresolved_seen.insert(raw.clone()) {
                        unresolved_batch.push(UnresolvedGenreObservation {
                            raw_value: raw,
                            source: source_key,
                        });
                    }
                }
            }
        }
        // Multi-target may produce duplicates if two raws map to overlapping
        // canonicals; dedup before fanning to inodes so we don't make
        // identical UPSERT calls.
        resolved_genres.sort_unstable();
        resolved_genres.dedup();
        resolved_styles.sort_unstable();
        resolved_styles.dedup();

        let source_ref = Some(format!("discogs:{}", discogs_id));
        for inode in &inodes {
            for &gid in &resolved_genres {
                ledger_rows.push(InodeGenreRow {
                    inode: *inode,
                    genre_id: gid,
                    source: source_key,
                    kind: genre_kind,
                    source_ref: source_ref.clone(),
                    raw_value: None,
                    confidence: None,
                });
            }
            for &gid in &resolved_styles {
                ledger_rows.push(InodeGenreRow {
                    inode: *inode,
                    genre_id: gid,
                    source: source_key,
                    kind: style_kind,
                    source_ref: source_ref.clone(),
                    raw_value: None,
                    confidence: None,
                });
            }
        }
    }

    if !ledger_rows.is_empty() {
        sender.write_inode_genres(ledger_rows, witness);
    }
    if !unresolved_batch.is_empty() {
        sender.bump_unresolved_genre_observations(unresolved_batch, witness);
    }

    // Advance the watermark to the largest `fetched_at` currently in the
    // cache. Even cache rows we *couldn't* process (no inodes, no MB linkage,
    // parse fail) get covered: they don't need re-attempting until something
    // changes, and the cache row's `fetched_at` won't bump until then.
    if let Ok(new_watermark) = read_db.max_discogs_cache_fetched_at() {
        if new_watermark > watermark {
            sender.set_app_metadata(
                DISCOGS_LEDGER_WATERMARK_KEY,
                &new_watermark.to_string(),
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveDiscogsGenreLedger: {} releases processed, \
         {} no inodes, {} no cache, {} parse fail; \
         {} resolved values, {} unresolved (watermark was {})",
        releases_processed,
        releases_no_inodes,
        releases_no_cache,
        releases_parse_fail,
        resolved_count,
        unresolved_count,
        watermark,
    ));

    Result::success(computation, Vec::new())
}
