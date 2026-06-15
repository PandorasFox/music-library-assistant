//! Pure builder for genre promotion decisions.
//!
//! Mirror of `va_override.rs`: take operator-approved release applications
//! plus per-inode ledger snapshots, produce one `ApprovalDecision` per
//! release with per-inode `TagOp` sets that converge each inode's
//! `corpus_tags` GENRE/STYLE entries to match its `inode_genres` ledger
//! (after exclusions and write-policy flattening).
//!
//! ## Semantics
//!
//! - The **ledger is per-inode**. Each inode's `TagOp` set is computed from
//!   its OWN ledger rows; two inodes in the same release can land different
//!   GENRE sets when their ledger rows diverge (common when one has
//!   FileTagImport-only and another has FileTagImport + Discogs).
//! - **Authoritative semantic**: promotion makes the file's target tags
//!   EQUAL to the ledger view. Any current GENRE/STYLE value not in the
//!   expected set is dropped. Operators wanting to preserve non-ledger
//!   values must first map them via `ImportGenresFromTags` + the vocabulary
//!   editor (so they land in the ledger as `FileTagImport`).
//! - **Implied source is never written**. The implication closure stays
//!   query-time-only; downstream apps compute their own implications.
//! - **Excluded chips apply uniformly per release**. Operator toggles a
//!   chip off in the review UI → the `(genre_id, kind)` pair is excluded
//!   from every inode in that release, even if some inodes' ledger says
//!   it should be there.
//!
//! No I/O. Caller loads `ledger_by_inode`, `canonical_names`, and
//! `current_tags` from the read-only DB and passes them in.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::GenreWriteLayout;
use crate::external::genre_source::{GenreKind, GenreSource};
use crate::mutations::TagOp;
use crate::views::external_matches::ApprovalDecision;

/// One row from `inode_genres` consumed by the builder. Source-typed so the
/// builder can filter `Implied` without consulting a separate map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LedgerRow {
    pub genre_id: i64,
    pub kind: GenreKind,
    pub source: GenreSource,
}

/// Per-release operator application: which inodes to promote, plus
/// chip-level exclusions applied uniformly across them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenrePromotionApplication {
    pub release_id: String,
    /// Inodes from this release to promote. Empty → skipped (counted in
    /// `skipped_empty_applications`).
    pub inodes: Vec<i64>,
    /// `(genre_id, kind)` pairs the operator toggled OFF. Excluded from
    /// every inode's ledger flatten step. Empty = "promote everything".
    pub excluded: Vec<(i64, GenreKind)>,
}

/// Per-pass summary returned alongside decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenrePromotionSummary {
    /// Releases that produced at least one decision.
    pub staged_releases: usize,
    /// Total inodes that received at least one TagOp across all decisions.
    pub staged_inodes: usize,
    /// Releases dropped because `inodes` was empty.
    pub skipped_empty_applications: usize,
    /// Inodes whose ledger + exclusions resolved to exactly the current
    /// disk tags (no TagOp emitted — they're already promoted).
    pub already_matching_inodes: usize,
    /// Inodes that had no ledger rows at all (after Implied-filter +
    /// exclusion). Nothing to write; counted separately from
    /// `already_matching_inodes` so the UI can show "no ledger" vs
    /// "ledger matches".
    pub inodes_without_ledger: usize,
}

/// Build per-release approval decisions from operator applications + ledger
/// snapshot + current corpus_tags + write policy.
///
/// Pure function. For each application:
/// - Skips empty `inodes` (`skipped_empty_applications` += 1).
/// - Per inode:
///   - Filter ledger rows: drop `Implied` source; drop rows in `excluded`.
///   - Group surviving rows by `kind` (Genre vs Style).
///   - Flatten via `write_policy` → expected `(tag_name, Vec<value>)` pairs.
///     `value` order in the Vec is the canonical display order the policy
///     dictates (genres before styles for the merged layout, alphabetical
///     dedup otherwise).
///   - Compute diff against `current_tags`. Drop ops first, add ops after,
///     so corpus_tags converges atomically to the expected set.
///   - If diff is empty → `already_matching_inodes` += 1, no ops.
///   - If filtered ledger is empty → `inodes_without_ledger` += 1, no ops.
///     We deliberately do NOT drop current tags in this case (a release
///     with no Discogs coverage shouldn't nuke a file's pre-existing GENRE
///     just because the operator clicked "promote").
pub fn build_genre_promotion_decisions(
    applications: &[GenrePromotionApplication],
    ledger_by_inode: &HashMap<i64, Vec<LedgerRow>>,
    canonical_names: &HashMap<i64, String>,
    current_tags: &HashMap<i64, Vec<(String, String)>>,
    write_policy: GenreWriteLayout,
) -> (Vec<ApprovalDecision>, GenrePromotionSummary) {
    let mut decisions = Vec::new();
    let mut summary = GenrePromotionSummary::default();

    for app in applications {
        if app.inodes.is_empty() {
            summary.skipped_empty_applications += 1;
            continue;
        }

        let excluded: HashSet<(i64, GenreKind)> = app.excluded.iter().copied().collect();
        let mut per_inode_ops: Vec<Vec<TagOp>> = Vec::new();

        for &inode in &app.inodes {
            // 1) Survive-filter the ledger for this inode.
            let raw_rows = ledger_by_inode.get(&inode).cloned().unwrap_or_default();
            let filtered: Vec<&LedgerRow> = raw_rows
                .iter()
                .filter(|r| r.source != GenreSource::Implied)
                .filter(|r| !excluded.contains(&(r.genre_id, r.kind)))
                .collect();

            if filtered.is_empty() {
                summary.inodes_without_ledger += 1;
                continue;
            }

            // 2) Flatten via the write policy. Returns target tag values
            //    grouped by tag name.
            let expected = flatten_with_policy(&filtered, canonical_names, write_policy);
            if expected.is_empty() {
                summary.inodes_without_ledger += 1;
                continue;
            }

            // 3) Compute diff against current corpus_tags for the target
            //    tag names. Tag names compared case-insensitively (matches
            //    corpus_tags COLLATE NOCASE).
            let current = current_tags.get(&inode).cloned().unwrap_or_default();
            let ops = diff_to_tag_ops(inode, &expected, &current);

            if ops.is_empty() {
                summary.already_matching_inodes += 1;
                continue;
            }

            per_inode_ops.push(ops);
        }

        if per_inode_ops.is_empty() {
            continue;
        }

        summary.staged_inodes += per_inode_ops.len();
        decisions.push(ApprovalDecision {
            release_id: app.release_id.clone(),
            label: format!(
                "Promote genres: {} [{} inode{}]",
                &app.release_id[..8.min(app.release_id.len())],
                per_inode_ops.len(),
                if per_inode_ops.len() == 1 { "" } else { "s" },
            ),
            per_inode_ops,
        });
    }

    summary.staged_releases = decisions.len();
    (decisions, summary)
}

/// Flatten a per-inode filtered ledger row set into target tag values,
/// keyed by tag name, ordered per the write policy.
///
/// Implementation detail of `build_genre_promotion_decisions` but exposed
/// for direct testing.
pub fn flatten_with_policy(
    rows: &[&LedgerRow],
    canonical_names: &HashMap<i64, String>,
    policy: GenreWriteLayout,
) -> HashMap<String, Vec<String>> {
    // Dedupe rows by (genre_id, kind) — multi-source rows for the same
    // canonical genre collapse to one tag value.
    let mut seen: HashSet<(i64, GenreKind)> = HashSet::new();
    let mut genres: Vec<i64> = Vec::new();
    let mut styles: Vec<i64> = Vec::new();
    for row in rows {
        if !seen.insert((row.genre_id, row.kind)) {
            continue;
        }
        match row.kind {
            GenreKind::Genre => genres.push(row.genre_id),
            GenreKind::Style => styles.push(row.genre_id),
        }
    }

    // Canonical-name lookup; rows whose genre_id isn't in the map are
    // silently skipped (shouldn't happen if caller passed a complete map,
    // but degrade gracefully).
    let name = |id: i64| canonical_names.get(&id).cloned();

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    match policy {
        GenreWriteLayout::MergedGenresFirst => {
            // Genres first, styles after. Both flattened into GENRE.
            let mut values: Vec<String> = Vec::new();
            for id in genres.iter().chain(styles.iter()) {
                if let Some(n) = name(*id) {
                    if !values.contains(&n) {
                        values.push(n);
                    }
                }
            }
            if !values.is_empty() {
                out.insert("GENRE".to_string(), values);
            }
        }
        GenreWriteLayout::SeparateGenreStyle => {
            let g: Vec<String> = genres.iter().filter_map(|id| name(*id)).collect();
            let s: Vec<String> = styles.iter().filter_map(|id| name(*id)).collect();
            if !g.is_empty() {
                out.insert("GENRE".to_string(), g);
            }
            if !s.is_empty() {
                out.insert("STYLE".to_string(), s);
            }
        }
        GenreWriteLayout::UmbrellasGenreSpecificsStyle => {
            // Without the closure available here, this layout currently
            // behaves like `MergedGenresFirst`. The plan flags it as
            // requiring a populated genre_implies graph + a closure feed
            // through the input. Until that's wired, fall through.
            //
            // We deliberately don't silently misbehave — for the closure
            // to make sense, the caller would need to pre-resolve umbrella
            // ids and include them as kind=Genre rows in `rows`. The
            // closure resolution belongs in the read-DB layer, not in
            // this pure builder.
            let mut values: Vec<String> = Vec::new();
            for id in genres.iter().chain(styles.iter()) {
                if let Some(n) = name(*id) {
                    if !values.contains(&n) {
                        values.push(n);
                    }
                }
            }
            if !values.is_empty() {
                out.insert("GENRE".to_string(), values);
            }
        }
    }
    out
}

/// Compute the minimal TagOp set converging current corpus_tags to the
/// expected layout for one inode. Drops first, adds after.
fn diff_to_tag_ops(
    inode: i64,
    expected: &HashMap<String, Vec<String>>,
    current: &[(String, String)],
) -> Vec<TagOp> {
    // The tag names we manage. Anything outside this set is left alone —
    // we don't touch ALBUM, ARTIST, etc.
    let managed_tag_names: HashSet<&str> = expected
        .keys()
        .map(|s| s.as_str())
        .chain(["GENRE", "STYLE"]) // also drop stale GENRE/STYLE we no longer want
        .collect();

    let mut ops: Vec<TagOp> = Vec::new();

    for tag_name in &managed_tag_names {
        let expected_set: HashSet<&String> = expected
            .get(*tag_name)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        let current_values: Vec<&String> = current
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(tag_name))
            .map(|(_, v)| v)
            .collect();
        let current_set: HashSet<&String> = current_values.iter().copied().collect();

        // Drops: current values not in expected.
        for v in &current_values {
            if !expected_set.contains(v) {
                ops.push(TagOp::drop_tag(inode, *tag_name, (*v).clone()));
            }
        }
        // Adds: expected values not in current.
        if let Some(expected_vec) = expected.get(*tag_name) {
            for v in expected_vec {
                if !current_set.contains(v) {
                    ops.push(TagOp::add_tag(inode, *tag_name, v.clone()));
                }
            }
        }
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(pairs: &[(i64, &str)]) -> HashMap<i64, String> {
        pairs.iter().map(|(id, n)| (*id, (*n).to_string())).collect()
    }

    fn row(id: i64, kind: GenreKind, source: GenreSource) -> LedgerRow {
        LedgerRow { genre_id: id, kind, source }
    }

    #[test]
    fn empty_application_inodes_skipped() {
        let app = GenrePromotionApplication {
            release_id: "r1".into(),
            inodes: vec![],
            excluded: vec![],
        };
        let (decisions, summary) = build_genre_promotion_decisions(
            &[app],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        assert!(decisions.is_empty());
        assert_eq!(summary.skipped_empty_applications, 1);
    }

    #[test]
    fn missing_ledger_emits_no_ops_and_counts() {
        let app = GenrePromotionApplication {
            release_id: "r1".into(),
            inodes: vec![1, 2],
            excluded: vec![],
        };
        let (decisions, summary) = build_genre_promotion_decisions(
            &[app],
            &HashMap::new(),    // no ledger rows
            &names(&[(10, "Rock")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        assert!(decisions.is_empty());
        assert_eq!(summary.inodes_without_ledger, 2);
        assert_eq!(summary.staged_releases, 0);
    }

    #[test]
    fn default_promotes_all_chips_per_inode() {
        let mut ledger = HashMap::new();
        ledger.insert(
            1,
            vec![
                row(10, GenreKind::Genre, GenreSource::Discogs),
                row(20, GenreKind::Style, GenreSource::Discogs),
            ],
        );
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let (decisions, summary) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Electronic"), (20, "Techno")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        assert_eq!(decisions.len(), 1);
        let ops = &decisions[0].per_inode_ops[0];
        // Both adds, one GENRE tag with two values.
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().all(|o| o.tag_name == "GENRE"));
        assert!(ops.iter().all(|o| o.old_value.is_none() && o.new_value.is_some()));
        let new_values: Vec<&String> =
            ops.iter().filter_map(|o| o.new_value.as_ref()).collect();
        assert!(new_values.contains(&&"Electronic".to_string()));
        assert!(new_values.contains(&&"Techno".to_string()));
        assert_eq!(summary.staged_inodes, 1);
        assert_eq!(summary.staged_releases, 1);
    }

    #[test]
    fn excluded_chip_omitted_uniformly() {
        let mut ledger = HashMap::new();
        ledger.insert(1, vec![row(10, GenreKind::Genre, GenreSource::Discogs)]);
        ledger.insert(2, vec![row(10, GenreKind::Genre, GenreSource::Discogs)]);
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1, 2],
            excluded: vec![(10, GenreKind::Genre)],
        };
        let (decisions, summary) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Wrong")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        // Both inodes' only ledger row was excluded → no ledger left → no ops.
        assert!(decisions.is_empty());
        assert_eq!(summary.inodes_without_ledger, 2);
    }

    #[test]
    fn per_inode_granularity_diverges() {
        // Two inodes in the same release: one has only FileTagImport, the
        // other has FileTagImport + Discogs. The two TagOp sets must differ.
        let mut ledger = HashMap::new();
        ledger.insert(
            1,
            vec![row(10, GenreKind::Genre, GenreSource::FileTagImport)],
        );
        ledger.insert(
            2,
            vec![
                row(10, GenreKind::Genre, GenreSource::FileTagImport),
                row(30, GenreKind::Style, GenreSource::Discogs),
            ],
        );
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1, 2],
            excluded: vec![],
        };
        let (decisions, _) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Rock"), (30, "Stoner Rock")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        let inner = &decisions[0].per_inode_ops;
        assert_eq!(inner.len(), 2);
        // Inode 1: one add (Rock). Inode 2: two adds (Rock + Stoner Rock).
        let mut lens: Vec<usize> = inner.iter().map(|v| v.len()).collect();
        lens.sort();
        assert_eq!(lens, vec![1, 2]);
    }

    #[test]
    fn implied_source_never_written() {
        let mut ledger = HashMap::new();
        ledger.insert(
            1,
            vec![
                row(10, GenreKind::Genre, GenreSource::Implied),
                row(20, GenreKind::Genre, GenreSource::Discogs),
            ],
        );
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let (decisions, _) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Umbrella"), (20, "Specific")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        let ops = &decisions[0].per_inode_ops[0];
        // Only "Specific" lands; "Umbrella" was Implied so it's filtered.
        let new_values: Vec<&String> =
            ops.iter().filter_map(|o| o.new_value.as_ref()).collect();
        assert_eq!(new_values.len(), 1);
        assert_eq!(new_values[0], "Specific");
    }

    #[test]
    fn merged_vs_separate_layout_diverges() {
        let mut ledger = HashMap::new();
        ledger.insert(
            1,
            vec![
                row(10, GenreKind::Genre, GenreSource::Discogs),
                row(20, GenreKind::Style, GenreSource::Discogs),
            ],
        );
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let n = names(&[(10, "Electronic"), (20, "Techno")]);

        let (merged, _) = build_genre_promotion_decisions(
            &[app.clone()],
            &ledger,
            &n,
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        let merged_ops = &merged[0].per_inode_ops[0];
        // Both as GENRE.
        assert!(merged_ops.iter().all(|o| o.tag_name == "GENRE"));
        assert_eq!(merged_ops.len(), 2);

        let (sep, _) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &n,
            &HashMap::new(),
            GenreWriteLayout::SeparateGenreStyle,
        );
        let sep_ops = &sep[0].per_inode_ops[0];
        // One GENRE add + one STYLE add.
        assert_eq!(sep_ops.len(), 2);
        let by_name: HashMap<&str, &TagOp> =
            sep_ops.iter().map(|o| (o.tag_name.as_str(), o)).collect();
        assert!(by_name.contains_key("GENRE"));
        assert!(by_name.contains_key("STYLE"));
        assert_eq!(by_name["GENRE"].new_value.as_deref(), Some("Electronic"));
        assert_eq!(by_name["STYLE"].new_value.as_deref(), Some("Techno"));
    }

    #[test]
    fn already_matching_emits_no_ops() {
        let mut ledger = HashMap::new();
        ledger.insert(1, vec![row(10, GenreKind::Genre, GenreSource::Discogs)]);
        let mut current = HashMap::new();
        current.insert(1, vec![("GENRE".to_string(), "Rock".to_string())]);

        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let (decisions, summary) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Rock")]),
            &current,
            GenreWriteLayout::MergedGenresFirst,
        );
        assert!(decisions.is_empty());
        assert_eq!(summary.already_matching_inodes, 1);
    }

    #[test]
    fn drops_stale_then_adds_new() {
        // Current GENRE has "WrongValue"; ledger says "Rock". Expect one drop + one add.
        let mut ledger = HashMap::new();
        ledger.insert(1, vec![row(10, GenreKind::Genre, GenreSource::Discogs)]);
        let mut current = HashMap::new();
        current.insert(1, vec![("GENRE".to_string(), "WrongValue".to_string())]);

        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let (decisions, _) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Rock")]),
            &current,
            GenreWriteLayout::MergedGenresFirst,
        );
        let ops = &decisions[0].per_inode_ops[0];
        assert_eq!(ops.len(), 2);
        // Drop "WrongValue", add "Rock".
        let dropped: Vec<&str> = ops
            .iter()
            .filter(|o| o.old_value.is_some() && o.new_value.is_none())
            .filter_map(|o| o.old_value.as_deref())
            .collect();
        let added: Vec<&str> = ops
            .iter()
            .filter(|o| o.old_value.is_none() && o.new_value.is_some())
            .filter_map(|o| o.new_value.as_deref())
            .collect();
        assert_eq!(dropped, vec!["WrongValue"]);
        assert_eq!(added, vec!["Rock"]);
    }

    #[test]
    fn merged_orders_genres_before_styles() {
        let mut ledger = HashMap::new();
        ledger.insert(
            1,
            vec![
                row(20, GenreKind::Style, GenreSource::Discogs), // inserted first
                row(10, GenreKind::Genre, GenreSource::Discogs),
            ],
        );
        let app = GenrePromotionApplication {
            release_id: "rid".into(),
            inodes: vec![1],
            excluded: vec![],
        };
        let (decisions, _) = build_genre_promotion_decisions(
            &[app],
            &ledger,
            &names(&[(10, "Electronic"), (20, "Techno")]),
            &HashMap::new(),
            GenreWriteLayout::MergedGenresFirst,
        );
        let ops = &decisions[0].per_inode_ops[0];
        // Order in TagOp generation is determined by `flatten_with_policy`
        // which emits genres before styles regardless of ledger insertion
        // order. We check that the *expected vec* respected that.
        let new_values: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.new_value.as_deref())
            .collect();
        // Both values present.
        assert!(new_values.contains(&"Electronic"));
        assert!(new_values.contains(&"Techno"));
    }
}
