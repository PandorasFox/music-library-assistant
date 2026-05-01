//! Config Editor Build/Apply
//!
//! Converts between `Config` struct and the flat group/field representation
//! used by the editor UI. Each field carries its own applier closure, so
//! build and apply logic are co-located — no separate match block needed.

use super::types::*;
use mm_meta::config::{
    AlbumArtOpinions, CanonicalizationOpinions, Config, CreditRoutingConfig,
    DiscExtractionOpinions, DuplicateAnalysisOpinions, ExternalMatchingConfig,
    HealthDetectionOpinions, MbTagNameConfig, Opinions, PackingWeights, PerformanceOpinions,
    RelationRouting, ReleasePackingOpinions, SidecarDeployMode, StartupOpinions, StartupView,
    TagSplittingOpinions,
};

// =============================================================================
// Enum mapping infrastructure
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
    StartupView::ExternalMatches => "Ext Authorities",
]);

config_enum_map!(SidecarDeployMode, SidecarDeployMode::PrimaryCover, [
    SidecarDeployMode::Disabled => "Disabled",
    SidecarDeployMode::PrimaryCover => "PrimaryCover",
    SidecarDeployMode::All => "All",
]);

// =============================================================================
// Field builder
// =============================================================================

/// Construct a ConfigField with original_value/original_source automatically
/// snapshotted from the initial value/source.
///
/// The `applier` writes the field's value back into a `Config`. Stored at build
/// time so that every field definition is self-contained (build + apply in one place).
fn field(
    label: &'static str,
    description: &'static str,
    help: &'static [&'static str],
    value: ConfigValue,
    source: FieldSource,
    restart_required: bool,
    applier: fn(&ConfigValue, &mut Config),
) -> ConfigField {
    ConfigField {
        label,
        description,
        help,
        original_value: value.clone(),
        original_source: source,
        value,
        source,
        restart_required,
        applier,
    }
}

// =============================================================================
// Routing table helpers
// =============================================================================

/// Column names for the credit routing BoolGrid.
const ROUTING_COLUMNS: &[&str] = &["artist", "title", "composer"];

/// Convert a RelationRouting to a row of booleans (matches ROUTING_COLUMNS order).
fn routing_to_bools(r: &RelationRouting) -> Vec<bool> {
    vec![r.artist, r.title, r.composer]
}

/// Convert a row of booleans back to a RelationRouting.
fn bools_to_routing(bools: &[bool]) -> RelationRouting {
    RelationRouting {
        artist: bools.first().copied().unwrap_or(false),
        title: bools.get(1).copied().unwrap_or(false),
        composer: bools.get(2).copied().unwrap_or(false),
    }
}

// =============================================================================
// Build
// =============================================================================

/// Build editor groups from the current config.
///
/// `kdl_content` is used to determine whether a field was loaded from the
/// config file (vs being at its default). If None, all fields show as Default.
pub fn build_groups_from_config(config: &Config, kdl_content: Option<&str>) -> Vec<ConfigGroup> {
    let defaults = mm_meta::config::Opinions::default();
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

    // Field generation macros — function-local so they capture ops/defaults/source_for.
    // Type token determines ConfigValue variant, comparison, and applier shape.
    // Suffix `!` on the type token sets restart_required = true.
    macro_rules! cf {
        (bool, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::Bool(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.$($p).+ = *b; } })
        };
        (bool!, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::Bool(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), true,
                |v, c| { if let ConfigValue::Bool(b) = v { c.opinions.$($p).+ = *b; } })
        };
        (float, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::Float(ops.$($p).+),
                source_for((ops.$($p).+ - defaults.$($p).+).abs() < f64::EPSILON, $kdl), false,
                |v, c| { if let ConfigValue::Float(f) = v { c.opinions.$($p).+ = *f; } })
        };
        (u32, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::UintU32(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::UintU32(n) = v { c.opinions.$($p).+ = *n; } })
        };
        (u32!, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::UintU32(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), true,
                |v, c| { if let ConfigValue::UintU32(n) = v { c.opinions.$($p).+ = *n; } })
        };
        (i64, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::SignedInt(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::SignedInt(n) = v { c.opinions.$($p).+ = *n; } })
        };
        (string, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::String(ops.$($p).+.clone()),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::String(s) = v { c.opinions.$($p).+ = s.clone(); } })
        };
        (string_list, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::StringList(ops.$($p).+.clone()),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::StringList(list) = v { c.opinions.$($p).+ = list.clone(); } })
        };
        (string_set, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help,
                ConfigValue::StringSet(ops.$($p).+.iter().cloned().collect()),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::StringSet(items) = v {
                    c.opinions.$($p).+ = items.iter().cloned().collect();
                } })
        };
        (string_list_map, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help,
                ConfigValue::StringListMap(ops.$($p).+.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::StringListMap(items) = v {
                    c.opinions.$($p).+ = items.iter().cloned().collect();
                } })
        };
        (enum $ty:ty, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help,
                ConfigValue::Enum { selected: ops.$($p).+.to_index(), options: <$ty>::OPTIONS.to_vec() },
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::Enum { selected, .. } = v {
                    c.opinions.$($p).+ = <$ty>::from_index(*selected);
                } })
        };
        (duration, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::Duration(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), false,
                |v, c| { if let ConfigValue::Duration(secs) = v { c.opinions.$($p).+ = *secs; } })
        };
        (optional_uint!, $($p:ident).+, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::OptionalUint(ops.$($p).+),
                source_for(ops.$($p).+ == defaults.$($p).+, $kdl), true,
                |v, c| { if let ConfigValue::OptionalUint(n) = v { c.opinions.$($p).+ = *n; } })
        };
    }

    // PackingWeights float field — takes explicit weight/default refs and a
    // two-segment path (weight_group.field) for the applier.
    macro_rules! pw {
        ($w:expr, $d:expr, $path:ident . $field:ident, $label:expr, $desc:expr, $help:expr, $kdl:expr) => {
            field($label, $desc, $help, ConfigValue::Float($w.$field),
                source_for(($w.$field - $d.$field).abs() < f64::EPSILON, $kdl), false,
                |v, c| { if let ConfigValue::Float(f) = v { c.opinions.release_packing.$path.$field = *f; } })
        };
    }

    vec![
        ConfigGroup { name: "Startup", collapsed: false, fields: vec![
            cf!(bool, startup.force_check_all_files_at_startup, "Force check all files at startup",
                "Bypass mtime optimization, verify all indexed files",
                &["TODO"], StartupOpinions::KDL_FORCE_CHECK),
            cf!(float, startup.vacuum_threshold, "Vacuum threshold",
                "Free-page ratio threshold for DB compaction prompt (0.0 disables)",
                &["TODO"], StartupOpinions::KDL_VACUUM_THRESHOLD),
            cf!(enum StartupView, startup.default_view, "Default view",
                "View to open after startup progress completes",
                &["TODO"], StartupOpinions::KDL_DEFAULT_VIEW),
        ]},
        ConfigGroup { name: "Duplicate Analysis", collapsed: false, fields: vec![
            cf!(float, duplicate_analysis.fingerprint_similarity_threshold, "Fingerprint similarity threshold",
                "Pairs below this similarity (0-100) are not duplicates",
                &["TODO"], DuplicateAnalysisOpinions::KDL_FP_THRESHOLD),
            cf!(i64, duplicate_analysis.duration_tolerance_ms, "Duration tolerance ms",
                "Tracks with duration diff above this are clustered separately",
                &["TODO"], DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE),
            cf!(bool, duplicate_analysis.elide_variant_titles, "Elide variant titles",
                "Skip dupe pairs where titles differ and contain remix/live/etc.",
                &["TODO"], DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS),
        ]},
        ConfigGroup { name: "Release Packing", collapsed: false, fields: vec![
            cf!(float, release_packing.duration_tolerance_pct, "Duration tolerance %",
                "Discard recording matches with duration diff above this fraction (0.0-1.0)",
                &["TODO"], ReleasePackingOpinions::KDL_DURATION_TOLERANCE_PCT),
            cf!(float, release_packing.min_confidence, "Min AcoustID confidence",
                "Discard recording matches below this confidence (0.0-1.0)",
                &["TODO"], ReleasePackingOpinions::KDL_MIN_CONFIDENCE),
            cf!(float, release_packing.title_preassign_threshold, "Title pre-assign threshold",
                "Title similarity threshold for elimination pre-assignment (0.0-1.0)",
                &["TODO"], ReleasePackingOpinions::KDL_TITLE_PREASSIGN_THRESHOLD),
            // Custom validation: only accept 0.0 or > 1.0
            field("Packing knot ratio", "Proposals/inodes ratio threshold for knot extraction (0 to disable)",
                &["TODO"],
                ConfigValue::Float(ops.release_packing.packing_knot_ratio),
                source_for(ops.release_packing.packing_knot_ratio == defaults.release_packing.packing_knot_ratio,
                    ReleasePackingOpinions::KDL_PACKING_KNOT_RATIO),
                false, |v, c| { if let ConfigValue::Float(f) = v {
                    if *f == 0.0 || *f > 1.0 { c.opinions.release_packing.packing_knot_ratio = *f; }
                } }),
            // usize <-> u32 cast
            field("Packing knot size limit", "Max component size before knot extraction (0 to disable)",
                &["TODO"],
                ConfigValue::UintU32(ops.release_packing.packing_knot_size_limit as u32),
                source_for(ops.release_packing.packing_knot_size_limit == defaults.release_packing.packing_knot_size_limit,
                    ReleasePackingOpinions::KDL_PACKING_KNOT_SIZE_LIMIT),
                false, |v, c| { if let ConfigValue::UintU32(n) = v {
                    c.opinions.release_packing.packing_knot_size_limit = *n as usize;
                } }),
            cf!(bool, release_packing.singles_before_incompletes, "Singles before incompletes",
                "Run single-track MIS round before incompletes",
                &["TODO"], ReleasePackingOpinions::KDL_SINGLES_BEFORE_INCOMPLETES),
            cf!(bool, release_packing.allow_resolve_knots_with_discographies, "Resolve knots with discographies",
                "Reduce knots to covering proposals (discography releases) when possible",
                &["TODO"], ReleasePackingOpinions::KDL_ALLOW_DISCOGRAPHY_REDUCTION),
            cf!(float, release_packing.low_confidence_max_acoustid_ratio, "Low confidence max AcoustID ratio",
                "Max AcoustID-matched fraction to trigger low-confidence downgrade (0.0-1.0)",
                &["TODO"], ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ACOUSTID_RATIO),
            cf!(float, release_packing.low_confidence_max_album_match, "Low confidence max album match",
                "Max avg album_match score to trigger low-confidence downgrade (0.0-1.0)",
                &["TODO"], ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ALBUM_MATCH),
            // u64 -> u32 cast (config editor lacks a u64 ConfigValue; 30min ≪ 2^32 sec)
            field("Idle full-repack after (seconds)",
                "Auto-promote incremental repacks to full once idle this long (0 = disable)",
                &["TODO"],
                ConfigValue::UintU32(ops.release_packing.idle_full_repack_after_secs as u32),
                source_for(ops.release_packing.idle_full_repack_after_secs == defaults.release_packing.idle_full_repack_after_secs,
                    ReleasePackingOpinions::KDL_IDLE_FULL_REPACK_AFTER_SECS),
                false, |v, c| { if let ConfigValue::UintU32(n) = v {
                    c.opinions.release_packing.idle_full_repack_after_secs = *n as u64;
                } }),
        ]},
        ConfigGroup { name: "Packing: Candidate Weights", collapsed: true, fields: {
            let w = &ops.release_packing.candidate_weights;
            let d = PackingWeights::candidate_defaults();
            vec![
                pw!(w, d, candidate_weights.acoustid_confidence, "AcoustID confidence", "Weight for fingerprint confidence (0.0-1.0)", &["TODO"], PackingWeights::KDL_ACOUSTID_CONFIDENCE),
                pw!(w, d, candidate_weights.duration_match, "Duration match", "Weight for duration match quality (0.0-1.0)", &["TODO"], PackingWeights::KDL_DURATION_MATCH),
                pw!(w, d, candidate_weights.title_match, "Title match", "Weight for title similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_TITLE_MATCH),
                pw!(w, d, candidate_weights.artist_match, "Artist match", "Weight for artist similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_ARTIST_MATCH),
                pw!(w, d, candidate_weights.album_match, "Album match", "Weight for album similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_ALBUM_MATCH),
                pw!(w, d, candidate_weights.track_number_match, "Track number match", "Weight for tracknumber matching slot position (0.0-1.0)", &["TODO"], PackingWeights::KDL_TRACK_NUMBER_MATCH),
            ]
        }},
        ConfigGroup { name: "Packing: Elimination Weights", collapsed: true, fields: {
            let w = &ops.release_packing.elimination_weights;
            let d = PackingWeights::elimination_defaults();
            vec![
                pw!(w, d, elimination_weights.acoustid_confidence, "AcoustID confidence", "Weight for fingerprint confidence (0.0-1.0)", &["TODO"], PackingWeights::KDL_ACOUSTID_CONFIDENCE),
                pw!(w, d, elimination_weights.duration_match, "Duration match", "Weight for duration match quality (0.0-1.0)", &["TODO"], PackingWeights::KDL_DURATION_MATCH),
                pw!(w, d, elimination_weights.title_match, "Title match", "Weight for title similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_TITLE_MATCH),
                pw!(w, d, elimination_weights.artist_match, "Artist match", "Weight for artist similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_ARTIST_MATCH),
                pw!(w, d, elimination_weights.album_match, "Album match", "Weight for album similarity (0.0-1.0)", &["TODO"], PackingWeights::KDL_ALBUM_MATCH),
                pw!(w, d, elimination_weights.track_number_match, "Track number match", "Weight for tracknumber matching slot position (0.0-1.0)", &["TODO"], PackingWeights::KDL_TRACK_NUMBER_MATCH),
            ]
        }},
        ConfigGroup { name: "Tag Splitting", collapsed: false, fields: vec![
            cf!(string_set, tag_splitting.collaboration_keywords, "Collaboration keywords",
                "Keywords like feat, ft, vs for artist collabs",
                &["TODO"], TagSplittingOpinions::KDL_COLLAB),
            cf!(string_list_map, tag_splitting.tag_separators, "Tag separators",
                "Per-tag separator strings",
                &["TODO"], Opinions::KDL_BLOCK_TAG_SPLITTING),
        ]},
        ConfigGroup { name: "External Matching", collapsed: false, fields: vec![
            cf!(string, external_matching.acoustid_api_key, "AcoustID API key",
                "API key for AcoustID fingerprint lookups (empty = disabled)",
                &["TODO"], ExternalMatchingConfig::KDL_ACOUSTID_KEY),
            cf!(u32, external_matching.requests_per_second, "Requests per second",
                "Rate limit for AcoustID API calls",
                &["TODO"], ExternalMatchingConfig::KDL_REQ_PER_SEC),
            cf!(bool, external_matching.auto_enrich_on_match, "Auto-enrich on match",
                "Auto-trigger MB enrichment when AcoustID matches arrive",
                &["TODO"], ExternalMatchingConfig::KDL_AUTO_ENRICH),
            cf!(u32, external_matching.mb_cache_ttl_days, "MB cache TTL (days)",
                "Days before re-fetching MusicBrainz cache entries",
                &["TODO"], ExternalMatchingConfig::KDL_MB_CACHE_TTL),
            cf!(u32, external_matching.mb_requests_per_second, "MB requests per second",
                "Rate limit ceiling for MusicBrainz API (adaptive backoff)",
                &["TODO"], ExternalMatchingConfig::KDL_MB_REQ_PER_SEC),
            // Custom applier: trim trailing slash
            field("MB base URL", "MusicBrainz API base URL (use a local mirror to bypass rate limits)",
                &["TODO"],
                ConfigValue::String(ops.external_matching.mb_base_url.clone()),
                source_for(ops.external_matching.mb_base_url == defaults.external_matching.mb_base_url,
                    ExternalMatchingConfig::KDL_MB_BASE_URL),
                false, |v, c| { if let ConfigValue::String(s) = v {
                    c.opinions.external_matching.mb_base_url = s.trim_end_matches('/').to_string();
                } }),
            cf!(string_list, external_matching.preferred_locales, "Preferred locales",
                "Locale preference for artist name resolution (e.g. en, ja)",
                &["TODO"], ExternalMatchingConfig::KDL_PREFERRED_LOCALES),
            cf!(string_list, external_matching.cover_art_types, "Cover art types",
                "CAA image types to fetch (e.g. Front, Back)",
                &["TODO"], ExternalMatchingConfig::KDL_COVER_ART_TYPES),
        ]},
        ConfigGroup { name: "MB Tag Names", collapsed: true, fields: vec![
            cf!(string, external_matching.mb_tag_names.recording, "Recording tag",
                "Vorbis Comment tag name for recording MBID",
                &["TODO"], MbTagNameConfig::KDL_RECORDING),
            cf!(string, external_matching.mb_tag_names.release, "Release tag",
                "Vorbis Comment tag name for release MBID",
                &["TODO"], MbTagNameConfig::KDL_RELEASE),
            cf!(string, external_matching.mb_tag_names.track, "Track tag",
                "Vorbis Comment tag name for track-on-release MBID",
                &["TODO"], MbTagNameConfig::KDL_TRACK),
            cf!(bool, external_matching.mb_tag_names.picard_compat, "Picard-compat aliases",
                "Also write Picard-style aliases (MUSICBRAINZ_ALBUMID, etc.) for Navidrome",
                &["TODO"], MbTagNameConfig::KDL_PICARD_COMPAT),
        ]},
        ConfigGroup { name: "Credit Routing", collapsed: true, fields: {
            let cr = &ops.external_matching.credit_routing;
            let cr_defaults = CreditRoutingConfig::default();
            vec![
                cf!(string, external_matching.credit_routing.feat_format, "Feat format",
                    "Template for vocalist title suffix ({artists} is replaced)",
                    &["TODO"], CreditRoutingConfig::KDL_FEAT_FORMAT),
                // max_feat_credits: Option<u32> mapped through OptionalUint (usize)
                field("Max feat credits", "Cap on artist names in feat suffix (auto = unlimited)",
                    &["TODO"],
                    ConfigValue::OptionalUint(cr.max_feat_credits.map(|n| n as usize)),
                    source_for(cr.max_feat_credits == cr_defaults.max_feat_credits,
                        CreditRoutingConfig::KDL_MAX_FEAT_CREDITS),
                    false, |v, c| { if let ConfigValue::OptionalUint(n) = v {
                        c.opinions.external_matching.credit_routing.max_feat_credits = n.map(|n| n as u32);
                    } }),
                // BoolGrid for the routing table
                {
                    let mut rows: Vec<(String, Vec<bool>)> = cr.routing
                        .iter()
                        .map(|(k, v)| (k.clone(), routing_to_bools(v)))
                        .collect();
                    rows.sort_by(|a, b| a.0.cmp(&b.0));

                    let default_rows: Vec<(String, Vec<bool>)> = {
                        let mut r: Vec<_> = cr_defaults.routing
                            .iter()
                            .map(|(k, v)| (k.clone(), routing_to_bools(v)))
                            .collect();
                        r.sort_by(|a, b| a.0.cmp(&b.0));
                        r
                    };

                    field("Relation routing", "Route recording credits to artist/title/composer tags",
                        &["Rows: MusicBrainz relation types (performer, vocal, instrument, remixer)",
                          "Columns: which tag fields receive the credited artist name",
                          "artist = ARTIST tag, title = feat. suffix, composer = COMPOSER tag"],
                        ConfigValue::BoolGrid {
                            columns: ROUTING_COLUMNS,
                            rows: rows.clone(),
                        },
                        source_for(rows == default_rows, CreditRoutingConfig::KDL_CREDIT_ROUTING),
                        false, |v, c| {
                            if let ConfigValue::BoolGrid { rows, .. } = v {
                                let mut routing = std::collections::HashMap::new();
                                for (name, bools) in rows {
                                    routing.insert(name.clone(), bools_to_routing(bools));
                                }
                                c.opinions.external_matching.credit_routing.routing = routing;
                            }
                        })
                },
            ]
        }},
        ConfigGroup { name: "Disc Extraction", collapsed: false, fields: vec![
            cf!(string, disc_extraction.disc_tag_name, "Disc tag name",
                "Tag name to write extracted disc identifier into",
                &["TODO"], DiscExtractionOpinions::KDL_DISC_TAG_NAME),
            cf!(bool, disc_extraction.map_letters_to_numbers, "Map letters to numbers",
                "Map letter prefixes to numbers (A\u{2192}1, B\u{2192}2, ...)",
                &["TODO"], DiscExtractionOpinions::KDL_MAP_LETTERS),
        ]},
        ConfigGroup { name: "Album Art", collapsed: false, fields: vec![
            cf!(enum SidecarDeployMode, album_art.sidecar_deploy_mode, "Sidecar deploy mode",
                "Deploy sidecar cover images alongside audio files to libraries",
                &["TODO"], AlbumArtOpinions::KDL_SIDECAR_DEPLOY),
        ]},
        ConfigGroup { name: "Canonicalization", collapsed: false, fields: vec![
            cf!(bool, canonicalization.strip_album_format_suffixes, "Strip album format suffixes",
                "Normalize EP/LP suffixes during album collision detection",
                &["TODO"], CanonicalizationOpinions::KDL_STRIP_SUFFIXES),
        ]},
        ConfigGroup { name: "Health Detection", collapsed: false, fields: vec![
            cf!(string_list, health_detection.required_tags, "Required tags",
                "Tags that must be present on every track",
                &["TODO"], HealthDetectionOpinions::KDL_REQUIRED_TAGS),
            cf!(bool, health_detection.album_artist_only_required_if_compilation, "Album artist only if compilation",
                "Only require album_artist on multi-artist albums",
                &["TODO"], HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION),
            cf!(string, health_detection.single_album_suffix, "Single album suffix",
                "Suffix appended when tagging as single",
                &["TODO"], HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX),
        ]},
        ConfigGroup { name: "Advanced", collapsed: false, fields: vec![
            cf!(bool, leave_transactions_open, "Leave transactions open",
                "Keep one open transaction; adds Transaction tab to view ring",
                &["TODO"], Opinions::KDL_LEAVE_TXN_OPEN),
            cf!(bool, auto_deploy, "Auto-deploy",
                "Automatically deploy and fix stale library entries",
                &["When enabled, files with DeployReady signals are automatically",
                  "hard-linked to the library after content analysis. Stale entries",
                  "are moved to correct paths. Conflicts are never auto-deployed."],
                Opinions::KDL_AUTO_DEPLOY),
            cf!(duration, watcher_poll_interval_secs, "Watcher poll interval",
                "Filesystem poll interval when inotify is unavailable (seconds)",
                &["TODO"], Opinions::KDL_WATCHER_POLL_INTERVAL),
        ]},
        ConfigGroup { name: "Performance", collapsed: false, fields: vec![
            cf!(optional_uint!, performance.worker_threads, "Worker threads",
                "Number of worker threads (auto = 2x logical cores)",
                &["TODO"], PerformanceOpinions::KDL_WORKER_THREADS),
            cf!(u32!, performance.db_cache_mb, "DB cache MB",
                "SQLite page cache size per connection in MB",
                &["TODO"], PerformanceOpinions::KDL_DB_CACHE),
        ]},
    ]
}

// =============================================================================
// Apply
// =============================================================================

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
            storage_root: "/tmp/test".into(),
            libraries_root: None,
            stash_root: None,
            source_dirs: vec![],
            opinions: mm_meta::config::Opinions::default(),
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
            rebuilt.opinions.duplicate_analysis.fingerprint_similarity_threshold,
            config.opinions.duplicate_analysis.fingerprint_similarity_threshold
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

        let startup_group = &groups[0];
        assert_eq!(startup_group.name, "Startup");
        let vacuum_field = &startup_group.fields[1];
        assert_eq!(vacuum_field.label, "Vacuum threshold");
        assert_eq!(vacuum_field.source, FieldSource::Loaded);
    }
}
