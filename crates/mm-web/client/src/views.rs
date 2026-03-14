//! View renderers — each lateral view's content as HTML Node trees.
//!
//! Typed functions accept mm-meta structs so the compiler catches field name
//! mismatches. Functions that remain on `&serde_json::Value` are generic
//! display utilities or use composite responses without a single mm-meta type.

use mm_meta::decisions::DecisionKey;
use mm_meta::views::{
    DeployStatus, EditHistoryData, ExternalMatchesData, InboxOverviewData, InsightsData,
};
use mm_meta::witch_types::{WitchStatus, WorkStateSnapshot};
use mm_ui::html::{self, div, h3, section, span, Node};
use mm_ui::protocol_binding::{DataQuery, ProtocolBinding};

// ============================================================================
// Shared helpers
// ============================================================================

/// Single key-value row.
pub fn kv(key: &str, val: &str) -> Node {
    div()
        .class("mm-kv")
        .child(span().class("mm-kv__key").text(key))
        .child(span().class("mm-kv__val").text(val))
        .into()
}

/// Section with a title and children.
pub fn titled_section(title: &str, items: Vec<Node>) -> Node {
    section()
        .class("mm-section")
        .child(h3().class("mm-section__title").text(title))
        .children(items)
        .into()
}


// ============================================================================
// Health view — typed
// ============================================================================

pub fn render_status_content(status: &WitchStatus) -> Node {
    let mut sections = Vec::new();

    // Work status with progress.
    let work = &status.work;
    let state_str = match work.state {
        WorkStateSnapshot::Idle => "Idle",
        WorkStateSnapshot::Working => "Working",
        WorkStateSnapshot::Done => "Done",
    };

    let mut work_items = Vec::new();
    work_items.push(kv("state", state_str));

    if work.session_queued > 0 {
        let pct = if work.session_queued > 0 {
            (work.total_processed as f64 / work.session_queued as f64 * 100.0) as u64
        } else {
            0
        };
        work_items.push(kv(
            "progress",
            &format!("{}/{} ({}%)", work.total_processed, work.session_queued, pct),
        ));
    }
    if work.pending > 0 {
        work_items.push(kv("pending", &work.pending.to_string()));
    }
    if status.db_queue_depth > 0 {
        work_items.push(kv("DB queue", &status.db_queue_depth.to_string()));
    }
    sections.push(titled_section("Work", work_items));

    // Task breakdown.
    if !work.pending_by_label.is_empty() {
        let items: Vec<Node> = work
            .pending_by_label
            .iter()
            .map(|(k, v)| kv(k, &v.to_string()))
            .collect();
        sections.push(titled_section("Pending Work", items));
    }

    // Transaction.
    if let Some(ref tx) = status.transaction {
        let tx_items = vec![
            kv("Label", &tx.label),
            kv("Decisions", &tx.decision_count.to_string()),
            kv("Mutations", &tx.mutation_count.to_string()),
        ];
        sections.push(titled_section("Transaction", tx_items));
    }

    // External fetch progress.
    if let Some(ref progress) = status.external_fetch_progress {
        let a = &progress.acoustid;
        let m = &progress.mb;
        let mut items = vec![
            kv(
                "AcoustID",
                &format!(
                    "{}/{} ({} matched, {} no match)",
                    a.processed, a.total, a.matched, a.no_match
                ),
            ),
            kv(
                "MusicBrainz",
                &format!(
                    "{}/{} ({} matched, {} no match)",
                    m.processed, m.total, m.matched, m.no_match
                ),
            ),
        ];
        if progress.acoustid_rps > 0.0 || progress.mb_rps > 0.0 {
            items.push(kv(
                "rate",
                &format!("{:.1} aid/s, {:.1} mb/s", progress.acoustid_rps, progress.mb_rps),
            ));
        }
        sections.push(titled_section("External Fetch", items));
    }

    // Last error.
    if let Some(ref err) = status.last_error {
        sections.push(
            div()
                .class("mm-alert mm-alert--warning")
                .child(span().class("mm-alert__text").text(err))
                .into(),
        );
    }

    div().attr("id", "mm-status-section").children(sections).into()
}

pub fn render_insights_content(insights: &InsightsData) -> Node {
    let mut sections = Vec::new();
    let corpus = &insights.bucket_corpus;

    // Intake alert banners.
    if corpus.files_unindexed > 0 {
        let binding = ProtocolBinding::Transaction {
            decision_key: DecisionKey::IntakeIndex,
            label: "Index unindexed files".to_string(),
            data_query: Some(DataQuery::IntakeConfirmation),
        };
        let binding_json = serde_json::to_string(&binding).unwrap();
        sections.push(
            div()
                .class("mm-alert")
                .child(span().class("mm-alert__text").text(
                    format!("{} unindexed files", corpus.files_unindexed),
                ))
                .child(
                    html::button()
                        .class("mm-btn mm-alert__action")
                        .attr("onclick", format!(
                            "window.__mm_execute('{}')",
                            binding_json,
                        ))
                        .text("Index Now"),
                )
                .into(),
        );
    }
    if corpus.files_missing > 0 {
        sections.push(
            div()
                .class("mm-alert mm-alert--warning")
                .child(span().class("mm-alert__text").text(
                    format!("{} missing files", corpus.files_missing),
                ))
                .into(),
        );
    }
    if corpus.corrupt_files > 0 {
        sections.push(
            div()
                .class("mm-alert mm-alert--warning")
                .child(span().class("mm-alert__text").text(
                    format!("{} corrupt files", corpus.corrupt_files),
                ))
                .into(),
        );
    }
    if corpus.files_relocated > 0 {
        sections.push(
            div()
                .class("mm-alert mm-alert--info")
                .child(span().class("mm-alert__text").text(
                    format!("{} relocated files", corpus.files_relocated),
                ))
                .into(),
        );
    }

    // Corpus file stats.
    let mut corpus_items = vec![
        kv("files in corpus", &corpus.files_in_corpus.to_string()),
        kv("indexed", &corpus.files_indexed.to_string()),
        kv("unindexed", &corpus.files_unindexed.to_string()),
    ];
    if corpus.files_missing > 0 {
        corpus_items.push(kv("missing", &corpus.files_missing.to_string()));
    }
    if corpus.directories_missing > 0 {
        corpus_items.push(kv("dirs missing", &corpus.directories_missing.to_string()));
    }
    if corpus.corrupt_files > 0 {
        corpus_items.push(kv("corrupt", &corpus.corrupt_files.to_string()));
    }
    if corpus.shit_format_files > 0 {
        corpus_items.push(kv("non-vorbis", &corpus.shit_format_files.to_string()));
    }
    if corpus.oob_tag_sync > 0 {
        corpus_items.push(kv("OOB tag sync", &corpus.oob_tag_sync.to_string()));
    }
    if corpus.oob_tag_conflict > 0 {
        corpus_items.push(kv("OOB tag conflict", &corpus.oob_tag_conflict.to_string()));
    }
    sections.push(titled_section("Corpus Files", corpus_items));

    // Tag health (placeholder/squash bucket).
    let ph = &insights.bucket_placeholder;
    let mut tag_items = Vec::new();
    if ph.cross_source_overlap_count > 0 {
        tag_items.push(kv("source overlaps", &ph.cross_source_overlap_count.to_string()));
    }
    if ph.release_overlap_count > 0 {
        tag_items.push(kv("release overlaps", &ph.release_overlap_count.to_string()));
    }
    if ph.subpar_duplicate_count > 0 {
        tag_items.push(kv("subpar duplicates", &ph.subpar_duplicate_count.to_string()));
    }
    if ph.redundant_duplicate_count > 0 {
        tag_items.push(kv("redundant duplicates", &ph.redundant_duplicate_count.to_string()));
    }
    for entry in &ph.tag_canonicity {
        tag_items.push(kv(&format!("{} canonicity", entry.tag_name), &entry.cluster_count.to_string()));
    }
    if ph.inconsistent_album_artist_count > 0 {
        tag_items.push(kv("album artist issues", &ph.inconsistent_album_artist_count.to_string()));
    }
    for entry in &ph.compound_tags {
        let total = entry.safe_count + entry.review_count;
        tag_items.push(kv(&format!("{} compound", entry.tag_name), &total.to_string()));
    }
    if ph.missing_album_single_count > 0 {
        tag_items.push(kv("missing album singles", &ph.missing_album_single_count.to_string()));
    }
    if ph.disc_extraction_count > 0 {
        tag_items.push(kv("disc extraction", &ph.disc_extraction_count.to_string()));
    }
    if ph.path_tag_mismatch_count > 0 {
        tag_items.push(kv("path/tag mismatch", &ph.path_tag_mismatch_count.to_string()));
    }
    if !tag_items.is_empty() {
        sections.push(titled_section("Tag Health", tag_items));
    }

    // Other signals.
    let other_items: Vec<Node> = insights
        .bucket_other
        .entries
        .iter()
        .filter(|e| e.count > 0)
        .map(|e| kv(&e.display_label, &e.count.to_string()))
        .collect();
    if !other_items.is_empty() {
        sections.push(titled_section("Signals", other_items));
    }

    if sections.is_empty() {
        span().class("mm-kv__val").text("No insights data").into()
    } else {
        div().attr("id", "mm-insights-section").children(sections).into()
    }
}

// ============================================================================
// External Matches view — typed
// ============================================================================

pub fn render_external_matches_content(data: &ExternalMatchesData) -> Node {
    let mut sections = Vec::new();

    let packing_items = vec![
        kv("Perfect", &data.packing_perfect_count.to_string()),
        kv("Full match", &data.packing_full_match_count.to_string()),
        kv("Singles", &data.packing_singles_count.to_string()),
        kv("Incomplete", &data.packing_incomplete_count.to_string()),
        kv("Low confidence", &data.packing_low_confidence_count.to_string()),
        kv("Knots", &data.packing_knots_count.to_string()),
    ];
    sections.push(titled_section("Release Packing", packing_items));

    let unsolved_fields = [
        (data.unsolved_conflict_count, "Conflicts"),
        (data.unsolved_no_release_count, "No release"),
        (data.unsolved_no_match_count, "No match"),
        (data.va_override_count, "VA overrides"),
        (data.pinned_conflict_count, "Pinned conflicts"),
    ];
    let unsolved_items: Vec<Node> = unsolved_fields
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| kv(label, &n.to_string()))
        .collect();
    if !unsolved_items.is_empty() {
        sections.push(titled_section("Unsolved", unsolved_items));
    }

    let bucket_items: Vec<Node> = data
        .confidence_buckets
        .iter()
        .map(|b| kv(&format!("{:?}", b.tier), &b.total.to_string()))
        .collect();
    if !bucket_items.is_empty() {
        sections.push(titled_section("Confidence Tiers", bucket_items));
    }

    div().children(sections).into()
}

// ============================================================================
// Edit History view — typed
// ============================================================================

pub fn render_edit_history_content(data: &EditHistoryData) -> Node {
    if data.sessions.is_empty() {
        return span().class("mm-kv__val").text("No edit sessions").into();
    }

    let items: Vec<Node> = data
        .sessions
        .iter()
        .map(|s| {
            div()
                .class("mm-history-session")
                .child(
                    div()
                        .class("mm-kv")
                        .child(
                            html::a()
                                .class("mm-link")
                                .attr("href", "#")
                                .attr(
                                    "onclick",
                                    format!(
                                        "event.preventDefault();window.__mm_expand_session('{}')",
                                        s.session_id
                                    ),
                                )
                                .text(&s.session_id),
                        )
                        .child(span().class("mm-kv__val").text(format!(
                            "{} edits, {} files — {}",
                            s.edit_count, s.inode_count, s.earliest_at
                        ))),
                )
                .child(
                    div()
                        .attr("id", format!("session-{}", s.session_id))
                        .class("mm-session-detail"),
                )
                .into()
        })
        .collect();
    titled_section("Edit Sessions", items)
}

/// Session detail remains untyped (lazy-loaded via separate fetch).
pub fn render_session_detail(detail: &serde_json::Value) -> Node {
    let edits = match detail.get("edits").and_then(|v| v.as_array()) {
        Some(e) => e,
        None => return span().class("mm-kv__val").text("No edits").into(),
    };
    let inode_paths = detail
        .get("inode_paths")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let rows: Vec<Node> = edits
        .iter()
        .filter_map(|edit| {
            let inode = edit.get("inode")?.as_i64()?;
            let field = edit.get("field_name")?.as_str()?;
            let old = edit.get("old_value").and_then(|v| v.as_str()).unwrap_or("∅");
            let new = edit.get("new_value").and_then(|v| v.as_str()).unwrap_or("∅");
            let path = inode_paths
                .get(&inode.to_string())
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            Some(
                div()
                    .class("mm-edit-row")
                    .child(span().class("mm-edit-path").text(path))
                    .child(span().class("mm-edit-field").text(field))
                    .child(
                        span()
                            .class("mm-edit-diff")
                            .child(span().class("mm-edit-old").text(old))
                            .child(span().class("mm-edit-arrow").text(" → "))
                            .child(span().class("mm-edit-new").text(new)),
                    )
                    .into(),
            )
        })
        .collect();
    div().class("mm-session-edits").children(rows).into()
}

// ============================================================================
// Inbox view — typed
// ============================================================================

pub fn render_inbox_content(data: &InboxOverviewData) -> Node {
    let items = vec![
        kv("Files in inbox", &data.file_in_inbox.to_string()),
        kv("Unindexed", &data.unindexed.to_string()),
        kv("Corpus matches", &data.corpus_match.to_string()),
        kv("Organizable", &data.organizable.to_string()),
        kv("Tag canonicity", &data.tag_canonicity.to_string()),
        kv("Missing tags", &data.missing_tags.to_string()),
        kv("Compound tags", &data.compound_tags.to_string()),
    ];
    titled_section("Inbox", items)
}

// ============================================================================
// Deploy view — typed
// ============================================================================

pub fn render_deploy_content(data: &DeployStatus) -> Node {
    let mut items = vec![kv(
        "Needs action",
        if data.needs_action { "yes" } else { "no" },
    )];
    for (name, count) in &data.library_file_counts {
        items.push(kv(name, &count.to_string()));
    }
    titled_section("Deploy Status", items)
}

// ============================================================================
// Transaction view — partially typed
// ============================================================================

pub fn render_transaction_content(
    status: &WitchStatus,
    details: Option<&serde_json::Value>,
) -> Node {
    let tx = match status.transaction {
        Some(ref tx) => tx,
        None => {
            return section()
                .class("mm-section")
                .child(h3().class("mm-section__title").text("Transaction"))
                .child(span().class("mm-kv__val").text("No active transaction"))
                .into()
        }
    };

    let mut sections = Vec::new();

    let mut summary = vec![
        kv("Label", &tx.label),
        kv("Decisions", &tx.decision_count.to_string()),
        kv("Mutations", &tx.mutation_count.to_string()),
    ];
    summary.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_tx_confirm()")
                    .text("Confirm"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-red)")
                    .attr("onclick", "window.__mm_tx_discard()")
                    .text("Discard"),
            )
            .into(),
    );
    sections.push(titled_section("Active Transaction", summary));

    if let Some(detail_arr) = details.and_then(|d| d.as_array()) {
        let decision_items: Vec<Node> = detail_arr
            .iter()
            .filter_map(|d| {
                let label = d.get("label")?.as_str().unwrap_or("?");
                let key = d.get("key")?;
                let key_str = serde_json::to_string(key).unwrap_or_default();
                let mutations = d.get("mutations").and_then(|m| m.as_array()).map_or(0, |a| a.len());
                Some(kv(label, &format!("{mutations} mutations — {key_str}")))
            })
            .collect();
        if !decision_items.is_empty() {
            sections.push(titled_section("Decisions", decision_items));
        }
    }
    div().children(sections).into()
}

// ============================================================================
// Config editor view (stays on serde_json::Value — generic field renderer)
// ============================================================================

pub fn render_config_editor(config: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    // Root-level fields.
    let mut root_items = Vec::new();
    if let Some(root) = config.get("root").and_then(|v| v.as_str()) {
        root_items.push(config_field_readonly("root", root));
    }
    if let Some(legacy) = config.get("legacy_enabled") {
        root_items.push(config_field_bool("legacy_enabled", legacy.as_bool().unwrap_or(false)));
    }
    sections.push(titled_section("General", root_items));

    // Source directories.
    if let Some(dirs) = config.get("source_dirs").and_then(|v| v.as_array()) {
        for (i, dir) in dirs.iter().enumerate() {
            let path = dir.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let libs = dir
                .get("libraries")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let mut items = vec![
                config_field_readonly("path", path),
                config_field_readonly("libraries", &libs),
            ];
            if let Some(schema) = dir.get("path_schema").and_then(|v| v.as_str()) {
                items.push(config_field_readonly("path schema", schema));
            }
            sections.push(titled_section(&format!("Source Directory {}", i + 1), items));
        }
    }

    // Opinions — render each sub-block as a form section.
    if let Some(opinions) = config.get("opinions").and_then(|v| v.as_object()) {
        let mut top_items = Vec::new();
        for (key, val) in opinions {
            if val.is_object() {
                continue;
            }
            top_items.push(config_field(key, val));
        }
        if !top_items.is_empty() {
            sections.push(titled_section("Opinions", top_items));
        }

        let block_labels = [
            ("quality_resolution", "Quality Resolution"),
            ("canonicalization", "Canonicalization"),
            ("startup", "Startup"),
            ("health_detection", "Health Detection"),
            ("performance", "Performance"),
            ("tag_splitting", "Tag Splitting"),
            ("duplicate_analysis", "Duplicate Analysis"),
            ("release_packing", "Release Packing"),
            ("inbox_organize", "Inbox Organize"),
            ("external_matching", "External Matching"),
            ("disc_extraction", "Disc Extraction"),
            ("album_art", "Album Art"),
        ];
        for (key, label) in block_labels {
            if let Some(block) = opinions.get(key).and_then(|v| v.as_object()) {
                let items: Vec<Node> = block
                    .iter()
                    .map(|(k, v)| config_field(k, v))
                    .collect();
                if !items.is_empty() {
                    sections.push(titled_section(label, items));
                }
            }
        }
    }

    // Save button.
    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_config_save()")
                    .text("Save"),
            )
            .into(),
    );

    div().class("mm-config-editor").children(sections).into()
}

fn config_field(key: &str, val: &serde_json::Value) -> Node {
    let display_key = key.replace('_', " ");
    match val {
        serde_json::Value::Bool(b) => config_field_bool(key, *b),
        serde_json::Value::Number(n) => {
            let field_id = format!("cfg-{key}");
            div()
                .class("mm-config-field")
                .child(html::label().attr("for", &field_id).text(&display_key))
                .child(
                    html::input()
                        .attr("type", "number")
                        .attr("id", &field_id)
                        .attr("name", key)
                        .attr("value", n.to_string())
                        .attr("step", "any")
                        .class("mm-config-input"),
                )
                .into()
        }
        serde_json::Value::String(s) => {
            let field_id = format!("cfg-{key}");
            div()
                .class("mm-config-field")
                .child(html::label().attr("for", &field_id).text(&display_key))
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", &field_id)
                        .attr("name", key)
                        .attr("value", s)
                        .class("mm-config-input"),
                )
                .into()
        }
        serde_json::Value::Null => {
            div()
                .class("mm-config-field")
                .child(html::label().text(&display_key))
                .child(span().class("mm-kv__val").text("—"))
                .into()
        }
        serde_json::Value::Array(arr) => {
            // Only render as editable if all elements are strings (flat list).
            // Arrays of tuples/objects can't round-trip through a text input.
            let all_strings = arr.iter().all(|v| v.is_string());
            if all_strings {
                let vals = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ");
                let field_id = format!("cfg-{key}");
                div()
                    .class("mm-config-field")
                    .child(html::label().attr("for", &field_id).text(&display_key))
                    .child(
                        html::input()
                            .attr("type", "text")
                            .attr("id", &field_id)
                            .attr("name", key)
                            .attr("value", vals)
                            .attr("placeholder", "comma-separated")
                            .class("mm-config-input"),
                    )
                    .into()
            } else {
                // Non-string array — show read-only summary
                config_field_readonly(&display_key, &format!("[{} items]", arr.len()))
            }
        }
        serde_json::Value::Object(obj) => {
            // Render nested objects read-only to avoid save clobbering
            // complex structures (HashMaps, nested configs).
            let summary: Vec<String> = obj
                .iter()
                .take(3)
                .map(|(k, _)| k.clone())
                .collect();
            let label = if obj.len() > 3 {
                format!("{{{}, ... +{}}}", summary.join(", "), obj.len() - 3)
            } else {
                format!("{{{}}}", summary.join(", "))
            };
            config_field_readonly(&display_key, &label)
        }
    }
}

fn config_field_bool(key: &str, val: bool) -> Node {
    let display_key = key.replace('_', " ");
    let field_id = format!("cfg-{key}");
    div()
        .class("mm-config-field mm-config-field--bool")
        .child(
            html::input()
                .attr("type", "checkbox")
                .attr("id", &field_id)
                .attr("name", key)
                .bool_attr_if("checked", val)
                .class("mm-config-checkbox"),
        )
        .child(html::label().attr("for", &field_id).text(&display_key))
        .into()
}

fn config_field_readonly(key: &str, val: &str) -> Node {
    div()
        .class("mm-config-field")
        .child(html::label().text(key))
        .child(span().class("mm-kv__val").text(val))
        .into()
}

// ============================================================================
// Packing browser view (stays on serde_json::Value)
// ============================================================================

pub fn render_packing_overview(ext_data: &ExternalMatchesData) -> Node {
    let categories = [
        ("perfect", "Perfect", ext_data.packing_perfect_count),
        ("full_match", "Full Match", ext_data.packing_full_match_count),
        ("single", "Singles", ext_data.packing_singles_count),
        ("incomplete", "Incomplete", ext_data.packing_incomplete_count),
        ("low_confidence", "Low Confidence", ext_data.packing_low_confidence_count),
    ];

    let items: Vec<Node> = categories
        .iter()
        .map(|(prefix, label, count)| {
            div()
                .class("mm-kv")
                .child(
                    html::a()
                        .class("mm-link")
                        .attr("href", "#")
                        .attr(
                            "onclick",
                            format!("event.preventDefault();window.__mm_packing_browse('{prefix}')"),
                        )
                        .text(&format!("{label} ({count})")),
                )
                .into()
        })
        .collect();

    let mut sections = vec![titled_section("Release Packing Categories", items)];

    if ext_data.packing_knots_count > 0 {
        sections.push(
            div()
                .class("mm-kv")
                .child(span().class("mm-kv__key").text("Knots"))
                .child(span().class("mm-kv__val").text(ext_data.packing_knots_count.to_string()))
                .into(),
        );
    }

    div().children(sections).into()
}

pub fn render_packing_browser_data(category: &str, data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    sections.push(
        div()
            .child(
                html::a()
                    .class("mm-link")
                    .attr("href", "#")
                    .attr("onclick", "event.preventDefault();window.__mm_navigate('Ext. Matches')")
                    .text("← Back to categories"),
            )
            .into(),
    );

    if let Some(packed) = data.get("packed").and_then(|v| v.as_array()) {
        if !packed.is_empty() {
            let items: Vec<Node> = packed
                .iter()
                .filter_map(|r| {
                    let title = r.get("release_title")?.as_str()?;
                    let artist = r.get("release_artist")?.as_str()?;
                    let assigned = r.get("assigned_count")?.as_u64().unwrap_or(0);
                    let total = r.get("total_tracks")?.as_u64().unwrap_or(0);
                    Some(
                        div()
                            .class("mm-packing-release")
                            .child(span().class("mm-packing-title").text(&format!("{artist} — {title}")))
                            .child(span().class("mm-packing-count").text(format!("{assigned}/{total}")))
                            .into(),
                    )
                })
                .collect();
            sections.push(titled_section(
                &format!("Packed Releases — {}", category.replace('_', " ")),
                items,
            ));
        }
    }

    if let Some(packing) = data.get("packing").and_then(|v| v.as_array()) {
        if !packing.is_empty() {
            let items: Vec<Node> = packing
                .iter()
                .take(100)
                .filter_map(|entry| {
                    let arr = entry.as_array()?;
                    let _inode = arr.first()?.as_i64()?;
                    let path = arr.get(1)?.as_str()?;
                    let track_data = arr.get(2)?;
                    let release = track_data.get("release_title")?.as_str().unwrap_or("?");
                    let score = track_data.get("score")?.as_f64().unwrap_or(0.0);
                    Some(
                        div()
                            .class("mm-kv")
                            .child(span().class("mm-kv__key").text(path))
                            .child(span().class("mm-kv__val").text(format!("{release} ({score:.1})")))
                            .into(),
                    )
                })
                .collect();
            if !items.is_empty() {
                sections.push(titled_section("Track Assignments", items));
            }
        }
    }

    if let Some(unfilled) = data.get("unfilled").and_then(|v| v.as_array()) {
        if !unfilled.is_empty() {
            let items: Vec<Node> = unfilled
                .iter()
                .take(50)
                .filter_map(|slot| {
                    let title = slot.get("track_title")?.as_str()?;
                    let release = slot.get("release_title")?.as_str()?;
                    let artist = slot.get("release_artist")?.as_str()?;
                    let medium = slot.get("medium_pos")?.as_u64().unwrap_or(0);
                    let track = slot.get("track_pos")?.as_u64().unwrap_or(0);
                    Some(kv(
                        &format!("{artist} — {release}"),
                        &format!("disc {medium} track {track}: {title}"),
                    ))
                })
                .collect();
            sections.push(titled_section(
                &format!("Unfilled Slots ({})", unfilled.len()),
                items,
            ));
        }
    }

    div().children(sections).into()
}

// ============================================================================
// Search view — server-side search, no upfront data load
// ============================================================================

/// Render the search view shell: search input + empty results container.
/// Data is fetched on demand via debounced `mm_search` calls.
pub fn render_search_view() -> Node {
    div()
        .child(
            div()
                .class("mm-search-bar")
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", "mm-search-input")
                        .attr("placeholder", "Search corpus files...")
                        .attr("oninput", "window.__mm_search(this.value)")
                        .attr("autocomplete", "off")
                        .class("mm-search-input"),
                ),
        )
        .child(div().attr("id", "mm-search-results"))
        .into()
}

/// Render search results from server response (Vec<SearchResult> as JSON).
pub fn render_search_results(data: &serde_json::Value) -> Node {
    let results = match data.as_array() {
        Some(a) => a,
        None => return span().class("mm-kv__val").text("No results").into(),
    };

    if results.is_empty() {
        return span().class("mm-kv__val").text("No results").into();
    }

    let mut rows = Vec::new();
    for entry in results {
        let inode = entry.get("inode").and_then(|v| v.as_i64()).unwrap_or(0);
        let path = entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let artist = entry.get("artist").and_then(|v| v.as_str()).unwrap_or("");
        let album = entry.get("album").and_then(|v| v.as_str()).unwrap_or("");
        let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");

        let tag_line = [artist, album, title]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" / ");

        rows.push(
            div()
                .class("mm-file-row")
                .child(
                    html::a()
                        .class("mm-link mm-file-row__path")
                        .attr("href", format!("#tags/{inode}"))
                        .text(path),
                )
                .child(span().class("mm-file-row__tags").text(&tag_line))
                .into(),
        );
    }

    let count = rows.len();
    let mut container = div();
    container = container.child(
        span().class("mm-kv__val").text(format!("{count} results")),
    );
    container = container.children(rows);
    container.into()
}

// ============================================================================
// Files view — lazy directory browser
// ============================================================================

/// Render the files view from a directory listing (Vec<DirectoryListingEntry> as JSON).
/// Initial call shows top-level directories; clicking expands via `mm_expand_dir`.
pub fn render_files_view(data: &serde_json::Value) -> Node {
    let entries = match data.as_array() {
        Some(a) => a,
        None => return span().class("mm-kv__val").text("No files loaded").into(),
    };

    let sections: Vec<Node> = entries
        .iter()
        .filter_map(|entry| render_listing_entry(entry))
        .collect();

    div().children(sections).into()
}

/// Render children fetched by `mm_expand_dir` into a container.
pub fn render_directory_children(data: &serde_json::Value) -> Node {
    let entries = match data.as_array() {
        Some(a) => a,
        None => return span().class("mm-kv__val").text("Error loading directory").into(),
    };

    let children: Vec<Node> = entries
        .iter()
        .filter_map(|entry| render_listing_entry(entry))
        .collect();

    div().children(children).into()
}

/// Render a single DirectoryListingEntry (directory or file).
fn render_listing_entry(entry: &serde_json::Value) -> Option<Node> {
    let is_dir = entry.get("is_dir")?.as_bool()?;
    let name = entry.get("name")?.as_str()?;
    let path = entry.get("path")?.as_str()?;

    if is_dir {
        let file_count = entry.get("file_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let dir_id = format!("mm-dir-{}", simple_hash(path));
        // Escape single quotes in path for the onclick JS string.
        let escaped_path = path.replace('\'', "\\'");

        let header = div()
            .class("mm-dir-header")
            .attr(
                "onclick",
                format!("window.__mm_expand_dir('{escaped_path}')"),
            )
            .child(span().text(format!("{name} ({file_count} files)")));

        let files_container = div()
            .class("mm-dir-files")
            .attr("id", &dir_id)
            .attr("style", "display:none");

        Some(
            div()
                .class("mm-dir-group")
                .child(header)
                .child(files_container)
                .into(),
        )
    } else {
        let inode = entry.get("inode").and_then(|v| v.as_i64()).unwrap_or(0);
        let duration_ms = entry.get("duration_ms").and_then(|v| v.as_i64());
        let bitrate = entry.get("bitrate_kbps").and_then(|v| v.as_i64());

        let mut meta_parts = Vec::new();
        if let Some(ms) = duration_ms {
            let secs = ms / 1000;
            meta_parts.push(format!("{}:{:02}", secs / 60, secs % 60));
        }
        if let Some(kbps) = bitrate {
            meta_parts.push(format!("{kbps}k"));
        }
        let meta = meta_parts.join(" ");

        Some(
            div()
                .class("mm-file-row")
                .child(
                    html::a()
                        .class("mm-link mm-file-row__path")
                        .attr("href", format!("#tags/{inode}"))
                        .text(name),
                )
                .child(span().class("mm-file-row__meta").text(&meta))
                .into(),
        )
    }
}

pub fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    h
}

// ============================================================================
// Tag editor view (stays on serde_json::Value)
// ============================================================================

pub fn render_tag_editor(inode: i64, path: &str, tags: &serde_json::Value) -> Node {
    let mut rows = Vec::new();

    if let Some(arr) = tags.as_array() {
        for (i, pair) in arr.iter().enumerate() {
            let (tag_name, tag_val) = if let Some(pair_arr) = pair.as_array() {
                let name = pair_arr.first().and_then(|v| v.as_str()).unwrap_or("");
                let val = pair_arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                (name, val)
            } else {
                continue;
            };

            let name_id = format!("tag-name-{i}");
            let val_id = format!("tag-val-{i}");

            rows.push(
                div()
                    .class("mm-tag-row")
                    .child(
                        html::input()
                            .attr("type", "text")
                            .attr("id", &name_id)
                            .attr("value", tag_name)
                            .attr("readonly", "")
                            .class("mm-tag-name"),
                    )
                    .child(
                        html::input()
                            .attr("type", "text")
                            .attr("id", &val_id)
                            .attr("value", tag_val)
                            .attr("data-inode", inode.to_string())
                            .attr("data-tag", tag_name)
                            .attr("data-original", tag_val)
                            .class("mm-tag-value"),
                    )
                    .into(),
            );
        }
    }

    div()
        .class("mm-tag-editor")
        .child(h3().class("mm-section__title").text(&format!("Tags — {path}")))
        .child(
            div()
                .class("mm-tag-editor__header")
                .child(span().class("mm-tag-header-name").text("Tag"))
                .child(span().class("mm-tag-header-value").text("Value")),
        )
        .children(rows)
        .into()
}
