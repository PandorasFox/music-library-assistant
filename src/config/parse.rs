//! config.kdl parsing: parse_kdl_config + all parse_*_opinions + parse_size_mb.

use anyhow::{Context, Result};
use std::path::PathBuf;
use super::types::*;

/// Parse a human-readable size string like "256mb", "1gb", "512" into MB.
/// Accepts: plain numbers (interpreted as MB), or suffixed with kb/mb/gb (case-insensitive).
fn parse_size_mb(s: &str) -> Option<u32> {
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
fn parse_quality_resolution_opinions(node: &kdl::KdlNode, opinions: &mut QualityResolutionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "inbox-bitrate-fuzz-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.inbox_bitrate_fuzz_percent = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse canonicalization opinions from KDL node
fn parse_canonicalization_opinions(node: &kdl::KdlNode, opinions: &mut CanonicalizationOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "strip-album-format-suffixes" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.strip_album_format_suffixes = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse startup opinions from KDL node
fn parse_startup_opinions(node: &kdl::KdlNode, opinions: &mut StartupOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "force-check-all-files" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.force_check_all_files_at_startup = val;
                        }
                    }
                }
                "vacuum-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.vacuum_threshold = val;
                        }
                    }
                }
                "default-view" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            match val {
                                "health" => opinions.default_view = StartupView::Health,
                                "search" => opinions.default_view = StartupView::Search,
                                "browser" => opinions.default_view = StartupView::Browser,
                                "inbox" => opinions.default_view = StartupView::Inbox,
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
                "required-tags" => {
                    let tags: Vec<String> = child
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                        .collect();
                    if !tags.is_empty() {
                        opinions.required_tags = tags;
                    }
                }
                "album-artist-only-required-if-compilation" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.album_artist_only_required_if_compilation = val;
                        }
                    }
                }
                "single-album-suffix" => {
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
                "worker-threads" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.worker_threads = Some(val as usize);
                            }
                        }
                    }
                }
                "db-cache" => {
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
                "timing-instrumentation" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.timing_instrumentation = val;
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
                "collab" => {
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

/// Parse duplicate-analysis opinions from KDL node
fn parse_duplicate_analysis_opinions(node: &kdl::KdlNode, opinions: &mut DuplicateAnalysisOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "fingerprint-similarity-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.fingerprint_similarity_threshold = val;
                        }
                    }
                }
                "duration-tolerance-ms" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            opinions.duration_tolerance_ms = val;
                        }
                    }
                }
                "elide-variant-titles" => {
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
                "acoustid-api-key" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.acoustid_api_key = val.to_string();
                        }
                    }
                }
                "requests-per-second" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.requests_per_second = val as u32;
                            }
                        }
                    }
                }
                "show-musicbrainz-url" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.show_musicbrainz_url = val;
                        }
                    }
                }
                _ => {}
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
                "disc-tag-name" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            opinions.disc_tag_name = val.to_string();
                        }
                    }
                }
                "map-letters-to-numbers" => {
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

/// Parse inbox-organize opinions from KDL node
fn parse_inbox_organize_opinions(node: &kdl::KdlNode, opinions: &mut InboxOrganizeOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "directory-granularity" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_string() {
                            match val {
                                "leaf" => opinions.directory_granularity = InboxOrganizeGranularity::Leaf,
                                "top-level" => opinions.directory_granularity = InboxOrganizeGranularity::TopLevel,
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

pub(crate) fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    let mut config = Config {
        root: PathBuf::new(),
        legacy_enabled: false,
        source_dirs: Vec::new(),
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            "root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.root = PathBuf::from(path_str);
                    }
                }
            }
            "legacy-library" => {
                // Boolean toggle: `legacy-library true`
                if let Some(entry) = node.entries().first() {
                    if let Some(val) = entry.value().as_bool() {
                        config.legacy_enabled = val;
                    }
                }
            }
            // dir stanzas are now in dirs.kdl — ignored here for backwards compat
            "dir" => {}
            "opinions" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        match child.name().value() {
                            "lossy-shit-formats-to-flac" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_bool() {
                                        config.opinions.lossy_shit_formats_to_flac = val;
                                    }
                                }
                            }
                            "quality-resolution" => {
                                parse_quality_resolution_opinions(child, &mut config.opinions.quality_resolution);
                            }
                            "canonicalization" => {
                                parse_canonicalization_opinions(child, &mut config.opinions.canonicalization);
                            }
                            "startup" => {
                                parse_startup_opinions(child, &mut config.opinions.startup);
                            }
                            "health-detection" => {
                                parse_health_detection_opinions(child, &mut config.opinions.health_detection);
                            }
                            "performance" => {
                                parse_performance_opinions(child, &mut config.opinions.performance);
                            }
                            "tag-splitting" => {
                                parse_tag_splitting_opinions(child, &mut config.opinions.tag_splitting);
                            }
                            "duplicate-analysis" => {
                                parse_duplicate_analysis_opinions(child, &mut config.opinions.duplicate_analysis);
                            }
                            "inbox-organize" => {
                                parse_inbox_organize_opinions(child, &mut config.opinions.inbox_organize);
                            }
                            "idle-rescan-interval" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(s) = entry.value().as_string() {
                                        if let Ok(dur) = humantime::parse_duration(s) {
                                            config.opinions.idle_rescan_interval_secs = dur.as_secs();
                                        }
                                    } else if let Some(val) = entry.value().as_i64() {
                                        // Legacy: bare integer seconds
                                        config.opinions.idle_rescan_interval_secs = val.max(0) as u64;
                                    }
                                }
                            }
                            "leave-transactions-open" => {
                                if let Some(entry) = child.entries().first() {
                                    if let Some(val) = entry.value().as_bool() {
                                        config.opinions.leave_transactions_open = val;
                                    }
                                }
                            }
                            "external-matching" => {
                                parse_external_matching_opinions(child, &mut config.opinions.external_matching);
                            }
                            "disc-extraction" => {
                                parse_disc_extraction_opinions(child, &mut config.opinions.disc_extraction);
                            }
                            "album-art" => {
                                parse_album_art_opinions(child, &mut config.opinions.album_art);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.root.as_os_str().is_empty() {
        anyhow::bail!("root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_root() {
        let kdl = r#"
root "/Volumes/cerberus/archive"

legacy-library true
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(config.root, PathBuf::from("/Volumes/cerberus/archive"));
        assert_eq!(config.corpus_dir(), PathBuf::from("/Volumes/cerberus/archive/corpus"));
        assert_eq!(config.libraries_dir(), PathBuf::from("/Volumes/cerberus/archive/libraries"));
        assert_eq!(config.stash_dir(), PathBuf::from("/Volumes/cerberus/archive/stash"));
        assert!(config.legacy_enabled);
        assert!(config.source_dirs.is_empty());
    }

    #[test]
    fn test_missing_root() {
        let kdl = r#"
legacy-library true
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("root not specified"));
    }

    #[test]
    fn test_legacy_disabled_by_default() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert!(!config.legacy_enabled);
    }

    #[test]
    fn test_opinions_parsing() {
        let kdl = r#"
root "/archive"

opinions {
    quality-resolution {
        inbox-bitrate-fuzz-percent 3.0
    }

    canonicalization {
        strip-album-format-suffixes false
    }
}
"#;

        let config = parse_kdl_config(kdl).unwrap();

        assert_eq!(config.opinions.quality_resolution.inbox_bitrate_fuzz_percent, 3.0);
        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_opinions_defaults() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();

        assert!(!config.opinions.lossy_shit_formats_to_flac);
        assert_eq!(config.opinions.quality_resolution.inbox_bitrate_fuzz_percent, 5.0);
        assert!(!config.opinions.canonicalization.strip_album_format_suffixes);
    }

    #[test]
    fn test_lossy_shit_formats_to_flac_opinion() {
        let kdl = r#"
root "/archive"

opinions {
    lossy-shit-formats-to-flac true
}
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert!(config.opinions.lossy_shit_formats_to_flac);

        // Explicit false
        let kdl_false = r#"
root "/archive"

opinions {
    lossy-shit-formats-to-flac false
}
"#;
        let config_false = parse_kdl_config(kdl_false).unwrap();
        assert!(!config_false.opinions.lossy_shit_formats_to_flac);
    }

    #[test]
    fn test_is_path_in_source() {
        let kdl = r#"
root "/archive"
"#;

        let mut config = parse_kdl_config(kdl).unwrap();
        config.source_dirs = super::super::dirs::parse_dirs_kdl(r#"
dir "web/releases/bandcamp" {
    library "music"
}

dir "web/releases/steam" {
    library "soundtracks"
}
"#).unwrap();

        // Relative paths (as stored in DB) should match
        assert!(config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/bandcamp/Artist/Album/track.flac"
        )));
        assert!(config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/steam/Game/Soundtrack/01.mp3"
        )));

        // Non-configured paths should not match
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/itunes/Artist/Album/track.flac"
        )));
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/physical/cd/Artist/Album/track.flac"
        )));

        // Exact prefix match (not substring)
        assert!(!config.is_path_in_source(std::path::Path::new(
            "corpus/web/releases/bandcamp-extra/Artist/track.flac"
        )));
    }

    #[test]
    fn test_startup_default_view_parsing() {
        // Default is Health when not specified
        let kdl = r#"root "/archive""#;
        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(config.opinions.startup.default_view, StartupView::Health);

        // Explicit values
        for (value, expected) in [
            ("health", StartupView::Health),
            ("search", StartupView::Search),
            ("browser", StartupView::Browser),
            ("inbox", StartupView::Inbox),
        ] {
            let kdl = format!(
                r#"root "/archive"
opinions {{
    startup {{
        default-view "{value}"
    }}
}}"#
            );
            let config = parse_kdl_config(&kdl).unwrap();
            assert_eq!(
                config.opinions.startup.default_view, expected,
                "default-view \"{value}\" should parse to {expected:?}"
            );
        }
    }
}
