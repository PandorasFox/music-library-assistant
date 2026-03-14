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

    // Corpus files bucket.
    if let Some(corpus) = insights.get("corpus_files") {
        sections.push(render_kv_section("Corpus Files", corpus));
    }

    // Placeholder bucket.
    if let Some(placeholders) = insights.get("placeholders") {
        sections.push(render_kv_section("Tag Health", placeholders));
    }

    // Other signals.
    if let Some(other) = insights.get("other_signals") {
        if let Some(arr) = other.as_array() {
            let items: Vec<Node> = arr
                .iter()
                .filter_map(|entry| {
                    let label = entry.get("label")?.as_str()?;
                    let count = entry.get("count")?.as_u64()?;
                    if count == 0 {
                        return None;
                    }
                    Some(
                        div()
                            .class("mm-kv")
                            .child(span().class("mm-kv__key").text(label))
                            .child(span().class("mm-kv__val").text(count.to_string()))
                            .into(),
                    )
                })
                .collect();
            if !items.is_empty() {
                sections.push(
                    section()
                        .class("mm-section")
                        .child(h3().class("mm-section__title").text("Signals"))
                        .children(items)
                        .into(),
                );
            }
        }
    }

    if sections.is_empty() {
        div()
            .class("mm-section")
            .child(span().class("mm-kv__val").text("No insights data"))
            .into()
    } else {
        div().children(sections).into()
    }
}

/// Placeholder content for views not yet implemented.
fn render_placeholder(view: LateralView) -> Node {
    section()
        .class("mm-section")
        .child(
            h3().class("mm-section__title")
                .text(view.label()),
        )
        .child(
            span()
                .class("mm-kv__val")
                .text("View not yet implemented in web UI"),
        )
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
            let deploy = api::get_query("deploy-status").await?;
            Ok(render_kv_section("Deploy Status", &deploy))
        }
        LateralView::Inbox => {
            let inbox = api::get_query("inbox-overview").await?;
            Ok(render_kv_section("Inbox Overview", &inbox))
        }
        LateralView::History => {
            let history = api::get_query("edit-history").await?;
            Ok(render_kv_section("Edit History", &history))
        }
        LateralView::ExternalMatches => {
            let matches = api::get_query("external-matches").await?;
            Ok(render_kv_section("External Matches", &matches))
        }
        other => Ok(render_placeholder(other)),
    }
}

async fn load_view_from_hash() -> Result<(), JsValue> {
    let view = current_view_from_hash();
    let content = load_view(view).await?;
    mount(&render_app_shell(view, false, content, Some("Connected")));
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
