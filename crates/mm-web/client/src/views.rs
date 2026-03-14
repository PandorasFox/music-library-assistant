//! View renderers — each lateral view's content as HTML Node trees.

use mm_ui::html::{self, div, h3, span, section, Node};

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

/// Render flat key-value section from a JSON object (skips nested).
pub fn render_kv_section(title: &str, json: &serde_json::Value) -> Node {
    let mut items = Vec::new();
    if let Some(obj) = json.as_object() {
        for (key, val) in obj {
            if val.is_object() || val.is_array() {
                continue;
            }
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Null => "—".into(),
                _ => continue,
            };
            let display_key = key.replace('_', " ");
            items.push(kv(&display_key, &val_str));
        }
    }
    titled_section(title, items)
}

// ============================================================================
// Health view
// ============================================================================

pub fn render_status_content(status: &serde_json::Value) -> Node {
    let mut sections = Vec::new();
    sections.push(render_kv_section("Witch", status));

    if let Some(work) = status.get("work") {
        sections.push(render_kv_section("Work", work));
        if let Some(pending) = work.get("pending_by_label").and_then(|v| v.as_object()) {
            if !pending.is_empty() {
                let items: Vec<Node> = pending
                    .iter()
                    .map(|(k, v)| kv(k, &v.as_u64().unwrap_or(0).to_string()))
                    .collect();
                sections.push(titled_section("Pending Work", items));
            }
        }
    }
    if let Some(tx) = status.get("transaction") {
        if !tx.is_null() {
            sections.push(render_kv_section("Transaction", tx));
        }
    }
    if let Some(progress) = status.get("external_fetch_progress") {
        if !progress.is_null() {
            sections.push(render_kv_section("External Fetch", progress));
        }
    }
    div().children(sections).into()
}

pub fn render_insights_content(insights: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    if let Some(corpus) = insights.get("corpus_files") {
        sections.push(render_kv_section("Corpus Files", corpus));
    }
    if let Some(placeholders) = insights.get("placeholders") {
        sections.push(render_kv_section("Tag Health", placeholders));
    }
    if let Some(other) = insights.get("other_signals").and_then(|v| v.as_array()) {
        let items: Vec<Node> = other
            .iter()
            .filter_map(|entry| {
                let label = entry.get("label")?.as_str()?;
                let count = entry.get("count")?.as_u64()?;
                if count == 0 { return None; }
                Some(kv(label, &count.to_string()))
            })
            .collect();
        if !items.is_empty() {
            sections.push(titled_section("Signals", items));
        }
    }
    if sections.is_empty() {
        span().class("mm-kv__val").text("No insights data").into()
    } else {
        div().children(sections).into()
    }
}

// ============================================================================
// External Matches view
// ============================================================================

pub fn render_external_matches_content(data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    let packing_fields = [
        ("packing_perfect_count", "Perfect"),
        ("packing_full_match_count", "Full match"),
        ("packing_singles_count", "Singles"),
        ("packing_incomplete_count", "Incomplete"),
        ("packing_low_confidence_count", "Low confidence"),
        ("packing_knots_count", "Knots"),
    ];
    let packing_items: Vec<Node> = packing_fields
        .iter()
        .filter_map(|(key, label)| {
            let n = data.get(key)?.as_u64()?;
            Some(kv(label, &n.to_string()))
        })
        .collect();
    if !packing_items.is_empty() {
        sections.push(titled_section("Release Packing", packing_items));
    }

    let unsolved_fields = [
        ("unsolved_conflict_count", "Conflicts"),
        ("unsolved_no_release_count", "No release"),
        ("unsolved_no_match_count", "No match"),
        ("va_override_count", "VA overrides"),
        ("pinned_conflict_count", "Pinned conflicts"),
    ];
    let unsolved_items: Vec<Node> = unsolved_fields
        .iter()
        .filter_map(|(key, label)| {
            let n = data.get(key)?.as_u64()?;
            if n == 0 { return None; }
            Some(kv(label, &n.to_string()))
        })
        .collect();
    if !unsolved_items.is_empty() {
        sections.push(titled_section("Unsolved", unsolved_items));
    }

    if let Some(buckets) = data.get("confidence_buckets").and_then(|v| v.as_array()) {
        let bucket_items: Vec<Node> = buckets
            .iter()
            .filter_map(|b| {
                let tier = b.get("tier")?.as_str().unwrap_or("?");
                let total = b.get("total")?.as_u64()?;
                Some(kv(tier, &total.to_string()))
            })
            .collect();
        if !bucket_items.is_empty() {
            sections.push(titled_section("Confidence Tiers", bucket_items));
        }
    }

    div().children(sections).into()
}

// ============================================================================
// Edit History view
// ============================================================================

pub fn render_edit_history_content(data: &serde_json::Value) -> Node {
    let sessions = match data.get("sessions").and_then(|v| v.as_array()) {
        Some(s) => s,
        None => return span().class("mm-kv__val").text("No edit history").into(),
    };
    if sessions.is_empty() {
        return span().class("mm-kv__val").text("No edit sessions").into();
    }

    let items: Vec<Node> = sessions
        .iter()
        .filter_map(|s| {
            let session_id = s.get("session_id")?.as_str()?;
            let earliest = s.get("earliest_at")?.as_str().unwrap_or("?");
            let edits = s.get("edit_count")?.as_u64().unwrap_or(0);
            let inodes = s.get("inode_count")?.as_u64().unwrap_or(0);
            Some(
                div()
                    .class("mm-history-session")
                    .child(
                        div()
                            .class("mm-kv")
                            .child(
                                html::a()
                                    .class("mm-link")
                                    .attr("href", "#")
                                    .attr("onclick", format!(
                                        "event.preventDefault();window.__mm_expand_session('{session_id}')"
                                    ))
                                    .text(session_id),
                            )
                            .child(span().class("mm-kv__val").text(
                                format!("{edits} edits, {inodes} files — {earliest}"),
                            )),
                    )
                    .child(div().attr("id", format!("session-{session_id}")).class("mm-session-detail"))
                    .into(),
            )
        })
        .collect();
    titled_section("Edit Sessions", items)
}

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
// Inbox view
// ============================================================================

pub fn render_inbox_content(data: &serde_json::Value) -> Node {
    let fields = [
        ("file_in_inbox", "Files in inbox"),
        ("unindexed", "Unindexed"),
        ("corpus_match", "Corpus matches"),
        ("organizable", "Organizable"),
        ("tag_canonicity", "Tag canonicity"),
        ("missing_tags", "Missing tags"),
        ("compound_tags", "Compound tags"),
    ];
    let items: Vec<Node> = fields
        .iter()
        .filter_map(|(key, label)| {
            let n = data.get(key)?.as_u64()?;
            Some(kv(label, &n.to_string()))
        })
        .collect();
    titled_section("Inbox", items)
}

// ============================================================================
// Deploy view
// ============================================================================

pub fn render_deploy_content(data: &serde_json::Value) -> Node {
    let mut items = Vec::new();
    if let Some(needs) = data.get("needs_action").and_then(|v| v.as_bool()) {
        items.push(kv("Needs action", if needs { "yes" } else { "no" }));
    }
    if let Some(libs) = data.get("library_file_counts").and_then(|v| v.as_array()) {
        for entry in libs {
            if let Some(arr) = entry.as_array() {
                let name = arr.first().and_then(|v| v.as_str()).unwrap_or("?");
                let count = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
                items.push(kv(name, &count.to_string()));
            }
        }
    }
    titled_section("Deploy Status", items)
}

// ============================================================================
// Transaction view
// ============================================================================

pub fn render_transaction_content(
    status: &serde_json::Value,
    details: Option<&serde_json::Value>,
) -> Node {
    let tx = status.get("transaction");
    let has_tx = tx.map_or(false, |t| !t.is_null());

    if !has_tx {
        return section()
            .class("mm-section")
            .child(h3().class("mm-section__title").text("Transaction"))
            .child(span().class("mm-kv__val").text("No active transaction"))
            .into();
    }

    let tx = tx.unwrap();
    let mut sections = Vec::new();

    let mut summary = Vec::new();
    if let Some(label) = tx.get("label").and_then(|v| v.as_str()) {
        summary.push(kv("Label", label));
    }
    if let Some(dc) = tx.get("decision_count").and_then(|v| v.as_u64()) {
        summary.push(kv("Decisions", &dc.to_string()));
    }
    if let Some(mc) = tx.get("mutation_count").and_then(|v| v.as_u64()) {
        summary.push(kv("Mutations", &mc.to_string()));
    }
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
// Config editor view
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
        // Top-level opinion scalars.
        let mut top_items = Vec::new();
        for (key, val) in opinions {
            if val.is_object() {
                continue; // sub-blocks handled below
            }
            top_items.push(config_field(key, val));
        }
        if !top_items.is_empty() {
            sections.push(titled_section("Opinions", top_items));
        }

        // Sub-blocks.
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

    div().class("mm-config-editor").children(sections).into()
}

/// Render a config field as the appropriate form input.
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
        }
        serde_json::Value::Object(obj) => {
            // Nested object — render as sub-fields.
            let items: Vec<Node> = obj.iter().map(|(k, v)| config_field(k, v)).collect();
            div()
                .class("mm-config-nested")
                .child(span().class("mm-config-nested-label").text(&display_key))
                .children(items)
                .into()
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
// Packing browser view
// ============================================================================

pub fn render_packing_overview(ext_data: &serde_json::Value) -> Node {
    let categories = [
        ("perfect", "Perfect", "packing_perfect_count"),
        ("full_match", "Full Match", "packing_full_match_count"),
        ("single", "Singles", "packing_singles_count"),
        ("incomplete", "Incomplete", "packing_incomplete_count"),
        ("low_confidence", "Low Confidence", "packing_low_confidence_count"),
    ];

    let items: Vec<Node> = categories
        .iter()
        .filter_map(|(prefix, label, count_key)| {
            let count = ext_data.get(count_key)?.as_u64().unwrap_or(0);
            Some(
                div()
                    .class("mm-kv")
                    .child(
                        html::a()
                            .class("mm-link")
                            .attr("href", "#")
                            .attr("onclick", format!(
                                "event.preventDefault();window.__mm_packing_browse('{prefix}')"
                            ))
                            .text(&format!("{label} ({count})")),
                    )
                    .into(),
            )
        })
        .collect();

    let mut sections = vec![titled_section("Release Packing Categories", items)];

    // Knots.
    let knots = ext_data.get("packing_knots_count").and_then(|v| v.as_u64()).unwrap_or(0);
    if knots > 0 {
        sections.push(
            div()
                .class("mm-kv")
                .child(span().class("mm-kv__key").text("Knots"))
                .child(span().class("mm-kv__val").text(knots.to_string()))
                .into(),
        );
    }

    div().children(sections).into()
}

pub fn render_packing_browser_data(category: &str, data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    // Back link.
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

    // Packed releases.
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

    // Per-track packing assignments.
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

    // Unfilled slots.
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
// Tag editor view (form-based)
// ============================================================================

/// Render tag editor for a single inode. `tags` is Vec<(String, String)>.
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
