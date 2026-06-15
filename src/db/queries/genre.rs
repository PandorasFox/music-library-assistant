//! Read-side queries for the genre vocabulary and per-inode ledger.
//!
//! All queries here are read-only — writes go through the DB write thread
//! (`crate::db::write_thread::SignalWriteSender`). See `write_thread/executor/genre_ops.rs`
//! for the write path.

use anyhow::Result;
use rusqlite::params;

use super::Database;

impl Database {
    /// Load the full `genre_aliases` table as a vector of `(alias, genre_id)` pairs.
    ///
    /// Computations call this once per pass to build an in-memory
    /// `genre_resolve::AliasMap` and then resolve raw strings without further DB
    /// I/O. Scales: corpus genre vocabulary is small (hundreds of rows at most).
    pub fn get_genre_alias_pairs(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT alias, genre_id FROM genre_aliases")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Return every (inode, raw GENRE value) pair from `corpus_tags` for the corpus zone.
    ///
    /// Filters by `inode_paths.zone = 'corpus'` so library-only files are skipped.
    /// One inode may appear multiple times (multi-value GENRE tags are stored as
    /// separate rows by the `corpus_tags` composite PK).
    pub fn get_corpus_genre_tag_values(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT ct.inode, ct.tag_value
             FROM corpus_tags ct
             JOIN inode_paths ip ON ip.inode = ct.inode AND ip.zone = 'corpus'
             WHERE ct.tag_name = 'GENRE'",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Return every (inode, raw GENRE value) pair for a specific corpus inode.
    ///
    /// Used by per-inode re-runs of the importer (dirty-aware path).
    pub fn get_corpus_genre_tag_values_for_inode(
        &self,
        inode: i64,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT ct.tag_value
             FROM corpus_tags ct
             JOIN inode_paths ip ON ip.inode = ct.inode AND ip.zone = 'corpus'
             WHERE ct.inode = ?1 AND ct.tag_name = 'GENRE'",
        )?;
        let rows = stmt
            .query_map(params![inode], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Count rows in `inode_genres` by source — used for verification queries
    /// and the Phase 2 vocabulary UI's "ledger coverage" display.
    pub fn count_inode_genres_by_source(&self) -> Result<Vec<(i64, i64)>> {
        let mut stmt = self.conn().prepare(
            "SELECT source, COUNT(*) FROM inode_genres GROUP BY source",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Load the full genre vocabulary in one shot.
    ///
    /// Issues three queries (names, aliases, implications) and one aggregate
    /// (ledger coverage), then stitches them client-side. Faster than joining
    /// in SQL when the editor wants the full thing, which is the common case.
    pub fn get_genre_vocabulary(
        &self,
    ) -> Result<mm_meta::domain_query_types::GenreVocabulary> {
        use mm_meta::domain_query_types::{GenreNameEntry, GenreVocabulary};
        use std::collections::HashMap;

        let conn = self.conn();

        // 1) canonical names
        let mut names: HashMap<i64, GenreNameEntry> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, canonical_name, display_name FROM genre_names ORDER BY canonical_name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (id, canonical_name, display_name) = r?;
                names.insert(
                    id,
                    GenreNameEntry {
                        id,
                        canonical_name,
                        display_name,
                        aliases: Vec::new(),
                        implies_parents: Vec::new(),
                        ledger_row_count: 0,
                    },
                );
            }
        }

        // 2) aliases (grouped onto names)
        {
            let mut stmt = conn.prepare(
                "SELECT alias, genre_id FROM genre_aliases ORDER BY alias COLLATE NOCASE",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
            for r in rows {
                let (alias, genre_id) = r?;
                if let Some(entry) = names.get_mut(&genre_id) {
                    entry.aliases.push(alias);
                }
            }
        }

        // 3) implications (grouped by child)
        {
            let mut stmt =
                conn.prepare("SELECT child_id, parent_id FROM genre_implies")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            for r in rows {
                let (child_id, parent_id) = r?;
                if let Some(entry) = names.get_mut(&child_id) {
                    entry.implies_parents.push(parent_id);
                }
            }
        }

        // 4) ledger coverage (per genre_id, summed across all sources)
        {
            let mut stmt = conn
                .prepare("SELECT genre_id, COUNT(*) FROM inode_genres GROUP BY genre_id")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            for r in rows {
                let (genre_id, count) = r?;
                if let Some(entry) = names.get_mut(&genre_id) {
                    entry.ledger_row_count = count;
                }
            }
        }

        // Stable output order: by canonical_name COLLATE NOCASE (already the
        // pull order from query 1, but HashMap iteration scrambled it). Re-sort.
        let mut entries: Vec<GenreNameEntry> = names.into_values().collect();
        entries.sort_by(|a, b| a.canonical_name.to_lowercase().cmp(&b.canonical_name.to_lowercase()));

        Ok(GenreVocabulary { entries })
    }

    /// Load the raw cached Discogs release JSON + fetched_at, if present.
    pub fn get_discogs_release_cache(
        &self,
        release_id: &str,
    ) -> Result<Option<(Vec<u8>, i64)>> {
        let row = self
            .conn()
            .query_row(
                "SELECT raw_json, fetched_at FROM discogs_release_cache WHERE release_id = ?1",
                params![release_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
            )
            .ok();
        Ok(row)
    }

    /// Return every (mb_release_id, discogs_release_id) pair the linkage
    /// table knows about. The Phase 5 computation uses this to fan Discogs
    /// genres back out to MB-release-keyed inode lists.
    pub fn get_discogs_linkage_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn().prepare(
            "SELECT mb_release_id, discogs_release_id FROM mb_release_discogs_links",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Like `get_discogs_linkage_pairs`, but filtered to pairs whose cached
    /// Discogs JSON was fetched (or refreshed) after `since`. Used by
    /// `DeriveDiscogsGenreLedger` to skip releases whose JSON hasn't changed
    /// since the last successful run — the ledger is idempotent and an
    /// untouched cache row would produce the same UPSERTs.
    pub fn get_discogs_linkage_pairs_since(
        &self,
        since: i64,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn().prepare(
            "SELECT l.mb_release_id, l.discogs_release_id
             FROM mb_release_discogs_links l
             JOIN discogs_release_cache c ON c.release_id = l.discogs_release_id
             WHERE c.fetched_at > ?1",
        )?;
        let rows = stmt
            .query_map(params![since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Largest `fetched_at` currently in `discogs_release_cache`. Used as the
    /// next watermark for `DeriveDiscogsGenreLedger`. Returns 0 if the cache
    /// is empty.
    pub fn max_discogs_cache_fetched_at(&self) -> Result<i64> {
        let val: Option<i64> = self
            .conn()
            .query_row(
                "SELECT MAX(fetched_at) FROM discogs_release_cache",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        Ok(val.unwrap_or(0))
    }

    /// Read a string value from the `app_metadata` key/value table.
    pub fn get_app_metadata(&self, key: &str) -> Result<Option<String>> {
        let val: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM app_metadata WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(val)
    }

    /// Return the Discogs release IDs the linkage table knows about that
    /// don't yet have a cached JSON blob. The scheduler uses this to populate
    /// the Discogs fetch queue at `init_batch` time. Sorted alphabetically for
    /// reproducible ordering.
    pub fn get_unfetched_discogs_release_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT l.discogs_release_id
             FROM mb_release_discogs_links l
             LEFT JOIN discogs_release_cache c
                ON c.release_id = l.discogs_release_id
             WHERE c.release_id IS NULL
             ORDER BY l.discogs_release_id",
        )?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// True if a `discogs_release_cache` row already exists for `release_id`.
    pub fn discogs_release_cached(&self, release_id: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) > 0 FROM discogs_release_cache WHERE release_id = ?1",
            params![release_id],
            |row| row.get::<_, bool>(0),
        )?)
    }

    /// Look up the Discogs release id linked to a given MB release id, if any.
    /// Returns the first row (linkage table can have multiple per MB release).
    pub fn discogs_release_for_mb_release(
        &self,
        mb_release_id: &str,
    ) -> Result<Option<String>> {
        let opt: Option<String> = self
            .conn()
            .query_row(
                "SELECT discogs_release_id
                 FROM mb_release_discogs_links
                 WHERE mb_release_id = ?1
                 LIMIT 1",
                params![mb_release_id],
                |row| row.get(0),
            )
            .ok();
        Ok(opt)
    }

    /// Load the per-release promotion review.
    ///
    /// Strategy: scan `signal_release_packing` once for the (inode, release_id,
    /// title, artist) tuple, scan `inode_genres` once for every ledger row,
    /// then join in Rust. Two batch queries beats per-release fan-out for the
    /// corpus sizes we care about (≤200k inodes, single-digit-ms scan time on
    /// SQLite).
    pub fn get_genre_promotion_review(
        &self,
    ) -> Result<mm_meta::domain_query_types::GenrePromotionReview> {
        use mm_meta::domain_query_types::{
            GenrePromotionChip, GenrePromotionReview, GenrePromotionReviewRow,
        };
        use std::collections::HashMap;

        let conn = self.conn();

        // 1) Per-inode → (release_id, title, artist) from signal_release_packing.
        struct InodeRelease {
            release_id: String,
            title: String,
            artist: String,
        }
        let mut inode_release: HashMap<i64, InodeRelease> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT inode, data FROM signal_release_packing")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let inode: i64 = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                if let Ok(d) = bincode::deserialize::<
                    crate::meta::signals::data::ReleasePackingData,
                >(&blob)
                {
                    inode_release.insert(
                        inode,
                        InodeRelease {
                            release_id: d.release_id,
                            title: d.release_title,
                            artist: d.release_artist,
                        },
                    );
                }
            }
        }
        if inode_release.is_empty() {
            return Ok(GenrePromotionReview::default());
        }

        // 2) genre_id → canonical_name (small, fully cached in memory).
        let mut canonical_names: HashMap<i64, String> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT id, display_name FROM genre_names")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                canonical_names.insert(row.get::<_, i64>(0)?, row.get::<_, String>(1)?);
            }
        }

        // 3) All ledger rows in one scan. Filter to inodes we care about
        //    (those packed to a release).
        struct LedgerHit {
            inode: i64,
            genre_id: i64,
            kind: i64,
            source: i64,
        }
        let mut ledger_hits: Vec<LedgerHit> = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT inode, genre_id, kind, source FROM inode_genres",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let inode: i64 = row.get(0)?;
                if !inode_release.contains_key(&inode) {
                    continue;
                }
                ledger_hits.push(LedgerHit {
                    inode,
                    genre_id: row.get(1)?,
                    kind: row.get(2)?,
                    source: row.get(3)?,
                });
            }
        }

        // 4) Current GENRE values across packed inodes (for the "current"
        //    summary column). Single pass over corpus_tags filtered by inode set.
        let mut current_genres_by_inode: HashMap<i64, Vec<String>> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT inode, tag_value FROM corpus_tags WHERE tag_name = 'GENRE'",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let inode: i64 = row.get(0)?;
                if !inode_release.contains_key(&inode) {
                    continue;
                }
                let value: String = row.get(1)?;
                current_genres_by_inode.entry(inode).or_default().push(value);
            }
        }

        // 5) Aggregate per-release.
        struct Agg {
            title: String,
            artist: String,
            inodes: std::collections::HashSet<i64>,
            // (genre_id, kind) → (inode set carrying it, sources bitset)
            chips: HashMap<(i64, u8), (std::collections::HashSet<i64>, u8)>,
        }
        let mut agg: HashMap<String, Agg> = HashMap::new();
        for (inode, rel) in &inode_release {
            let entry = agg.entry(rel.release_id.clone()).or_insert_with(|| Agg {
                title: rel.title.clone(),
                artist: rel.artist.clone(),
                inodes: std::collections::HashSet::new(),
                chips: HashMap::new(),
            });
            entry.inodes.insert(*inode);
        }
        for hit in &ledger_hits {
            let release_id = match inode_release.get(&hit.inode) {
                Some(r) => &r.release_id,
                None => continue,
            };
            let entry = match agg.get_mut(release_id) {
                Some(e) => e,
                None => continue,
            };
            let chip = entry
                .chips
                .entry((hit.genre_id, hit.kind as u8))
                .or_insert_with(|| (std::collections::HashSet::new(), 0u8));
            chip.0.insert(hit.inode);
            // Pack source bit. Discriminants are 1..=6; bit (n-1) is set.
            if hit.source >= 1 && hit.source <= 8 {
                chip.1 |= 1u8 << ((hit.source as u8).saturating_sub(1));
            }
        }

        // 6) Build rows. Skip releases with no chips (no ledger coverage at all).
        let mut rows: Vec<GenrePromotionReviewRow> = Vec::new();
        for (release_id, a) in agg {
            if a.chips.is_empty() {
                continue;
            }

            // current_genre_summary: most-common value across all packed inodes.
            // current_genre_uniform: every packed inode shares the most-common.
            let mut value_counts: HashMap<String, usize> = HashMap::new();
            let mut inodes_with_value = 0usize;
            for inode in &a.inodes {
                if let Some(values) = current_genres_by_inode.get(inode) {
                    inodes_with_value += 1;
                    for v in values {
                        *value_counts.entry(v.clone()).or_insert(0) += 1;
                    }
                }
            }
            let (current_genre_summary, current_genre_uniform) = value_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(v, count)| {
                    let uniform = inodes_with_value == a.inodes.len() && count == inodes_with_value;
                    (Some(v), uniform)
                })
                .unwrap_or((None, false));

            // Build chip list, then sort: genres first, then styles, each by
            // inode_count desc (most-common chips visually at the front).
            let mut chips: Vec<GenrePromotionChip> = a
                .chips
                .into_iter()
                .filter_map(|((genre_id, kind), (inodes, sources_bits))| {
                    let canonical_name = canonical_names.get(&genre_id).cloned()?;
                    Some(GenrePromotionChip {
                        genre_id,
                        canonical_name,
                        kind,
                        inode_count: inodes.len(),
                        sources_bits,
                    })
                })
                .collect();
            chips.sort_by(|a, b| {
                a.kind
                    .cmp(&b.kind)
                    .then_with(|| b.inode_count.cmp(&a.inode_count))
                    .then_with(|| a.canonical_name.cmp(&b.canonical_name))
            });

            // Roll up source bits across chips.
            let sources_bits = chips.iter().fold(0u8, |acc, c| acc | c.sources_bits);

            rows.push(GenrePromotionReviewRow {
                release_id,
                release_title: a.title,
                release_artist: a.artist,
                packed_inode_count: a.inodes.len(),
                proposed_chips: chips,
                current_genre_summary,
                current_genre_uniform,
                sources_bits,
            });
        }

        // Stable display order: title COLLATE NOCASE.
        rows.sort_by(|a, b| {
            a.release_title
                .to_lowercase()
                .cmp(&b.release_title.to_lowercase())
                .then_with(|| a.release_id.cmp(&b.release_id))
        });

        Ok(GenrePromotionReview { rows })
    }

    /// Per-inode chip detail for one release (lazy expand affordance).
    pub fn get_genre_promotion_inode_detail(
        &self,
        release_id: &str,
    ) -> Result<mm_meta::domain_query_types::GenrePromotionInodeDetail> {
        use mm_meta::domain_query_types::{
            GenrePromotionChip, GenrePromotionInodeDetail, GenrePromotionInodeRow,
        };
        use std::collections::HashMap;

        let conn = self.conn();

        // Inodes packed to this release.
        let inodes = self.get_inodes_for_packed_release(release_id)?;
        if inodes.is_empty() {
            return Ok(GenrePromotionInodeDetail {
                release_id: release_id.to_string(),
                rows: Vec::new(),
            });
        }

        // Canonical names + paths for the inode set.
        let mut canonical_names: HashMap<i64, String> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT id, display_name FROM genre_names")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                canonical_names.insert(row.get::<_, i64>(0)?, row.get::<_, String>(1)?);
            }
        }
        let inode_set: std::collections::HashSet<i64> = inodes.iter().copied().collect();
        let mut paths: HashMap<i64, String> = HashMap::new();
        {
            // One representative corpus path per inode.
            let mut stmt = conn.prepare(
                "SELECT inode, path FROM inode_paths WHERE zone = 'corpus'",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let inode: i64 = row.get(0)?;
                if inode_set.contains(&inode) {
                    paths.entry(inode).or_insert_with(|| row.get::<_, String>(1).unwrap_or_default());
                }
            }
        }

        // Ledger rows for this inode set.
        let mut by_inode: HashMap<i64, Vec<GenrePromotionChip>> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT inode, genre_id, kind, source FROM inode_genres",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let inode: i64 = row.get(0)?;
                if !inode_set.contains(&inode) {
                    continue;
                }
                let genre_id: i64 = row.get(1)?;
                let kind: i64 = row.get(2)?;
                let source: i64 = row.get(3)?;
                let canonical = match canonical_names.get(&genre_id) {
                    Some(n) => n.clone(),
                    None => continue,
                };
                let bits = if (1..=8).contains(&source) {
                    1u8 << ((source as u8).saturating_sub(1))
                } else {
                    0
                };
                // Find existing chip for this inode + (genre_id, kind), or push.
                let chip_vec = by_inode.entry(inode).or_default();
                if let Some(existing) = chip_vec
                    .iter_mut()
                    .find(|c| c.genre_id == genre_id && c.kind == kind as u8)
                {
                    existing.sources_bits |= bits;
                } else {
                    chip_vec.push(GenrePromotionChip {
                        genre_id,
                        canonical_name: canonical,
                        kind: kind as u8,
                        inode_count: 1, // per-inode detail; count is always 1
                        sources_bits: bits,
                    });
                }
            }
        }

        let mut rows: Vec<GenrePromotionInodeRow> = inodes
            .into_iter()
            .map(|inode| {
                let mut chips = by_inode.remove(&inode).unwrap_or_default();
                chips.sort_by(|a, b| {
                    a.kind
                        .cmp(&b.kind)
                        .then_with(|| a.canonical_name.cmp(&b.canonical_name))
                });
                GenrePromotionInodeRow {
                    inode,
                    path: paths.remove(&inode).unwrap_or_default(),
                    chips,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(GenrePromotionInodeDetail {
            release_id: release_id.to_string(),
            rows,
        })
    }

    /// Aggregate coverage counters for the Health view.
    pub fn get_genre_coverage_summary(
        &self,
    ) -> Result<mm_meta::domain_query_types::GenreCoverageSummary> {
        use mm_meta::domain_query_types::GenreCoverageSummary;

        let conn = self.conn();
        let total_audio_inodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM audio_info", [], |row| row.get(0))
            .unwrap_or(0);
        let inodes_with_any_ledger: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT inode) FROM inode_genres",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let inodes_with_discogs_ledger: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT inode) FROM inode_genres WHERE source = 3",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let inodes_with_filetag_ledger: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT inode) FROM inode_genres WHERE source = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        // Already promoted: rough proxy — every ledger genre_id maps to a
        // canonical name that already appears as a GENRE corpus_tag for the
        // same inode. Cheap, no per-inode walk in Rust.
        let inodes_already_promoted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                    SELECT ig.inode FROM inode_genres ig
                    JOIN genre_names gn ON gn.id = ig.genre_id
                    LEFT JOIN corpus_tags ct
                        ON ct.inode = ig.inode AND ct.tag_name = 'GENRE'
                        AND ct.tag_value = gn.display_name
                    WHERE ig.source <> 6
                    GROUP BY ig.inode
                    HAVING SUM(CASE WHEN ct.tag_value IS NULL THEN 1 ELSE 0 END) = 0
                )",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let pending_promotion = (inodes_with_any_ledger - inodes_already_promoted).max(0);
        let unresolved_observations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM unresolved_genre_observations",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(GenreCoverageSummary {
            total_audio_inodes,
            inodes_with_any_ledger,
            inodes_with_discogs_ledger,
            inodes_with_filetag_ledger,
            inodes_already_promoted,
            pending_promotion,
            unresolved_observations,
        })
    }

    /// Load `inode_genres` rows for a set of inodes, returning per-inode
    /// ledger vectors keyed by inode. Used by the promote builder feed.
    pub fn get_ledger_for_inodes(
        &self,
        inodes: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<mm_meta::external::genre_promote::LedgerRow>>>
    {
        use mm_meta::external::genre_promote::LedgerRow;
        use mm_meta::external::genre_source::{GenreKind, GenreSource};
        use std::collections::{HashMap, HashSet};

        let inode_set: HashSet<i64> = inodes.iter().copied().collect();
        let mut out: HashMap<i64, Vec<LedgerRow>> = HashMap::new();
        let mut stmt = self.conn().prepare(
            "SELECT inode, genre_id, kind, source FROM inode_genres",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let inode: i64 = row.get(0)?;
            if !inode_set.contains(&inode) {
                continue;
            }
            let genre_id: i64 = row.get(1)?;
            let kind = match GenreKind::from_key(row.get::<_, i64>(2)?) {
                Some(k) => k,
                None => continue,
            };
            let source = match GenreSource::from_key(row.get::<_, i64>(3)?) {
                Some(s) => s,
                None => continue,
            };
            out.entry(inode)
                .or_default()
                .push(LedgerRow { genre_id, kind, source });
        }
        Ok(out)
    }

    /// Load canonical (id → display_name) mapping in one shot.
    pub fn get_canonical_genre_names(
        &self,
    ) -> Result<std::collections::HashMap<i64, String>> {
        use std::collections::HashMap;
        let mut out = HashMap::new();
        let mut stmt = self
            .conn()
            .prepare("SELECT id, display_name FROM genre_names")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            out.insert(row.get::<_, i64>(0)?, row.get::<_, String>(1)?);
        }
        Ok(out)
    }

    /// Load the unresolved-observation queue, sorted by observation count desc.
    pub fn get_unresolved_genre_observations(
        &self,
    ) -> Result<mm_meta::domain_query_types::UnresolvedGenreObservations> {
        use mm_meta::domain_query_types::{UnresolvedGenreObservations, UnresolvedGenreRow};

        let mut stmt = self.conn().prepare(
            "SELECT raw_value, source, observation_count, last_seen_at
             FROM unresolved_genre_observations
             ORDER BY observation_count DESC, last_seen_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(UnresolvedGenreRow {
                    raw_value: row.get(0)?,
                    source: row.get::<_, i64>(1)? as u8,
                    observation_count: row.get(2)?,
                    last_seen_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(UnresolvedGenreObservations { rows })
    }
}
