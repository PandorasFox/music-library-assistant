//! KDL write-back: apply_config_edits_to_kdl, write_config_to_disk, KDL node helpers.

use super::types::*;
use anyhow::{Context, Result};
use mm_utils::get_config_dir;
use std::fs;

/// Apply config edits to a KDL document in place, preserving comments and formatting.
///
/// Parses `original_kdl` into a `KdlDocument`, then for each opinion field that
/// differs between `old_config` and `new_config`, modifies only the changed node.
/// Returns the modified KDL text.
pub fn apply_config_edits_to_kdl(
    original_kdl: &str,
    old_config: &Config,
    new_config: &Config,
) -> Result<String> {
    let mut doc: kdl::KdlDocument = original_kdl
        .parse()
        .context("Failed to parse original KDL for edit")?;

    // Ensure "opinions" block exists
    if doc.get("opinions").is_none() {
        let mut node = kdl::KdlNode::new("opinions");
        node.set_children(kdl::KdlDocument::new());
        doc.nodes_mut().push(node);
    }

    let opinions_node = doc
        .get_mut("opinions")
        .context("'opinions' block should exist after creation")?;
    let opinions_doc = opinions_node.ensure_children();

    // --- Startup ---
    let old_s = &old_config.opinions.startup;
    let new_s = &new_config.opinions.startup;
    if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup
        || new_s.vacuum_threshold != old_s.vacuum_threshold
        || new_s.default_view != old_s.default_view
    {
        let startup = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_STARTUP);
        if new_s.force_check_all_files_at_startup != old_s.force_check_all_files_at_startup {
            set_or_create_bool_node(
                startup,
                StartupOpinions::KDL_FORCE_CHECK,
                new_s.force_check_all_files_at_startup,
            );
        }
        if new_s.vacuum_threshold != old_s.vacuum_threshold {
            set_or_create_float_node(
                startup,
                StartupOpinions::KDL_VACUUM_THRESHOLD,
                new_s.vacuum_threshold,
            );
        }
        if new_s.default_view != old_s.default_view {
            let view_str = match new_s.default_view {
                StartupView::Health => "health",
                StartupView::Search => "search",
                StartupView::Browser => "browser",
                StartupView::ExternalMatches => "external-matches",
            };
            set_or_create_string_node(startup, StartupOpinions::KDL_DEFAULT_VIEW, view_str);
        }
    }

    // --- Canonicalization ---
    if new_config
        .opinions
        .canonicalization
        .strip_album_format_suffixes
        != old_config
            .opinions
            .canonicalization
            .strip_album_format_suffixes
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_CANONICALIZATION);
        set_or_create_bool_node(
            block,
            CanonicalizationOpinions::KDL_STRIP_SUFFIXES,
            new_config
                .opinions
                .canonicalization
                .strip_album_format_suffixes,
        );
    }

    // --- Health Detection ---
    let old_hd = &old_config.opinions.health_detection;
    let new_hd = &new_config.opinions.health_detection;
    if new_hd.required_tags != old_hd.required_tags
        || new_hd.album_artist_only_required_if_compilation
            != old_hd.album_artist_only_required_if_compilation
        || new_hd.single_album_suffix != old_hd.single_album_suffix
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_HEALTH_DETECTION);
        if new_hd.required_tags != old_hd.required_tags {
            // Remove old node and create new one with all tag values
            block
                .nodes_mut()
                .retain(|n| n.name().value() != HealthDetectionOpinions::KDL_REQUIRED_TAGS);
            let mut node = kdl::KdlNode::new(HealthDetectionOpinions::KDL_REQUIRED_TAGS);
            for tag in &new_hd.required_tags {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(tag.clone())));
            }
            block.nodes_mut().push(node);
        }
        if new_hd.album_artist_only_required_if_compilation
            != old_hd.album_artist_only_required_if_compilation
        {
            set_or_create_bool_node(
                block,
                HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION,
                new_hd.album_artist_only_required_if_compilation,
            );
        }
        if new_hd.single_album_suffix != old_hd.single_album_suffix {
            set_or_create_string_node(
                block,
                HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX,
                &new_hd.single_album_suffix,
            );
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
            set_or_create_float_node(
                block,
                DuplicateAnalysisOpinions::KDL_FP_THRESHOLD,
                new_da.fingerprint_similarity_threshold,
            );
        }
        if new_da.duration_tolerance_ms != old_da.duration_tolerance_ms {
            set_or_create_int_node(
                block,
                DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE,
                new_da.duration_tolerance_ms,
            );
        }
        if new_da.elide_variant_titles != old_da.elide_variant_titles {
            set_or_create_bool_node(
                block,
                DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS,
                new_da.elide_variant_titles,
            );
        }
    }

    // --- Release Packing ---
    let old_rp = &old_config.opinions.release_packing;
    let new_rp = &new_config.opinions.release_packing;
    if old_rp != new_rp {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_RELEASE_PACKING);
        if (new_rp.duration_tolerance_pct - old_rp.duration_tolerance_pct).abs() > f64::EPSILON {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_DURATION_TOLERANCE_PCT,
                new_rp.duration_tolerance_pct,
            );
        }
        if (new_rp.min_confidence - old_rp.min_confidence).abs() > f64::EPSILON {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_MIN_CONFIDENCE,
                new_rp.min_confidence,
            );
        }
        if new_rp.candidate_weights != old_rp.candidate_weights {
            serialize_packing_weights(
                block,
                ReleasePackingOpinions::KDL_CANDIDATE_WEIGHTS,
                &new_rp.candidate_weights,
            );
        }
        if new_rp.elimination_weights != old_rp.elimination_weights {
            serialize_packing_weights(
                block,
                ReleasePackingOpinions::KDL_ELIMINATION_WEIGHTS,
                &new_rp.elimination_weights,
            );
        }
        if (new_rp.title_preassign_threshold - old_rp.title_preassign_threshold).abs()
            > f64::EPSILON
        {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_TITLE_PREASSIGN_THRESHOLD,
                new_rp.title_preassign_threshold,
            );
        }
        if new_rp.packing_knot_ratio != old_rp.packing_knot_ratio {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_PACKING_KNOT_RATIO,
                new_rp.packing_knot_ratio,
            );
        }
        if new_rp.packing_knot_size_limit != old_rp.packing_knot_size_limit {
            set_or_create_int_node(
                block,
                ReleasePackingOpinions::KDL_PACKING_KNOT_SIZE_LIMIT,
                new_rp.packing_knot_size_limit as i64,
            );
        }
        if new_rp.singles_before_incompletes != old_rp.singles_before_incompletes {
            set_or_create_bool_node(
                block,
                ReleasePackingOpinions::KDL_SINGLES_BEFORE_INCOMPLETES,
                new_rp.singles_before_incompletes,
            );
        }
        if new_rp.allow_resolve_knots_with_discographies
            != old_rp.allow_resolve_knots_with_discographies
        {
            set_or_create_bool_node(
                block,
                ReleasePackingOpinions::KDL_ALLOW_DISCOGRAPHY_REDUCTION,
                new_rp.allow_resolve_knots_with_discographies,
            );
        }
        if (new_rp.low_confidence_max_acoustid_ratio - old_rp.low_confidence_max_acoustid_ratio)
            .abs()
            > f64::EPSILON
        {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ACOUSTID_RATIO,
                new_rp.low_confidence_max_acoustid_ratio,
            );
        }
        if (new_rp.low_confidence_max_album_match - old_rp.low_confidence_max_album_match).abs()
            > f64::EPSILON
        {
            set_or_create_float_node(
                block,
                ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ALBUM_MATCH,
                new_rp.low_confidence_max_album_match,
            );
        }
    }

    // --- Leave Transactions Open ---
    if new_config.opinions.leave_transactions_open != old_config.opinions.leave_transactions_open {
        set_or_create_bool_node(
            opinions_doc,
            Opinions::KDL_LEAVE_TXN_OPEN,
            new_config.opinions.leave_transactions_open,
        );
    }

    // --- Watcher Poll Interval ---
    if new_config.opinions.watcher_poll_interval_secs
        != old_config.opinions.watcher_poll_interval_secs
    {
        set_or_create_int_node(
            opinions_doc,
            Opinions::KDL_WATCHER_POLL_INTERVAL,
            new_config.opinions.watcher_poll_interval_secs as i64,
        );
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
            block
                .nodes_mut()
                .retain(|n| n.name().value() != TagSplittingOpinions::KDL_COLLAB);
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
            block
                .nodes_mut()
                .retain(|n| n.name().value() == TagSplittingOpinions::KDL_COLLAB);
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
        || new_em.auto_enrich_on_match != old_em.auto_enrich_on_match
        || new_em.mb_cache_ttl_days != old_em.mb_cache_ttl_days
        || new_em.preferred_locales != old_em.preferred_locales
        || new_em.cover_art_sanctity != old_em.cover_art_sanctity
        || new_em.cover_art_types != old_em.cover_art_types
    {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_EXTERNAL_MATCHING);
        if new_em.acoustid_api_key != old_em.acoustid_api_key {
            set_or_create_string_node(
                block,
                ExternalMatchingConfig::KDL_ACOUSTID_KEY,
                &new_em.acoustid_api_key,
            );
        }
        if new_em.requests_per_second != old_em.requests_per_second {
            set_or_create_int_node(
                block,
                ExternalMatchingConfig::KDL_REQ_PER_SEC,
                new_em.requests_per_second as i64,
            );
        }
        if new_em.mb_requests_per_second != old_em.mb_requests_per_second {
            set_or_create_int_node(
                block,
                ExternalMatchingConfig::KDL_MB_REQ_PER_SEC,
                new_em.mb_requests_per_second as i64,
            );
        }
        if new_em.mb_base_url != old_em.mb_base_url {
            set_or_create_string_node(
                block,
                ExternalMatchingConfig::KDL_MB_BASE_URL,
                &new_em.mb_base_url,
            );
        }
        if new_em.auto_enrich_on_match != old_em.auto_enrich_on_match {
            set_or_create_bool_node(
                block,
                ExternalMatchingConfig::KDL_AUTO_ENRICH,
                new_em.auto_enrich_on_match,
            );
        }
        if new_em.mb_cache_ttl_days != old_em.mb_cache_ttl_days {
            set_or_create_int_node(
                block,
                ExternalMatchingConfig::KDL_MB_CACHE_TTL,
                new_em.mb_cache_ttl_days as i64,
            );
        }
        if new_em.preferred_locales != old_em.preferred_locales {
            // Remove old node and create new one with all locale values
            block
                .nodes_mut()
                .retain(|n| n.name().value() != ExternalMatchingConfig::KDL_PREFERRED_LOCALES);
            let mut node = kdl::KdlNode::new(ExternalMatchingConfig::KDL_PREFERRED_LOCALES);
            for locale in &new_em.preferred_locales {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(locale.clone())));
            }
            block.nodes_mut().push(node);
        }
        if new_em.cover_art_sanctity != old_em.cover_art_sanctity {
            set_or_create_string_node(
                block,
                ExternalMatchingConfig::KDL_COVER_ART_SANCTITY,
                new_em.cover_art_sanctity.as_str(),
            );
        }
        if new_em.cover_art_types != old_em.cover_art_types {
            block
                .nodes_mut()
                .retain(|n| n.name().value() != ExternalMatchingConfig::KDL_COVER_ART_TYPES);
            let mut node = kdl::KdlNode::new(ExternalMatchingConfig::KDL_COVER_ART_TYPES);
            for art_type in &new_em.cover_art_types {
                node.push(kdl::KdlEntry::new(kdl::KdlValue::String(art_type.clone())));
            }
            block.nodes_mut().push(node);
        }
    }

    // --- MB Tag Names ---
    let old_tn = &old_config.opinions.external_matching.mb_tag_names;
    let new_tn = &new_config.opinions.external_matching.mb_tag_names;
    if new_tn != old_tn {
        let em_block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_EXTERNAL_MATCHING);
        let tn_block = ensure_child_block(em_block, MbTagNameConfig::KDL_MB_TAG_NAMES);
        if new_tn.recording != old_tn.recording {
            set_or_create_string_node(tn_block, MbTagNameConfig::KDL_RECORDING, &new_tn.recording);
        }
        if new_tn.release != old_tn.release {
            set_or_create_string_node(tn_block, MbTagNameConfig::KDL_RELEASE, &new_tn.release);
        }
        if new_tn.track != old_tn.track {
            set_or_create_string_node(tn_block, MbTagNameConfig::KDL_TRACK, &new_tn.track);
        }
        if new_tn.picard_compat != old_tn.picard_compat {
            set_or_create_bool_node(tn_block, MbTagNameConfig::KDL_PICARD_COMPAT, new_tn.picard_compat);
        }
    }

    // --- Credit Routing ---
    let old_cr = &old_config.opinions.external_matching.credit_routing;
    let new_cr = &new_config.opinions.external_matching.credit_routing;
    if new_cr.feat_format != old_cr.feat_format
        || new_cr.max_feat_credits != old_cr.max_feat_credits
        || new_cr.routing != old_cr.routing
    {
        let em_block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_EXTERNAL_MATCHING);
        let cr_block =
            ensure_child_block(em_block, CreditRoutingConfig::KDL_CREDIT_ROUTING);
        if new_cr.feat_format != old_cr.feat_format {
            set_or_create_string_node(
                cr_block,
                CreditRoutingConfig::KDL_FEAT_FORMAT,
                &new_cr.feat_format,
            );
        }
        if new_cr.max_feat_credits != old_cr.max_feat_credits {
            match new_cr.max_feat_credits {
                Some(n) => {
                    set_or_create_int_node(
                        cr_block,
                        CreditRoutingConfig::KDL_MAX_FEAT_CREDITS,
                        n as i64,
                    );
                }
                None => {
                    cr_block
                        .nodes_mut()
                        .retain(|n| n.name().value() != CreditRoutingConfig::KDL_MAX_FEAT_CREDITS);
                }
            }
        }
        if new_cr.routing != old_cr.routing {
            // Remove old relation type nodes (everything that isn't feat-format or max-feat-credits)
            cr_block.nodes_mut().retain(|n| {
                let name = n.name().value();
                name == CreditRoutingConfig::KDL_FEAT_FORMAT
                    || name == CreditRoutingConfig::KDL_MAX_FEAT_CREDITS
            });
            // Write new routing entries
            let mut sorted_keys: Vec<&String> = new_cr.routing.keys().collect();
            sorted_keys.sort();
            for key in sorted_keys {
                let route = &new_cr.routing[key];
                let mut node = kdl::KdlNode::new(key.as_str());
                node.push(kdl::KdlEntry::new_prop("artist", route.artist));
                node.push(kdl::KdlEntry::new_prop("title", route.title));
                node.push(kdl::KdlEntry::new_prop("composer", route.composer));
                cr_block.nodes_mut().push(node);
            }
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
            set_or_create_string_node(
                block,
                DiscExtractionOpinions::KDL_DISC_TAG_NAME,
                &new_de.disc_tag_name,
            );
        }
        if new_de.map_letters_to_numbers != old_de.map_letters_to_numbers {
            set_or_create_bool_node(
                block,
                DiscExtractionOpinions::KDL_MAP_LETTERS,
                new_de.map_letters_to_numbers,
            );
        }
    }

    // --- Performance ---
    let old_p = &old_config.opinions.performance;
    let new_p = &new_config.opinions.performance;
    if new_p.worker_threads != old_p.worker_threads || new_p.db_cache_mb != old_p.db_cache_mb {
        let block = ensure_child_block(opinions_doc, Opinions::KDL_BLOCK_PERFORMANCE);
        if new_p.worker_threads != old_p.worker_threads {
            match new_p.worker_threads {
                Some(n) => {
                    set_or_create_int_node(block, PerformanceOpinions::KDL_WORKER_THREADS, n as i64)
                }
                None => {
                    block
                        .nodes_mut()
                        .retain(|n| n.name().value() != PerformanceOpinions::KDL_WORKER_THREADS);
                }
            }
        }
        if new_p.db_cache_mb != old_p.db_cache_mb {
            set_or_create_int_node(
                block,
                PerformanceOpinions::KDL_DB_CACHE,
                new_p.db_cache_mb as i64,
            );
        }
    }

    Ok(doc.to_string())
}

/// Write config edits to disk with comment-preserving KDL modification.
///
/// 1. Backs up existing config.kdl -> config.kdl.bak
/// 2. Applies edits to original KDL text
/// 3. Writes modified KDL to config.kdl
pub fn write_config_to_disk(
    original_kdl: &str,
    old_config: &Config,
    new_config: &Config,
) -> Result<()> {
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

/// Serialize a PackingWeights struct into a KDL child block.
fn serialize_packing_weights(doc: &mut kdl::KdlDocument, name: &str, weights: &PackingWeights) {
    let block = ensure_child_block(doc, name);
    set_or_create_float_node(
        block,
        PackingWeights::KDL_ACOUSTID_CONFIDENCE,
        weights.acoustid_confidence,
    );
    set_or_create_float_node(
        block,
        PackingWeights::KDL_DURATION_MATCH,
        weights.duration_match,
    );
    set_or_create_float_node(block, PackingWeights::KDL_TITLE_MATCH, weights.title_match);
    set_or_create_float_node(
        block,
        PackingWeights::KDL_ARTIST_MATCH,
        weights.artist_match,
    );
    set_or_create_float_node(block, PackingWeights::KDL_ALBUM_MATCH, weights.album_match);
    set_or_create_float_node(
        block,
        PackingWeights::KDL_TRACK_NUMBER_MATCH,
        weights.track_number_match,
    );
    // Remove stale directory-cohesion node if present (dimension removed)
    block
        .nodes_mut()
        .retain(|n| n.name().value() != "directory-cohesion");
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
    let child_node = doc
        .get_mut(name)
        .expect("ensure_child_block: node should exist after creation");
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
    use super::super::parse::parse_kdl_config;
    use super::*;
    use std::path::PathBuf;

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
        assert!(
            result.contains("browser"),
            "Should contain 'browser': {}",
            result
        );
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
        assert_eq!(reparsed.storage_root, config.storage_root);
        assert_eq!(
            reparsed.opinions.startup.default_view,
            config.opinions.startup.default_view
        );
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
        assert!(
            result.contains("browser"),
            "Result KDL should contain 'browser': {}",
            result
        );
        let reparsed = parse_kdl_config(&result).unwrap();
        assert_eq!(reparsed.opinions.startup.default_view, StartupView::Browser);
        // Root should be preserved
        assert_eq!(reparsed.storage_root, PathBuf::from("/archive"));
    }

    #[test]
    fn test_apply_config_edits_inserts_new_block() {
        let kdl = r#"root "/archive"
"#;
        let old_config = parse_kdl_config(kdl).unwrap();
        let mut new_config = old_config.clone();
        new_config
            .opinions
            .canonicalization
            .strip_album_format_suffixes = true;

        let result = apply_config_edits_to_kdl(kdl, &old_config, &new_config).unwrap();
        let reparsed = parse_kdl_config(&result).unwrap();
        assert!(
            reparsed
                .opinions
                .canonicalization
                .strip_album_format_suffixes,
        );
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
