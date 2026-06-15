//! Pure genre-string resolver.
//!
//! Given a raw observed genre string (from a file tag, MusicBrainz response,
//! Discogs release, etc.), normalize it and look it up against a snapshot of
//! the `genre_aliases` table. The alias map is **many-to-many**: a single
//! raw value can resolve to multiple canonical genres (e.g. a comma-joined
//! tag like "alternative, pop, rock" might be mapped to three canonicals).
//!
//! Returns either the matched `genre_ids` or `Unresolved` — callers route
//! unresolved strings to `unresolved_genre_observations` so the alias editor
//! can surface them.
//!
//! No I/O. The caller is responsible for loading the alias snapshot and for
//! persisting any side effects.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Snapshot of `genre_aliases` for one resolver pass.
///
/// Keys are normalized via [`normalize_genre`] (and lowercased — see
/// `build_alias_map`) before lookup; loaders should normalize each row's
/// `alias` column the same way at population time. Each entry can carry
/// multiple `genre_id` targets because the alias table is composite-keyed
/// on `(alias, genre_id)`.
pub type AliasMap = HashMap<String, Vec<i64>>;

/// Outcome of a single resolve call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveOutcome {
    /// Raw string mapped to one or more canonical `genre_id`s. The Vec is
    /// non-empty (an empty match would be reported as `Unresolved`).
    Resolved {
        /// Distinct canonical ids this raw resolves to, sorted for
        /// deterministic downstream ordering.
        genre_ids: Vec<i64>,
        /// The original (un-normalized) string the caller passed in, so we
        /// can store it on `inode_genres.raw_value` for audit.
        raw: String,
    },
    /// Raw string did not match any alias. Caller should bump the queue in
    /// `unresolved_genre_observations`.
    Unresolved { raw: String },
}

/// Normalize a raw genre string for alias-table lookup.
///
/// - trims leading/trailing ASCII whitespace,
/// - collapses interior runs of whitespace to a single space,
/// - does NOT case-fold — the `genre_aliases` table is `COLLATE NOCASE`,
///   so storage handles case, and we preserve the operator's chosen display
///   casing in case it matters for later audit.
///
/// Returns the empty string for inputs that are whitespace-only; callers
/// should treat empty as "no observation" and skip.
pub fn normalize_genre(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_was_space = true; // suppresses leading whitespace
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(ch);
            prev_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Resolve a single raw genre string against a normalized alias snapshot.
///
/// `aliases` MUST be keyed by `normalize_genre(...).to_lowercase()` (build
/// via [`build_alias_map`]).
pub fn resolve_genre(raw: &str, aliases: &AliasMap) -> ResolveOutcome {
    let normalized = normalize_genre(raw);
    if normalized.is_empty() {
        return ResolveOutcome::Unresolved { raw: raw.to_string() };
    }
    let key = normalized.to_lowercase();
    if let Some(ids) = aliases.get(&key) {
        if !ids.is_empty() {
            return ResolveOutcome::Resolved {
                genre_ids: ids.clone(),
                raw: raw.to_string(),
            };
        }
    }
    ResolveOutcome::Unresolved { raw: raw.to_string() }
}

/// Build an in-memory alias map from raw `(alias, genre_id)` pairs.
///
/// Loaders call this after `SELECT alias, genre_id FROM genre_aliases`.
/// Keys are normalized + lowercased so [`resolve_genre`] can do a single
/// O(1) lookup per raw input. Multiple rows with the same alias text but
/// different `genre_id`s collect into one entry's Vec, preserving the
/// many-to-many semantic of the underlying composite-PK table.
///
/// Within each entry, ids are sorted + deduped for deterministic output.
pub fn build_alias_map(rows: impl IntoIterator<Item = (String, i64)>) -> AliasMap {
    let mut map: AliasMap = AliasMap::new();
    for (alias, id) in rows {
        let key = normalize_genre(&alias).to_lowercase();
        if key.is_empty() {
            continue;
        }
        map.entry(key).or_default().push(id);
    }
    for ids in map.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, i64)]) -> AliasMap {
        build_alias_map(pairs.iter().map(|(a, i)| ((*a).to_string(), *i)))
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_genre("Drum  and   Bass"), "Drum and Bass");
        assert_eq!(normalize_genre("  Rock  "), "Rock");
        assert_eq!(normalize_genre("Hip\tHop"), "Hip Hop");
        assert_eq!(normalize_genre(""), "");
        assert_eq!(normalize_genre("   "), "");
    }

    #[test]
    fn normalize_preserves_case_and_punctuation() {
        assert_eq!(normalize_genre("R&B"), "R&B");
        assert_eq!(normalize_genre("Drum'n'Bass"), "Drum'n'Bass");
    }

    #[test]
    fn resolve_matches_single_target() {
        let m = map(&[("Rock", 10), ("Drum and Bass", 20)]);
        match resolve_genre("Rock", &m) {
            ResolveOutcome::Resolved { genre_ids, raw } => {
                assert_eq!(genre_ids, vec![10]);
                assert_eq!(raw, "Rock");
            }
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn resolve_matches_multi_target() {
        // One alias mapped to three canonicals — exactly the
        // "alternative, pop, rock" → [Alt, Pop, Rock] use case.
        let m = map(&[
            ("alternative, pop, rock", 10),
            ("alternative, pop, rock", 20),
            ("alternative, pop, rock", 30),
        ]);
        match resolve_genre("alternative, pop, rock", &m) {
            ResolveOutcome::Resolved { genre_ids, raw } => {
                // Sorted + deduped by build_alias_map.
                assert_eq!(genre_ids, vec![10, 20, 30]);
                assert_eq!(raw, "alternative, pop, rock");
            }
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn resolve_case_insensitive() {
        let m = map(&[("Rock", 10)]);
        let out = resolve_genre("ROCK", &m);
        assert!(matches!(out, ResolveOutcome::Resolved { ref genre_ids, .. } if genre_ids == &vec![10]));
        let out = resolve_genre("rock", &m);
        assert!(matches!(out, ResolveOutcome::Resolved { ref genre_ids, .. } if genre_ids == &vec![10]));
    }

    #[test]
    fn resolve_whitespace_normalized() {
        let m = map(&[("Drum and Bass", 20)]);
        let out = resolve_genre("  Drum   and  Bass  ", &m);
        match out {
            ResolveOutcome::Resolved { genre_ids, raw } => {
                assert_eq!(genre_ids, vec![20]);
                // Raw preserved verbatim for audit
                assert_eq!(raw, "  Drum   and  Bass  ");
            }
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn resolve_unknown_returns_unresolved() {
        let m = map(&[("Rock", 10)]);
        match resolve_genre("Jazz", &m) {
            ResolveOutcome::Unresolved { raw } => assert_eq!(raw, "Jazz"),
            _ => panic!("expected unresolved"),
        }
    }

    #[test]
    fn resolve_empty_returns_unresolved() {
        let m = map(&[("Rock", 10)]);
        assert!(matches!(
            resolve_genre("", &m),
            ResolveOutcome::Unresolved { .. }
        ));
        assert!(matches!(
            resolve_genre("   ", &m),
            ResolveOutcome::Unresolved { .. }
        ));
    }

    #[test]
    fn build_alias_map_dedupes_same_pair_and_sorts_targets() {
        // Same (normalized alias, id) pair appearing twice = one entry.
        // Different targets accumulate. Final Vec is sorted + deduped.
        let m = build_alias_map([
            ("Rock".to_string(), 11),
            ("rock".to_string(), 11),     // duplicate (alias, id)
            ("ROCK ".to_string(), 22),    // same alias, new id
            ("ROCK".to_string(), 11),     // duplicate (alias, id) again
            ("Rock".to_string(), 33),     // third target
        ]);
        assert_eq!(m.len(), 1);
        let ids = m.values().next().unwrap();
        assert_eq!(ids, &vec![11, 22, 33]);
    }

    #[test]
    fn build_alias_map_keeps_separate_aliases_separate() {
        let m = build_alias_map([
            ("Rock".to_string(), 10),
            ("Pop".to_string(), 20),
        ]);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("rock"), Some(&vec![10]));
        assert_eq!(m.get("pop"), Some(&vec![20]));
    }
}
