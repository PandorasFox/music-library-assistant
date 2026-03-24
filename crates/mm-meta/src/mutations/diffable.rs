//! Field-level diffing infrastructure for transaction review.
//!
//! The `Diffable` trait produces `DiffEntry` values by comparing two instances
//! of the same type field-by-field. A declarative macro covers struct types
//! mechanically; leaf types (primitives, strings, enums) get manual impls.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::PathBuf;

use super::types::DiffEntry;

/// Recursively diff two values, emitting `DiffEntry` items for changed fields.
///
/// `prefix` is the dotted path context (e.g. `"opinions.performance"`).
pub trait Diffable {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>);
}

/// Implement `Diffable` for a struct by diffing each named field.
///
/// Usage:
/// ```ignore
/// impl_diffable_struct!(MyStruct { field_a, field_b, field_c });
/// ```
macro_rules! impl_diffable_struct {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl Diffable for $ty {
            fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
                $(
                    self.$field.diff_against(
                        &other.$field,
                        &format!(concat!("{}.", stringify!($field)), prefix),
                        out,
                    );
                )*
            }
        }
    };
}

// ============================================================================
// Leaf type impls
// ============================================================================

/// Helper: emit a single DiffEntry if two Debug-formatted values differ.
fn diff_leaf<T: Debug + PartialEq>(old: &T, new: &T, label: &str, out: &mut Vec<DiffEntry>) {
    if old != new {
        out.push(DiffEntry::new(label, format!("{:?}", old), format!("{:?}", new)));
    }
}

impl Diffable for bool {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for f64 {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for u32 {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for u64 {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for i64 {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for usize {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for String {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        if self != other {
            out.push(DiffEntry::new(prefix, self, other));
        }
    }
}

impl Diffable for PathBuf {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        if self != other {
            out.push(DiffEntry::new(
                prefix,
                self.display(),
                other.display(),
            ));
        }
    }
}

impl<T: Diffable> Diffable for Option<T>
where
    T: Debug,
{
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        match (self, other) {
            (Some(a), Some(b)) => a.diff_against(b, prefix, out),
            (None, None) => {}
            (old, new) => {
                out.push(DiffEntry::new(
                    prefix,
                    format!("{:?}", old),
                    format!("{:?}", new),
                ));
            }
        }
    }
}

impl<T: Debug + PartialEq> Diffable for Vec<T> {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        if self != other {
            out.push(DiffEntry::new(
                prefix,
                format!("{:?}", self),
                format!("{:?}", other),
            ));
        }
    }
}

impl<V: Debug + PartialEq> Diffable for HashMap<String, V> {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        // Keys removed
        for key in self.keys() {
            if !other.contains_key(key) {
                out.push(DiffEntry::new(
                    format!("{}.{}", prefix, key),
                    format!("{:?}", &self[key]),
                    "(removed)",
                ));
            }
        }
        // Keys added or changed
        for (key, new_val) in other {
            match self.get(key) {
                None => {
                    out.push(DiffEntry::new(
                        format!("{}.{}", prefix, key),
                        "(none)",
                        format!("{:?}", new_val),
                    ));
                }
                Some(old_val) if old_val != new_val => {
                    out.push(DiffEntry::new(
                        format!("{}.{}", prefix, key),
                        format!("{:?}", old_val),
                        format!("{:?}", new_val),
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Diffable for HashSet<String> {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        if self != other {
            let mut old: Vec<_> = self.iter().collect();
            old.sort();
            let mut new: Vec<_> = other.iter().collect();
            new.sort();
            out.push(DiffEntry::new(
                prefix,
                format!("{:?}", old),
                format!("{:?}", new),
            ));
        }
    }
}

// ============================================================================
// Config struct impls
// ============================================================================

use crate::config::{
    AlbumArtOpinions, CanonicalizationOpinions, CreditRoutingConfig, DebugOpinions,
    DiscExtractionOpinions, DuplicateAnalysisOpinions, ExternalMatchingConfig,
    HealthDetectionOpinions, Opinions,
    PackingWeights, PerformanceOpinions, QualityResolutionOpinions, RelationRouting,
    ReleasePackingOpinions, SidecarDeployMode, SourceDir, StartupOpinions, StartupView,
    TagSplittingOpinions,
};
use crate::config::PathTagSchema;

// --- Enums (leaf-level, use Debug formatting) ---

impl Diffable for StartupView {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for SidecarDeployMode {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

impl Diffable for PathTagSchema {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        if self.template != other.template {
            out.push(DiffEntry::new(prefix, &self.template, &other.template));
        }
    }
}

impl Diffable for RelationRouting {
    fn diff_against(&self, other: &Self, prefix: &str, out: &mut Vec<DiffEntry>) {
        diff_leaf(self, other, prefix, out);
    }
}

// --- Structs ---

// QualityResolutionOpinions has no fields — always equal.
impl Diffable for QualityResolutionOpinions {
    fn diff_against(&self, _other: &Self, _prefix: &str, _out: &mut Vec<DiffEntry>) {}
}
impl_diffable_struct!(CanonicalizationOpinions { strip_album_format_suffixes });
impl_diffable_struct!(StartupOpinions { force_check_all_files_at_startup, vacuum_threshold, default_view });
impl_diffable_struct!(HealthDetectionOpinions { required_tags, album_artist_only_required_if_compilation, single_album_suffix });
impl_diffable_struct!(PerformanceOpinions { worker_threads, db_cache_mb });
impl_diffable_struct!(TagSplittingOpinions { collaboration_keywords, tag_separators });
impl_diffable_struct!(DuplicateAnalysisOpinions { fingerprint_similarity_threshold, duration_tolerance_ms, elide_variant_titles });
impl_diffable_struct!(PackingWeights { acoustid_confidence, duration_match, title_match, artist_match, album_match, track_number_match });
impl_diffable_struct!(ReleasePackingOpinions {
    duration_tolerance_pct, min_confidence, candidate_weights, elimination_weights,
    title_preassign_threshold, packing_knot_ratio, packing_knot_size_limit,
    singles_before_incompletes, allow_resolve_knots_with_discographies,
    low_confidence_max_acoustid_ratio, low_confidence_max_album_match,
});
impl_diffable_struct!(CreditRoutingConfig { routing, feat_format });
impl_diffable_struct!(ExternalMatchingConfig {
    acoustid_api_key, requests_per_second, mb_requests_per_second, mb_base_url,
    auto_enrich_on_match, mb_cache_ttl_days, preferred_locales, tag_templates, credit_routing,
    cover_art_fetch, cover_art_upgrade, cover_art_types,
});
impl_diffable_struct!(DiscExtractionOpinions { disc_tag_name, map_letters_to_numbers });
impl_diffable_struct!(AlbumArtOpinions { sidecar_deploy_mode });

// DebugOpinions has no fields — always equal.
impl Diffable for DebugOpinions {
    fn diff_against(&self, _other: &Self, _prefix: &str, _out: &mut Vec<DiffEntry>) {}
}

impl_diffable_struct!(Opinions {
    quality_resolution, canonicalization, startup,
    health_detection, performance, tag_splitting, duplicate_analysis, release_packing,
    leave_transactions_open, external_matching, disc_extraction,
    album_art, debug, watcher_poll_interval_secs, session_lifetime_days,
});

impl_diffable_struct!(SourceDir {
    path, libraries, can_stash_dupes, interior_dupes, path_schema, enable_acoustid,
    pinned_release,
});

// HashMap<String, RelationRouting> needs the HashMap impl + RelationRouting: Debug + PartialEq.
// RelationRouting already derives Debug + PartialEq via the struct definition, and we have
// HashMap<String, V: Debug + PartialEq> covered above. ✓

// HashMap<String, Vec<String>> for tag_separators: Vec<String> is Debug + PartialEq. ✓

// Vec<(String, String)> for tag_templates: (String, String) is Debug + PartialEq. ✓
