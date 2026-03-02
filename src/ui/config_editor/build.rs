//! Config Editor Build/Apply
//!
//! Converts between `Config` struct and the flat group/field representation
//! used by the editor UI. Each field carries its own applier closure, so
//! build and apply logic are co-located — no separate match block needed.

use crate::config::{Config, StartupView, InboxOrganizeGranularity, SidecarDeployMode};
use super::types::*;

/// Construct a ConfigField with original_value/original_source automatically
/// snapshotted from the initial value/source.
///
/// The `applier` writes the field's value back into a `Config`. Stored at build
/// time so that every field definition is self-contained (build + apply in one place).
fn field(
    label: &'static str,
    description: &'static str,
    value: ConfigValue,
    source: FieldSource,
    restart_required: bool,
    applier: fn(&ConfigValue, &mut Config),
) -> ConfigField {
    ConfigField {
        label,
        description,
        original_value: value.clone(),
        original_source: source,
        value,
        source,
        restart_required,
        applier,
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
                    source_for(ops.lossy_shit_formats_to_flac == defaults.lossy_shit_formats_to_flac, "lossy-shit-formats-to-flac"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.lossy_shit_formats_to_flac = *b; } }),
            ],
        },
        // Group 2: Startup
        ConfigGroup {
            name: "Startup",
            collapsed: false,
            fields: vec![
                field("Force check all files at startup", "Bypass mtime optimization, verify all indexed files",
                    ConfigValue::Bool(ops.startup.force_check_all_files_at_startup),
                    source_for(ops.startup.force_check_all_files_at_startup == defaults.startup.force_check_all_files_at_startup, "force-check-all-files-at-startup"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.startup.force_check_all_files_at_startup = *b; } }),
                field("Vacuum threshold", "Free-page ratio threshold for DB compaction prompt (0.0 disables)",
                    ConfigValue::Float(ops.startup.vacuum_threshold),
                    source_for((ops.startup.vacuum_threshold - defaults.startup.vacuum_threshold).abs() < f64::EPSILON, "vacuum-threshold"),
                    false,
                    |v, c| { if let ConfigValue::Float(f) = v { c.opinions.startup.vacuum_threshold = *f; } }),
                field("Default view", "View to open after startup progress completes",
                    ConfigValue::Enum { selected: startup_view_index(ops.startup.default_view), options: STARTUP_VIEW_OPTIONS.to_vec() },
                    source_for(ops.startup.default_view == defaults.startup.default_view, "default-view"),
                    false,
                    |v, c| { if let ConfigValue::Enum { selected, .. } = v { c.opinions.startup.default_view = startup_view_from_index(*selected); } }),
            ],
        },
        // Group 3: Quality Resolution
        ConfigGroup {
            name: "Quality Resolution",
            collapsed: false,
            fields: vec![
                field("Inbox bitrate fuzz percent", "Inbox-to-corpus bitrate tolerance for equivalence",
                    ConfigValue::Float(ops.quality_resolution.inbox_bitrate_fuzz_percent),
                    source_for((ops.quality_resolution.inbox_bitrate_fuzz_percent - defaults.quality_resolution.inbox_bitrate_fuzz_percent).abs() < f64::EPSILON, "inbox-bitrate-fuzz-percent"),
                    false,
                    |v, c| { if let ConfigValue::Float(f) = v { c.opinions.quality_resolution.inbox_bitrate_fuzz_percent = *f; } }),
            ],
        },
        // Group 5: Canonicalization
        ConfigGroup {
            name: "Canonicalization",
            collapsed: false,
            fields: vec![
                field("Strip album format suffixes", "Normalize EP/LP suffixes during album collision detection",
                    ConfigValue::Bool(ops.canonicalization.strip_album_format_suffixes),
                    source_for(ops.canonicalization.strip_album_format_suffixes == defaults.canonicalization.strip_album_format_suffixes, "strip-album-format-suffixes"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.canonicalization.strip_album_format_suffixes = *b; } }),
            ],
        },
        // Group 6: Health Detection
        ConfigGroup {
            name: "Health Detection",
            collapsed: false,
            fields: vec![
                field("Required tags", "Tags that must be present on every track",
                    ConfigValue::StringList(ops.health_detection.required_tags.clone()),
                    source_for(ops.health_detection.required_tags == defaults.health_detection.required_tags, "required-tags"),
                    false,
                    |v, c| { if let ConfigValue::StringList(list) = v { c.opinions.health_detection.required_tags = list.clone(); } }),
                field("Album artist only if compilation", "Only require album_artist on multi-artist albums",
                    ConfigValue::Bool(ops.health_detection.album_artist_only_required_if_compilation),
                    source_for(ops.health_detection.album_artist_only_required_if_compilation == defaults.health_detection.album_artist_only_required_if_compilation, "album-artist-only-required-if-compilation"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.health_detection.album_artist_only_required_if_compilation = *b; } }),
                field("Single album suffix", "Suffix appended when tagging as single",
                    ConfigValue::String(ops.health_detection.single_album_suffix.clone()),
                    source_for(ops.health_detection.single_album_suffix == defaults.health_detection.single_album_suffix, "single-album-suffix"),
                    false,
                    |v, c| { if let ConfigValue::String(s) = v { c.opinions.health_detection.single_album_suffix = s.clone(); } }),
            ],
        },
        // Group 7: Duplicate Analysis
        ConfigGroup {
            name: "Duplicate Analysis",
            collapsed: false,
            fields: vec![
                field("Fingerprint similarity threshold", "Pairs below this similarity (0-100) are not duplicates",
                    ConfigValue::Float(ops.duplicate_analysis.fingerprint_similarity_threshold),
                    source_for((ops.duplicate_analysis.fingerprint_similarity_threshold - defaults.duplicate_analysis.fingerprint_similarity_threshold).abs() < f64::EPSILON, "fingerprint-similarity-threshold"),
                    false,
                    |v, c| { if let ConfigValue::Float(f) = v { c.opinions.duplicate_analysis.fingerprint_similarity_threshold = *f; } }),
                field("Duration tolerance ms", "Tracks with duration diff above this are clustered separately",
                    ConfigValue::SignedInt(ops.duplicate_analysis.duration_tolerance_ms),
                    source_for(ops.duplicate_analysis.duration_tolerance_ms == defaults.duplicate_analysis.duration_tolerance_ms, "duration-tolerance-ms"),
                    false,
                    |v, c| { if let ConfigValue::SignedInt(n) = v { c.opinions.duplicate_analysis.duration_tolerance_ms = *n; } }),
                field("Elide variant titles", "Skip dupe pairs where titles differ and contain remix/live/etc.",
                    ConfigValue::Bool(ops.duplicate_analysis.elide_variant_titles),
                    source_for(ops.duplicate_analysis.elide_variant_titles == defaults.duplicate_analysis.elide_variant_titles, "elide-variant-titles"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.duplicate_analysis.elide_variant_titles = *b; } }),
            ],
        },
        // Group 8: Inbox Organize
        ConfigGroup {
            name: "Inbox Organize",
            collapsed: false,
            fields: vec![
                field("Directory granularity", "How to group inbox directories for organize workflow",
                    ConfigValue::Enum { selected: granularity_index(ops.inbox_organize.directory_granularity), options: GRANULARITY_OPTIONS.to_vec() },
                    source_for(ops.inbox_organize.directory_granularity == defaults.inbox_organize.directory_granularity, "directory-granularity"),
                    false,
                    |v, c| { if let ConfigValue::Enum { selected, .. } = v { c.opinions.inbox_organize.directory_granularity = granularity_from_index(*selected); } }),
            ],
        },
        // Group 9: Performance
        ConfigGroup {
            name: "Performance",
            collapsed: false,
            fields: vec![
                field("Worker threads", "Number of worker threads (auto = 2x logical cores)",
                    ConfigValue::OptionalUint(ops.performance.worker_threads),
                    source_for(ops.performance.worker_threads == defaults.performance.worker_threads, "worker-threads"),
                    true,
                    |v, c| { if let ConfigValue::OptionalUint(n) = v { c.opinions.performance.worker_threads = *n; } }),
                field("DB cache MB", "SQLite page cache size per connection in MB",
                    ConfigValue::UintU32(ops.performance.db_cache_mb),
                    source_for(ops.performance.db_cache_mb == defaults.performance.db_cache_mb, "db-cache-mb"),
                    true,
                    |v, c| { if let ConfigValue::UintU32(n) = v { c.opinions.performance.db_cache_mb = *n; } }),
                field("Timing instrumentation", "Enable stats display and atomic counter updates",
                    ConfigValue::Bool(ops.performance.timing_instrumentation),
                    source_for(ops.performance.timing_instrumentation == defaults.performance.timing_instrumentation, "timing-instrumentation"),
                    true,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.performance.timing_instrumentation = *b; } }),
            ],
        },
        // Group 10: Tag Splitting
        ConfigGroup {
            name: "Tag Splitting",
            collapsed: false,
            fields: vec![
                field("Collaboration keywords", "Keywords like feat, ft, vs for artist collabs",
                    ConfigValue::StringSet(ops.tag_splitting.collaboration_keywords.iter().cloned().collect()),
                    source_for(ops.tag_splitting.collaboration_keywords == defaults.tag_splitting.collaboration_keywords, "collab"),
                    false,
                    |v, c| { if let ConfigValue::StringSet(items) = v { c.opinions.tag_splitting.collaboration_keywords = items.iter().cloned().collect(); } }),
                field("Tag separators", "Per-tag separator strings",
                    ConfigValue::StringListMap(ops.tag_splitting.tag_separators.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    source_for(ops.tag_splitting.tag_separators == defaults.tag_splitting.tag_separators, "tag-separators"),
                    false,
                    |v, c| { if let ConfigValue::StringListMap(items) = v { c.opinions.tag_splitting.tag_separators = items.iter().cloned().collect(); } }),
            ],
        },
        // Group 11: Disc Extraction
        ConfigGroup {
            name: "Disc Extraction",
            collapsed: false,
            fields: vec![
                field("Disc tag name", "Tag name to write extracted disc identifier into",
                    ConfigValue::String(ops.disc_extraction.disc_tag_name.clone()),
                    source_for(ops.disc_extraction.disc_tag_name == defaults.disc_extraction.disc_tag_name, "disc-tag-name"),
                    false,
                    |v, c| { if let ConfigValue::String(s) = v { c.opinions.disc_extraction.disc_tag_name = s.clone(); } }),
                field("Map letters to numbers", "Map letter prefixes to numbers (A\u{2192}1, B\u{2192}2, ...)",
                    ConfigValue::Bool(ops.disc_extraction.map_letters_to_numbers),
                    source_for(ops.disc_extraction.map_letters_to_numbers == defaults.disc_extraction.map_letters_to_numbers, "map-letters-to-numbers"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.disc_extraction.map_letters_to_numbers = *b; } }),
            ],
        },
        // Group 12: Album Art
        ConfigGroup {
            name: "Album Art",
            collapsed: false,
            fields: vec![
                field("Sidecar deploy mode", "Deploy sidecar cover images alongside audio files to libraries",
                    ConfigValue::Enum { selected: sidecar_deploy_index(ops.album_art.sidecar_deploy_mode), options: SIDECAR_DEPLOY_OPTIONS.to_vec() },
                    source_for(ops.album_art.sidecar_deploy_mode == defaults.album_art.sidecar_deploy_mode, "sidecar-deploy-mode"),
                    false,
                    |v, c| { if let ConfigValue::Enum { selected, .. } = v { c.opinions.album_art.sidecar_deploy_mode = sidecar_deploy_from_index(*selected); } }),
            ],
        },
        // Group 13: Advanced
        ConfigGroup {
            name: "Advanced",
            collapsed: false,
            fields: vec![
                field("Idle rescan interval", "Idle time before auto-rescanning corpus/inbox (e.g. 3m, 180s, disabled)",
                    ConfigValue::Duration(ops.idle_rescan_interval_secs),
                    source_for(ops.idle_rescan_interval_secs == defaults.idle_rescan_interval_secs, "idle-rescan-interval"),
                    false,
                    |v, c| { if let ConfigValue::Duration(secs) = v { c.opinions.idle_rescan_interval_secs = *secs; } }),
                field("Leave transactions open", "Keep one open transaction; adds Transaction tab to view ring",
                    ConfigValue::Bool(ops.leave_transactions_open),
                    source_for(ops.leave_transactions_open == defaults.leave_transactions_open, "leave-transactions-open"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.leave_transactions_open = *b; } }),
            ],
        },
        // Group 14: External Matching
        ConfigGroup {
            name: "External Matching",
            collapsed: false,
            fields: vec![
                field("AcoustID API key", "API key for AcoustID fingerprint lookups (empty = disabled)",
                    ConfigValue::String(ops.external_matching.acoustid_api_key.clone()),
                    source_for(ops.external_matching.acoustid_api_key == defaults.external_matching.acoustid_api_key, "acoustid-api-key"),
                    false,
                    |v, c| { if let ConfigValue::String(s) = v { c.opinions.external_matching.acoustid_api_key = s.clone(); } }),
                field("Requests per second", "Rate limit for AcoustID API calls",
                    ConfigValue::UintU32(ops.external_matching.requests_per_second),
                    source_for(ops.external_matching.requests_per_second == defaults.external_matching.requests_per_second, "requests-per-second"),
                    false,
                    |v, c| { if let ConfigValue::UintU32(n) = v { c.opinions.external_matching.requests_per_second = *n; } }),
                field("Show MusicBrainz URL", "Show raw MusicBrainz URL instead of short clickable label in match review",
                    ConfigValue::Bool(ops.external_matching.show_musicbrainz_url),
                    source_for(ops.external_matching.show_musicbrainz_url == defaults.external_matching.show_musicbrainz_url, "show-musicbrainz-url"),
                    false,
                    |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.external_matching.show_musicbrainz_url = *b; } }),
            ],
        },
    ]
}

/// Apply edited groups back to a Config, producing a new Config.
///
/// Only fields with `source == Edited` are applied; others keep the
/// base config's values. Each field's stored applier handles the write.
pub fn apply_groups_to_config(base: &Config, groups: &[ConfigGroup]) -> Config {
    let mut config = base.clone();

    for group in groups {
        for field in &group.fields {
            if field.source != FieldSource::Edited {
                continue;
            }
            (field.applier)(&field.value, &mut config);
        }
    }

    config
}

// =============================================================================
// Enum mapping helpers
// =============================================================================

const STARTUP_VIEW_OPTIONS: &[&str] = &["Health", "Search", "Browser", "Inbox"];

fn startup_view_index(v: StartupView) -> usize {
    match v {
        StartupView::Health => 0,
        StartupView::Search => 1,
        StartupView::Browser => 2,
        StartupView::Inbox => 3,
    }
}

fn startup_view_from_index(i: usize) -> StartupView {
    match i {
        0 => StartupView::Health,
        1 => StartupView::Search,
        2 => StartupView::Browser,
        3 => StartupView::Inbox,
        _ => StartupView::Health,
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

const SIDECAR_DEPLOY_OPTIONS: &[&str] = &["Disabled", "PrimaryCover", "All"];

fn sidecar_deploy_index(m: SidecarDeployMode) -> usize {
    match m {
        SidecarDeployMode::Disabled => 0,
        SidecarDeployMode::PrimaryCover => 1,
        SidecarDeployMode::All => 2,
    }
}

fn sidecar_deploy_from_index(i: usize) -> SidecarDeployMode {
    match i {
        0 => SidecarDeployMode::Disabled,
        1 => SidecarDeployMode::PrimaryCover,
        2 => SidecarDeployMode::All,
        _ => SidecarDeployMode::PrimaryCover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            root: "/tmp/test".into(),
            legacy_enabled: false,
            source_dirs: vec![],
            opinions: crate::config::Opinions::default(),
        }
    }

    #[test]
    fn test_build_apply_roundtrip() {
        let config = test_config();
        let groups = build_groups_from_config(&config, None);
        let rebuilt = apply_groups_to_config(&config, &groups);

        // Verify key fields survive the round-trip
        assert_eq!(rebuilt.opinions.startup.default_view, config.opinions.startup.default_view);
        assert_eq!(rebuilt.opinions.quality_resolution.inbox_bitrate_fuzz_percent, config.opinions.quality_resolution.inbox_bitrate_fuzz_percent);
        assert_eq!(rebuilt.opinions.duplicate_analysis.fingerprint_similarity_threshold, config.opinions.duplicate_analysis.fingerprint_similarity_threshold);
        assert_eq!(rebuilt.opinions.disc_extraction.disc_tag_name, config.opinions.disc_extraction.disc_tag_name);
        assert_eq!(rebuilt.opinions.disc_extraction.map_letters_to_numbers, config.opinions.disc_extraction.map_letters_to_numbers);
        assert_eq!(rebuilt.opinions.album_art.sidecar_deploy_mode, config.opinions.album_art.sidecar_deploy_mode);
    }

    #[test]
    fn test_source_detection_default() {
        let config = test_config();
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
        let config = test_config();

        // If the KDL content mentions "vacuum-threshold", that field should be Loaded
        let groups = build_groups_from_config(&config, Some("vacuum-threshold 0.1"));

        let startup_group = &groups[1];
        assert_eq!(startup_group.name, "Startup");
        let vacuum_field = &startup_group.fields[1];
        assert_eq!(vacuum_field.label, "Vacuum threshold");
        assert_eq!(vacuum_field.source, FieldSource::Loaded);
    }
}
