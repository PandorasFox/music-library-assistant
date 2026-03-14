//! mm-web-client: WASM entry point for the Music Magic web UI.
//!
//! Builds mm-ui Node trees from API data and mounts them to the DOM.
//! Session token persisted in localStorage. View state in URL hash.
//! v1 uses full re-render via innerHTML — no diffing.

mod api;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use mm_ui::html::widgets::{render_status_bar, render_titlebar};
use mm_ui::html::{self, div, h3, span, section, Node};
use mm_ui::lateral_view::LateralView;

// ============================================================================
// Entry Point
// ============================================================================

#[wasm_bindgen(start)]
pub fn main() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("mm-web-client panic: {info}").into());
    }));

    spawn_local(async {
        if let Err(e) = init().await {
            web_sys::console::error_1(&format!("init error: {e:?}").into());
            mount_error(&format!("Failed to connect: {e:?}"));
        }
    });
}

async fn init() -> Result<(), JsValue> {
    let needs_setup = api::setup_check().await?;
    if needs_setup {
        mount(&render_setup_needed());
        return Ok(());
    }

    // If we have a saved token, try loading the main view directly.
    if api::has_token() {
        match load_view_from_hash().await {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Token expired or invalid — clear and show login.
                api::clear_token();
            }
        }
    }

    mount(&render_login(None));
    Ok(())
}

// ============================================================================
// View State (URL hash)
// ============================================================================

fn current_view_from_hash() -> LateralView {
    let hash = web_sys::window()
        .unwrap()
        .location()
        .hash()
        .unwrap_or_default();
    let name = hash.trim_start_matches('#');
    match name {
        "config" => LateralView::Config,
        "search" => LateralView::Search,
        "files" => LateralView::Files,
        "health" => LateralView::Health,
        "history" => LateralView::History,
        "transaction" => LateralView::Transaction,
        "inbox" => LateralView::Inbox,
        "deploy" => LateralView::Deploy,
        "external-matches" => LateralView::ExternalMatches,
        _ => LateralView::Health,
    }
}

fn view_to_hash(view: LateralView) -> &'static str {
    match view {
        LateralView::Config => "#config",
        LateralView::Search => "#search",
        LateralView::Files => "#files",
        LateralView::Health => "#health",
        LateralView::History => "#history",
        LateralView::Transaction => "#transaction",
        LateralView::Inbox => "#inbox",
        LateralView::Deploy => "#deploy",
        LateralView::ExternalMatches => "#external-matches",
    }
}

fn set_view_hash(view: LateralView) {
    web_sys::window()
        .unwrap()
        .location()
        .set_hash(view_to_hash(view))
        .ok();
}

// ============================================================================
// DOM Mounting
// ============================================================================

fn get_root() -> web_sys::Element {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id("mm-root")
        .expect("missing #mm-root element")
}

fn mount(tree: &Node) {
    let root = get_root();
    root.set_inner_html(&tree.to_html());
}

fn mount_error(msg: &str) {
    let tree = div()
        .class("mm-login")
        .child(div().class("mm-login__title").text("Music Magic"))
        .child(div().class("mm-login__error").text(msg));
    mount(&tree.into());
}

// ============================================================================
// View Renderers
// ============================================================================

fn render_setup_needed() -> Node {
    div()
        .class("mm-login")
        .child(div().class("mm-login__title").text("Music Magic"))
        .child(
            div()
                .class("mm-login__error")
                .text("First-time setup required. Use the TUI client to complete setup."),
        )
        .into()
}

fn render_login(error: Option<&str>) -> Node {
    let mut form = div().class("mm-login");
    form = form.child(div().class("mm-login__title").text("Music Magic"));

    let fields = div()
        .class("mm-login__form")
        .attr("id", "login-form")
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Username"))
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", "login-user")
                        .attr("autocomplete", "username"),
                ),
        )
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Password"))
                .child(
                    html::input()
                        .attr("type", "password")
                        .attr("id", "login-pass")
                        .attr("autocomplete", "current-password"),
                ),
        )
        .child(
            html::button()
                .class("mm-login__submit")
                .attr("id", "login-btn")
                .attr("onclick", "window.__mm_login()")
                .text("Login"),
        );

    form = form.child(fields);

    if let Some(err) = error {
        form = form.child(div().class("mm-login__error").text(err));
    }

    form.into()
}

fn render_app_shell(
    active_view: LateralView,
    transactions_open: bool,
    content: Node,
    status: Option<&str>,
) -> Node {
    div()
        .attr("id", "mm-app")
        .child(render_titlebar(active_view, transactions_open))
        .child(div().class("mm-content").child(content))
        .child(render_status_bar(status, None))
        .into()
}

// ============================================================================
// Structured data renderers
// ============================================================================

/// Render a flat key-value section from a JSON object.
/// Skips nested objects/arrays — those get their own sections.
fn render_kv_section(title: &str, json: &serde_json::Value) -> Node {
    let mut items = Vec::new();
    if let Some(obj) = json.as_object() {
        for (key, val) in obj {
            // Skip complex nested values — render them separately.
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
            items.push(
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(display_key))
                    .child(span().class("mm-kv__val").text(val_str))
                    .into(),
            );
        }
    }
    section()
        .class("mm-section")
        .child(h3().class("mm-section__title").text(title))
        .children(items)
        .into()
}

/// Render WitchStatus with structured sections.
fn render_status_content(status: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    // Top-level scalar fields.
    sections.push(render_kv_section("Witch", status));

    // Work subsection.
    if let Some(work) = status.get("work") {
        sections.push(render_kv_section("Work", work));
        if let Some(pending) = work.get("pending_by_label").and_then(|v| v.as_object()) {
            if !pending.is_empty() {
                let items: Vec<Node> = pending
                    .iter()
                    .map(|(k, v)| {
                        div()
                            .class("mm-kv")
                            .child(span().class("mm-kv__key").text(k))
                            .child(
                                span()
                                    .class("mm-kv__val")
                                    .text(v.as_u64().unwrap_or(0).to_string()),
                            )
                            .into()
                    })
                    .collect();
                sections.push(
                    section()
                        .class("mm-section")
                        .child(h3().class("mm-section__title").text("Pending Work"))
                        .children(items)
                        .into(),
                );
            }
        }
    }

    // Transaction snapshot.
    if let Some(tx) = status.get("transaction") {
        if !tx.is_null() {
            sections.push(render_kv_section("Transaction", tx));
        }
    }

    // External fetch progress.
    if let Some(progress) = status.get("external_fetch_progress") {
        if !progress.is_null() {
            sections.push(render_kv_section("External Fetch", progress));
        }
    }

    div().children(sections).into()
}

/// Render InsightsData with structured buckets.
fn render_insights_content(insights: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    if let Some(corpus) = insights.get("corpus_files") {
        sections.push(render_kv_section("Corpus Files", corpus));
    }
    if let Some(placeholders) = insights.get("placeholders") {
        sections.push(render_kv_section("Tag Health", placeholders));
    }

    // Signal list — only show non-zero.
    if let Some(other) = insights.get("other_signals").and_then(|v| v.as_array()) {
        let items: Vec<Node> = other
            .iter()
            .filter_map(|entry| {
                let label = entry.get("label")?.as_str()?;
                let count = entry.get("count")?.as_u64()?;
                if count == 0 {
                    return None;
                }
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

/// Render ExternalMatchesData with packing counts and confidence buckets.
fn render_external_matches_content(data: &serde_json::Value) -> Node {
    let mut sections = Vec::new();

    // Packing summary counts.
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

    // Unsolved counts.
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
            if n == 0 {
                return None;
            }
            Some(kv(label, &n.to_string()))
        })
        .collect();
    if !unsolved_items.is_empty() {
        sections.push(titled_section("Unsolved", unsolved_items));
    }

    // Confidence buckets.
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

    // Untagged entries.
    if let Some(untagged) = data.get("untagged_entries").and_then(|v| v.as_array()) {
        if !untagged.is_empty() {
            let items: Vec<Node> = untagged
                .iter()
                .take(50) // Cap display
                .filter_map(|e| {
                    let path = e.get("path")?.as_str()?;
                    let conf = e.get("confidence")?.as_f64()?;
                    Some(kv(path, &format!("{conf:.0}%")))
                })
                .collect();
            sections.push(titled_section(
                &format!("Untagged Entries ({})", untagged.len()),
                items,
            ));
        }
    }

    div().children(sections).into()
}

/// Render edit history session list with expand links.
fn render_edit_history_content(data: &serde_json::Value) -> Node {
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
                                    .attr(
                                        "onclick",
                                        format!(
                                            "event.preventDefault();window.__mm_expand_session('{session_id}')"
                                        ),
                                    )
                                    .text(session_id),
                            )
                            .child(
                                span()
                                    .class("mm-kv__val")
                                    .text(format!("{edits} edits, {inodes} files — {earliest}")),
                            ),
                    )
                    .child(div().attr("id", format!("session-{session_id}")).class("mm-session-detail"))
                    .into(),
            )
        })
        .collect();

    titled_section("Edit Sessions", items)
}

/// Render inbox overview with counts.
fn render_inbox_content(data: &serde_json::Value) -> Node {
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

/// Render deploy status with library file counts.
fn render_deploy_content(data: &serde_json::Value) -> Node {
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

/// Render transaction view — shows active transaction with confirm/discard.
fn render_transaction_content(
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

    // Transaction summary.
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

    // Action buttons.
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

    // Decision details.
    if let Some(detail_arr) = details.and_then(|d| d.as_array()) {
        let decision_items: Vec<Node> = detail_arr
            .iter()
            .filter_map(|d| {
                let label = d.get("label")?.as_str().unwrap_or("?");
                let key = d.get("key")?;
                let key_str = serde_json::to_string(key).unwrap_or_default();
                let mutations = d
                    .get("mutations")
                    .and_then(|m| m.as_array())
                    .map_or(0, |a| a.len());
                Some(
                    div()
                        .class("mm-kv")
                        .child(span().class("mm-kv__key").text(label))
                        .child(
                            span()
                                .class("mm-kv__val")
                                .text(format!("{mutations} mutations — {key_str}")),
                        )
                        .into(),
                )
            })
            .collect();
        if !decision_items.is_empty() {
            sections.push(titled_section("Decisions", decision_items));
        }
    }

    div().children(sections).into()
}

/// Placeholder content for views not yet implemented.
fn render_placeholder(view: LateralView) -> Node {
    section()
        .class("mm-section")
        .child(h3().class("mm-section__title").text(view.label()))
        .child(span().class("mm-kv__val").text("View not yet implemented in web UI"))
        .into()
}

// ============================================================================
// Node helpers
// ============================================================================

/// Single key-value row.
fn kv(key: &str, val: &str) -> Node {
    div()
        .class("mm-kv")
        .child(span().class("mm-kv__key").text(key))
        .child(span().class("mm-kv__val").text(val))
        .into()
}

/// Section with a title and children.
fn titled_section(title: &str, items: Vec<Node>) -> Node {
    section()
        .class("mm-section")
        .child(h3().class("mm-section__title").text(title))
        .children(items)
        .into()
}

// ============================================================================
// View Loading
// ============================================================================

async fn load_view(view: LateralView) -> Result<Node, JsValue> {
    match view {
        LateralView::Health => {
            let status = api::get_status().await?;
            let insights = api::get_query("insights").await.ok();
            let mut children = vec![render_status_content(&status)];
            if let Some(ins) = insights {
                children.push(render_insights_content(&ins));
            }
            Ok(div().children(children).into())
        }
        LateralView::Deploy => {
            let data = api::get_query("deploy-status").await?;
            Ok(render_deploy_content(&data))
        }
        LateralView::Inbox => {
            let data = api::get_query("inbox-overview").await?;
            Ok(render_inbox_content(&data))
        }
        LateralView::History => {
            let data = api::get_query("edit-history").await?;
            Ok(render_edit_history_content(&data))
        }
        LateralView::ExternalMatches => {
            let data = api::get_query("external-matches").await?;
            Ok(render_external_matches_content(&data))
        }
        LateralView::Transaction => {
            let status = api::get_status().await?;
            let details = api::tx_details().await.ok();
            Ok(render_transaction_content(&status, details.as_ref()))
        }
        other => Ok(render_placeholder(other)),
    }
}

async fn load_view_from_hash() -> Result<(), JsValue> {
    let view = current_view_from_hash();
    // Check if a transaction is active to show the Transaction tab.
    let status = if view == LateralView::Health || view == LateralView::Transaction {
        api::get_status().await.ok()
    } else {
        None
    };
    let tx_open = status
        .as_ref()
        .and_then(|s| s.get("transaction"))
        .map_or(false, |t| !t.is_null());

    let content = load_view(view).await?;
    mount(&render_app_shell(view, tx_open, content, Some("Connected")));
    Ok(())
}

// ============================================================================
// Global callbacks (wasm_bindgen exports → window globals via index.html)
// ============================================================================

#[wasm_bindgen]
pub fn mm_login() {
    spawn_local(async {
        if let Err(e) = do_login().await {
            web_sys::console::error_1(&format!("login error: {e:?}").into());
        }
    });
}

async fn do_login() -> Result<(), JsValue> {
    let doc = web_sys::window().unwrap().document().unwrap();

    let user_el = doc
        .get_element_by_id("login-user")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?;
    let pass_el = doc
        .get_element_by_id("login-pass")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?;

    let username = user_el.value();
    let password = pass_el.value();

    match api::login(&username, &password).await {
        Ok(_token) => {
            set_view_hash(LateralView::Health);
            load_view_from_hash().await?;
            Ok(())
        }
        Err(e) => {
            mount(&render_login(Some(&format!("{e:?}"))));
            Ok(())
        }
    }
}

/// Navigate to a lateral view. Called from tab click handlers.
#[wasm_bindgen]
pub fn mm_navigate(view_name: &str) {
    let view = match view_name {
        "Config" => LateralView::Config,
        "Search" => LateralView::Search,
        "Files" => LateralView::Files,
        "Health" => LateralView::Health,
        "History" => LateralView::History,
        "Transaction" => LateralView::Transaction,
        "Inbox" => LateralView::Inbox,
        "Deploy" => LateralView::Deploy,
        "Ext. Matches" => LateralView::ExternalMatches,
        _ => return,
    };
    set_view_hash(view);
    spawn_local(async move {
        if let Err(e) = load_view_from_hash().await {
            web_sys::console::error_1(&format!("navigate error: {e:?}").into());
        }
    });
}

/// Log out — clear token and show login form.
#[wasm_bindgen]
pub fn mm_logout() {
    api::clear_token();
    mount(&render_login(None));
}

/// Confirm the active transaction.
#[wasm_bindgen]
pub fn mm_tx_confirm() {
    spawn_local(async {
        match api::tx_confirm().await {
            Ok(_) => {
                if let Err(e) = load_view_from_hash().await {
                    web_sys::console::error_1(&format!("post-confirm error: {e:?}").into());
                }
            }
            Err(e) => web_sys::console::error_1(&format!("tx confirm error: {e:?}").into()),
        }
    });
}

/// Discard the active transaction.
#[wasm_bindgen]
pub fn mm_tx_discard() {
    spawn_local(async {
        match api::tx_discard().await {
            Ok(_) => {
                if let Err(e) = load_view_from_hash().await {
                    web_sys::console::error_1(&format!("post-discard error: {e:?}").into());
                }
            }
            Err(e) => web_sys::console::error_1(&format!("tx discard error: {e:?}").into()),
        }
    });
}

/// Expand an edit history session to show its edits.
#[wasm_bindgen]
pub fn mm_expand_session(session_id: &str) {
    let sid = session_id.to_string();
    spawn_local(async move {
        let target_id = format!("session-{sid}");
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = match doc.get_element_by_id(&target_id) {
            Some(el) => el,
            None => return,
        };

        // Toggle: if already has content, clear it.
        if !container.inner_html().is_empty() {
            container.set_inner_html("");
            return;
        }

        match api::get_query_with("session-edit-detail", &format!("session_id={sid}")).await {
            Ok(detail) => {
                let node = render_session_detail(&detail);
                container.set_inner_html(&node.to_html());
            }
            Err(e) => {
                container.set_inner_html(&format!("<span class='mm-kv__val'>Error: {e:?}</span>"));
            }
        }
    });
}

/// Render expanded session edit detail.
fn render_session_detail(detail: &serde_json::Value) -> Node {
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
            let old = edit
                .get("old_value")
                .and_then(|v| v.as_str())
                .unwrap_or("∅");
            let new = edit
                .get("new_value")
                .and_then(|v| v.as_str())
                .unwrap_or("∅");
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
