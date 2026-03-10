//! Config Editor Build/Apply
//!
//! Converts between `Config` struct and the flat group/field representation
//! used by the editor UI. Each field carries its own applier closure, so
//! build and apply logic are co-located — no separate match block needed.

use super::types::*;
use crate::config::{
    AlbumArtOpinions, CanonicalizationOpinions, Config, DiscExtractionOpinions,
    DuplicateAnalysisOpinions, ExternalMatchingConfig, HealthDetectionOpinions,
    InboxOrganizeGranularity, InboxOrganizeOpinions, Opinions, PackingWeights, PerformanceOpinions,
    QualityResolutionOpinions, ReleasePackingOpinions, SidecarDeployMode, StartupOpinions,
    StartupView, TagSplittingOpinions,
};

// =============================================================================
// Declarative macros for config boilerplate reduction
// =============================================================================

/// Map a fieldless enum to index/from_index/OPTIONS for ConfigValue::Enum usage.
macro_rules! config_enum_map {
    ($ty:ty, $default:expr, [$($variant:path => $label:expr),+ $(,)?]) => {
        impl ConfigEnumMap for $ty {
            const OPTIONS: &'static [&'static str] = &[$($label),+];
            fn to_index(self) -> usize {
                let variants: &[Self] = &[$($variant),+];
                variants.iter().position(|v| core::mem::discriminant(v) == core::mem::discriminant(&self)).unwrap_or(0)
            }
            fn from_index(i: usize) -> Self {
                let variants: &[Self] = &[$($variant),+];
                variants.get(i).copied().unwrap_or($default)
            }
        }
    };
}

/// Trait for enums that can be mapped to/from config editor indices.
trait ConfigEnumMap: Sized + Copy {
    const OPTIONS: &'static [&'static str];
    fn to_index(self) -> usize;
    fn from_index(i: usize) -> Self;
}

config_enum_map!(StartupView, StartupView::Health, [
    StartupView::Health => "Health",
    StartupView::Search => "Search",
    StartupView::Browser => "Browser",
    StartupView::Inbox => "Inbox",
    StartupView::ExternalMatches => "Ext Matches",
]);

config_enum_map!(InboxOrganizeGranularity, InboxOrganizeGranularity::Leaf, [
    InboxOrganizeGranularity::Leaf => "Leaf",
    InboxOrganizeGranularity::TopLevel => "TopLevel",
]);

config_enum_map!(SidecarDeployMode, SidecarDeployMode::PrimaryCover, [
    SidecarDeployMode::Disabled => "Disabled",
    SidecarDeployMode::PrimaryCover => "PrimaryCover",
    SidecarDeployMode::All => "All",
]);

/// Generate a ConfigField for a PackingWeights float field.
macro_rules! packing_weight {
    ($source_for:expr, $w:expr, $d:expr, $path:ident . $field:ident, $label:expr, $desc:expr, $kdl:expr) => {
        field(
            $label, $desc,
            ConfigValue::Float($w.$field),
            $source_for(($w.$field - $d.$field).abs() < f64::EPSILON, $kdl),
            false,
            |v, c| { if let ConfigValue::Float(f) = v { c.opinions.release_packing.$path.$field = *f; } },
        )
    };
}

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
    let in_kdl = |path: &str| -> bool { kdl_content.is_some_and(|content| content.contains(path)) };

    // Determine field source: if value differs from default it's Loaded (must be from file),
    // otherwise check if the key exists in KDL content.
    let source_for = |matches_default: bool, kdl_key: &str| -> FieldSource {
        if !matches_default || in_kdl(kdl_key) {
            FieldSource::Loaded
        } else {
            FieldSource::Default
        }
    };

    vec![
        // General
        ConfigGroup {
            name: "General",
            collapsed: false,
            fields: vec![field(
                "Lossy shit formats to FLAC",
                "Capture lossy formats to FLAC instead of transcoding to Opus",
                ConfigValue::Bool(ops.lossy_shit_formats_to_flac),
                source_for(
                    ops.lossy_shit_formats_to_flac == defaults.lossy_shit_formats_to_flac,
                    Opinions::KDL_LOSSY_SHIT,
                ),
                false,
                |v, c| {
                    if let ConfigValue::Bool(b) = v {
                        c.opinions.lossy_shit_formats_to_flac = *b;
                    }
                },
            )],
        },
        // Startup
        ConfigGroup {
            name: "Startup",
            collapsed: false,
            fields: vec![
                field(
                    "Force check all files at startup",
                    "Bypass mtime optimization, verify all indexed files",
                    ConfigValue::Bool(ops.startup.force_check_all_files_at_startup),
                    source_for(
                        ops.startup.force_check_all_files_at_startup
                            == defaults.startup.force_check_all_files_at_startup,
                        StartupOpinions::KDL_FORCE_CHECK,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.startup.force_check_all_files_at_startup = *b;
                        }
                    },
                ),
                field(
                    "Vacuum threshold",
                    "Free-page ratio threshold for DB compaction prompt (0.0 disables)",
                    ConfigValue::Float(ops.startup.vacuum_threshold),
                    source_for(
                        (ops.startup.vacuum_threshold - defaults.startup.vacuum_threshold).abs()
                            < f64::EPSILON,
                        StartupOpinions::KDL_VACUUM_THRESHOLD,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.startup.vacuum_threshold = *f;
                        }
                    },
                ),
                field(
                    "Default view",
                    "View to open after startup progress completes",
                    ConfigValue::Enum {
                        selected: ops.startup.default_view.to_index(),
                        options: StartupView::OPTIONS.to_vec(),
                    },
                    source_for(
                        ops.startup.default_view == defaults.startup.default_view,
                        StartupOpinions::KDL_DEFAULT_VIEW,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Enum { selected, .. } = v {
                            c.opinions.startup.default_view = StartupView::from_index(*selected);
                        }
                    },
                ),
            ],
        },
        // Duplicate Analysis
        ConfigGroup {
            name: "Duplicate Analysis",
            collapsed: false,
            fields: vec![
                field(
                    "Fingerprint similarity threshold",
                    "Pairs below this similarity (0-100) are not duplicates",
                    ConfigValue::Float(ops.duplicate_analysis.fingerprint_similarity_threshold),
                    source_for(
                        (ops.duplicate_analysis.fingerprint_similarity_threshold
                            - defaults.duplicate_analysis.fingerprint_similarity_threshold)
                            .abs()
                            < f64::EPSILON,
                        DuplicateAnalysisOpinions::KDL_FP_THRESHOLD,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions
                                .duplicate_analysis
                                .fingerprint_similarity_threshold = *f;
                        }
                    },
                ),
                field(
                    "Duration tolerance ms",
                    "Tracks with duration diff above this are clustered separately",
                    ConfigValue::SignedInt(ops.duplicate_analysis.duration_tolerance_ms),
                    source_for(
                        ops.duplicate_analysis.duration_tolerance_ms
                            == defaults.duplicate_analysis.duration_tolerance_ms,
                        DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::SignedInt(n) = v {
                            c.opinions.duplicate_analysis.duration_tolerance_ms = *n;
                        }
                    },
                ),
                field(
                    "Elide variant titles",
                    "Skip dupe pairs where titles differ and contain remix/live/etc.",
                    ConfigValue::Bool(ops.duplicate_analysis.elide_variant_titles),
                    source_for(
                        ops.duplicate_analysis.elide_variant_titles
                            == defaults.duplicate_analysis.elide_variant_titles,
                        DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.duplicate_analysis.elide_variant_titles = *b;
                        }
                    },
                ),
            ],
        },
        // Release Packing
        ConfigGroup {
            name: "Release Packing",
            collapsed: false,
            fields: vec![
                field(
                    "Duration tolerance %",
                    "Discard recording matches with duration diff above this fraction (0.0-1.0)",
                    ConfigValue::Float(ops.release_packing.duration_tolerance_pct),
                    source_for(
                        (ops.release_packing.duration_tolerance_pct
                            - defaults.release_packing.duration_tolerance_pct)
                            .abs()
                            < f64::EPSILON,
                        ReleasePackingOpinions::KDL_DURATION_TOLERANCE_PCT,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.release_packing.duration_tolerance_pct = *f;
                        }
                    },
                ),
                field(
                    "Min AcoustID confidence",
                    "Discard recording matches below this confidence (0.0-1.0)",
                    ConfigValue::Float(ops.release_packing.min_confidence),
                    source_for(
                        (ops.release_packing.min_confidence
                            - defaults.release_packing.min_confidence)
                            .abs()
                            < f64::EPSILON,
                        ReleasePackingOpinions::KDL_MIN_CONFIDENCE,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.release_packing.min_confidence = *f;
                        }
                    },
                ),
                field(
                    "Title pre-assign threshold",
                    "Title similarity threshold for elimination pre-assignment (0.0-1.0)",
                    ConfigValue::Float(ops.release_packing.title_preassign_threshold),
                    source_for(
                        (ops.release_packing.title_preassign_threshold
                            - defaults.release_packing.title_preassign_threshold)
                            .abs()
                            < f64::EPSILON,
                        ReleasePackingOpinions::KDL_TITLE_PREASSIGN_THRESHOLD,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.release_packing.title_preassign_threshold = *f;
                        }
                    },
                ),
                field(
                    "Packing knot ratio",
                    "Proposals/inodes ratio threshold for knot extraction (0 to disable)",
                    ConfigValue::Float(ops.release_packing.packing_knot_ratio),
                    source_for(
                        ops.release_packing.packing_knot_ratio
                            == defaults.release_packing.packing_knot_ratio,
                        ReleasePackingOpinions::KDL_PACKING_KNOT_RATIO,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            if *f == 0.0 || *f > 1.0 {
                                c.opinions.release_packing.packing_knot_ratio = *f;
                            }
                        }
                    },
                ),
                field(
                    "Packing knot size limit",
                    "Max component size before knot extraction (0 to disable)",
                    ConfigValue::UintU32(ops.release_packing.packing_knot_size_limit as u32),
                    source_for(
                        ops.release_packing.packing_knot_size_limit
                            == defaults.release_packing.packing_knot_size_limit,
                        ReleasePackingOpinions::KDL_PACKING_KNOT_SIZE_LIMIT,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::UintU32(n) = v {
                            c.opinions.release_packing.packing_knot_size_limit = *n as usize;
                        }
                    },
                ),
                field(
                    "Singles before incompletes",
                    "Run single-track MIS round before incompletes",
                    ConfigValue::Bool(ops.release_packing.singles_before_incompletes),
                    source_for(
                        ops.release_packing.singles_before_incompletes
                            == defaults.release_packing.singles_before_incompletes,
                        ReleasePackingOpinions::KDL_SINGLES_BEFORE_INCOMPLETES,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.release_packing.singles_before_incompletes = *b;
                        }
                    },
                ),
                field(
                    "Resolve knots with discographies",
                    "Reduce knots to covering proposals (discography releases) when possible",
                    ConfigValue::Bool(ops.release_packing.allow_resolve_knots_with_discographies),
                    source_for(
                        ops.release_packing.allow_resolve_knots_with_discographies
                            == defaults.release_packing.allow_resolve_knots_with_discographies,
                        ReleasePackingOpinions::KDL_ALLOW_DISCOGRAPHY_REDUCTION,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.release_packing.allow_resolve_knots_with_discographies = *b;
                        }
                    },
                ),
                field(
                    "Low confidence max AcoustID ratio",
                    "Max AcoustID-matched fraction to trigger low-confidence downgrade (0.0-1.0)",
                    ConfigValue::Float(ops.release_packing.low_confidence_max_acoustid_ratio),
                    source_for(
                        (ops.release_packing.low_confidence_max_acoustid_ratio
                            - defaults.release_packing.low_confidence_max_acoustid_ratio)
                            .abs()
                            < f64::EPSILON,
                        ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ACOUSTID_RATIO,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.release_packing.low_confidence_max_acoustid_ratio = *f;
                        }
                    },
                ),
                field(
                    "Low confidence max album match",
                    "Max avg album_match score to trigger low-confidence downgrade (0.0-1.0)",
                    ConfigValue::Float(ops.release_packing.low_confidence_max_album_match),
                    source_for(
                        (ops.release_packing.low_confidence_max_album_match
                            - defaults.release_packing.low_confidence_max_album_match)
                            .abs()
                            < f64::EPSILON,
                        ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ALBUM_MATCH,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Float(f) = v {
                            c.opinions.release_packing.low_confidence_max_album_match = *f;
                        }
                    },
                ),
            ],
        },
        // Candidate Weights (AcoustID-backed scoring)
        ConfigGroup {
            name: "Packing: Candidate Weights",
            collapsed: true,
            fields: {
                let w = &ops.release_packing.candidate_weights;
                let d = PackingWeights::candidate_defaults();
                vec![
                    packing_weight!(source_for, w, d, candidate_weights.acoustid_confidence, "AcoustID confidence", "Weight for fingerprint confidence (0.0-1.0)", PackingWeights::KDL_ACOUSTID_CONFIDENCE),
                    packing_weight!(source_for, w, d, candidate_weights.duration_match, "Duration match", "Weight for duration match quality (0.0-1.0)", PackingWeights::KDL_DURATION_MATCH),
                    packing_weight!(source_for, w, d, candidate_weights.title_match, "Title match", "Weight for title similarity (0.0-1.0)", PackingWeights::KDL_TITLE_MATCH),
                    packing_weight!(source_for, w, d, candidate_weights.artist_match, "Artist match", "Weight for artist similarity (0.0-1.0)", PackingWeights::KDL_ARTIST_MATCH),
                    packing_weight!(source_for, w, d, candidate_weights.album_match, "Album match", "Weight for album similarity (0.0-1.0)", PackingWeights::KDL_ALBUM_MATCH),
                    packing_weight!(source_for, w, d, candidate_weights.track_number_match, "Track number match", "Weight for tracknumber matching slot position (0.0-1.0)", PackingWeights::KDL_TRACK_NUMBER_MATCH),
                ]
            },
        },
        // Elimination Weights (tag-only gap-filling scoring)
        ConfigGroup {
            name: "Packing: Elimination Weights",
            collapsed: true,
            fields: {
                let w = &ops.release_packing.elimination_weights;
                let d = PackingWeights::elimination_defaults();
                vec![
                    packing_weight!(source_for, w, d, elimination_weights.acoustid_confidence, "AcoustID confidence", "Weight for fingerprint confidence (0.0-1.0)", PackingWeights::KDL_ACOUSTID_CONFIDENCE),
                    packing_weight!(source_for, w, d, elimination_weights.duration_match, "Duration match", "Weight for duration match quality (0.0-1.0)", PackingWeights::KDL_DURATION_MATCH),
                    packing_weight!(source_for, w, d, elimination_weights.title_match, "Title match", "Weight for title similarity (0.0-1.0)", PackingWeights::KDL_TITLE_MATCH),
                    packing_weight!(source_for, w, d, elimination_weights.artist_match, "Artist match", "Weight for artist similarity (0.0-1.0)", PackingWeights::KDL_ARTIST_MATCH),
                    packing_weight!(source_for, w, d, elimination_weights.album_match, "Album match", "Weight for album similarity (0.0-1.0)", PackingWeights::KDL_ALBUM_MATCH),
                    packing_weight!(source_for, w, d, elimination_weights.track_number_match, "Track number match", "Weight for tracknumber matching slot position (0.0-1.0)", PackingWeights::KDL_TRACK_NUMBER_MATCH),
                ]
            },
        },
        // Tag Splitting
        ConfigGroup {
            name: "Tag Splitting",
            collapsed: false,
            fields: vec![
                field(
                    "Collaboration keywords",
                    "Keywords like feat, ft, vs for artist collabs",
                    ConfigValue::StringSet(
                        ops.tag_splitting
                            .collaboration_keywords
                            .iter()
                            .cloned()
                            .collect(),
                    ),
                    source_for(
                        ops.tag_splitting.collaboration_keywords
                            == defaults.tag_splitting.collaboration_keywords,
                        TagSplittingOpinions::KDL_COLLAB,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::StringSet(items) = v {
                            c.opinions.tag_splitting.collaboration_keywords =
                                items.iter().cloned().collect();
                        }
                    },
                ),
                field(
                    "Tag separators",
                    "Per-tag separator strings",
                    ConfigValue::StringListMap(
                        ops.tag_splitting
                            .tag_separators
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    source_for(
                        ops.tag_splitting.tag_separators == defaults.tag_splitting.tag_separators,
                        Opinions::KDL_BLOCK_TAG_SPLITTING,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::StringListMap(items) = v {
                            c.opinions.tag_splitting.tag_separators =
                                items.iter().cloned().collect();
                        }
                    },
                ),
            ],
        },
        // External Matching
        ConfigGroup {
            name: "External Matching",
            collapsed: false,
            fields: vec![
                field(
                    "AcoustID API key",
                    "API key for AcoustID fingerprint lookups (empty = disabled)",
                    ConfigValue::String(ops.external_matching.acoustid_api_key.clone()),
                    source_for(
                        ops.external_matching.acoustid_api_key
                            == defaults.external_matching.acoustid_api_key,
                        ExternalMatchingConfig::KDL_ACOUSTID_KEY,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::String(s) = v {
                            c.opinions.external_matching.acoustid_api_key = s.clone();
                        }
                    },
                ),
                field(
                    "Requests per second",
                    "Rate limit for AcoustID API calls",
                    ConfigValue::UintU32(ops.external_matching.requests_per_second),
                    source_for(
                        ops.external_matching.requests_per_second
                            == defaults.external_matching.requests_per_second,
                        ExternalMatchingConfig::KDL_REQ_PER_SEC,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::UintU32(n) = v {
                            c.opinions.external_matching.requests_per_second = *n;
                        }
                    },
                ),
                field(
                    "Auto-enrich on match",
                    "Auto-trigger MB enrichment when AcoustID matches arrive",
                    ConfigValue::Bool(ops.external_matching.auto_enrich_on_match),
                    source_for(
                        ops.external_matching.auto_enrich_on_match
                            == defaults.external_matching.auto_enrich_on_match,
                        ExternalMatchingConfig::KDL_AUTO_ENRICH,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.external_matching.auto_enrich_on_match = *b;
                        }
                    },
                ),
                field(
                    "MB cache TTL (days)",
                    "Days before re-fetching MusicBrainz cache entries",
                    ConfigValue::UintU32(ops.external_matching.mb_cache_ttl_days),
                    source_for(
                        ops.external_matching.mb_cache_ttl_days
                            == defaults.external_matching.mb_cache_ttl_days,
                        ExternalMatchingConfig::KDL_MB_CACHE_TTL,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::UintU32(n) = v {
                            c.opinions.external_matching.mb_cache_ttl_days = *n;
                        }
                    },
                ),
                field(
                    "MB requests per second",
                    "Rate limit ceiling for MusicBrainz API (adaptive backoff)",
                    ConfigValue::UintU32(ops.external_matching.mb_requests_per_second),
                    source_for(
                        ops.external_matching.mb_requests_per_second
                            == defaults.external_matching.mb_requests_per_second,
                        ExternalMatchingConfig::KDL_MB_REQ_PER_SEC,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::UintU32(n) = v {
                            c.opinions.external_matching.mb_requests_per_second = *n;
                        }
                    },
                ),
                field(
                    "MB base URL",
                    "MusicBrainz API base URL (use a local mirror to bypass rate limits)",
                    ConfigValue::String(ops.external_matching.mb_base_url.clone()),
                    source_for(
                        ops.external_matching.mb_base_url == defaults.external_matching.mb_base_url,
                        ExternalMatchingConfig::KDL_MB_BASE_URL,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::String(s) = v {
                            c.opinions.external_matching.mb_base_url =
                                s.trim_end_matches('/').to_string();
                        }
                    },
                ),
            ],
        },
        // Disc Extraction
        ConfigGroup {
            name: "Disc Extraction",
            collapsed: false,
            fields: vec![
                field(
                    "Disc tag name",
                    "Tag name to write extracted disc identifier into",
                    ConfigValue::String(ops.disc_extraction.disc_tag_name.clone()),
                    source_for(
                        ops.disc_extraction.disc_tag_name == defaults.disc_extraction.disc_tag_name,
                        DiscExtractionOpinions::KDL_DISC_TAG_NAME,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::String(s) = v {
                            c.opinions.disc_extraction.disc_tag_name = s.clone();
                        }
                    },
                ),
                field(
                    "Map letters to numbers",
                    "Map letter prefixes to numbers (A\u{2192}1, B\u{2192}2, ...)",
                    ConfigValue::Bool(ops.disc_extraction.map_letters_to_numbers),
                    source_for(
                        ops.disc_extraction.map_letters_to_numbers
                            == defaults.disc_extraction.map_letters_to_numbers,
                        DiscExtractionOpinions::KDL_MAP_LETTERS,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.disc_extraction.map_letters_to_numbers = *b;
                        }
                    },
                ),
            ],
        },
        // Album Art
        ConfigGroup {
            name: "Album Art",
            collapsed: false,
            fields: vec![field(
                "Sidecar deploy mode",
                "Deploy sidecar cover images alongside audio files to libraries",
                ConfigValue::Enum {
                    selected: ops.album_art.sidecar_deploy_mode.to_index(),
                    options: SidecarDeployMode::OPTIONS.to_vec(),
                },
                source_for(
                    ops.album_art.sidecar_deploy_mode == defaults.album_art.sidecar_deploy_mode,
                    AlbumArtOpinions::KDL_SIDECAR_DEPLOY,
                ),
                false,
                |v, c| {
                    if let ConfigValue::Enum { selected, .. } = v {
                        c.opinions.album_art.sidecar_deploy_mode =
                            SidecarDeployMode::from_index(*selected);
                    }
                },
            )],
        },
        // Quality Resolution
        ConfigGroup {
            name: "Quality Resolution",
            collapsed: false,
            fields: vec![field(
                "Inbox bitrate fuzz percent",
                "Inbox-to-corpus bitrate tolerance for equivalence",
                ConfigValue::Float(ops.quality_resolution.inbox_bitrate_fuzz_percent),
                source_for(
                    (ops.quality_resolution.inbox_bitrate_fuzz_percent
                        - defaults.quality_resolution.inbox_bitrate_fuzz_percent)
                        .abs()
                        < f64::EPSILON,
                    QualityResolutionOpinions::KDL_BITRATE_FUZZ,
                ),
                false,
                |v, c| {
                    if let ConfigValue::Float(f) = v {
                        c.opinions.quality_resolution.inbox_bitrate_fuzz_percent = *f;
                    }
                },
            )],
        },
        // Canonicalization
        ConfigGroup {
            name: "Canonicalization",
            collapsed: false,
            fields: vec![field(
                "Strip album format suffixes",
                "Normalize EP/LP suffixes during album collision detection",
                ConfigValue::Bool(ops.canonicalization.strip_album_format_suffixes),
                source_for(
                    ops.canonicalization.strip_album_format_suffixes
                        == defaults.canonicalization.strip_album_format_suffixes,
                    CanonicalizationOpinions::KDL_STRIP_SUFFIXES,
                ),
                false,
                |v, c| {
                    if let ConfigValue::Bool(b) = v {
                        c.opinions.canonicalization.strip_album_format_suffixes = *b;
                    }
                },
            )],
        },
        // Health Detection
        ConfigGroup {
            name: "Health Detection",
            collapsed: false,
            fields: vec![
                field(
                    "Required tags",
                    "Tags that must be present on every track",
                    ConfigValue::StringList(ops.health_detection.required_tags.clone()),
                    source_for(
                        ops.health_detection.required_tags
                            == defaults.health_detection.required_tags,
                        HealthDetectionOpinions::KDL_REQUIRED_TAGS,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::StringList(list) = v {
                            c.opinions.health_detection.required_tags = list.clone();
                        }
                    },
                ),
                field(
                    "Album artist only if compilation",
                    "Only require album_artist on multi-artist albums",
                    ConfigValue::Bool(
                        ops.health_detection
                            .album_artist_only_required_if_compilation,
                    ),
                    source_for(
                        ops.health_detection
                            .album_artist_only_required_if_compilation
                            == defaults
                                .health_detection
                                .album_artist_only_required_if_compilation,
                        HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions
                                .health_detection
                                .album_artist_only_required_if_compilation = *b;
                        }
                    },
                ),
                field(
                    "Single album suffix",
                    "Suffix appended when tagging as single",
                    ConfigValue::String(ops.health_detection.single_album_suffix.clone()),
                    source_for(
                        ops.health_detection.single_album_suffix
                            == defaults.health_detection.single_album_suffix,
                        HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::String(s) = v {
                            c.opinions.health_detection.single_album_suffix = s.clone();
                        }
                    },
                ),
            ],
        },
        // Inbox Organize
        ConfigGroup {
            name: "Inbox Organize",
            collapsed: false,
            fields: vec![field(
                "Directory granularity",
                "How to group inbox directories for organize workflow",
                ConfigValue::Enum {
                    selected: ops.inbox_organize.directory_granularity.to_index(),
                    options: InboxOrganizeGranularity::OPTIONS.to_vec(),
                },
                source_for(
                    ops.inbox_organize.directory_granularity
                        == defaults.inbox_organize.directory_granularity,
                    InboxOrganizeOpinions::KDL_DIR_GRANULARITY,
                ),
                false,
                |v, c| {
                    if let ConfigValue::Enum { selected, .. } = v {
                        c.opinions.inbox_organize.directory_granularity =
                            InboxOrganizeGranularity::from_index(*selected);
                    }
                },
            )],
        },
        // Advanced
        ConfigGroup {
            name: "Advanced",
            collapsed: false,
            fields: vec![
                field(
                    "Idle rescan interval",
                    "Idle time before auto-rescanning corpus/inbox (e.g. 3m, 180s, disabled)",
                    ConfigValue::Duration(ops.idle_rescan_interval_secs),
                    source_for(
                        ops.idle_rescan_interval_secs == defaults.idle_rescan_interval_secs,
                        Opinions::KDL_IDLE_RESCAN,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Duration(secs) = v {
                            c.opinions.idle_rescan_interval_secs = *secs;
                        }
                    },
                ),
                field(
                    "Leave transactions open",
                    "Keep one open transaction; adds Transaction tab to view ring",
                    ConfigValue::Bool(ops.leave_transactions_open),
                    source_for(
                        ops.leave_transactions_open == defaults.leave_transactions_open,
                        Opinions::KDL_LEAVE_TXN_OPEN,
                    ),
                    false,
                    |v, c| {
                        if let ConfigValue::Bool(b) = v {
                            c.opinions.leave_transactions_open = *b;
                        }
                    },
                ),
            ],
        },
        // Performance
        ConfigGroup {
            name: "Performance",
            collapsed: false,
            fields: vec![
                field(
                    "Worker threads",
                    "Number of worker threads (auto = 2x logical cores)",
                    ConfigValue::OptionalUint(ops.performance.worker_threads),
                    source_for(
                        ops.performance.worker_threads == defaults.performance.worker_threads,
                        PerformanceOpinions::KDL_WORKER_THREADS,
                    ),
                    true,
                    |v, c| {
                        if let ConfigValue::OptionalUint(n) = v {
                            c.opinions.performance.worker_threads = *n;
                        }
                    },
                ),
                field(
                    "DB cache MB",
                    "SQLite page cache size per connection in MB",
                    ConfigValue::UintU32(ops.performance.db_cache_mb),
                    source_for(
                        ops.performance.db_cache_mb == defaults.performance.db_cache_mb,
                        PerformanceOpinions::KDL_DB_CACHE,
                    ),
                    true,
                    |v, c| {
                        if let ConfigValue::UintU32(n) = v {
                            c.opinions.performance.db_cache_mb = *n;
                        }
                    },
                ),
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
        assert_eq!(
            rebuilt.opinions.startup.default_view,
            config.opinions.startup.default_view
        );
        assert_eq!(
            rebuilt
                .opinions
                .quality_resolution
                .inbox_bitrate_fuzz_percent,
            config
                .opinions
                .quality_resolution
                .inbox_bitrate_fuzz_percent
        );
        assert_eq!(
            rebuilt
                .opinions
                .duplicate_analysis
                .fingerprint_similarity_threshold,
            config
                .opinions
                .duplicate_analysis
                .fingerprint_similarity_threshold
        );
        assert_eq!(
            rebuilt.opinions.disc_extraction.disc_tag_name,
            config.opinions.disc_extraction.disc_tag_name
        );
        assert_eq!(
            rebuilt.opinions.disc_extraction.map_letters_to_numbers,
            config.opinions.disc_extraction.map_letters_to_numbers
        );
        assert_eq!(
            rebuilt.opinions.album_art.sidecar_deploy_mode,
            config.opinions.album_art.sidecar_deploy_mode
        );
    }

    #[test]
    fn test_source_detection_default() {
        let config = test_config();
        let groups = build_groups_from_config(&config, None);

        // With no KDL content and default values, all sources should be Default
        for group in &groups {
            for field in &group.fields {
                assert_eq!(
                    field.source,
                    FieldSource::Default,
                    "Field '{}' in group '{}' should be Default",
                    field.label,
                    group.name
                );
            }
        }
    }

    #[test]
    fn test_source_detection_loaded() {
        let config = test_config();

        // If the KDL content mentions StartupOpinions::KDL_VACUUM_THRESHOLD, that field should be Loaded
        let groups = build_groups_from_config(&config, Some("vacuum-threshold 0.1"));

        let startup_group = &groups[1];
        assert_eq!(startup_group.name, "Startup");
        let vacuum_field = &startup_group.fields[1];
        assert_eq!(vacuum_field.label, "Vacuum threshold");
        assert_eq!(vacuum_field.source, FieldSource::Loaded);
    }
}
