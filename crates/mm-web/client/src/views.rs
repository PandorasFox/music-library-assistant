//! View renderers — each lateral view's content as HTML Node trees.
//!
//! Typed functions accept mm-meta structs so the compiler catches field name
//! mismatches. Functions that remain on `&serde_json::Value` are generic
//! display utilities or use composite responses without a single mm-meta type.

use mm_meta::domain_query_types::SessionEditDetail;
use mm_meta::protocol::DecisionDetail;
use mm_meta::views::cluster_deploy::DeployModalData;
use mm_meta::views::{
    DeployStatus, EditHistoryData, ExternalMatchesData, InsightsData,
};
use mm_meta::witch_types::{WitchStatus, WorkStateSnapshot};
use mm_ui::domain_types::DeployTab;
use mm_ui::view_state::lateral::deploy::{DeployViewData, initial_tab};
use mm_ui::view_state::lateral::history::{
    EditDetailEntry, SessionListEntry, group_edits, relative_timestamp,
};
use mm_ui::html::style::color_to_css;
use mm_ui::html::{self, div, h3, section, span, Node};
use mm_ui::view_state::lateral::health::{BucketEntry, CachedBucketEntries, InsightType};
use mm_ui::resolutions::tag_canonicity::TagCanonicityViewState;
use mm_ui::resolutions::compound_split::CompoundSplitViewState;
use mm_ui::resolutions::directory_cluster::DirectoryClusterState;
use mm_ui::resolutions::manual_review::ManualReviewState;
use mm_ui::resolutions::missing_album::MissingAlbumState;
use mm_ui::resolutions::disc_extraction::DiscExtractionState;

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

/// Key-value row with a wizard detail toggle (🪄).
/// Clicking the wand opens a centered overlay with detail content.
/// `title` is shown as a heading in the overlay pane.
pub fn kv_wizard(key: &str, val: &str, wizard_id: &str, title: &str, detail_items: Vec<Node>) -> Node {
    div()
        .class("mm-kv mm-kv--wizard")
        .child(span().class("mm-kv__key").text(key))
        .child(span().class("mm-kv__val").text(val))
        .child(
            span()
                .class("mm-wizard__toggle")
                .attr("data-wizard-id", wizard_id)
                .attr("data-wizard-title", title)
                .attr("onclick", "window.__mm_wizard_toggle(this)")
                .text("\u{1FA84}"),
        )
        .child(
            div()
                .class("mm-wizard__data mm-hidden")
                .attr("id", format!("mm-wd-{wizard_id}"))
                .children(detail_items),
        )
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
// Bucket entry rendering (Insights)
// ============================================================================

/// Render a single `BucketEntry` (from insights) as a colored kv row.
/// Actionable entries become clickable links to resolution routes.
fn render_bucket_entry(entry: &BucketEntry) -> Node {
    let count_str = match entry.count {
        Some(c) => c.to_string(),
        None => "-".to_string(),
    };
    let css_color = color_to_css(entry.color);
    let style = format!("color:{css_color}");

    match insight_route(&entry.insight_type) {
        Some(route) => {
            div()
                .class("mm-kv mm-kv--clickable")
                .attr("onclick", &format!("window.__mm_navigate_route('{route}')"))
                .attr("style", &format!("{style};cursor:pointer"))
                .child(span().class("mm-kv__key").text(&entry.label))
                .child(span().class("mm-kv__val").text(&count_str))
                .child(span().class("mm-kv__action").text("\u{2192}"))
                .into()
        }
        None => {
            div()
                .class("mm-kv")
                .attr("style", &style)
                .child(span().class("mm-kv__key").text(&entry.label))
                .child(span().class("mm-kv__val").text(&count_str))
                .into()
        }
    }
}


/// Map an `InsightType` to its web resolution route.
///
/// Returns `None` for informational entries that have no resolution workflow.
/// The route strings match the hash-based routing used by the web client.
fn insight_route(ty: &InsightType) -> Option<String> {
    match ty {
        InsightType::CorpusMtimeOnly | InsightType::CorpusOobTagSync => {
            Some("resolve/oob-sync".into())
        }
        InsightType::CorpusOobTagConflict => Some("resolve/oob-conflict/two-way".into()),
        InsightType::CorpusFilesUnindexed => None, // auto-indexed by Witch
        InsightType::CorpusFilesMissing => Some("resolve/missing-files/restorable".into()),
        InsightType::CorpusDirectoriesMissing => Some("resolve/missing-directories".into()),
        InsightType::CorpusFilesRelocated => Some("resolve/moved-files".into()),
        InsightType::CorpusCorruptFiles => Some("resolve/corrupt-files".into()),
        InsightType::CorpusLosslessRemuxCandidates => Some("resolve/lossless-remux".into()),
        InsightType::CrossSourceOverlaps | InsightType::ReleaseOverlaps => {
            Some("resolve/directory-cluster".into())
        }
        InsightType::SubparDuplicates => Some("resolve/subpar-duplicates".into()),
        InsightType::RedundantDuplicates => Some("resolve/redundant-duplicates".into()),
        InsightType::SameRecordingDifferentRelease => None, // informational
        InsightType::InconsistentAlbumArtist => {
            Some("resolve/inconsistent-album-artist/ALBUMARTIST".into())
        }
        InsightType::TagCanonicity { ref tag_name } => {
            let encoded = js_sys::encode_uri_component(tag_name);
            Some(format!(
                "resolve/tag-canonicity/{encoded}?zone=corpus&filter_existing_canonicals=true"
            ))
        }
        InsightType::CompoundTagValueSafe { ref tag_name } => {
            let encoded = js_sys::encode_uri_component(tag_name);
            Some(format!(
                "resolve/compound-split/{encoded}?zone=corpus&safe=true"
            ))
        }
        InsightType::CompoundTagValueReview { ref tag_name } => {
            let encoded = js_sys::encode_uri_component(tag_name);
            Some(format!(
                "resolve/compound-split/{encoded}?zone=corpus&safe=false"
            ))
        }
        InsightType::MissingAlbumSingle => Some("resolve/missing-album".into()),
        InsightType::DiscExtraction => Some("resolve/disc-extraction".into()),
        InsightType::PathTagMismatch => Some("resolve/path-tag-mismatch".into()),
        InsightType::OtherSignal { .. } => None,
        // Informational entries
        InsightType::CorpusFilesInCorpus
        | InsightType::CorpusFilesIndexed
        | InsightType::CorpusImagesInCorpus => None,
    }
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
    let entries = CachedBucketEntries::from_insights_data(insights);
    let mut sections = Vec::new();

    // Alert banners for high-priority corpus issues (web-specific affordance).
    for entry in &entries.corpus {
        let count = entry.count.unwrap_or(0);
        if count == 0 {
            continue;
        }
        let (alert_class, button_label) = match entry.insight_type {
            InsightType::CorpusFilesMissing | InsightType::CorpusCorruptFiles => {
                ("mm-alert mm-alert--warning", "Resolve")
            }
            InsightType::CorpusFilesRelocated => ("mm-alert mm-alert--info", "Resolve"),
            _ => continue,
        };
        let mut alert = div()
            .class(alert_class)
            .child(span().class("mm-alert__text").text(
                format!("{} {}", count, entry.label.to_lowercase()),
            ));
        if let Some(route) = insight_route(&entry.insight_type) {
            alert = alert.child(
                html::button()
                    .class("mm-btn mm-alert__action")
                    .attr("onclick", &format!("window.__mm_navigate_route('{route}')"))
                    .text(button_label),
            );
        }
        sections.push(alert.into());
    }

    // Corpus file stats.
    let corpus_items: Vec<Node> = entries.corpus.iter().map(|e| render_bucket_entry(e)).collect();
    sections.push(titled_section("Corpus Files", corpus_items));

    // Tag health bucket.
    if !entries.placeholder.is_empty() {
        let tag_items: Vec<Node> = entries.placeholder.iter().map(|e| render_bucket_entry(e)).collect();
        sections.push(titled_section("Tag Health", tag_items));
    }

    // Other signals bucket.
    let other_items: Vec<Node> = entries
        .other
        .iter()
        .filter(|e| e.count.unwrap_or(0) > 0)
        .map(|e| render_bucket_entry(e))
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
// External Matches view — structured to match TUI sections
// ============================================================================

/// Full External Matches page using the same three-section layout as the TUI:
/// Actions, Matches (untagged + confidence tiers), Release Packing.
///
/// Structure preserves `mm-fetch-progress` and `mm-external-data` div IDs
/// for the polling refresh to update independently.
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

/// Render external matches data section (for polling refresh).
///
/// Rebuilds the Matches + Release Packing sections with current data,
/// maintaining the same structure as the full page render.
pub fn render_external_matches_data(data: &ExternalMatchesData) -> Node {
    use mm_meta::signals::packing_category::PackingCategory;
    let mut sections = Vec::new();

    // Matches section.
    let has_matches = !data.untagged_entries.is_empty() || !data.confidence_buckets.is_empty();
    if has_matches {
        let mut match_items: Vec<Node> = Vec::new();

        if !data.untagged_entries.is_empty() {
            match_items.push(kv(
                "Untagged matches",
                &data.untagged_entries.len().to_string(),
            ));
        }

        for b in &data.confidence_buckets {
            let label = format!("{} ({})", b.tier.label(), b.total);
            let href = format!("#/external-matches/acoustid/{}", tier_to_route_str(b.tier));
            match_items.push(nav_kv(&label, &b.total.to_string(), &href));
        }

        sections.push(titled_section("Matches", match_items));
    }

    // Release Packing section.
    let packing_order: Vec<(PackingCategory, usize, Option<&str>)> = vec![
        (PackingCategory::Perfect, data.packing_perfect_count, Some("perfect")),
        (PackingCategory::FullMatches, data.packing_full_match_count, Some("full-match")),
        (PackingCategory::Singles, data.packing_singles_count, Some("singles")),
        (PackingCategory::Incomplete, data.packing_incomplete_count, Some("incomplete")),
        (PackingCategory::LowConfidence, data.packing_low_confidence_count, Some("low-confidence")),
        (PackingCategory::Knots, data.packing_knots_count, None),
        (PackingCategory::UnsolvedConflict, data.unsolved_conflict_count, None),
        (PackingCategory::UnsolvedNoRelease, data.unsolved_no_release_count, None),
        (PackingCategory::UnsolvedNoMatch, data.unsolved_no_match_count, None),
    ];

    let mut packing_items: Vec<Node> = Vec::new();
    for (cat, count, route) in &packing_order {
        if *count > 0 {
            match route {
                Some(r) => {
                    let href = format!("#/external-matches/review/{r}");
                    packing_items.push(nav_kv(cat.label(), &count.to_string(), &href));
                }
                None => {
                    packing_items.push(kv(cat.label(), &count.to_string()));
                }
            }
        }
    }

    if data.pinned_conflict_count > 0 {
        packing_items.push(kv("Pinned conflicts", &data.pinned_conflict_count.to_string()));
    }
    if data.va_override_count > 0 {
        packing_items.push(kv("VA overrides", &data.va_override_count.to_string()));
    }

    if !packing_items.is_empty() {
        sections.push(titled_section("Release Packing", packing_items));
    }

    div().children(sections).into()
}

/// Map display-level ConfidenceTier to the AcoustidConfidence route segment.
/// Perfect/VeryHigh/High all map to "high", Medium -> "medium", Low -> "low".
fn tier_to_route_str(tier: mm_meta::views::ConfidenceTier) -> &'static str {
    use mm_meta::views::ConfidenceTier;
    match tier {
        ConfidenceTier::Perfect | ConfidenceTier::VeryHigh | ConfidenceTier::High => "high",
        ConfidenceTier::Medium => "medium",
        ConfidenceTier::Low => "low",
    }
}

// ============================================================================
// Edit History view — typed using SessionListEntry + EditDetailEntry
// ============================================================================

/// Render session list using typed `SessionListEntry` from mm-ui.
///
/// Wraps each `EditSessionSummary` in a `SessionListEntry` to get consistent
/// rendering with the TUI (relative timestamps via `relative_timestamp()`).
pub fn render_edit_history_content(data: &EditHistoryData) -> Node {
    if data.sessions.is_empty() {
        return span().class("mm-kv__val").text("No edit sessions").into();
    }

    let entries: Vec<SessionListEntry> = data
        .sessions
        .iter()
        .map(|s| SessionListEntry { summary: s.clone() })
        .collect();

    let items: Vec<Node> = entries
        .iter()
        .map(|entry| {
            let s = &entry.summary;
            let relative = relative_timestamp(&s.earliest_at);
            let label = if s.session_id.len() > 30 {
                format!("{}...", &s.session_id.chars().take(27).collect::<String>())
            } else {
                s.session_id.clone()
            };

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
                                .text(&label),
                        )
                        .child(span().class("mm-kv__val").text(format!(
                            "{} edit{}, {} file{} \u{2014} {}",
                            s.edit_count,
                            if s.edit_count == 1 { "" } else { "s" },
                            s.inode_count,
                            if s.inode_count == 1 { "" } else { "s" },
                            relative,
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

/// Render session detail using typed `SessionEditDetail` and `EditDetailEntry`.
///
/// Groups edits via the shared `group_edits()` from mm-ui, producing the same
/// CommonEdit / FileHeader / FileEdit structure as the TUI.
pub fn render_session_detail_typed(detail: &SessionEditDetail) -> Node {
    if detail.edits.is_empty() {
        return span().class("mm-kv__val").text("No edits").into();
    }

    let entries = group_edits(detail.edits.clone(), &detail.inode_paths);

    let rows: Vec<Node> = entries
        .iter()
        .filter_map(|entry| match entry {
            EditDetailEntry::CommonEdit {
                field_name,
                old_value,
                new_value,
                paths,
                ..
            } => {
                let old = old_value.as_deref().unwrap_or("\u{2205}");
                let new = new_value.as_deref().unwrap_or("\u{2205}");
                Some(
                    div()
                        .class("mm-edit-row mm-edit-row--common")
                        .child(
                            span()
                                .class("mm-edit-field")
                                .text(format!("{} ({} files)", field_name, paths.len())),
                        )
                        .child(
                            span()
                                .class("mm-edit-diff")
                                .child(span().class("mm-edit-old").text(old))
                                .child(span().class("mm-edit-arrow").text(" \u{2192} "))
                                .child(span().class("mm-edit-new").text(new)),
                        )
                        .into(),
                )
            }
            EditDetailEntry::PerFileSeparator => Some(
                html::hr().class("mm-separator").into(),
            ),
            EditDetailEntry::FileHeader { path } => Some(
                div()
                    .class("mm-edit-file-header")
                    .child(span().class("mm-edit-path").text(path))
                    .into(),
            ),
            EditDetailEntry::FileEdit { edit, path } => {
                let old = edit.old_value.as_deref().unwrap_or("\u{2205}");
                let new = edit.new_value.as_deref().unwrap_or("\u{2205}");
                Some(
                    div()
                        .class("mm-edit-row")
                        .child(span().class("mm-edit-path").text(path))
                        .child(span().class("mm-edit-field").text(&edit.field_name))
                        .child(
                            span()
                                .class("mm-edit-diff")
                                .child(span().class("mm-edit-old").text(old))
                                .child(span().class("mm-edit-arrow").text(" \u{2192} "))
                                .child(span().class("mm-edit-new").text(new)),
                        )
                        .into(),
                )
            }
        })
        .collect();
    div().class("mm-session-edits").children(rows).into()
}

// ============================================================================
// Deploy view — typed using DeployViewData + DeployTab
// ============================================================================

/// Render deploy content using the shared `DeployViewData` enum.
///
/// Fetches `DeployModalData` from the server, then branches:
/// - `UpToDate` → simple library file counts
/// - `Preview`  → tabbed display matching the TUI (Healthy/New/Conflicts/Leftover/Stale)
pub fn render_deploy_content(status: &DeployStatus, modal: Option<&DeployModalData>) -> Node {
    let view_data = match modal {
        Some(data) if status.needs_action => DeployViewData::Preview {
            cached_data: data.clone(),
        },
        _ => DeployViewData::UpToDate {
            library_file_counts: status.library_file_counts.clone(),
        },
    };

    match view_data {
        DeployViewData::UpToDate { library_file_counts } => {
            let mut items = vec![kv("Status", "Up to date")];
            for (name, count) in &library_file_counts {
                items.push(kv(name, &count.to_string()));
            }
            titled_section("Deploy", items)
        }
        DeployViewData::Preview { cached_data } => {
            render_deploy_preview(&cached_data)
        }
    }
}

/// Render the tabbed deploy preview with per-tab content.
fn render_deploy_preview(data: &DeployModalData) -> Node {
    let counts = data.tab_counts();
    let active = initial_tab(data);
    let mut sections = Vec::new();

    // Action buttons at top.
    if data.total_operations() > 0 {
        sections.push(
            div()
                .class("mm-buttons")
                .child(
                    html::button()
                        .class("mm-btn")
                        .attr("style", "border-color:var(--c-green)")
                        .attr("onclick", "window.__mm_stage_deploy()")
                        .text("Stage Deploy"),
                )
                .into(),
        );
    }

    // Summary line.
    let total_ops = data.total_operations();
    sections.push(kv("Operations", &format!("{} total", total_ops)));

    // Tab bar — clickable tabs that switch content via JS.
    let tab_bar = div()
        .class("mm-tab-bar")
        .children(DeployTab::all().iter().map(|tab| {
            let label = format!("{} ({})", tab.label(), counts[tab.index()]);
            let is_active = *tab == active;
            div()
                .class("mm-tab")
                .class_if("mm-tab--active", is_active)
                .attr(
                    "onclick",
                    format!("window.__mm_deploy_tab('{}')", tab.label().to_lowercase()),
                )
                .text(label)
                .into()
        }));
    sections.push(tab_bar.into());

    // Per-tab content panels (all rendered, only active shown via CSS/JS).
    for tab in DeployTab::all() {
        let panel_id = format!("deploy-tab-{}", tab.label().to_lowercase());
        let display = if *tab == active { "" } else { "display:none" };
        let content = render_deploy_tab_content(data, *tab);
        sections.push(
            div()
                .attr("id", &panel_id)
                .attr("style", display)
                .class("mm-deploy-panel")
                .child(content)
                .into(),
        );
    }

    div().children(sections).into()
}

/// Render content for a single deploy tab.
fn render_deploy_tab_content(data: &DeployModalData, tab: DeployTab) -> Node {
    match tab {
        DeployTab::Healthy => {
            if data.healthy.is_empty() {
                return span().class("mm-kv__val").text("No healthy files").into();
            }
            let items: Vec<Node> = data
                .healthy
                .iter()
                .take(200)
                .map(|f| kv(&f.library_name, &f.deploy_path))
                .collect();
            let mut result = vec![titled_section(
                &format!("Healthy ({})", data.healthy.len()),
                items,
            )];
            if data.healthy.len() > 200 {
                result.push(
                    span()
                        .class("mm-kv__val")
                        .text(format!("...and {} more", data.healthy.len() - 200))
                        .into(),
                );
            }
            div().children(result).into()
        }
        DeployTab::New => {
            if data.new_by_dir.is_empty() {
                return span().class("mm-kv__val").text("No new files to deploy").into();
            }
            let items: Vec<Node> = data
                .new_by_dir
                .iter()
                .map(|d| {
                    let label = if d.sidecar_count > 0 {
                        format!("{} files + {} sidecars", d.count, d.sidecar_count)
                    } else {
                        format!("{} files", d.count)
                    };
                    kv(&d.directory, &label)
                })
                .collect();
            titled_section(&format!("New ({})", data.new.len()), items)
        }
        DeployTab::Conflicts => {
            if data.conflicts.is_empty() {
                return span().class("mm-kv__val").text("No conflicts").into();
            }
            let items: Vec<Node> = data
                .conflicts
                .iter()
                .map(|c| {
                    let files: Vec<&str> = c
                        .conflicting_files
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect();
                    kv(&c.deploy_path, &files.join(", "))
                })
                .collect();
            titled_section(
                &format!("Conflicts ({})", data.conflicts.len()),
                items,
            )
        }
        DeployTab::Leftover => {
            if data.leftover_by_dir.is_empty() {
                return span().class("mm-kv__val").text("No leftover files").into();
            }
            let items: Vec<Node> = data
                .leftover_by_dir
                .iter()
                .map(|d| kv(&d.directory, &format!("{} files", d.count)))
                .collect();
            titled_section(
                &format!("Leftover ({})", data.leftover.len()),
                items,
            )
        }
        DeployTab::Stale => {
            if data.stale.is_empty() {
                return span().class("mm-kv__val").text("No stale files").into();
            }
            let items: Vec<Node> = data
                .stale
                .iter()
                .take(200)
                .map(|f| {
                    div()
                        .class("mm-edit-row")
                        .child(span().class("mm-edit-old").text(&f.library_path))
                        .child(span().class("mm-edit-arrow").text(" \u{2192} "))
                        .child(span().class("mm-edit-new").text(&f.expected_path))
                        .into()
                })
                .collect();
            let mut result = vec![titled_section(
                &format!("Stale ({})", data.stale.len()),
                items,
            )];
            if data.stale.len() > 200 {
                result.push(
                    span()
                        .class("mm-kv__val")
                        .text(format!("...and {} more", data.stale.len() - 200))
                        .into(),
                );
            }
            div().children(result).into()
        }
    }
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

        let diff_entries = mm_meta::mutations::coalesce_diff_entries(
            d.mutations.iter().flat_map(|m| m.diff_entries()).collect(),
        );

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
            // Split "[inodes] TAG" labels into purple inodes + cyan tag name.
            let label_node = if let Some(bracket_end) = entry.label.find(']') {
                let inodes_part = &entry.label[..=bracket_end];
                let tag_part = entry.label[bracket_end + 1..].trim_start();
                span()
                    .class("mm-edit-field")
                    .child(span().class("mm-edit-inodes").text(inodes_part))
                    .text(format!(" {tag_part}"))
            } else {
                span().class("mm-edit-field").text(&entry.label)
            };

            decision_items.push(
                div()
                    .class("mm-edit-row")
                    .child(label_node)
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
    if let Some(root) = config.get("storage_root").and_then(|v| v.as_str()) {
        root_items.push(config_field_readonly("storage root", root));
    }
    if let Some(root) = config.get("libraries_root").and_then(|v| v.as_str()) {
        root_items.push(config_field_readonly("libraries root", root));
    }
    if let Some(root) = config.get("stash_root").and_then(|v| v.as_str()) {
        root_items.push(config_field_readonly("stash root", root));
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
pub fn render_release_review(data: &mm_meta::views::external_matches::ReleaseReviewData) -> Node {
    let mut sections = Vec::new();

    sections.push(
        div()
            .child(
                html::a()
                    .class("mm-link")
                    .attr("href", "#/external-matches")
                    .text("\u{2190} Back to External Matches"),
            )
            .into(),
    );

    let count = data.releases.len();

    // "Approve All" button when there are releases to approve.
    if count > 0 {
        let all_ids: Vec<&str> = data.releases.iter().map(|r| r.release_id.as_str()).collect();
        let ids_json = serde_json::to_string(&all_ids).unwrap_or_default();
        sections.push(
            div()
                .class("mm-buttons")
                .child(
                    html::button()
                        .class("mm-btn")
                        .attr("onclick", &format!(
                            "window.__mm_approve_releases('{}')",
                            ids_json.replace('\'', "\\'"),
                        ))
                        .text(&format!("Approve All ({count})")),
                )
                .into(),
        );
    }

    for release in &data.releases {
        let avg_pct = (release.avg_confidence * 100.0) as u32;

        let mut track_items = Vec::new();

        // Per-release approve button.
        let rid_json = serde_json::to_string(&[&release.release_id]).unwrap_or_default();
        track_items.push(
            div()
                .class("mm-buttons")
                .child(
                    html::button()
                        .class("mm-btn mm-btn--sm")
                        .attr("onclick", &format!(
                            "window.__mm_approve_releases('{}')",
                            rid_json.replace('\'', "\\'"),
                        ))
                        .text("Approve"),
                )
                .into(),
        );

        track_items.push(kv("Category", &release.category));
        track_items.push(kv("Tracks", &format!("{}/{} matched", release.matched_count, release.track_count)));
        track_items.push(kv("Avg Confidence", &format!("{avg_pct}%")));

        for track in &release.tracks {
            let val = if let Some(ref name) = track.matched_display_name {
                let conf_str = track.confidence
                    .map(|c| format!(" [{:.0}%]", c * 100.0))
                    .unwrap_or_default();
                format!("{name}{conf_str}")
            } else {
                "(unmatched)".to_string()
            };

            track_items.push(kv(
                &format!("{}. {}", track.position, track.mb_title),
                &val,
            ));
        }

        sections.push(titled_section(
            &format!("{} \u{2014} {}", release.artist, release.title),
            track_items,
        ));
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

        let config_btn = span()
            .class("mm-dir-config-btn")
            .attr(
                "onclick",
                format!("event.stopPropagation(); window.__mm_open_dir_config('{escaped_path}')"),
            )
            .text("Config");

        let bulk_tags_btn = span()
            .class("mm-dir-config-btn")
            .attr(
                "onclick",
                format!("event.stopPropagation(); window.__mm_bulk_tag_dir('{escaped_path}')"),
            )
            .text("Bulk Tags");

        let header = div()
            .class("mm-dir-header")
            .attr(
                "onclick",
                format!("window.__mm_expand_dir('{escaped_path}')"),
            )
            .child(span().text(format!("{name} ({file_count} files)")))
            .child(config_btn)
            .child(bulk_tags_btn);

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
// Dir config editor overlay
// ============================================================================

/// Render the directory config editor popup.
///
/// `path` is the corpus-relative directory path.
/// `source_dir` is the current config (None if no explicit config).
pub fn render_dir_config_editor(path: &str, source_dir: &Option<mm_meta::config::SourceDir>) -> Node {
    let (libraries, can_stash_dupes, interior_dupes, path_schema, enable_acoustid, pinned_release) =
        match source_dir {
            Some(sd) => (
                sd.libraries.join(", "),
                sd.can_stash_dupes,
                sd.interior_dupes,
                sd.path_schema.as_ref().map(|s| s.template.as_str()).unwrap_or(""),
                sd.enable_acoustid,
                sd.pinned_release.as_deref().unwrap_or(""),
            ),
            None => (String::new(), None, None, "", None, ""),
        };

    let escaped_path = path.replace('\'', "\\'");

    let mut rows = Vec::new();

    // Libraries
    rows.push(
        div()
            .class("mm-dir-config__field")
            .child(html::label().attr("for", "dc-libraries").text("Libraries"))
            .child(
                html::input()
                    .attr("type", "text")
                    .attr("id", "dc-libraries")
                    .attr("name", "libraries")
                    .attr("value", &libraries)
                    .attr("placeholder", "(none)")
                    .class("mm-input"),
            )
            .into(),
    );

    // Tri-state booleans
    for (id, label_text, value) in [
        ("dc-can-stash-dupes", "Can stash dupes", can_stash_dupes),
        ("dc-interior-dupes", "Interior dupes", interior_dupes),
        ("dc-enable-acoustid", "AcoustID lookup", enable_acoustid),
    ] {
        let opt_inherit = html::option().attr("value", "inherit").text("(inherit)");
        let opt_true = html::option().attr("value", "true").text("Yes");
        let opt_false = html::option().attr("value", "false").text("No");
        let (opt_inherit, opt_true, opt_false) = match value {
            None => (opt_inherit.bool_attr("selected"), opt_true, opt_false),
            Some(true) => (opt_inherit, opt_true.bool_attr("selected"), opt_false),
            Some(false) => (opt_inherit, opt_true, opt_false.bool_attr("selected")),
        };
        rows.push(
            div()
                .class("mm-dir-config__field")
                .child(html::label().attr("for", id).text(label_text))
                .child(
                    html::select()
                        .attr("id", id)
                        .attr("name", id)
                        .class("mm-select")
                        .child(opt_inherit)
                        .child(opt_true)
                        .child(opt_false),
                )
                .into(),
        );
    }

    // Path schema
    rows.push(
        div()
            .class("mm-dir-config__field")
            .child(html::label().attr("for", "dc-path-schema").text("Path schema"))
            .child(
                html::input()
                    .attr("type", "text")
                    .attr("id", "dc-path-schema")
                    .attr("name", "path_schema")
                    .attr("value", path_schema)
                    .attr("placeholder", "(inherit)")
                    .class("mm-input"),
            )
            .into(),
    );

    // Pinned release
    rows.push(
        div()
            .class("mm-dir-config__field")
            .child(html::label().attr("for", "dc-pinned-release").text("Pinned release"))
            .child(
                html::input()
                    .attr("type", "text")
                    .attr("id", "dc-pinned-release")
                    .attr("name", "pinned_release")
                    .attr("value", pinned_release)
                    .attr("placeholder", "(none)")
                    .class("mm-input"),
            )
            .into(),
    );

    let buttons = div()
        .class("mm-dir-config__buttons")
        .child(
            html::button()
                .class("mm-btn")
                .attr("style", "border-color:var(--c-green)")
                .attr("onclick", format!("window.__mm_dir_config_save('{escaped_path}')"))
                .text("Confirm"),
        )
        .child(
            html::button()
                .class("mm-btn")
                .attr("style", "border-color:var(--c-red)")
                .attr("onclick", "window.__mm_dir_config_cancel()")
                .text("Cancel"),
        );

    div()
        .class("mm-dir-config-editor")
        .child(h3().text(&format!("Config: {path}")))
        .children(rows)
        .child(buttons)
        .into()
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
                    .child(
                        html::button()
                            .class("mm-btn mm-tag-delete")
                            .attr("type", "button")
                            .attr("data-tag-name", tag_name)
                            .attr(
                                "onclick",
                                "window.__mm_tag_delete(this.dataset.tagName, this)",
                            )
                            .text("\u{00d7}"),
                    )
                    .into(),
            );
        }
    }

    // Status message area (hidden by default, shown by JS on save/error).
    let status = div()
        .attr("id", "mm-tag-status")
        .class("mm-tag-status")
        .attr("style", "display:none");

    // Toolbar: Back, Add Tag, Save Changes.
    let toolbar = div()
        .class("mm-buttons mm-tag-toolbar")
        .child(
            html::button()
                .class("mm-btn")
                .attr("type", "button")
                .attr("onclick", "history.back()")
                .text("Back"),
        )
        .child(
            html::button()
                .class("mm-btn")
                .attr("type", "button")
                .attr("onclick", "window.__mm_tag_add()")
                .text("+ Add Tag"),
        )
        .child(
            html::button()
                .class("mm-btn mm-tag-save-btn")
                .attr("type", "button")
                .attr("onclick", "window.__mm_tag_save()")
                .text("Save Changes"),
        );

    div()
        .class("mm-tag-editor")
        .attr("data-inode", inode.to_string())
        .child(h3().class("mm-section__title").text(&format!("Tags \u{2014} {path}")))
        .child(toolbar)
        .child(status)
        .child(
            div()
                .class("mm-tag-editor__header")
                .child(span().class("mm-tag-header-name").text("Tag"))
                .child(span().class("mm-tag-header-value").text("Value"))
                .child(span().class("mm-tag-header-action").text("")),
        )
        .child(
            div()
                .attr("id", "mm-tag-rows")
                .children(rows),
        )
        .into()
}

/// Render a bulk (multi-file) tag editor from a server-computed `BulkTagAggregate`.
///
/// The `data` JSON has shape: `{ file_count, inodes, dir_label, tags: [{ name, uniform_value, presence }] }`.
/// Tags uniform across all files are editable; mixed/partial tags are shown readonly.
pub fn render_bulk_tag_editor(data: &serde_json::Value) -> Node {
    let file_count = data.get("file_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let dir_label = data.get("dir_label").and_then(|v| v.as_str()).unwrap_or("");
    let inodes: Vec<i64> = data
        .get("inodes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    let tags = data.get("tags").and_then(|v| v.as_array());

    let mut rows = Vec::new();

    if let Some(tags_arr) = tags {
        for (i, tag) in tags_arr.iter().enumerate() {
            let tag_name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let uniform_value = tag.get("uniform_value").and_then(|v| v.as_str());
            let presence = tag.get("presence").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            let is_uniform = uniform_value.is_some() && presence == file_count;
            let display_value = if let Some(val) = uniform_value {
                if presence == file_count { val } else { "[partial]" }
            } else if presence == file_count {
                "[mixed]"
            } else {
                "[partial]"
            };

            let name_id = format!("tag-name-{i}");
            let val_id = format!("tag-val-{i}");

            let mut val_input = html::input()
                .attr("type", "text")
                .attr("id", &val_id)
                .attr("value", display_value)
                .attr("data-tag", tag_name)
                .attr("data-original", display_value)
                .class("mm-tag-value");

            if !is_uniform {
                val_input = val_input
                    .attr("readonly", "")
                    .class("mm-tag-value mm-tag-value--mixed");
            }

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
                    .child(val_input)
                    .child({
                        let mut btn = html::button()
                            .class("mm-btn mm-tag-delete")
                            .attr("type", "button")
                            .attr("data-tag-name", tag_name)
                            .attr(
                                "onclick",
                                "window.__mm_tag_delete(this.dataset.tagName, this)",
                            )
                            .text("\u{00d7}");
                        // For non-uniform tags, embed value→inodes mapping
                        // so the save logic can build correct per-file drop ops.
                        if !is_uniform {
                            if let Some(vi) = tag.get("value_inodes") {
                                btn = btn.attr("data-value-inodes", &vi.to_string());
                            }
                        }
                        btn
                    })
                    .into(),
            );
        }
    }

    let inodes_csv: String = inodes.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");

    let status = div()
        .attr("id", "mm-tag-status")
        .class("mm-tag-status")
        .attr("style", "display:none");

    let toolbar = div()
        .class("mm-buttons mm-tag-toolbar")
        .child(
            html::button()
                .class("mm-btn")
                .attr("type", "button")
                .attr("onclick", "history.back()")
                .text("Back"),
        )
        .child(
            html::button()
                .class("mm-btn")
                .attr("type", "button")
                .attr("onclick", "window.__mm_tag_add()")
                .text("+ Add Tag"),
        )
        .child(
            html::button()
                .class("mm-btn mm-tag-save-btn")
                .attr("type", "button")
                .attr("onclick", "window.__mm_bulk_tag_save()")
                .text("Save Changes"),
        );

    div()
        .class("mm-tag-editor mm-tag-editor--bulk")
        .attr("data-inodes", &inodes_csv)
        .child(
            h3().class("mm-section__title")
                .text(&format!("Bulk Tags \u{2014} {dir_label} ({file_count} files)")),
        )
        .child(toolbar)
        .child(status)
        .child(
            div()
                .class("mm-tag-editor__header")
                .child(span().class("mm-tag-header-name").text("Tag"))
                .child(span().class("mm-tag-header-value").text("Value"))
                .child(span().class("mm-tag-header-action").text("")),
        )
        .child(
            div()
                .attr("id", "mm-tag-rows")
                .children(rows),
        )
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
            ("Drop All", "var(--c-yellow)", "window.__mm_resolve_action('ConfirmDrop')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
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
            ("Stash All", "var(--c-yellow)", "window.__mm_resolve_action('ConfirmStashAll')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
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
            ("Accept All", "var(--c-green)", "window.__mm_resolve_action('Acknowledge')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
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
            ("Stash All", "var(--c-yellow)", "window.__mm_resolve_action('ConfirmStashAll')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
        ],
    )
}

/// Render lossless remux resolution data.
pub fn render_lossless_remux(data: &serde_json::Value) -> Node {
    let files: Vec<String> = data
        .get("files")
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
                "Lossless Remux Candidates ({} files)",
                files.len(),
            ))
            .into(),
    );

    if !files.is_empty() {
        let rows: Vec<Node> = files
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
        sections.push(titled_section("Remux to FLAC", rows));
    }

    let mut btn_row = div().class("mm-buttons");
    if !files.is_empty() {
        btn_row = btn_row.child(
            html::button()
                .class("mm-btn")
                .attr("style", "border-color:var(--c-green)")
                .attr("onclick", "window.__mm_resolve_action('Confirm')")
                .text("Remux to FLAC"),
        );
    }
    btn_row = btn_row.child(
        html::button()
            .class("mm-btn")
            .attr("style", "border-color:var(--c-white)")
            .attr("onclick", "window.__mm_resolve_action('Cancel')")
            .text("Cancel"),
    );
    sections.push(btn_row.into());

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
            ("Restore All", "var(--c-green)", "window.__mm_resolve_action('ConfirmRestore')"),
            ("Drop All", "var(--c-yellow)", "window.__mm_resolve_action('ConfirmDrop')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
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
            ("Drop All", "var(--c-red)", "window.__mm_resolve_action('ConfirmDrop')"),
            ("Cancel", "var(--c-white)", "window.__mm_resolve_action('Cancel')"),
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
// Old dump-all renderers for V3 resolution types removed — replaced by
// per-group renderers below (render_*_group functions).

// ============================================================================
// Per-group renderers for V3 (paginated) resolution modals
// ============================================================================

/// Shared pagination header: title + prev/next buttons with position indicator.
fn render_pagination_header(title: &str, current_1indexed: usize, total: usize) -> Node {
    let prev_disabled = current_1indexed <= 1;
    let next_disabled = current_1indexed >= total;

    let mut prev_btn = html::button()
        .class("mm-btn mm-pagination__btn")
        .attr("onclick", "window.__mm_resolve_page('prev')")
        .text("\u{2039} Prev");
    if prev_disabled {
        prev_btn = prev_btn.class("mm-btn--disabled").attr("disabled", "true");
    }

    let mut next_btn = html::button()
        .class("mm-btn mm-pagination__btn")
        .attr("onclick", "window.__mm_resolve_page('next')")
        .text("Next \u{203a}");
    if next_disabled {
        next_btn = next_btn.class("mm-btn--disabled").attr("disabled", "true");
    }

    div()
        .class("mm-pagination")
        .child(h3().class("mm-pagination__title").text(title))
        .child(
            div()
                .class("mm-pagination__nav")
                .child(prev_btn)
                .child(span().class("mm-pagination__pos").text(format!("{} of {}", current_1indexed, total)))
                .child(next_btn),
        )
        .into()
}

/// Render the current group of a TagCanonicityViewState with a custom title prefix.
pub fn render_canonicity_group_titled(title_prefix: &str, state: &TagCanonicityViewState) -> Node {
    let data = &state.state.data;
    let inner = &data.inner;
    let idx = data.current_cluster;
    let total = inner.clusters.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No canonicity clusters").into();
    }

    let cluster = match inner.clusters.get(idx) {
        Some(c) => c,
        None => return span().class("mm-kv__val").text("Cluster index out of bounds").into(),
    };

    let title = format!("{} \u{2014} {}", title_prefix, inner.tag_name);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    // Decision field input
    let suggested = state.field.value();
    sections.push(
        div()
            .class("mm-decision-field")
            .child(html::label().class("mm-decision-field__label").text(&state.field.label))
            .child(
                html::input()
                    .attr("type", "text")
                    .attr("id", "mm-decision-field")
                    .attr("value", suggested)
                    .class("mm-decision-field__input"),
            )
            .into(),
    );

    // Variant list — each variant gets a wizard toggle showing all files
    let mut variant_items = Vec::new();
    for (vi, variant) in cluster.variants.iter().enumerate() {
        let file_count = variant.files.len();
        let file_names: Vec<&str> = variant.files.iter().map(|f| f.display_name.as_str()).collect();
        let preview = if file_names.len() <= 3 {
            file_names.join(", ")
        } else {
            format!("{}, ... +{}", file_names[..3].join(", "), file_names.len() - 3)
        };
        let detail: Vec<Node> = variant
            .files
            .iter()
            .map(|f| div().class("mm-wizard__item").text(&f.display_name).into())
            .collect();
        variant_items.push(kv_wizard(
            &format!("\u{201c}{}\u{201d} ({} files)", variant.value, file_count),
            &preview,
            &format!("v{vi}"),
            &format!("Track list for \u{201c}{}\u{201d}", variant.value),
            detail,
        ));
    }

    let heading = match &cluster.confirmed_canonical {
        Some(canonical) => format!("Canonical: \"{}\"", canonical),
        None => match &cluster.suggested_canonical {
            Some(suggested_val) => format!("Suggested: \"{}\"", suggested_val),
            None => "No suggestion".to_string(),
        },
    };
    sections.push(titled_section(&heading, variant_items));

    // Buttons
    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_confirm_with_field()")
                    .text("Confirm"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('FlagCanonical')")
                    .text("Flag Canonical"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render the current group of a CompoundSplitViewState.
pub fn render_compound_split_group(state: &CompoundSplitViewState) -> Node {
    let data = &state.state.data;
    let inner = &data.inner;
    let idx = data.current_group;
    let total = inner.groups.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No compound split groups").into();
    }

    let group = match inner.groups.get(idx) {
        Some(g) => g,
        None => return span().class("mm-kv__val").text("Group index out of bounds").into(),
    };

    let title = format!("Compound Split \u{2014} {} \"{}\"", group.tag_name, group.compound_value);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    // Decision field input
    let current_value = state.field.value();
    sections.push(
        div()
            .class("mm-decision-field")
            .child(html::label().class("mm-decision-field__label").text(&state.field.label))
            .child(
                html::input()
                    .attr("type", "text")
                    .attr("id", "mm-decision-field")
                    .attr("value", current_value)
                    .class("mm-decision-field__input"),
            )
            .into(),
    );

    // File list
    let mut file_items = Vec::new();
    if !group.matching_parts.is_empty() {
        file_items.push(kv("Matching parts", &group.matching_parts.join(", ")));
    }
    for file in &group.files {
        file_items.push(kv("", &file.display_name));
    }

    sections.push(titled_section(
        &format!("{} files", group.files.len()),
        file_items,
    ));

    // Buttons
    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_confirm_with_field()")
                    .text("Confirm Split"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('Canonicalize')")
                    .text("Mark as Entity"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render the current group of a DirectoryClusterState.
pub fn render_directory_cluster_group(state: &DirectoryClusterState) -> Node {
    let data = &state.data;
    let inner = &data.inner;
    let idx = data.current_cluster;
    let total = inner.clusters.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No directory clusters").into();
    }

    let cluster = match inner.clusters.get(idx) {
        Some(c) => c,
        None => return span().class("mm-kv__val").text("Cluster index out of bounds").into(),
    };

    let title = format!("Directory Cluster \u{2014} {}", cluster.cluster_key);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    let mut items = Vec::new();
    items.push(kv("Overlap", &format!("{} track pairs", cluster.overlap_count)));

    for (di, dir) in cluster.directories.iter().enumerate() {
        let file_count = dir.inodes.len();
        let detail: Vec<Node> = dir
            .paths
            .iter()
            .map(|p| div().class("mm-wizard__item").text(p).into())
            .collect();
        items.push(kv_wizard(
            &dir.path_suffix,
            &format!("{} files, {}", file_count, dir.format_summary),
            &format!("d{di}"),
            &format!("Files in {}", dir.path_suffix),
            detail,
        ));
    }

    sections.push(titled_section(
        &format!("{} directories", cluster.directories.len()),
        items,
    ));

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('Stash')")
                    .text("Stash"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-magenta)")
                    .attr("onclick", "window.__mm_resolve_action('MarkExpected')")
                    .text("Mark Expected"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render the current group of a ManualReviewState.
pub fn render_manual_review_group(title_prefix: &str, state: &ManualReviewState) -> Node {
    let data = &state.data;
    let inner = &data.inner;
    let idx = data.current_group;
    let total = inner.groups.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No review groups").into();
    }

    let group = match inner.groups.get(idx) {
        Some(g) => g,
        None => return span().class("mm-kv__val").text("Group index out of bounds").into(),
    };

    let title = format!("{} \u{2014} {}", title_prefix, group.label);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    let mut items = Vec::new();
    for (fi, file) in group.files.iter().enumerate() {
        let key = if file.context.is_empty() {
            String::new()
        } else {
            file.corpus_path.clone()
        };
        let val = if file.context.is_empty() {
            &file.corpus_path
        } else {
            &file.context
        };

        if let Some(ref meta) = file.meta {
            let mut detail = Vec::new();
            detail.push(
                div()
                    .class("mm-wizard__item")
                    .text(format!("Format: {}", meta.file_type))
                    .into(),
            );
            if let Some(dur) = meta.duration_ms {
                let secs = dur / 1000;
                detail.push(
                    div()
                        .class("mm-wizard__item")
                        .text(format!("Duration: {}:{:02}", secs / 60, secs % 60))
                        .into(),
                );
            }
            if let Some(br) = meta.bitrate_kbps {
                detail.push(
                    div()
                        .class("mm-wizard__item")
                        .text(format!("Bitrate: {} kbps", br))
                        .into(),
                );
            }
            if let Some(sr) = meta.sample_rate {
                detail.push(
                    div()
                        .class("mm-wizard__item")
                        .text(format!("Sample rate: {} Hz", sr))
                        .into(),
                );
            }
            detail.push(
                div()
                    .class("mm-wizard__item")
                    .text(format!(
                        "Size: {:.1} MB{}",
                        meta.file_size as f64 / 1_048_576.0,
                        if meta.has_pictures { " \u{1f5bc}" } else { "" },
                    ))
                    .into(),
            );
            for (tag_name, tag_val) in &meta.tags {
                detail.push(
                    div()
                        .class("mm-wizard__item mm-wizard__item--tag")
                        .text(format!("{}: {}", tag_name, tag_val))
                        .into(),
                );
            }
            items.push(kv_wizard(&key, val, &format!("f{fi}"), &format!("Details \u{2014} {}", file.corpus_path), detail));
        } else {
            items.push(kv(&key, val));
        }
    }

    sections.push(titled_section(
        &format!("{} files", group.files.len()),
        items,
    ));

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('Stash')")
                    .text("Stash Selected"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-blue)")
                    .attr("onclick", "window.__mm_resolve_action('MarkExpected')")
                    .text("Mark Expected"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render the current group of a MissingAlbumState.
pub fn render_missing_album_group(state: &MissingAlbumState) -> Node {
    let data = &state.data;
    let idx = data.current_group;
    let total = data.signals.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No missing album groups").into();
    }

    let signal = match data.signals.get(idx) {
        Some(s) => s,
        None => return span().class("mm-kv__val").text("Signal index out of bounds").into(),
    };

    let title = format!("Missing Album \u{2014} {}", signal.data.artist);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    let mut items = Vec::new();
    for track in &signal.data.tracks {
        items.push(kv(&track.title, &track.path));
    }

    sections.push(titled_section(
        &format!("{} tracks", signal.data.tracks.len()),
        items,
    ));

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-green)")
                    .attr("onclick", "window.__mm_resolve_action('PerTrackTitle')")
                    .text("Per-Track Title"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-cyan)")
                    .attr("onclick", "window.__mm_resolve_action('AllSingles')")
                    .text("All Singles"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('Suppress')")
                    .text("Suppress"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}

/// Render the current group of a DiscExtractionState.
pub fn render_disc_extraction_group(state: &DiscExtractionState) -> Node {
    let data = &state.data;
    let inner = &data.inner;
    let idx = data.current_group;
    let total = inner.groups.len();

    if total == 0 {
        return span().class("mm-kv__val").text("No disc extraction groups").into();
    }

    let group = match inner.groups.get(idx) {
        Some(g) => g,
        None => return span().class("mm-kv__val").text("Group index out of bounds").into(),
    };

    let title = format!("Disc Extraction \u{2014} {}", group.description);
    let mut sections = vec![render_pagination_header(&title, idx + 1, total)];

    let mut items = Vec::new();
    items.push(kv("Disc value", &group.disc_value));

    for file in &group.files {
        items.push(kv(
            &file.path,
            &format!("{} \u{2192} {}", file.original_value, file.cleaned_value),
        ));
    }

    sections.push(titled_section(
        &format!("{} files", group.files.len()),
        items,
    ));

    sections.push(
        div()
            .class("mm-buttons")
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-cyan)")
                    .attr("onclick", "window.__mm_resolve_action('Apply')")
                    .text("Apply"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-yellow)")
                    .attr("onclick", "window.__mm_resolve_action('Skip')")
                    .text("Skip"),
            )
            .child(
                html::button()
                    .class("mm-btn")
                    .attr("style", "border-color:var(--c-white)")
                    .attr("onclick", "window.__mm_resolve_action('Cancel')")
                    .text("Cancel"),
            )
            .into(),
    );

    div().class("mm-resolution").children(sections).into()
}
