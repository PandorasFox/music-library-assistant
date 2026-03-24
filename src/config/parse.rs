//! config.kdl parsing: parse_kdl_config + all parse_*_opinions + parse_size_mb.

use super::types::*;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Parse a human-readable size string like "256mb", "1gb", "512" into MB.
/// Accepts: plain numbers (interpreted as MB), or suffixed with kb/mb/gb (case-insensitive).
pub(crate) fn parse_size_mb(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    if let Some(num_str) = s.strip_suffix("gb") {
        num_str.trim().parse::<u32>().ok().map(|n| n * 1024)
    } else if let Some(num_str) = s.strip_suffix("mb") {
        num_str.trim().parse::<u32>().ok()
    } else if let Some(num_str) = s.strip_suffix("kb") {
        // KB rounds up to nearest MB (minimum 1MB)
        num_str.trim().parse::<u32>().ok().map(|n| n.div_ceil(1024))
    } else {
        // Plain number = MB
        s.parse::<u32>().ok()
    }
}

/// Parse quality-resolution opinions from KDL node
fn parse_quality_resolution_opinions(
    _node: &kdl::KdlNode,
    _opinions: &mut QualityResolutionOpinions,
) {
    // All quality-resolution fields removed (inbox zone removed).
}

/// Parse canonicalization opinions from KDL node
fn parse_canonicalization_opinions(node: &kdl::KdlNode, opinions: &mut CanonicalizationOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == CanonicalizationOpinions::KDL_STRIP_SUFFIXES {
                if let Some(entry) = child.entries().first() {
                    if let Some(val) = entry.value().as_bool() {
                        opinions.strip_album_format_suffixes = val;
                    }
                }
            }
        }
    }
}

/// Parse startup opinions from KDL node
fn parse_startup_opinions(node: &kdl::KdlNode, opinions: &mut StartupOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                StartupOpinions::KDL_FORCE_CHECK => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.force_check_all_files_at_startup = val;
                        }
                    }
                }
                StartupOpinions::KDL_VACUUM_THRESHOLD => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.vacuum_threshold = val;
                        }
                    }
                }
                StartupOpinions::KDL_DEFAULT_VIEW => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            match val {
                                "health" => opinions.default_view = StartupView::Health,
                                "search" => opinions.default_view = StartupView::Search,
                                "browser" => opinions.default_view = StartupView::Browser,
                                "external-matches" => opinions.default_view = StartupView::ExternalMatches,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse health detection opinions from KDL node
fn parse_health_detection_opinions(node: &kdl::KdlNode, opinions: &mut HealthDetectionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                HealthDetectionOpinions::KDL_REQUIRED_TAGS => {
                    let tags: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !tags.is_empty() {
                        opinions.required_tags = tags;
                    }
                }
                HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.album_artist_only_required_if_compilation = val;
                        }
                    }
                }
                HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.single_album_suffix = val.to_string();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse performance opinions from KDL node
fn parse_performance_opinions(node: &kdl::KdlNode, opinions: &mut PerformanceOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                PerformanceOpinions::KDL_WORKER_THREADS => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.worker_threads = Some(val as usize);
                            }
                        }
                    }
                }
                PerformanceOpinions::KDL_DB_CACHE => {
                    if let Some(entry) = child.entries().first() {
                        // Try string first (e.g., "256mb", "1gb")
                        if let Some(s) = entry.value().as_string() {
                            if let Some(mb) = parse_size_mb(s) {
                                opinions.db_cache_mb = mb;
                            }
                        }
                        // Fall back to plain integer (interpreted as MB)
                        else if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.db_cache_mb = val as u32;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse tag-splitting opinions from KDL node.
///
/// Expected format:
/// ```kdl
/// tag-splitting {
///     collab "feat" "featuring" "ft" "with" "vs"
///     artist ";"
///     genre ";" ","
/// }
/// ```
///
/// - `collab` node: list of collaboration keywords (replaces defaults if present)
/// - Other nodes: tag name with list of separator strings
fn parse_tag_splitting_opinions(node: &kdl::KdlNode, opinions: &mut TagSplittingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let node_name = child.name().value();

            match node_name {
                TagSplittingOpinions::KDL_COLLAB => {
                    // Parse collaboration keywords
                    let keywords: std::collections::HashSet<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !keywords.is_empty() {
                        opinions.collaboration_keywords = keywords;
                    }
                }
                _ => {
                    // Treat as tag name with separator list
                    let tag_name = node_name.to_uppercase();
                    let separators: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !separators.is_empty() {
                        opinions.tag_separators.insert(tag_name, separators);
                    }
                }
            }
        }
    }
}

/// Parse release-packing opinions from KDL node.
fn parse_packing_weights(node: &kdl::KdlNode, weights: &mut PackingWeights) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let val = child.entries().first().and_then(|e| e.value().as_f64());
            if let Some(v) = val {
                match child.name().value() {
                    PackingWeights::KDL_ACOUSTID_CONFIDENCE => weights.acoustid_confidence = v,
                    PackingWeights::KDL_DURATION_MATCH => weights.duration_match = v,
                    PackingWeights::KDL_TITLE_MATCH => weights.title_match = v,
                    PackingWeights::KDL_ARTIST_MATCH => weights.artist_match = v,
                    PackingWeights::KDL_ALBUM_MATCH => weights.album_match = v,
                    PackingWeights::KDL_TRACK_NUMBER_MATCH => weights.track_number_match = v,
                    // "directory-cohesion" silently ignored for backwards compatibility
                    _ => {}
                }
            }
        }
    }
}

fn parse_release_packing_opinions(node: &kdl::KdlNode, opinions: &mut ReleasePackingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                ReleasePackingOpinions::KDL_DURATION_TOLERANCE_PCT => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.duration_tolerance_pct = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_MIN_CONFIDENCE => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.min_confidence = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_CANDIDATE_WEIGHTS => {
                    parse_packing_weights(child, &mut opinions.candidate_weights);
                }
                ReleasePackingOpinions::KDL_ELIMINATION_WEIGHTS => {
                    parse_packing_weights(child, &mut opinions.elimination_weights);
                }
                ReleasePackingOpinions::KDL_TITLE_PREASSIGN_THRESHOLD => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.title_preassign_threshold = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_PACKING_KNOT_RATIO => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            // 0 disables, otherwise must be > 1.0
                            if val == 0.0 || val > 1.0 {
                                opinions.packing_knot_ratio = val;
                            }
                        }
                    }
                }
                ReleasePackingOpinions::KDL_PACKING_KNOT_SIZE_LIMIT => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            // 0 disables
                            opinions.packing_knot_size_limit = val.max(0) as usize;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_SINGLES_BEFORE_INCOMPLETES => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.singles_before_incompletes = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_ALLOW_DISCOGRAPHY_REDUCTION => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.allow_resolve_knots_with_discographies = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ACOUSTID_RATIO => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.low_confidence_max_acoustid_ratio = val;
                        }
                    }
                }
                ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ALBUM_MATCH => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.low_confidence_max_album_match = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse duplicate-analysis opinions from KDL node
fn parse_duplicate_analysis_opinions(
    node: &kdl::KdlNode,
    opinions: &mut DuplicateAnalysisOpinions,
) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                DuplicateAnalysisOpinions::KDL_FP_THRESHOLD => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.fingerprint_similarity_threshold = val;
                        }
                    }
                }
                DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            opinions.duration_tolerance_ms = val;
                        }
                    }
                }
                DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.elide_variant_titles = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse external-matching opinions from KDL node
fn parse_external_matching_opinions(node: &kdl::KdlNode, opinions: &mut ExternalMatchingConfig) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                ExternalMatchingConfig::KDL_ACOUSTID_KEY => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.acoustid_api_key = val.to_string();
                        }
                    }
                }
                ExternalMatchingConfig::KDL_REQ_PER_SEC => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.requests_per_second = val as u32;
                            }
                        }
                    }
                }
                ExternalMatchingConfig::KDL_MB_REQ_PER_SEC => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.mb_requests_per_second = val as u32;
                            }
                        }
                    }
                }
                ExternalMatchingConfig::KDL_MB_BASE_URL => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.mb_base_url = val.trim_end_matches('/').to_string();
                        }
                    }
                }
                ExternalMatchingConfig::KDL_AUTO_ENRICH => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.auto_enrich_on_match = val;
                        }
                    }
                }
                ExternalMatchingConfig::KDL_MB_CACHE_TTL => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.mb_cache_ttl_days = val as u32;
                            }
                        }
                    }
                }
                ExternalMatchingConfig::KDL_PREFERRED_LOCALES => {
                    // Multi-value node: preferred-locales "en" "ja"
                    let locales: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !locales.is_empty() {
                        opinions.preferred_locales = locales;
                    }
                }
                ExternalMatchingConfig::KDL_TAG_TEMPLATES => {
                    parse_tag_templates(child, &mut opinions.tag_templates);
                }
                ExternalMatchingConfig::KDL_CREDIT_ROUTING => {
                    parse_credit_routing(child, &mut opinions.credit_routing);
                }
                MbTagNameConfig::KDL_MB_TAG_NAMES => {
                    parse_mb_tag_names(child, &mut opinions.mb_tag_names);
                }
                ExternalMatchingConfig::KDL_COVER_ART_TYPES => {
                    // Multi-value node: cover-art-types "Front" "Back"
                    let types: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !types.is_empty() {
                        opinions.cover_art_types = types;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse tag-templates block: each child node is `tag-name "template string"`.
fn parse_tag_templates(node: &kdl::KdlNode, templates: &mut Vec<(String, String)>) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let tag_name = child.name().value().to_uppercase().replace('-', "_");
            if let Some(entry) = child.entries().first() {
                if let Some(val) = entry.value().as_string() {
                    templates.push((tag_name, val.to_string()));
                }
            }
        }
    }
}

/// Parse credit-routing block: each child node is a relation type with bool properties.
///
/// ```kdl
/// credit-routing {
///     performer artist=true
///     vocal artist=true title=true
///     instrument artist=true
///     remixer title=true
///     feat-format "feat. {artists}"
/// }
/// ```
fn parse_credit_routing(node: &kdl::KdlNode, config: &mut CreditRoutingConfig) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let name = child.name().value();
            if name == CreditRoutingConfig::KDL_FEAT_FORMAT {
                if let Some(entry) = child.entries().first() {
                    if let Some(val) = entry.value().as_string() {
                        config.feat_format = val.to_string();
                    }
                }
                continue;
            }
            if name == CreditRoutingConfig::KDL_MAX_FEAT_CREDITS {
                if let Some(entry) = child.entries().first() {
                    if let Some(val) = entry.value().as_i64() {
                        config.max_feat_credits = if val <= 0 { None } else { Some(val as u32) };
                    }
                }
                continue;
            }
            // Each relation type node has named bool properties: artist, title, composer
            let get_bool = |prop_name: &str| -> bool {
                child
                    .get(prop_name)
                    .and_then(|v| v.value().as_bool())
                    .unwrap_or(false)
            };
            let routing = RelationRouting {
                artist: get_bool("artist"),
                title: get_bool("title"),
                composer: get_bool("composer"),
            };
            config.routing.insert(name.to_string(), routing);
        }
    }
}

/// Parse mb-tag-names block: configurable Vorbis Comment names for MB entity IDs.
///
/// ```kdl
/// mb-tag-names {
///     recording "MUSICBRAINZ_RECORDING"
///     release "MUSICBRAINZ_RELEASE"
///     track "MUSICBRAINZ_TRACK"
/// }
/// ```
fn parse_mb_tag_names(node: &kdl::KdlNode, config: &mut MbTagNameConfig) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                MbTagNameConfig::KDL_PICARD_COMPAT => {
                    if let Some(val) = child.entries().first().and_then(|e| e.value().as_bool()) {
                        config.picard_compat = val;
                    }
                }
                name => {
                    let val = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .map(|s| s.to_uppercase());
                    let Some(val) = val else { continue };
                    match name {
                        MbTagNameConfig::KDL_RECORDING => config.recording = val,
                        MbTagNameConfig::KDL_RELEASE => config.release = val,
                        MbTagNameConfig::KDL_TRACK => config.track = val,
                        _ => {}
                    }
                }
            }
        }
    }
}

/// Parse album-art opinions from KDL node
fn parse_album_art_opinions(_node: &kdl::KdlNode, _opinions: &mut AlbumArtOpinions) {
    // Album art embed/upgrade fields removed; sidecar_deploy_mode is parsed elsewhere.
}

/// Parse disc-extraction opinions from KDL node
fn parse_disc_extraction_opinions(node: &kdl::KdlNode, opinions: &mut DiscExtractionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                DiscExtractionOpinions::KDL_DISC_TAG_NAME => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.disc_tag_name = val.to_string();
                        }
                    }
                }
                DiscExtractionOpinions::KDL_MAP_LETTERS => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.map_letters_to_numbers = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub(crate) fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    let mut config = Config {
        storage_root: PathBuf::new(),
        libraries_root: None,
        stash_root: None,
        source_dirs: Vec::new(),
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            "storage-root" | "root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.storage_root = PathBuf::from(path_str);
                    }
                }
            }
            "libraries-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.libraries_root = Some(PathBuf::from(path_str));
                    }
                }
            }
            "stash-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.stash_root = Some(PathBuf::from(path_str));
                    }
                }
            }
            // legacy stanzas silently ignored
            "legacy-library" | "dir" => {}
            "opinions" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        match child.name().value() {
                            Opinions::KDL_BLOCK_QUALITY_RESOLUTION => {
                                parse_quality_resolution_opinions(
                                    child,
                                    &mut config.opinions.quality_resolution,
                                );
                            }
                            Opinions::KDL_BLOCK_CANONICALIZATION => {
                                parse_canonicalization_opinions(
                                    child,
                                    &mut config.opinions.canonicalization,
                                );
                            }
                            Opinions::KDL_BLOCK_STARTUP => {
                                parse_startup_opinions(child, &mut config.opinions.startup);
                            }
                            Opinions::KDL_BLOCK_HEALTH_DETECTION => {
                                parse_health_detection_opinions(
                                    child,
                                    &mut config.opinions.health_detection,
                                );
                            }
                            Opinions::KDL_BLOCK_PERFORMANCE => {
                                parse_performance_opinions(child, &mut config.opinions.performance);
                            }
                            Opinions::KDL_BLOCK_TAG_SPLITTING => {
                                parse_tag_splitting_opinions(
                                    child,
                                    &mut config.opinions.tag_splitting,
                                );
                            }
                            Opinions::KDL_BLOCK_DUPLICATE_ANALYSIS => {
                                parse_duplicate_analysis_opinions(
                                    child,
                                    &mut config.opinions.duplicate_analysis,
                                );
                            }
                            Opinions::KDL_BLOCK_RELEASE_PACKING => {
                                parse_release_packing_opinions(
                                    child,
                                    &mut config.opinions.release_packing,
                                );
                            }
                            Opinions::KDL_LEAVE_TXN_OPEN => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_bool() {
                                        config.opinions.leave_transactions_open = val;
                                    }
                                }
                            }
                            Opinions::KDL_WATCHER_POLL_INTERVAL => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_i64() {
                                        if val > 0 {
                                            config.opinions.watcher_poll_interval_secs = val as u64;
                                        }
                                    }
                                }
                            }
                            Opinions::KDL_SESSION_LIFETIME => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(s) = entry.value().as_string() {
                                        if s == "close" {
                                            config.opinions.session_lifetime_days = None;
                                        }
                                    } else if let Some(val) = entry.value().as_i64() {
                                        if val > 0 {
                                            config.opinions.session_lifetime_days = Some(val as u64);
                                        }
                                    }
                                }
                            }
                            Opinions::KDL_BLOCK_EXTERNAL_MATCHING => {
                                parse_external_matching_opinions(
                                    child,
                                    &mut config.opinions.external_matching,
                                );
                            }
                            Opinions::KDL_BLOCK_DISC_EXTRACTION => {
                                parse_disc_extraction_opinions(
                                    child,
                                    &mut config.opinions.disc_extraction,
                                );
                            }
                            Opinions::KDL_BLOCK_ALBUM_ART => {
                                parse_album_art_opinions(child, &mut config.opinions.album_art);
                            }
                            // "debug" block silently ignored (all debug options removed)
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.storage_root.as_os_str().is_empty() {
        anyhow::bail!("storage-root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::t;

    #[test]
    fn test_parse_storage_root() {
        let kdl = r#"
storage-root "/Volumes/cerberus/archive"
"#;

        let config = t!(parse_kdl_config(kdl));
        assert_eq!(config.storage_root, PathBuf::from("/Volumes/cerberus/archive"));
        assert_eq!(
            config.libraries_dir(),
            PathBuf::from("/Volumes/cerberus/archive/libraries")
        );
        assert_eq!(
            config.stash_dir(),
            PathBuf::from("/Volumes/cerberus/archive/stash")
        );
        assert!(config.source_dirs.is_empty());
    }

    #[test]
    fn test_parse_legacy_root_compat() {
        // Old "root" key still works
        let kdl = r#"
root "/Volumes/cerberus/archive"
"#;
        let config = t!(parse_kdl_config(kdl));
        assert_eq!(config.storage_root, PathBuf::from("/Volumes/cerberus/archive"));
    }

    #[test]
    fn test_parse_multi_root() {
        let kdl = r#"
storage-root "/mnt/pool/archive/audio"
libraries-root "/mnt/pool/libraries"
stash-root "/mnt/pool/stash"
"#;
        let config = t!(parse_kdl_config(kdl));
        assert_eq!(config.storage_root, PathBuf::from("/mnt/pool/archive/audio"));
        assert_eq!(config.libraries_root, Some(PathBuf::from("/mnt/pool/libraries")));
        assert_eq!(config.stash_root, Some(PathBuf::from("/mnt/pool/stash")));
        assert_eq!(config.libraries_dir(), PathBuf::from("/mnt/pool/libraries"));
        assert_eq!(config.stash_dir(), PathBuf::from("/mnt/pool/stash"));
    }

    #[test]
    fn test_missing_root() {
        let kdl = r#""#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("storage-root not specified"));
    }

    #[test]
    fn test_opinions_parsing() {
        let kdl = r#"
root "/archive"

opinions {
    canonicalization {
        strip-album-format-suffixes false
    }
}
"#;

        let config = t!(parse_kdl_config(kdl));

        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_opinions_defaults() {
        let kdl = r#"
root "/archive"
"#;

        let config = t!(parse_kdl_config(kdl));

        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_is_path_in_source() {
        let kdl = r#"
root "/archive"
"#;

        let mut config = t!(parse_kdl_config(kdl));
        config.source_dirs = t!(super::super::dirs::parse_dirs_kdl(
            r#"
dir "web/releases/bandcamp" {
    library "music"
}

dir "web/releases/steam" {
    library "soundtracks"
}
"#,
        ));

        // Zone-relative paths (as stored in DB) should match
        assert!(config.is_path_in_source(std::path::Path::new(
            "web/releases/bandcamp/Artist/Album/track.flac"
        )));
        assert!(config.is_path_in_source(std::path::Path::new(
            "web/releases/steam/Game/Soundtrack/01.mp3"
        )));

        // Non-configured paths should not match
        assert!(!config.is_path_in_source(std::path::Path::new(
            "web/releases/itunes/Artist/Album/track.flac"
        )));
        assert!(!config.is_path_in_source(std::path::Path::new(
            "physical/cd/Artist/Album/track.flac"
        )));

        // Exact prefix match (not substring)
        assert!(!config.is_path_in_source(std::path::Path::new(
            "web/releases/bandcamp-extra/Artist/track.flac"
        )));
    }

    #[test]
    fn test_startup_default_view_parsing() {
        // Default is Health when not specified
        let kdl = r#"root "/archive""#;
        let config = t!(parse_kdl_config(kdl));
        assert_eq!(config.opinions.startup.default_view, StartupView::Health);

        // Explicit values
        for (value, expected) in [
            ("health", StartupView::Health),
            ("search", StartupView::Search),
            ("browser", StartupView::Browser),
        ] {
            let kdl = format!(
                r#"root "/archive"
opinions {{
    startup {{
        default-view "{value}"
    }}
}}"#
            );
            let config = t!(parse_kdl_config(&kdl));
            assert_eq!(
                config.opinions.startup.default_view, expected,
                "default-view \"{value}\" should parse to {expected:?}"
            );
        }
    }
}
