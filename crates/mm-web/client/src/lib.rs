//! mm-web-client: WASM entry point for the Music Magic web UI.
//!
//! Builds mm-ui Node trees from API data and mounts them to the DOM.
//! Session token persisted in localStorage. View state in URL hash.
//! v1 uses full re-render via innerHTML — no diffing.

mod api;
mod views;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use mm_ui::html::widgets::{render_status_bar, render_titlebar};
use mm_ui::html::{self, div, Node};
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
    let status = api::setup_check().await?;
    if status.needs_setup {
        mount(&render_setup_form(None, status.suggested_root.as_deref()));
        return Ok(());
    }

    if api::has_token() {
        match load_view_from_hash().await {
            Ok(()) => return Ok(()),
            Err(_) => api::clear_token(),
        }
    }

    mount(&render_login(None));
    Ok(())
}

// ============================================================================
// View State (URL hash)
// ============================================================================

fn current_view_from_hash() -> &'static str {
    let hash = web_sys::window()
        .unwrap()
        .location()
        .hash()
        .unwrap_or_default();
    let name = hash.trim_start_matches('#');
    // Return the raw hash — load_view interprets it, supporting sub-views.
    // Leak the string to get a &'static str (fine, small set of values).
    Box::leak(name.to_string().into_boxed_str())
}

fn view_for_hash(hash: &str) -> LateralView {
    let base = hash.split('/').next().unwrap_or(hash);
    match base {
        "config" => LateralView::Config,
        "search" => LateralView::Search,
        "files" => LateralView::Files,
        "health" => LateralView::Health,
        "history" => LateralView::History,
        "transaction" => LateralView::Transaction,
        "inbox" => LateralView::Inbox,
        "deploy" => LateralView::Deploy,
        "external-matches" => LateralView::ExternalMatches,
        _ if hash.starts_with("packing/") => LateralView::ExternalMatches,
        _ => LateralView::Health,
    }
}

fn view_to_hash(view: LateralView) -> &'static str {
    match view {
        LateralView::Config => "config",
        LateralView::Search => "search",
        LateralView::Files => "files",
        LateralView::Health => "health",
        LateralView::History => "history",
        LateralView::Transaction => "transaction",
        LateralView::Inbox => "inbox",
        LateralView::Deploy => "deploy",
        LateralView::ExternalMatches => "external-matches",
    }
}

fn set_hash(hash: &str) {
    web_sys::window()
        .unwrap()
        .location()
        .set_hash(hash)
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
// View Renderers (login / shell)
// ============================================================================

fn render_setup_form(error: Option<&str>, suggested_root: Option<&str>) -> Node {
    let mut form = div().class("mm-login");
    form = form.child(div().class("mm-login__title").text("Music Magic"));
    form = form.child(div().class("mm-login__subtitle").text("First-Time Setup"));

    let mut root_input = html::input()
        .attr("type", "text")
        .attr("id", "setup-root")
        .attr("placeholder", "/path/to/music");
    if let Some(root) = suggested_root {
        root_input = root_input.attr("value", root);
    }

    let fields = div()
        .class("mm-login__form")
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Archive Root"))
                .child(root_input),
        )
        .child(
            div()
                .class("mm-login__field")
                .child(html::label().text("Username"))
                .child(
                    html::input()
                        .attr("type", "text")
                        .attr("id", "setup-user")
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
                        .attr("id", "setup-pass")
                        .attr("autocomplete", "new-password"),
                ),
        )
        .child(
            html::button()
                .class("mm-login__submit")
                .attr("onclick", "window.__mm_setup()")
                .text("Complete Setup"),
        );

    form = form.child(fields);

    if let Some(err) = error {
        form = form.child(div().class("mm-login__error").text(err));
    }

    form.into()
}

fn render_login(error: Option<&str>) -> Node {
    let mut form = div().class("mm-login");
    form = form.child(div().class("mm-login__title").text("Music Magic"));

    let fields = div()
        .class("mm-login__form")
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

fn render_placeholder(view: LateralView) -> Node {
    views::titled_section(
        view.label(),
        vec![mm_ui::html::span()
            .class("mm-kv__val")
            .text("View not yet implemented in web UI")
            .into()],
    )
}

// ============================================================================
// View Loading
// ============================================================================

async fn load_view(hash: &str) -> Result<Node, JsValue> {
    // Sub-view routing.
    if let Some(category) = hash.strip_prefix("packing/") {
        let data = api::get_query_with("packing-browser-data", &format!("category_prefix={category}")).await?;
        return Ok(views::render_packing_browser_data(category, &data));
    }
    if let Some(inode_str) = hash.strip_prefix("tags/") {
        let inode: i64 = inode_str.parse().map_err(|_| JsValue::from_str("bad inode"))?;
        let tags = api::get_query_with("corpus-tags", &format!("inode={inode}")).await?;
        let path = inode_str; // TODO: resolve inode → path
        return Ok(views::render_tag_editor(inode, path, &tags));
    }

    let view = view_for_hash(hash);
    match view {
        LateralView::Health => {
            let status = api::get_status().await?;
            let insights = api::get_query("insights").await.ok();
            let mut children = vec![views::render_status_content(&status)];
            if let Some(ins) = insights {
                children.push(views::render_insights_content(&ins));
            }
            Ok(div().children(children).into())
        }
        LateralView::Config => {
            let config = api::get_config().await?;
            Ok(views::render_config_editor(&config))
        }
        LateralView::Deploy => {
            let data = api::get_query("deploy-status").await?;
            Ok(views::render_deploy_content(&data))
        }
        LateralView::Inbox => {
            let data = api::get_query("inbox-overview").await?;
            Ok(views::render_inbox_content(&data))
        }
        LateralView::History => {
            let data = api::get_query("edit-history").await?;
            Ok(views::render_edit_history_content(&data))
        }
        LateralView::ExternalMatches => {
            let data = api::get_query("external-matches").await?;
            let mut children = vec![views::render_external_matches_content(&data)];
            children.push(views::render_packing_overview(&data));
            Ok(div().children(children).into())
        }
        LateralView::Transaction => {
            let status = api::get_status().await?;
            let details = api::tx_details().await.ok();
            Ok(views::render_transaction_content(&status, details.as_ref()))
        }
        other => Ok(render_placeholder(other)),
    }
}

async fn load_view_from_hash() -> Result<(), JsValue> {
    let hash = current_view_from_hash();
    let view = view_for_hash(hash);

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

    let content = load_view(hash).await?;
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

    match api::login(&user_el.value(), &pass_el.value()).await {
        Ok(_) => {
            set_hash("health");
            load_view_from_hash().await?;
            Ok(())
        }
        Err(e) => {
            mount(&render_login(Some(&format!("{e:?}"))));
            Ok(())
        }
    }
}

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
    set_hash(view_to_hash(view));
    spawn_local(async move {
        if let Err(e) = load_view_from_hash().await {
            web_sys::console::error_1(&format!("navigate error: {e:?}").into());
        }
    });
}

#[wasm_bindgen]
pub fn mm_logout() {
    api::clear_token();
    mount(&render_login(None));
}

#[wasm_bindgen]
pub fn mm_tx_confirm() {
    spawn_local(async {
        match api::tx_confirm().await {
            Ok(_) => { load_view_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx confirm error: {e:?}").into()),
        }
    });
}

#[wasm_bindgen]
pub fn mm_tx_discard() {
    spawn_local(async {
        match api::tx_discard().await {
            Ok(_) => { load_view_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx discard error: {e:?}").into()),
        }
    });
}

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
        if !container.inner_html().is_empty() {
            container.set_inner_html("");
            return;
        }
        match api::get_query_with("session-edit-detail", &format!("session_id={sid}")).await {
            Ok(detail) => {
                let node = views::render_session_detail(&detail);
                container.set_inner_html(&node.to_html());
            }
            Err(e) => {
                container.set_inner_html(&format!("<span class='mm-kv__val'>Error: {e:?}</span>"));
            }
        }
    });
}

/// Complete first-time setup.
#[wasm_bindgen]
pub fn mm_setup() {
    spawn_local(async {
        if let Err(e) = do_setup().await {
            web_sys::console::error_1(&format!("setup error: {e:?}").into());
        }
    });
}

async fn do_setup() -> Result<(), JsValue> {
    let doc = web_sys::window().unwrap().document().unwrap();
    let root = doc
        .get_element_by_id("setup-root")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();
    let user = doc
        .get_element_by_id("setup-user")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();
    let pass = doc
        .get_element_by_id("setup-pass")
        .unwrap()
        .dyn_into::<web_sys::HtmlInputElement>()?
        .value();

    if root.is_empty() {
        mount(&render_setup_form(Some("Archive root is required"), None));
        return Ok(());
    }

    match api::setup_complete(&root, &user, &pass).await {
        Ok(()) => {
            // Setup done. If credentials were provided, log in automatically.
            if !user.is_empty() && !pass.is_empty() {
                match api::login(&user, &pass).await {
                    Ok(_) => {
                        set_hash("health");
                        load_view_from_hash().await?;
                    }
                    Err(_) => mount(&render_login(None)),
                }
            } else {
                mount(&render_login(None));
            }
            Ok(())
        }
        Err(e) => {
            mount(&render_setup_form(Some(&format!("{e:?}")), None));
            Ok(())
        }
    }
}

/// Navigate into a packing category browser.
#[wasm_bindgen]
pub fn mm_packing_browse(category: &str) {
    let cat = category.to_string();
    set_hash(&format!("packing/{cat}"));
    spawn_local(async move {
        if let Err(e) = load_view_from_hash().await {
            web_sys::console::error_1(&format!("packing browse error: {e:?}").into());
        }
    });
}
