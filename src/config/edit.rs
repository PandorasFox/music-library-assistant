//! KDL write-back: apply_config_edits_to_kdl, write_config_to_disk, KDL node helpers.

use anyhow::{Context, Result};
use std::fs;
use super::types::*;
use mm_utils::get_config_dir;

/// Apply config edits to a KDL document in place, preserving comments and formatting.
///
/// Parses `original_kdl` into a `KdlDocument`, then for each opinion field that
/// differs between `old_config` and `new_config`, modifies only the changed node.
/// Returns the modified KDL text.
pub fn apply_config_edits_to_kdl(original_kdl: &str, old_config: &Config, new_config: &Config) -> Result<String> {
    let mut doc: kdl::KdlDocument = original_kdl.parse()
        .context("Failed to parse original KDL for edit")?;

    // Ensure "opinions" block exists
    if doc.get("opinions").is_none() {
        let mut node = kdl::KdlNode::new("opinions");
        node.set_children(kdl::KdlDocument::new());
        doc.nodes_mut().push(node);
    }

    let opinions_node = doc.get_mut("opinions").unwrap();
    let opinions_doc = opinions_node.ensure_children();

    // --- General opinions (direct children of "opinions") ---
    if new_config.opinions.lossy_shit_formats_to_flac != old_config.opinions.lossy_shit_formats_to_flac {
        set_or_create_bool_node(opinions_doc, Opinions::KDL_LOSSY_SHIT, new_config.opinions.lossy_shit_formats_to_flac);
    }

    // --- Startup ---
    let old_s = &old_config.opinions.startup;
    let new_s = &new_config.opinions.startup;
    if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup
        || new_s.vacuum_threshold != old_s.vacuum_threshold
        || new_s.default_view != old_s.default_view
    {
        let startup = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_STARTUP);
        if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup {
            set_or_create_bool_node(startup, StartupOpinions::KDL_FORCE_CHECK, new_s.force_check_all_files_at_startup);
        }
        if new_s.vacuum_threshold != old_s.vacuum_threshold {
            set_or_create_float_node(startup, StartupOpinions::KDL_VACUUM_THRESHOLD, new_s.vacuum_threshold);
        }
        if new_s.default_view != old_s.default_view {
            let view_str = match new_s.default_view {
                StartupView::Health => "health",
                StartupView::Search => "search",
                StartupView::Browser => "browser",
                StartupView::Inbox => "inbox",
            };
            set_or_create_string_node(startup, StartupOpinions::KDL_DEFAULT_VIEW, view_str);
        }
    }

    // --- Quality Resolution ---
    let old_qr = &old_config.opinions.quality_resolution;
    let new_qr = &new_config.opinions.quality_resolution;
    if new_qr.inbox_bitrate_fuzz_percent != old_qr.inbox_bitrate_fuzz_percent
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_QUALITY_RESOLUTION);
        set_or_create_float_node(block, QualityResolutionOpinions::KDL_BITRATE_FUZZ, new_qr.inbox_bitrate_fuzz_percent);
    }

    // --- Canonicalization ---
    if new_config.opinions.canonicalization.strip_album_format_suffixes != old_config.opinions.canonicalization.strip_album_format_suffixes {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_CANONICALIZATION);
        set_or_create_bool_node(block, CanonicalizationOpinions::KDL_STRIP_SUFFIXES, new_config.opinions.canonicalization.strip_album_format_suffixes);
    }

    // --- Health Detection ---
    let old_hd = &old_config.opinions.health_detection;
    let new_hd = &new_config.opinions.health_detection;
    if new_hd.required_tags != old_hd.required_tags
        || new_hd.album_artist_only_required_if_compilation != old_hd.album_artist_only_required_if_compilation
        || new_hd.single_album_suffix != old_hd.single_album_suffix
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_HEALTH_DETECTION);
        if new_hd.required_tags != old_hd.required_tags {
            // Remove old node and create new one with all tag values
            block.nodes_mut().retain(|n| n.name().value() != HealthDetectionOpinions::KDL_REQUIRED_TAGS);
            let mut node = kdl::KdlNode::new(HealthDetectionOpinions::KDL_REQUIRED_TAGS);
            for tag in &new_hd.required_tags {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(tag.clone())));
            }
            block.nodes_mut().push(node);
        }
        if new_hd.album_artist_only_required_if_compilation != old_hd.album_artist_only_required_if_compilation {
            set_or_create_bool_node(block, HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION, new_hd.album_artist_only_required_if_compilation);
        }
        if new_hd.single_album_suffix != old_hd.single_album_suffix {
            set_or_create_string_node(block, HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX, &new_hd.single_album_suffix);
        }
    }

    // --- Duplicate Analysis ---
    let old_da = &old_config.opinions.duplicate_analysis;
    let new_da = &new_config.opinions.duplicate_analysis;
    if new_da.fingerprint_similarity_threshold != old_da.fingerprint_similarity_threshold
        || new_da.duration_tolerance_ms != old_da.duration_tolerance_ms
        || new_da.elide_variant_titles != old_da.elide_variant_titles
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_DUPLICATE_ANALYSIS);
        if new_da.fingerprint_similarity_threshold != old_da.fingerprint_similarity_threshold {
            set_or_create_float_node(block, DuplicateAnalysisOpinions::KDL_FP_THRESHOLD, new_da.fingerprint_similarity_threshold);
        }
        if new_da.duration_tolerance_ms != old_da.duration_tolerance_ms {
            set_or_create_int_node(block, DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE, new_da.duration_tolerance_ms);
        }
        if new_da.elide_variant_titles != old_da.elide_variant_titles {
            set_or_create_bool_node(block, DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS, new_da.elide_variant_titles);
        }
    }

    // --- Idle Rescan Interval ---
    if new_config.opinions.idle_rescan_interval_secs != old_config.opinions.idle_rescan_interval_secs {
        let dur = std::time::Duration::from_secs(new_config.opinions.idle_rescan_interval_secs);
        let formatted = humantime::format_duration(dur).to_string();
        set_or_create_string_node(opinions_doc, Opinions::KDL_IDLE_RESCAN, &formatted);
    }

    // --- Leave Transactions Open ---
    if new_config.opinions.leave_transactions_open != old_config.opinions.leave_transactions_open {
        set_or_create_bool_node(opinions_doc, Opinions::KDL_LEAVE_TXN_OPEN, new_config.opinions.leave_transactions_open);
    }

    // --- Inbox Organize ---
    if new_config.opinions.inbox_organize.directory_granularity != old_config.opinions.inbox_organize.directory_granularity {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_INBOX_ORGANIZE);
        let gran_str = match new_config.opinions.inbox_organize.directory_granularity {
            InboxOrganizeGranularity::Leaf => "leaf",
            InboxOrganizeGranularity::TopLevel => "top-level",
        };
        set_or_create_string_node(block, InboxOrganizeOpinions::KDL_DIR_GRANULARITY, gran_str);
    }

    // --- Tag Splitting ---
    let old_ts = &old_config.opinions.tag_splitting;
    let new_ts = &new_config.opinions.tag_splitting;
    if new_ts.collaboration_keywords != old_ts.collaboration_keywords
        || new_ts.tag_separators != old_ts.tag_separators
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_TAG_SPLITTING);

        // Write collab keywords if changed
        if new_ts.collaboration_keywords != old_ts.collaboration_keywords {
            block.nodes_mut().retain(|n| n.name().value() != TagSplittingOpinions::KDL_COLLAB);
            let mut node = kdl::KdlNode::new(TagSplittingOpinions::KDL_COLLAB);
            let mut keywords: Vec<&String> = new_ts.collaboration_keywords.iter().collect();
            keywords.sort();
            for kw in keywords {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(kw.clone())));
            }
            block.nodes_mut().push(node);
        }

        // Write tag separators if changed
        if new_ts.tag_separators != old_ts.tag_separators {
            // Remove old tag separator nodes (all non-collab nodes)
            block.nodes_mut().retain(|n| n.name().value() == TagSplittingOpinions::KDL_COLLAB);
            let mut tags: Vec<(&String, &Vec<String>)> = new_ts.tag_separators.iter().collect();
            tags.sort_by_key(|(k, _)| *k);
            for (tag_name, seps) in tags {
                let mut tag_node = kdl::KdlNode::new(tag_name.to_lowercase().as_str());
                for sep in seps {
                    tag_node.push(kdl::KdlEntry::new(kdl::KdlValue::String(sep.clone())));
                }
                block.nodes_mut().push(tag_node);
            }
        }
    }

    // --- External Matching ---
    let old_em = &old_config.opinions.external_matching;
    let new_em = &new_config.opinions.external_matching;
    if new_em.acoustid_api_key != old_em.acoustid_api_key
        || new_em.requests_per_second != old_em.requests_per_second
        || new_em.mb_requests_per_second != old_em.mb_requests_per_second
        || new_em.mb_base_url != old_em.mb_base_url
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_EXTERNAL_MATCHING);
        if new_em.acoustid_api_key != old_em.acoustid_api_key {
            set_or_create_string_node(block, ExternalMatchingConfig::KDL_ACOUSTID_KEY, &new_em.acoustid_api_key);
        }
        if new_em.requests_per_second != old_em.requests_per_second {
            set_or_create_int_node(block, ExternalMatchingConfig::KDL_REQ_PER_SEC, new_em.requests_per_second as i64);
        }
        if new_em.mb_requests_per_second != old_em.mb_requests_per_second {
            set_or_create_int_node(block, ExternalMatchingConfig::KDL_MB_REQ_PER_SEC, new_em.mb_requests_per_second as i64);
        }
        if new_em.mb_base_url != old_em.mb_base_url {
            set_or_create_string_node(block, ExternalMatchingConfig::KDL_MB_BASE_URL, &new_em.mb_base_url);
        }
    }

    // --- Disc Extraction ---
    let old_de = &old_config.opinions.disc_extraction;
    let new_de = &new_config.opinions.disc_extraction;
    if new_de.disc_tag_name != old_de.disc_tag_name
        || new_de.map_letters_to_numbers != old_de.map_letters_to_numbers
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_DISC_EXTRACTION);
        if new_de.disc_tag_name != old_de.disc_tag_name {
            set_or_create_string_node(block, DiscExtractionOpinions::KDL_DISC_TAG_NAME, &new_de.disc_tag_name);
        }
        if new_de.map_letters_to_numbers != old_de.map_letters_to_numbers {
            set_or_create_bool_node(block, DiscExtractionOpinions::KDL_MAP_LETTERS, new_de.map_letters_to_numbers);
        }
    }

    // --- Performance ---
    let old_p = &old_config.opinions.performance;
    let new_p = &new_config.opinions.performance;
    if new_p.worker_threads != old_p.worker_threads
        || new_p.db_cache_mb != old_p.db_cache_mb
        || new_p.timing_instrumentation != old_p.timing_instrumentation
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_PERFORMANCE);
        if new_p.worker_threads != old_p.worker_threads {
            match new_p.worker_threads {
                Some(n) => set_or_create_int_node(block, PerformanceOpinions::KDL_WORKER_THREADS, n as i64),
                None => { block.nodes_mut().retain(|n| n.name().value() != PerformanceOpinions::KDL_WORKER_THREADS); }
            }
        }
        if new_p.db_cache_mb != old_p.db_cache_mb {
            set_or_create_int_node(block, PerformanceOpinions::KDL_DB_CACHE, new_p.db_cache_mb as i64);
        }
        if new_p.timing_instrumentation != old_p.timing_instrumentation {
            set_or_create_bool_node(block, PerformanceOpinions::KDL_TIMING, new_p.timing_instrumentation);
        }
    }

    // --- Debug ---
    let old_d = &old_config.opinions.debug;
    let new_d = &new_config.opinions.debug;
    if new_d.memory_logging != old_d.memory_logging {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_DEBUG);
        set_or_create_bool_node(block, DebugOpinions::KDL_MEMORY_LOGGING, new_d.memory_logging);
    }

    Ok(doc.to_string())
}

/// Write config edits to disk with comment-preserving KDL modification.
///
/// 1. Backs up existing config.kdl -> config.kdl.bak
/// 2. Applies edits to original KDL text
/// 3. Writes modified KDL to config.kdl
pub fn write_config_to_disk(original_kdl: &str, old_config: &Config, new_config: &Config) -> Result<()> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");
    let backup_path = config_dir.join("config.kdl.bak");

    // Backup existing config
    if config_path.exists() {
        fs::copy(&config_path, &backup_path)
            .with_context(|| format!("Failed to backup config to {:?}", backup_path))?;
    }

    // Apply edits to KDL
    let modified_kdl = apply_config_edits_to_kdl(original_kdl, old_config, new_config)?;

    // Write modified KDL
    fs::write(&config_path, &modified_kdl)
        .with_context(|| format!("Failed to write config to {:?}", config_path))?;

    Ok(())
}

// KDL modification helpers

/// Get or create a child block node within a KdlDocument.
fn ensure_child_block<'a>(doc: &'a mut kdl::KdlDocument, name: &str) -> &'a mut kdl::KdlDocument {
    // Check if node exists, create if not
    if doc.get(name).is_none() {
        let mut node = kdl::KdlNode::new(name);
        node.set_children(kdl::KdlDocument::new());
        doc.nodes_mut().push(node);
    }
    let child_node = doc.get_mut(name).expect("ensure_child_block: node should exist after creation");
    child_node.ensure_children()
}

/// Set or create a bool-valued node within a KdlDocument.
///
/// Note: `clear_fmt()` is called on modified entries so the serializer
/// uses the new value instead of the cached representation.
fn set_or_create_bool_node(doc: &mut kdl::KdlDocument, name: &str, value: bool) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Bool(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Bool(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Bool(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create a float-valued node within a KdlDocument.
fn set_or_create_float_node(doc: &mut kdl::KdlDocument, name: &str, value: f64) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Base10Float(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10Float(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10Float(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create an integer-valued node within a KdlDocument.
fn set_or_create_int_node(doc: &mut kdl::KdlDocument, name: &str, value: i64) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::Base10(value));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10(value)));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::Base10(value)));
        doc.nodes_mut().push(node);
    }
}

/// Set or create a string-valued node within a KdlDocument.
fn set_or_create_string_node(doc: &mut kdl::KdlDocument, name: &str, value: &str) {
    if let Some(node) = doc.get_mut(name) {
        if let Some(entry) = node.entries_mut().first_mut() {
            entry.set_value(kdl::KdlValue::String(value.to_string()));
            entry.clear_fmt();
        } else {
            node.push(kdl::KdlEntry::new(kdl::KdlValue::String(value.to_string())));
        }
    } else {
        let mut node = kdl::KdlNode::new(name);
        node.push(kdl::KdlEntry::new(kdl::KdlValue::String(value.to_string())));
        doc.nodes_mut().push(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use super::super::parse::parse_kdl_config;

    #[test]
    fn test_kdl_direct_modification() {
        let kdl_str = r#"root "/archive"

opinions {
    startup {
        default-view "health"
    }
}
"#;
        let mut doc: kdl::KdlDocument = kdl_str.parse().unwrap();

        // Navigate: opinions -> startup -> default-view
        let opinions = doc.get_mut("opinions").unwrap();
        let opinions_children = opinions.ensure_children();
        let startup = opinions_children.get_mut("startup").unwrap();
        let startup_children = startup.ensure_children();
        let dv = startup_children.get_mut("default-view").unwrap();
        let entry = dv.entries_mut().first_mut().unwrap();
        entry.set_value(kdl::KdlValue::String("browser".to_string()));
        entry.clear_fmt(); // Required: clear cached repr so new value serializes

        let result = doc.to_string();
        assert!(result.contains("browser"), "Should contain 'browser': {}", result);
    }

    #[test]
    fn test_apply_config_edits_preserves_unchanged() {
        let kdl = r#"root "/archive"

// Important comment
opinions {
    startup {
        default-view "health"
    }
}
"#;
        let config = parse_kdl_config(kdl).unwrap();
        // Apply with no changes — should round-trip
        let result = apply_config_edits_to_kdl(kdl, &config, &config).unwrap();
        // Re-parse the result and verify it's equivalent
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.root, config.root);
        assert_eq!(reparsed.opinions.startup.default_view, config.opinions.startup.default_view);
    }

    #[test]
    fn test_apply_config_edits_changes_single_field() {
        let kdl = r#"root "/archive"

opinions {
    startup {
        default-view "health"
    }
}
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.startup.default_view = StartupView::Browser;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        // Verify the KDL text contains "browser"
        assert!(result.contains("browser"), "Result KDL should contain 'browser': {}", result);
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.startup.default_view, StartupView::Browser);
        // Root should be preserved
        assert_eq!(reparsed.root, PathBuf::from("/archive"));
    }

    #[test]
    fn test_apply_config_edits_inserts_new_block() {
        let kdl = r#"root "/archive"
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.quality_resolution.inbox_bitrate_fuzz_percent = 3.0;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.quality_resolution.inbox_bitrate_fuzz_percent, 3.0);
    }

    #[test]
    fn test_apply_config_edits_bool_toggle() {
        let kdl = r#"root "/archive"

opinions {
    duplicate-analysis {
        elide-variant-titles true
    }
}
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.duplicate_analysis.elide_variant_titles = false;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert!(!reparsed.opinions.duplicate_analysis.elide_variant_titles);
    }

    #[test]
    fn test_apply_config_edits_performance() {
        let kdl = r#"root "/archive"
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config.opinions.performance.worker_threads = Some(12);
        new_config.opinions.performance.db_cache_mb = 64;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.performance.worker_threads, Some(12));
        assert_eq!(reparsed.opinions.performance.db_cache_mb, 64);

        // Verify reverting worker_threads to auto removes the node
        let mut reverted = new_config.clone();
        reverted.opinions.performance.worker_threads = None;
        let result2 = apply_config_edits_to_kdl(&result, &new_config, &reverted).unwrap();
        let reparsed2 = parse_kdl_config(&result2).unwrap();
        assert_eq!(reparsed2.opinions.performance.worker_threads, None);
        assert_eq!(reparsed2.opinions.performance.db_cache_mb, 64); // preserved
    }
}
