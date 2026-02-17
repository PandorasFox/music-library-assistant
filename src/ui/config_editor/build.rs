//! Config Editor Build/Apply
//!
//! Converts between `Config` struct and the flat group/field representation
//! used by the editor UI. `build_groups_from_config` populates the editor;
//! `apply_groups_to_config` writes edits back.

use crate::config::{Config, StartupView, InboxOrganizeGranularity};
use super::types::*;

/// Construct a ConfigField with original_value/original_source automatically
/// snapshotted from the initial value/source.
///
/// `_default_value` is accepted for documentation but not stored — the original
/// value (from the loaded config) serves as the reset target, not the compiled default.
fn field(
    label: &'static str,
    description: &'static str,
    value: ConfigValue,
    _default_value: ConfigValue,
    source: FieldSource,
    restart_required: bool,
) -> ConfigField {
    ConfigField {
        label,
        description,
        original_value: value.clone(),
        original_source: source,
        value,
        source,
        restart_required,
    }
}

/// Build editor groups from the current config.
///
/// `kdl_content` is used to determine whether a field was loaded from the
/// config file (vs being at its default). If None, all fields show as Default.
pub fn build_groups_from_config(config: &Config, kdl_content: Option<&str>) -> Vec<ConfigGroup> {
    let defaults = crate::config::Opinions::default();
    let ops = &config.opinions;

    // Helper: check if a KDL node path exists in the file content
    let in_kdl = |path: &str| -> bool {
        kdl_content.map_or(false, |content| content.contains(path))
    };

    // Determine field source: if value differs from default it's Loaded (must be from file),
    // otherwise check if the key exists in KDL content.
    let source_for = |matches_default: bool, kdl_key: &str| -> FieldSource {
        if !matches_default {
            FieldSource::Loaded
        } else if in_kdl(kdl_key) {
            FieldSource::Loaded // Explicitly set to default value in file
        } else {
            FieldSource::Default
        }
    };

    vec![
        // Group 1: General
        ConfigGroup {
            name: "General",
            collapsed: false,
            fields: vec![
                field("Lossy shit formats to FLAC", "Capture lossy formats to FLAC instead of transcoding to Opus",
                    ConfigValue::Bool(ops.lossy_shit_formats_to_flac),
                    ConfigValue::Bool(defaults.lossy_shit_formats_to_flac),
                    source_for(ops.lossy_shit_formats_to_flac == defaults.lossy_shit_formats_to_flac, "lossy-shit-formats-to-flac"),
                    false),
            ],
        },
        // Group 2: Startup
        ConfigGroup {
            name: "Startup",
            collapsed: false,
            fields: vec![
                field("Force check all files at startup", "Bypass mtime optimization, verify all indexed files",
                    ConfigValue::Bool(ops.startup.force_check_all_files_at_startup),
                    ConfigValue::Bool(defaults.startup.force_check_all_files_at_startup),
                    source_for(ops.startup.force_check_all_files_at_startup == defaults.startup.force_check_all_files_at_startup, "force-check-all-files-at-startup"),
                    false),
                field("Vacuum threshold", "Free-page ratio threshold for DB compaction prompt (0.0 disables)",
                    ConfigValue::Float(ops.startup.vacuum_threshold),
                    ConfigValue::Float(defaults.startup.vacuum_threshold),
                    source_for((ops.startup.vacuum_threshold - defaults.startup.vacuum_threshold).abs() < f64::EPSILON, "vacuum-threshold"),
                    false),
                field("Default view", "View to open after startup progress completes",
                    ConfigValue::Enum { selected: startup_view_index(ops.startup.default_view), options: STARTUP_VIEW_OPTIONS.to_vec() },
                    ConfigValue::Enum { selected: startup_view_index(defaults.startup.default_view), options: STARTUP_VIEW_OPTIONS.to_vec() },
                    source_for(ops.startup.default_view == defaults.startup.default_view, "default-view"),
                    false),
            ],
        },
        // Group 3: Fingerprint Matching
        ConfigGroup {
            name: "Fingerprint Matching",
            collapsed: false,
            fields: vec![
                field("Duration tolerance percent", "Duration difference above this % = different track",
                    ConfigValue::Float(ops.fingerprint_matching.duration_tolerance_percent),
                    ConfigValue::Float(defaults.fingerprint_matching.duration_tolerance_percent),
                    source_for((ops.fingerprint_matching.duration_tolerance_percent - defaults.fingerprint_matching.duration_tolerance_percent).abs() < f64::EPSILON, "duration-tolerance-percent"),
                    false),
                field("Require matching track number", "Same dir + different track# = not duplicate",
                    ConfigValue::Bool(ops.fingerprint_matching.require_matching_track_number),
                    ConfigValue::Bool(defaults.fingerprint_matching.require_matching_track_number),
                    source_for(ops.fingerprint_matching.require_matching_track_number == defaults.fingerprint_matching.require_matching_track_number, "require-matching-track-number"),
                    false),
                field("Require matching album", "Different albums can still be duplicates",
                    ConfigValue::Bool(ops.fingerprint_matching.require_matching_album),
                    ConfigValue::Bool(defaults.fingerprint_matching.require_matching_album),
                    source_for(ops.fingerprint_matching.require_matching_album == defaults.fingerprint_matching.require_matching_album, "require-matching-album"),
                    false),
            ],
        },
        // Group 4: Quality Resolution
        ConfigGroup {
            name: "Quality Resolution",
            collapsed: false,
            fields: vec![
                field("Auto resolve format tier", "FLAC beats MP3 automatically",
                    ConfigValue::Bool(ops.quality_resolution.auto_resolve_format_tier),
                    ConfigValue::Bool(defaults.quality_resolution.auto_resolve_format_tier),
                    source_for(ops.quality_resolution.auto_resolve_format_tier == defaults.quality_resolution.auto_resolve_format_tier, "auto-resolve-format-tier"),
                    false),
                field("Bitrate threshold percent", "Bitrate diff above this % = clear winner",
                    ConfigValue::Float(ops.quality_resolution.bitrate_threshold_percent),
                    ConfigValue::Float(defaults.quality_resolution.bitrate_threshold_percent),
                    source_for((ops.quality_resolution.bitrate_threshold_percent - defaults.quality_resolution.bitrate_threshold_percent).abs() < f64::EPSILON, "bitrate-threshold-percent"),
                    false),
                field("Inbox bitrate fuzz percent", "Inbox-to-corpus bitrate tolerance for equivalence",
                    ConfigValue::Float(ops.quality_resolution.inbox_bitrate_fuzz_percent),
                    ConfigValue::Float(defaults.quality_resolution.inbox_bitrate_fuzz_percent),
                    source_for((ops.quality_resolution.inbox_bitrate_fuzz_percent - defaults.quality_resolution.inbox_bitrate_fuzz_percent).abs() < f64::EPSILON, "inbox-bitrate-fuzz-percent"),
                    false),
            ],
        },
        // Group 5: Canonicalization
        ConfigGroup {
            name: "Canonicalization",
            collapsed: false,
            fields: vec![
                field("Strip album format suffixes", "Normalize EP/LP suffixes during album collision detection",
                    ConfigValue::Bool(ops.canonicalization.strip_album_format_suffixes),
                    ConfigValue::Bool(defaults.canonicalization.strip_album_format_suffixes),
                    source_for(ops.canonicalization.strip_album_format_suffixes == defaults.canonicalization.strip_album_format_suffixes, "strip-album-format-suffixes"),
                    false),
            ],
        },
        // Group 6: Health Detection
        ConfigGroup {
            name: "Health Detection",
            collapsed: false,
            fields: vec![
                field("Required tags", "Tags that must be present on every track",
                    ConfigValue::StringList(ops.health_detection.required_tags.clone()),
                    ConfigValue::StringList(defaults.health_detection.required_tags.clone()),
                    source_for(ops.health_detection.required_tags == defaults.health_detection.required_tags, "required-tags"),
                    false),
                field("Album artist only if compilation", "Only require album_artist on multi-artist albums",
                    ConfigValue::Bool(ops.health_detection.album_artist_only_required_if_compilation),
                    ConfigValue::Bool(defaults.health_detection.album_artist_only_required_if_compilation),
                    source_for(ops.health_detection.album_artist_only_required_if_compilation == defaults.health_detection.album_artist_only_required_if_compilation, "album-artist-only-required-if-compilation"),
                    false),
                field("Single album suffix", "Suffix appended when tagging as single",
                    ConfigValue::String(ops.health_detection.single_album_suffix.clone()),
                    ConfigValue::String(defaults.health_detection.single_album_suffix.clone()),
                    source_for(ops.health_detection.single_album_suffix == defaults.health_detection.single_album_suffix, "single-album-suffix"),
                    false),
            ],
        },
        // Group 7: Duplicate Analysis
        ConfigGroup {
            name: "Duplicate Analysis",
            collapsed: false,
            fields: vec![
                field("Fingerprint similarity threshold", "Pairs below this similarity (0-100) are not duplicates",
                    ConfigValue::Float(ops.duplicate_analysis.fingerprint_similarity_threshold),
                    ConfigValue::Float(defaults.duplicate_analysis.fingerprint_similarity_threshold),
                    source_for((ops.duplicate_analysis.fingerprint_similarity_threshold - defaults.duplicate_analysis.fingerprint_similarity_threshold).abs() < f64::EPSILON, "fingerprint-similarity-threshold"),
                    false),
                field("Duration tolerance ms", "Tracks with duration diff above this are clustered separately",
                    ConfigValue::SignedInt(ops.duplicate_analysis.duration_tolerance_ms),
                    ConfigValue::SignedInt(defaults.duplicate_analysis.duration_tolerance_ms),
                    source_for(ops.duplicate_analysis.duration_tolerance_ms == defaults.duplicate_analysis.duration_tolerance_ms, "duration-tolerance-ms"),
                    false),
                field("Cross directory max keys", "Max diverging dir keys for cross-directory overlap signal",
                    ConfigValue::Uint(ops.duplicate_analysis.cross_directory_max_keys),
                    ConfigValue::Uint(defaults.duplicate_analysis.cross_directory_max_keys),
                    source_for(ops.duplicate_analysis.cross_directory_max_keys == defaults.duplicate_analysis.cross_directory_max_keys, "cross-directory-max-keys"),
                    false),
                field("Within directory min keys", "Min diverging dir keys to skip (likely variants)",
                    ConfigValue::Uint(ops.duplicate_analysis.within_directory_min_keys),
                    ConfigValue::Uint(defaults.duplicate_analysis.within_directory_min_keys),
                    source_for(ops.duplicate_analysis.within_directory_min_keys == defaults.duplicate_analysis.within_directory_min_keys, "within-directory-min-keys"),
                    false),
                field("Elide variant titles", "Skip dupe pairs where titles differ and contain remix/live/etc.",
                    ConfigValue::Bool(ops.duplicate_analysis.elide_variant_titles),
                    ConfigValue::Bool(defaults.duplicate_analysis.elide_variant_titles),
                    source_for(ops.duplicate_analysis.elide_variant_titles == defaults.duplicate_analysis.elide_variant_titles, "elide-variant-titles"),
                    false),
            ],
        },
        // Group 8: Inbox Organize
        ConfigGroup {
            name: "Inbox Organize",
            collapsed: false,
            fields: vec![
                field("Directory granularity", "How to group inbox directories for organize workflow",
                    ConfigValue::Enum { selected: granularity_index(ops.inbox_organize.directory_granularity), options: GRANULARITY_OPTIONS.to_vec() },
                    ConfigValue::Enum { selected: granularity_index(defaults.inbox_organize.directory_granularity), options: GRANULARITY_OPTIONS.to_vec() },
                    source_for(ops.inbox_organize.directory_granularity == defaults.inbox_organize.directory_granularity, "directory-granularity"),
                    false),
            ],
        },
        // Group 9: Performance
        ConfigGroup {
            name: "Performance",
            collapsed: false,
            fields: vec![
                field("Worker threads", "Number of worker threads (auto = 2x logical cores)",
                    ConfigValue::OptionalUint(ops.performance.worker_threads),
                    ConfigValue::OptionalUint(defaults.performance.worker_threads),
                    source_for(ops.performance.worker_threads == defaults.performance.worker_threads, "worker-threads"),
                    true),
                field("DB cache MB", "SQLite page cache size per connection in MB",
                    ConfigValue::UintU32(ops.performance.db_cache_mb),
                    ConfigValue::UintU32(defaults.performance.db_cache_mb),
                    source_for(ops.performance.db_cache_mb == defaults.performance.db_cache_mb, "db-cache-mb"),
                    true),
                field("Timing instrumentation", "Enable stats display and atomic counter updates",
                    ConfigValue::Bool(ops.performance.timing_instrumentation),
                    ConfigValue::Bool(defaults.performance.timing_instrumentation),
                    source_for(ops.performance.timing_instrumentation == defaults.performance.timing_instrumentation, "timing-instrumentation"),
                    true),
            ],
        },
        // Group 10: Tag Splitting
        ConfigGroup {
            name: "Tag Splitting",
            collapsed: false,
            fields: vec![
                field("Collaboration keywords", "Keywords like feat, ft, vs for artist collabs",
                    ConfigValue::StringSet(ops.tag_splitting.collaboration_keywords.iter().cloned().collect()),
                    ConfigValue::StringSet(defaults.tag_splitting.collaboration_keywords.iter().cloned().collect()),
                    source_for(ops.tag_splitting.collaboration_keywords == defaults.tag_splitting.collaboration_keywords, "collab"),
                    false),
                field("Canonicalization synonyms", "Substitutions during matching (e.g., and -> &)",
                    ConfigValue::StringPairMap(ops.tag_splitting.canonicalization_synonyms.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    ConfigValue::StringPairMap(defaults.tag_splitting.canonicalization_synonyms.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    source_for(ops.tag_splitting.canonicalization_synonyms == defaults.tag_splitting.canonicalization_synonyms, "synonyms"),
                    false),
                field("Tag separators", "Per-tag separator strings",
                    ConfigValue::StringListMap(ops.tag_splitting.tag_separators.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    ConfigValue::StringListMap(defaults.tag_splitting.tag_separators.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    source_for(ops.tag_splitting.tag_separators == defaults.tag_splitting.tag_separators, "tag-separators"),
                    false),
            ],
        },
    ]
}

/// Apply edited groups back to a Config, producing a new Config.
///
/// Only fields with `source == Edited` are applied; others keep the
/// base config's values.
pub fn apply_groups_to_config(base: &Config, groups: &[ConfigGroup]) -> Config {
    let mut config = base.clone();

    for group in groups {
        for field in &group.fields {
            if field.source != FieldSource::Edited {
                continue;
            }
            apply_field(&mut config, group.name, field);
        }
    }

    config
}

/// Apply a single edited field to the config.
fn apply_field(config: &mut Config, group_name: &str, field: &ConfigField) {
    match (group_name, field.label) {
        ("General", "Lossy shit formats to FLAC") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.lossy_shit_formats_to_flac = *v; }
        }
        ("Startup", "Force check all files at startup") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.startup.force_check_all_files_at_startup = *v; }
        }
        ("Startup", "Vacuum threshold") => {
            if let ConfigValue::Float(v) = &field.value { config.opinions.startup.vacuum_threshold = *v; }
        }
        ("Startup", "Default view") => {
            if let ConfigValue::Enum { selected, .. } = &field.value {
                config.opinions.startup.default_view = startup_view_from_index(*selected);
            }
        }
        ("Fingerprint Matching", "Duration tolerance percent") => {
            if let ConfigValue::Float(v) = &field.value { config.opinions.fingerprint_matching.duration_tolerance_percent = *v; }
        }
        ("Fingerprint Matching", "Require matching track number") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.fingerprint_matching.require_matching_track_number = *v; }
        }
        ("Fingerprint Matching", "Require matching album") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.fingerprint_matching.require_matching_album = *v; }
        }
        ("Quality Resolution", "Auto resolve format tier") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.quality_resolution.auto_resolve_format_tier = *v; }
        }
        ("Quality Resolution", "Bitrate threshold percent") => {
            if let ConfigValue::Float(v) = &field.value { config.opinions.quality_resolution.bitrate_threshold_percent = *v; }
        }
        ("Quality Resolution", "Inbox bitrate fuzz percent") => {
            if let ConfigValue::Float(v) = &field.value { config.opinions.quality_resolution.inbox_bitrate_fuzz_percent = *v; }
        }
        ("Canonicalization", "Strip album format suffixes") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.canonicalization.strip_album_format_suffixes = *v; }
        }
        ("Health Detection", "Required tags") => {
            if let ConfigValue::StringList(v) = &field.value { config.opinions.health_detection.required_tags = v.clone(); }
        }
        ("Health Detection", "Album artist only if compilation") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.health_detection.album_artist_only_required_if_compilation = *v; }
        }
        ("Health Detection", "Single album suffix") => {
            if let ConfigValue::String(v) = &field.value { config.opinions.health_detection.single_album_suffix = v.clone(); }
        }
        ("Duplicate Analysis", "Fingerprint similarity threshold") => {
            if let ConfigValue::Float(v) = &field.value { config.opinions.duplicate_analysis.fingerprint_similarity_threshold = *v; }
        }
        ("Duplicate Analysis", "Duration tolerance ms") => {
            if let ConfigValue::SignedInt(v) = &field.value { config.opinions.duplicate_analysis.duration_tolerance_ms = *v; }
        }
        ("Duplicate Analysis", "Cross directory max keys") => {
            if let ConfigValue::Uint(v) = &field.value { config.opinions.duplicate_analysis.cross_directory_max_keys = *v; }
        }
        ("Duplicate Analysis", "Within directory min keys") => {
            if let ConfigValue::Uint(v) = &field.value { config.opinions.duplicate_analysis.within_directory_min_keys = *v; }
        }
        ("Duplicate Analysis", "Elide variant titles") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.duplicate_analysis.elide_variant_titles = *v; }
        }
        ("Inbox Organize", "Directory granularity") => {
            if let ConfigValue::Enum { selected, .. } = &field.value {
                config.opinions.inbox_organize.directory_granularity = granularity_from_index(*selected);
            }
        }
        ("Performance", "Worker threads") => {
            if let ConfigValue::OptionalUint(v) = &field.value { config.opinions.performance.worker_threads = *v; }
        }
        ("Performance", "DB cache MB") => {
            if let ConfigValue::UintU32(v) = &field.value { config.opinions.performance.db_cache_mb = *v; }
        }
        ("Performance", "Timing instrumentation") => {
            if let ConfigValue::Bool(v) = &field.value { config.opinions.performance.timing_instrumentation = *v; }
        }
        ("Tag Splitting", "Collaboration keywords") => {
            if let ConfigValue::StringSet(v) = &field.value {
                config.opinions.tag_splitting.collaboration_keywords = v.iter().cloned().collect();
            }
        }
        ("Tag Splitting", "Canonicalization synonyms") => {
            if let ConfigValue::StringPairMap(v) = &field.value {
                config.opinions.tag_splitting.canonicalization_synonyms = v.iter().cloned().collect();
            }
        }
        ("Tag Splitting", "Tag separators") => {
            if let ConfigValue::StringListMap(v) = &field.value {
                config.opinions.tag_splitting.tag_separators = v.iter().cloned().collect();
            }
        }
        _ => {} // Unknown fields are ignored
    }
}

// =============================================================================
// Enum mapping helpers
// =============================================================================

const STARTUP_VIEW_OPTIONS: &[&str] = &["Insights", "Search", "Browser", "Inbox"];

fn startup_view_index(v: StartupView) -> usize {
    match v {
        StartupView::Insights => 0,
        StartupView::Search => 1,
        StartupView::Browser => 2,
        StartupView::Inbox => 3,
    }
}

fn startup_view_from_index(i: usize) -> StartupView {
    match i {
        0 => StartupView::Insights,
        1 => StartupView::Search,
        2 => StartupView::Browser,
        3 => StartupView::Inbox,
        _ => StartupView::Insights,
    }
}

const GRANULARITY_OPTIONS: &[&str] = &["Leaf", "TopLevel"];

fn granularity_index(g: InboxOrganizeGranularity) -> usize {
    match g {
        InboxOrganizeGranularity::Leaf => 0,
        InboxOrganizeGranularity::TopLevel => 1,
    }
}

fn granularity_from_index(i: usize) -> InboxOrganizeGranularity {
    match i {
        0 => InboxOrganizeGranularity::Leaf,
        1 => InboxOrganizeGranularity::TopLevel,
        _ => InboxOrganizeGranularity::Leaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_apply_roundtrip() {
        let config = Config {
            root: "/tmp/test".into(),
            legacy_enabled: false,
            source_dirs: vec![],
            opinions: crate::config::Opinions::default(),
        };

        let groups = build_groups_from_config(&config, None);
        let rebuilt = apply_groups_to_config(&config, &groups);

        // Verify key fields survive the round-trip
        assert_eq!(rebuilt.opinions.startup.default_view, config.opinions.startup.default_view);
        assert_eq!(rebuilt.opinions.quality_resolution.auto_resolve_format_tier, config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(rebuilt.opinions.duplicate_analysis.fingerprint_similarity_threshold, config.opinions.duplicate_analysis.fingerprint_similarity_threshold);
    }

    #[test]
    fn test_source_detection_default() {
        let config = Config {
            root: "/tmp/test".into(),
            legacy_enabled: false,
            source_dirs: vec![],
            opinions: crate::config::Opinions::default(),
        };

        let groups = build_groups_from_config(&config, None);

        // With no KDL content and default values, all sources should be Default
        for group in &groups {
            for field in &group.fields {
                assert_eq!(field.source, FieldSource::Default,
                    "Field '{}' in group '{}' should be Default", field.label, group.name);
            }
        }
    }

    #[test]
    fn test_source_detection_loaded() {
        let config = Config {
            root: "/tmp/test".into(),
            legacy_enabled: false,
            source_dirs: vec![],
            opinions: crate::config::Opinions::default(),
        };

        // If the KDL content mentions "vacuum-threshold", that field should be Loaded
        let groups = build_groups_from_config(&config, Some("vacuum-threshold 0.1"));

        let startup_group = &groups[1];
        assert_eq!(startup_group.name, "Startup");
        let vacuum_field = &startup_group.fields[1];
        assert_eq!(vacuum_field.label, "Vacuum threshold");
        assert_eq!(vacuum_field.source, FieldSource::Loaded);
    }
}
