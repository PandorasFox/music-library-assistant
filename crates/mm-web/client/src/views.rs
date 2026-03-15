//! View renderers — each lateral view's content as HTML Node trees.
//!
//! Typed functions accept mm-meta structs so the compiler catches field name
//! mismatches. Functions that remain on `&serde_json::Value` are generic
//! display utilities or use composite responses without a single mm-meta type.

use mm_meta::protocol::DecisionDetail;
use mm_meta::views::{
    DeployStatus, EditHistoryData, ExternalMatchesData, InboxOverviewData, InsightsData,
};
use mm_meta::witch_types::{WitchStatus, WorkStateSnapshot};
use mm_ui::html::{self, div, h3, section, span, Node};

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

/// Clickable key-value row that navigates to a resolution route.
fn kv_resolve(key: &str, val: &str, route: &str) -> Node {
    div()
        .class("mm-kv mm-kv--clickable")
        .attr("onclick", &format!("window.__mm_navigate_route('{}')", route))
        .attr("style", "cursor: pointer;")
        .child(span().class("mm-kv__key").text(key))
        .child(span().class("mm-kv__val").text(val))
        .child(span().class("mm-kv__action").text("\u{2192}"))
        .into()
}

/// Key-value row with navigation link (hash-based routing).
fn nav_kv(key: &str, val: &str, href: &str) -> Node {
    div()
        .class("mm-kv mm-kv--clickable")
        .attr("onclick", &format!("location.hash='{}'", href))
        .attr("style", "cursor: pointer;")
        .child(span().class("mm-kv__key").text(key))
        .child(span().class("mm-kv__val").text(val))
        .child(span().class("mm-kv__action").text("\u{2192}"))
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
        sections.push(
            div()
                .class("mm-alert")
                .child(span().class("mm-alert__text").text(
                    format!("{} unindexed files", corpus.files_unindexed),
                ))
                .child(
                    html::button()
                        .class("mm-btn mm-alert__action")
                        .attr("onclick", "window.__mm_index_now()")
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
                .child(
                    html::button()
                        .class("mm-btn mm-alert__action")
                        .attr("onclick", "window.__mm_navigate_route('resolve/missing-files/restorable')")
                        .text("Resolve"),
                )
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
                .child(
                    html::button()
                        .class("mm-btn mm-alert__action")
                        .attr("onclick", "window.__mm_navigate_route('resolve/corrupt-files')")
                        .text("Resolve"),
                )
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
                .child(
                    html::button()
                        .class("mm-btn mm-alert__action")
                        .attr("onclick", "window.__mm_navigate_route('resolve/moved-files')")
                        .text("Resolve"),
                )
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
        corpus_items.push(kv_resolve("missing", &corpus.files_missing.to_string(), "resolve/missing-files/restorable"));
    }
    if corpus.directories_missing > 0 {
        corpus_items.push(kv_resolve("dirs missing", &corpus.directories_missing.to_string(), "resolve/missing-directories"));
    }
    if corpus.corrupt_files > 0 {
        corpus_items.push(kv_resolve("corrupt", &corpus.corrupt_files.to_string(), "resolve/corrupt-files"));
    }
    if corpus.shit_format_files > 0 {
        corpus_items.push(kv_resolve("non-vorbis", &corpus.shit_format_files.to_string(), "resolve/lossless-remux"));
    }
    if corpus.oob_tag_sync > 0 {
        corpus_items.push(kv_resolve("OOB tag sync", &corpus.oob_tag_sync.to_string(), "resolve/oob-sync"));
    }
    if corpus.oob_tag_conflict > 0 {
        corpus_items.push(kv_resolve("OOB tag conflict", &corpus.oob_tag_conflict.to_string(), "resolve/oob-conflict/two-way"));
    }
    sections.push(titled_section("Corpus Files", corpus_items));

    // Tag health (placeholder/squash bucket).
    let ph = &insights.bucket_placeholder;
    let mut tag_items = Vec::new();
    if ph.cross_source_overlap_count > 0 {
        tag_items.push(kv_resolve("source overlaps", &ph.cross_source_overlap_count.to_string(), "resolve/directory-cluster"));
    }
    if ph.release_overlap_count > 0 {
        tag_items.push(kv_resolve("release overlaps", &ph.release_overlap_count.to_string(), "resolve/directory-cluster"));
    }
    if ph.subpar_duplicate_count > 0 {
        tag_items.push(kv_resolve("subpar duplicates", &ph.subpar_duplicate_count.to_string(), "resolve/subpar-duplicates"));
    }
    if ph.redundant_duplicate_count > 0 {
        tag_items.push(kv_resolve("redundant duplicates", &ph.redundant_duplicate_count.to_string(), "resolve/redundant-duplicates"));
    }
    for entry in &ph.tag_canonicity {
        let encoded_tag = js_sys::encode_uri_component(&entry.tag_name);
        tag_items.push(kv_resolve(
            &format!("{} canonicity", entry.tag_name),
            &entry.cluster_count.to_string(),
            &format!("resolve/tag-canonicity/{}?zone=corpus&filter_existing_canonicals=true", encoded_tag),
        ));
    }
    if ph.inconsistent_album_artist_count > 0 {
        tag_items.push(kv_resolve("album artist issues", &ph.inconsistent_album_artist_count.to_string(), "resolve/inconsistent-album-artist/ALBUMARTIST"));
    }
    for entry in &ph.compound_tags {
        let total = entry.safe_count + entry.review_count;
        let encoded_tag = js_sys::encode_uri_component(&entry.tag_name);
        tag_items.push(kv_resolve(
            &format!("{} compound", entry.tag_name),
            &total.to_string(),
            &format!("resolve/compound-split/{}?zone=corpus&safe=true", encoded_tag),
        ));
    }
    if ph.missing_album_single_count > 0 {
        tag_items.push(kv_resolve("missing album singles", &ph.missing_album_single_count.to_string(), "resolve/missing-album"));
    }
    if ph.disc_extraction_count > 0 {
        tag_items.push(kv_resolve("disc extraction", &ph.disc_extraction_count.to_string(), "resolve/disc-extraction"));
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

/// Full External Matches page: progress + data sections with stable IDs for polling.
pub fn render_external_matches_page(data: &ExternalMatchesData, status: Option<&WitchStatus>) -> Node {
    let progress = if let Some(s) = status {
        render_fetch_progress_section(s)
    } else {
        div().into()
    };

    div()
        .child(
            div()
                .class("mm-buttons")
                .child(
                    html::button()
                        .class("mm-btn")
                        .attr("onclick", "window.__mm_queue_task('ExternalFetch')")
                        .text("Fetch External Data"),
                )
                .child(
                    html::button()
                        .class("mm-btn")
                        .attr("onclick", "window.__mm_queue_task('ReleasePacking')")
                        .text("Run Packing"),
                ),
        )
        .child(div().attr("id", "mm-fetch-progress").child(progress))
        .child(div().attr("id", "mm-external-data").child(render_external_matches_data(data)))
        .into()
}

/// Render fetch progress from WitchStatus (polled section).
pub fn render_fetch_progress_section(status: &WitchStatus) -> Node {
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
        titled_section("External Fetch (active)", items)
    } else if status.is_external_fetch_active {
        titled_section("External Fetch", vec![kv("status", "Starting...")])
    } else {
        titled_section("External Fetch", vec![kv("status", "Idle")])
    }
}

/// Render external matches data (counts, tiers, packing — polled section).
pub fn render_external_matches_data(data: &ExternalMatchesData) -> Node {
    let mut sections = Vec::new();

    // Confidence tiers with navigation links.
    let bucket_items: Vec<Node> = data
        .confidence_buckets
        .iter()
        .map(|b| {
            let label = format!("{:?} ({})", b.tier, b.tier.label());
            let href = format!("#/external-matches/acoustid/{}", tier_to_route_str(b.tier));
            nav_kv(&label, &b.total.to_string(), &href)
        })
        .collect();
    if !bucket_items.is_empty() {
        sections.push(titled_section("Confidence Tiers", bucket_items));
    }

    // Packing categories with navigation links.
    let packing_items = vec![
        nav_kv("Perfect", &data.packing_perfect_count.to_string(), "#/external-matches/review/perfect"),
        nav_kv("Full match", &data.packing_full_match_count.to_string(), "#/external-matches/review/full-match"),
        nav_kv("Singles", &data.packing_singles_count.to_string(), "#/external-matches/review/singles"),
        nav_kv("Incomplete", &data.packing_incomplete_count.to_string(), "#/external-matches/review/incomplete"),
        nav_kv("Low confidence", &data.packing_low_confidence_count.to_string(), "#/external-matches/review/low-confidence"),
        kv("Knots", &data.packing_knots_count.to_string()),
    ];
    sections.push(titled_section("Release Packing", packing_items));

    // Unsolved.
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

    div().children(sections).into()
}

/// Map display-level ConfidenceTier to the AcoustidConfidence route segment.
/// Perfect/VeryHigh/High all map to "high", Medium→"medium", Low→"low".
fn tier_to_route_str(tier: mm_meta::views::ConfidenceTier) -> &'static str {
    use mm_meta::views::ConfidenceTier;
    match tier {
        ConfidenceTier::Perfect | ConfidenceTier::VeryHigh | ConfidenceTier::High => "high",
        ConfidenceTier::Medium => "medium",
        ConfidenceTier::Low => "low",
    }
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
    details: &[DecisionDetail],
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
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("onclick", "window.__mm_navigate_route('review')")
                    .text("Review"),
            )
            .into(),
    );
    sections.push(titled_section("Active Transaction", summary));

    if !details.is_empty() {
        let decision_items: Vec<Node> = details
            .iter()
            .map(|d| {
                let key_json = serde_json::to_string(&d.key).unwrap_or_default();
                let escaped_key = key_json.replace('\'', "\\'");
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(&d.label))
                    .child(span().class("mm-kv__val").text(
                        format!("{} mutations", d.mutations.len()),
                    ))
                    .child(
                        html::button()
                            .class("mm-btn mm-btn--small")
                            .attr("style", "border-color:var(--c-red)")
                            .attr(
                                "onclick",
                                format!("window.__mm_tx_remove('{escaped_key}')"),
                            )
                            .text("Remove"),
                    )
                    .into()
            })
            .collect();
        sections.push(titled_section("Decisions", decision_items));
    }
    div().children(sections).into()
}

/// Render the transaction review view — detailed diff display.
pub fn render_transaction_review(
    status: &WitchStatus,
    details: &[DecisionDetail],
) -> Node {
    if status.transaction.is_none() {
        return section()
            .class("mm-section")
            .child(h3().class("mm-section__title").text("Transaction Review"))
            .child(span().class("mm-kv__val").text("No active transaction"))
            .into();
    }

    let mut sections = Vec::new();

    // Buttons at top.
    sections.push(
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
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("onclick", "window.__mm_navigate_route('transaction')")
                    .text("Back"),
            )
            .into(),
    );

    // Decision details with diffs.
    for d in details {
        let key_json = serde_json::to_string(&d.key).unwrap_or_default();
        let escaped_key = key_json.replace('\'', "\\'");

        let diff_entries: Vec<_> = d.mutations.iter().flat_map(|m| m.diff_entries()).collect();

        let mut decision_items = vec![
            div()
                .class("mm-kv")
                .child(span().class("mm-kv__val").text(
                    format!("{} mutations", d.mutations.len()),
                ))
                .child(
                    html::button()
                        .class("mm-btn mm-btn--small")
                        .attr("style", "border-color:var(--c-red)")
                        .attr(
                            "onclick",
                            format!("window.__mm_tx_remove('{escaped_key}')"),
                        )
                        .text("Remove"),
                )
                .into(),
        ];

        // Render diff entries as a table-like layout.
        for entry in &diff_entries {
            decision_items.push(
                div()
                    .class("mm-edit-row")
                    .child(span().class("mm-edit-field").text(&entry.label))
                    .child(
                        span()
                            .class("mm-edit-diff")
                            .child(span().class("mm-edit-old").text(&entry.old_value))
                            .child(span().class("mm-edit-arrow").text(" \u{2192} "))
                            .child(span().class("mm-edit-new").text(&entry.new_value)),
                    )
                    .into(),
            );
        }

        if diff_entries.is_empty() {
            decision_items.push(
                span()
                    .class("mm-kv__val")
                    .text(format!("{} mutations (no diff preview)", d.mutations.len()))
                    .into(),
            );
        }

        sections.push(titled_section(&d.label, decision_items));
    }

    if details.is_empty() {
        sections.push(
            span()
                .class("mm-kv__val")
                .text("No decisions staged")
                .into(),
        );
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
// External match overlay views
// ============================================================================

/// Render AcoustID match browse results.
pub fn render_acoustid_matches(data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    sections.push(
        div()
            .child(
                html::a()
                    .class("mm-link")
                    .attr("href", "#")
                    .attr("onclick", "event.preventDefault();window.__mm_navigate('Ext. Matches')")
                    .text("\u{2190} Back to External Matches"),
            )
            .into(),
    );

    let entries = data.as_array();
    let count = entries.map_or(0, |e| e.len());

    let items: Vec<Node> = entries
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let display_name = entry.get("display_name")?.as_str()?;
                    let confidence = entry.get("confidence")?.as_f64().unwrap_or(0.0);
                    let pct = (confidence * 100.0) as u32;
                    let recording_title = entry.get("recording_title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)");
                    let recording_artist = entry.get("recording_artist")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)");
                    let recording_id = entry.get("recording_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let mb_url = format!("https://musicbrainz.org/recording/{recording_id}");

                    Some(
                        div()
                            .class("mm-kv")
                            .child(span().class("mm-kv__key").text(
                                format!("{display_name}  [{pct}%]"),
                            ))
                            .child(
                                html::a()
                                    .class("mm-link")
                                    .attr("href", &mb_url)
                                    .attr("target", "_blank")
                                    .attr("rel", "noopener")
                                    .text(format!("{recording_artist} — {recording_title}")),
                            )
                            .into(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    sections.push(titled_section(
        &format!("AcoustID Matches ({count})"),
        items,
    ));

    div().children(sections).into()
}

/// Render release review browse results.
pub fn render_release_review(data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    sections.push(
        div()
            .child(
                html::a()
                    .class("mm-link")
                    .attr("href", "#")
                    .attr("onclick", "event.preventDefault();window.__mm_navigate('Ext. Matches')")
                    .text("\u{2190} Back to External Matches"),
            )
            .into(),
    );

    let releases = data.get("releases").and_then(|v| v.as_array());
    let count = releases.map_or(0, |r| r.len());

    if let Some(releases) = releases {
        for release in releases {
            let title = release.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            let artist = release.get("artist").and_then(|v| v.as_str()).unwrap_or("?");
            let track_count = release.get("track_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let matched_count = release.get("matched_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let avg_confidence = release.get("avg_confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let avg_pct = (avg_confidence * 100.0) as u32;
            let category = release.get("category").and_then(|v| v.as_str()).unwrap_or("?");

            let mut track_items = Vec::new();
            track_items.push(kv("Category", category));
            track_items.push(kv("Tracks", &format!("{matched_count}/{track_count} matched")));
            track_items.push(kv("Avg Confidence", &format!("{avg_pct}%")));

            if let Some(tracks) = release.get("tracks").and_then(|v| v.as_array()) {
                for track in tracks {
                    let position = track.get("position").and_then(|v| v.as_u64()).unwrap_or(0);
                    let mb_title = track.get("mb_title").and_then(|v| v.as_str()).unwrap_or("?");
                    let matched_name = track.get("matched_display_name")
                        .and_then(|v| v.as_str());
                    let track_confidence = track.get("confidence")
                        .and_then(|v| v.as_f64());

                    let val = if let Some(name) = matched_name {
                        let conf_str = track_confidence
                            .map(|c| format!(" [{:.0}%]", c * 100.0))
                            .unwrap_or_default();
                        format!("{name}{conf_str}")
                    } else {
                        "(unmatched)".to_string()
                    };

                    track_items.push(kv(
                        &format!("{position}. {mb_title}"),
                        &val,
                    ));
                }
            }

            sections.push(titled_section(
                &format!("{artist} \u{2014} {title}"),
                track_items,
            ));
        }
    }

    if count == 0 {
        sections.push(
            span().class("mm-kv__val").text("No releases match this filter").into(),
        );
    }

    div().children(sections).into()
}

/// Placeholder for single release detail view (query not yet implemented).
pub fn render_release_detail_placeholder(release_id: &str) -> Node {
    let mut sections = Vec::new();

    sections.push(
        div()
            .child(
                html::a()
                    .class("mm-link")
                    .attr("href", "#")
                    .attr("onclick", "event.preventDefault();window.__mm_navigate('Ext. Matches')")
                    .text("\u{2190} Back to External Matches"),
            )
            .into(),
    );

    sections.push(titled_section(
        &format!("Release Detail \u{2014} {release_id}"),
        vec![
            span().class("mm-kv__val").text("Detail query not yet implemented").into(),
        ],
    ));

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

// ============================================================================
// Resolution views — generic renderer for resolution modals
// ============================================================================

/// Render a resolution modal from server-fetched data.
///
/// `title` — modal heading text
/// `items` — display strings for the item list
/// `buttons` — tuples of (label, css_color_var, onclick_js)
pub fn render_resolution_view(
    title: &str,
    items: &[String],
    buttons: &[(&str, &str, &str)],
) -> Node {
    let mut sections = Vec::new();

    // Title.
    sections.push(h3().class("mm-section__title").text(title).into());

    // Item list (scrollable).
    if items.is_empty() {
        sections.push(
            span().class("mm-kv__val").text("No items").into(),
        );
    } else {
        let rows: Vec<Node> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(format!("{}", i + 1)))
                    .child(span().class("mm-kv__val").text(item))
                    .into()
            })
            .collect();
        sections.push(
            div()
                .class("mm-resolution-items")
                .children(rows)
                .into(),
        );
    }

    // Button row.
    let btn_nodes: Vec<Node> = buttons
        .iter()
        .map(|(label, color, onclick)| {
            html::button()
                .class("mm-btn")
                .attr("style", format!("border-color:{color}"))
                .attr("onclick", *onclick)
                .text(*label)
                .into()
        })
        .collect();
    sections.push(
        div().class("mm-buttons").children(btn_nodes).into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render missing directory resolution data.
pub fn render_missing_directories(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("directories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Missing Directories ({})", items.len()),
        &items,
        &[
            ("Drop All", "var(--c-yellow)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render corrupt file resolution data.
pub fn render_corrupt_files(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    entry.get("corpus_path").and_then(|v| v.as_str()).map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Corrupt Files ({})", items.len()),
        &items,
        &[
            ("Stash All", "var(--c-yellow)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render moved files resolution data.
pub fn render_moved_files(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let old = entry.get("old_path")?.as_str()?;
                    let new = entry.get("new_path")?.as_str()?;
                    Some(format!("{old} \u{2192} {new}"))
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Moved Files ({})", items.len()),
        &items,
        &[
            ("Accept All", "var(--c-green)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render subpar duplicate resolution data.
pub fn render_subpar_duplicates(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let path = entry.get("corpus_path")?.as_str()?;
                    let reason = entry.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                    let superior = entry.get("superior_path").and_then(|v| v.as_str()).unwrap_or("");
                    let score = entry.get("similarity_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    Some(format!("{path}  [{reason}, {score:.0}% match vs {superior}]"))
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Subpar Duplicates ({})", items.len()),
        &items,
        &[
            ("Stash All", "var(--c-yellow)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render lossless remux (shit-format) resolution data.
pub fn render_lossless_remux(data: &serde_json::Value) -> Node {
    let lossless: Vec<String> = data
        .get("lossless_files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let path = entry.get("corpus_path")?.as_str()?;
                    let fmt = entry.get("file_type").and_then(|v| v.as_str()).unwrap_or("?");
                    Some(format!("{path}  [{fmt}]"))
                })
                .collect()
        })
        .unwrap_or_default();

    let lossy: Vec<String> = data
        .get("lossy_files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let path = entry.get("corpus_path")?.as_str()?;
                    let fmt = entry.get("file_type").and_then(|v| v.as_str()).unwrap_or("?");
                    Some(format!("{path}  [{fmt}]"))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!(
                "Non-Vorbis Files ({} lossless, {} lossy)",
                lossless.len(),
                lossy.len(),
            ))
            .into(),
    );

    if !lossless.is_empty() {
        let rows: Vec<Node> = lossless
            .iter()
            .enumerate()
            .map(|(i, item)| {
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(format!("{}", i + 1)))
                    .child(span().class("mm-kv__val").text(item))
                    .into()
            })
            .collect();
        sections.push(titled_section("Lossless (remux to FLAC)", rows));
    }

    if !lossy.is_empty() {
        let rows: Vec<Node> = lossy
            .iter()
            .enumerate()
            .map(|(i, item)| {
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(format!("{}", i + 1)))
                    .child(span().class("mm-kv__val").text(item))
                    .into()
            })
            .collect();
        sections.push(titled_section("Lossy (transcode to Opus)", rows));
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Remux All Lossless"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render restorable missing files.
pub fn render_missing_files_restorable(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("restorable")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let corpus = entry.get("corpus_path")?.as_str()?;
                    let library = entry.get("library_path").and_then(|v| v.as_str()).unwrap_or("?");
                    Some(format!("{corpus}  [restore from {library}]"))
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Missing Files — Restorable ({})", items.len()),
        &items,
        &[
            ("Restore All", "var(--c-green)", "window.__mm_resolve_cancel()"),
            ("Drop All", "var(--c-yellow)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render non-restorable missing files.
pub fn render_missing_files_permanent(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("non_restorable")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    entry.get("corpus_path").and_then(|v| v.as_str()).map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Missing Files — Permanent ({})", items.len()),
        &items,
        &[
            ("Drop All", "var(--c-red)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render inbox/corpus match resolution data.
pub fn render_inbox_corpus_match(data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .get("entries")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let inbox_path = entry.get("inbox_path")?.as_str()?;
                    let quality = entry.get("inbox_quality").and_then(|v| v.as_str()).unwrap_or("?");
                    let class = entry.get("classification").and_then(|v| v.as_str()).unwrap_or("?");
                    let matches = entry.get("corpus_matches")
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len());
                    Some(format!("{inbox_path}  [{quality}, {class}, {matches} match(es)]"))
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("Inbox/Corpus Matches ({})", items.len()),
        &items,
        &[
            ("Stash Safe", "var(--c-green)", "window.__mm_resolve_cancel()"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()"),
        ],
    )
}

/// Render OOB resolution files for a single bucket type (or all).
pub fn render_oob_conflict_bucket(title: &str, data: &serde_json::Value) -> Node {
    let items: Vec<String> = data
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let path = entry.get("path")?.as_str()?;
                    let mismatch_count = entry.get("mismatches")
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len());
                    if mismatch_count > 0 {
                        Some(format!("{path}  [{mismatch_count} tag(s)]"))
                    } else {
                        Some(path.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    render_resolution_view(
        &format!("{title} ({})", items.len()),
        &items,
        &[("Cancel", "var(--c-white)", "window.__mm_resolve_cancel()")],
    )
}

/// Render directory cluster resolution data.
pub fn render_directory_clusters(data: &serde_json::Value) -> Node {
    let clusters = data.get("clusters").and_then(|v| v.as_array());
    let cluster_count = clusters.map_or(0, |c| c.len());

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!("Directory Clusters ({} clusters)", cluster_count))
            .into(),
    );

    if let Some(clusters) = clusters {
        for (idx, cluster) in clusters.iter().enumerate() {
            let cluster_key = cluster.get("cluster_key").and_then(|v| v.as_str()).unwrap_or("?");
            let overlap = cluster.get("overlap_count").and_then(|v| v.as_u64()).unwrap_or(0);

            let mut items = Vec::new();
            items.push(kv("Overlap", &format!("{} track pairs", overlap)));

            if let Some(dirs) = cluster.get("directories").and_then(|v| v.as_array()) {
                for dir in dirs {
                    let path_suffix = dir.get("path_suffix").and_then(|v| v.as_str()).unwrap_or("?");
                    let format_summary = dir.get("format_summary").and_then(|v| v.as_str()).unwrap_or("?");
                    let file_count = dir.get("inodes").and_then(|v| v.as_array()).map_or(0, |a| a.len());
                    items.push(kv(
                        path_suffix,
                        &format!("{} files, {}", file_count, format_summary),
                    ));
                }
            }

            sections.push(titled_section(
                &format!("Cluster {} — {}", idx + 1, cluster_key),
                items,
            ));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Stash"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-blue)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Mark Expected"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

// ============================================================================
// Cluster-nav resolution views
// ============================================================================

/// Render tag canonicity resolution data (all clusters as expandable sections).
pub fn render_tag_canonicity(data: &serde_json::Value) -> Node {
    render_tag_canonicity_titled("Tag Canonicity", data)
}

/// Render tag canonicity data with a custom title prefix.
/// Used by both TagCanonicity and InconsistentAlbumArtist routes
/// (same data shape, different heading).
pub fn render_tag_canonicity_titled(title_prefix: &str, data: &serde_json::Value) -> Node {
    let tag_name = data.get("tag_name").and_then(|v| v.as_str()).unwrap_or("?");
    let clusters = data.get("clusters").and_then(|v| v.as_array());

    let cluster_count = clusters.map_or(0, |c| c.len());
    let mut sections = Vec::new();

    sections.push(
        h3().class("mm-section__title")
            .text(format!("{} — {} ({} clusters)", title_prefix, tag_name, cluster_count))
            .into(),
    );

    if let Some(clusters) = clusters {
        for (idx, cluster) in clusters.iter().enumerate() {
            let suggested = cluster.get("suggested_canonical")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let confirmed = cluster.get("confirmed_canonical")
                .and_then(|v| v.as_str());

            let mut variant_items = Vec::new();
            if let Some(variants) = cluster.get("variants").and_then(|v| v.as_array()) {
                for variant in variants {
                    let value = variant.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                    let files = variant.get("files").and_then(|v| v.as_array());
                    let file_count = files.map_or(0, |f| f.len());
                    let file_names: Vec<&str> = files
                        .map(|f| {
                            f.iter()
                                .filter_map(|file| file.get("display_name").and_then(|v| v.as_str()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let preview = if file_names.len() <= 3 {
                        file_names.join(", ")
                    } else {
                        format!("{}, ... +{}", file_names[..3].join(", "), file_names.len() - 3)
                    };
                    variant_items.push(
                        kv(
                            &format!("\"{value}\" ({file_count} files)"),
                            &preview,
                        ),
                    );
                }
            }

            let heading = if let Some(canonical) = confirmed {
                format!("Cluster {} — canonical: \"{}\"", idx + 1, canonical)
            } else {
                format!("Cluster {} — suggested: \"{}\"", idx + 1, suggested)
            };
            sections.push(titled_section(&heading, variant_items));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Accept Suggestions"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render compound split resolution data (all groups as expandable sections).
pub fn render_compound_split(data: &serde_json::Value) -> Node {
    let groups = data.get("groups").and_then(|v| v.as_array());
    let group_count = groups.map_or(0, |g| g.len());

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!("Compound Split ({} groups)", group_count))
            .into(),
    );

    if let Some(groups) = groups {
        for (idx, group) in groups.iter().enumerate() {
            let tag_name = group.get("tag_name").and_then(|v| v.as_str()).unwrap_or("?");
            let compound = group.get("compound_value").and_then(|v| v.as_str()).unwrap_or("?");
            let split_parts: Vec<&str> = group.get("split_parts")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let mut items = Vec::new();
            items.push(kv("Compound value", compound));
            items.push(kv("Split into", &split_parts.join(", ")));

            if let Some(files) = group.get("files").and_then(|v| v.as_array()) {
                for file in files {
                    let name = file.get("display_name").and_then(|v| v.as_str()).unwrap_or("?");
                    items.push(kv("", name));
                }
            }

            sections.push(titled_section(
                &format!("Group {} — {} \"{}\"", idx + 1, tag_name, compound),
                items,
            ));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Split All"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render missing album (single signals) resolution data.
pub fn render_missing_album(data: &serde_json::Value) -> Node {
    let signals = data.as_array();
    let signal_count = signals.map_or(0, |s| s.len());

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!("Missing Album ({} groups)", signal_count))
            .into(),
    );

    if let Some(signals) = signals {
        for (idx, signal) in signals.iter().enumerate() {
            let key = signal.get("key").and_then(|v| v.as_str()).unwrap_or("?");
            let signal_data = signal.get("data");
            let artist = signal_data
                .and_then(|d| d.get("artist"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");

            let mut items = Vec::new();
            if let Some(tracks) = signal_data.and_then(|d| d.get("tracks")).and_then(|v| v.as_array()) {
                for track in tracks {
                    let title = track.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                    let path = track.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                    items.push(kv(title, path));
                }
            }

            sections.push(titled_section(
                &format!("Group {} — {} ({})", idx + 1, artist, key),
                items,
            ));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Set Album"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render disc extraction resolution data.
pub fn render_disc_extraction(data: &serde_json::Value) -> Node {
    let groups = data.get("groups").and_then(|v| v.as_array());
    let group_count = groups.map_or(0, |g| g.len());

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!("Disc Extraction ({} groups)", group_count))
            .into(),
    );

    if let Some(groups) = groups {
        for (idx, group) in groups.iter().enumerate() {
            let description = group.get("description").and_then(|v| v.as_str()).unwrap_or("?");
            let disc_value = group.get("disc_value").and_then(|v| v.as_str()).unwrap_or("?");

            let mut items = Vec::new();
            items.push(kv("Disc value", disc_value));

            if let Some(files) = group.get("files").and_then(|v| v.as_array()) {
                for file in files {
                    let path = file.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                    let original = file.get("original_value").and_then(|v| v.as_str()).unwrap_or("?");
                    let cleaned = file.get("cleaned_value").and_then(|v| v.as_str()).unwrap_or("?");
                    items.push(kv(path, &format!("{original} \u{2192} {cleaned}")));
                }
            }

            sections.push(titled_section(
                &format!("Group {} — {}", idx + 1, description),
                items,
            ));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Extract All"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

// ============================================================================
// Group-review resolution views
// ============================================================================

/// Render manual review resolution data (redundant duplicates, deploy conflicts,
/// metadata duplicates, same recording).
pub fn render_manual_review(title: &str, data: &serde_json::Value) -> Node {
    let groups = data.get("groups").and_then(|v| v.as_array());
    let group_count = groups.map_or(0, |g| g.len());

    let mut sections = Vec::new();
    sections.push(
        h3().class("mm-section__title")
            .text(format!("{title} ({group_count} groups)"))
            .into(),
    );

    if let Some(groups) = groups {
        for (idx, group) in groups.iter().enumerate() {
            let label = group.get("label").and_then(|v| v.as_str()).unwrap_or("?");

            let mut items = Vec::new();
            if let Some(files) = group.get("files").and_then(|v| v.as_array()) {
                for file in files {
                    let path = file.get("corpus_path").and_then(|v| v.as_str()).unwrap_or("?");
                    let context = file.get("context").and_then(|v| v.as_str()).unwrap_or("");
                    if context.is_empty() {
                        items.push(kv("", path));
                    } else {
                        items.push(kv(path, context));
                    }
                }
            }

            sections.push(titled_section(
                &format!("Group {} — {}", idx + 1, label),
                items,
            ));
        }
    }

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_cancel()")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}
