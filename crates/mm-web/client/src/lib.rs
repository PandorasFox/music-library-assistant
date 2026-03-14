//! mm-web-client: WASM entry point for the Music Magic web UI.
//!
//! Builds mm-ui Node trees from API data and mounts them to the DOM.
//! Session token persisted in localStorage. View state encoded in URL hash
//! via the shared [`mm_ui::route::Route`] type.
//! v1 uses full re-render via innerHTML — no diffing.

mod api;
mod views;

use std::cell::RefCell;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use mm_ui::html::widgets::{render_status_bar, render_titlebar};
use mm_ui::html::{self, div, Node};
use mm_ui::lateral_view::LateralView;
use mm_ui::route::{self, Route};

thread_local! {
    static SEARCH_DATA: RefCell<Option<serde_json::Value>> = const { RefCell::new(None) };
    static CONFIG_DATA: RefCell<Option<serde_json::Value>> = const { RefCell::new(None) };
    /// JS interval handle for health view polling. Cleared on navigation.
    static POLL_HANDLE: RefCell<Option<i32>> = const { RefCell::new(None) };
}

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
        match load_from_hash().await {
            Ok(()) => return Ok(()),
            Err(_) => api::clear_token(),
        }
    }

    mount(&render_login(None));
    Ok(())
}

// ============================================================================
// View State (URL hash ↔ Route)
// ============================================================================

/// Parse the current URL hash into a Route.
/// Falls back to Health on parse failure.
fn current_route() -> Route {
    let hash = web_sys::window()
        .unwrap()
        .location()
        .hash()
        .unwrap_or_default();
    let raw = hash.trim_start_matches('#');

    // Split path and query params.
    let (path, query) = if let Some((p, q)) = raw.split_once('?') {
        let pairs: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        (p, pairs)
    } else {
        (raw, vec![])
    };

    Route::from_url(path, &query).unwrap_or(Route::Health(route::HealthRoute::default()))
}

/// Determine the LateralView for titlebar highlighting.
/// Overlay routes map to their parent lateral view.
fn lateral_view_for_route(route: &Route) -> LateralView {
    route.lateral_view().unwrap_or_else(|| match route {
        Route::PackingBrowser(_) | Route::KnotBrowser(_) => LateralView::ExternalMatches,
        Route::TagEditor(_) => LateralView::Search,
        Route::Resolution(_) => LateralView::Health,
        Route::TransactionReview(_) => LateralView::Transaction,
        _ => LateralView::Health,
    })
}

/// Navigate to a route by updating the URL hash.
fn navigate_to(route: &Route) {
    let url = route.to_url();
    // Strip leading '/' for hash — browser prepends '#'.
    let hash = url.strip_prefix('/').unwrap_or(&url);
    web_sys::window()
        .unwrap()
        .location()
        .set_hash(hash)
        .ok();
}

/// Convenience: build a default route for a lateral view.
fn route_for_lateral(view: LateralView) -> Route {
    match view {
        LateralView::Config => Route::Config(Default::default()),
        LateralView::Search => Route::Search(Default::default()),
        LateralView::Files => Route::Files(Default::default()),
        LateralView::Health => Route::Health(Default::default()),
        LateralView::History => Route::History(Default::default()),
        LateralView::Transaction => Route::Transaction(Default::default()),
        LateralView::Inbox => Route::Inbox(Default::default()),
        LateralView::Deploy => Route::Deploy(Default::default()),
        LateralView::ExternalMatches => Route::ExternalMatches(Default::default()),
    }
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
        .attr("placeholder", "/path/to/music")
        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()");
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
                        .attr("autocomplete", "username")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()"),
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
                        .attr("autocomplete", "new-password")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_setup()"),
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
                        .attr("autocomplete", "username")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_login()"),
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
                        .attr("autocomplete", "current-password")
                        .attr("onkeydown", "if(event.key==='Enter')window.__mm_login()"),
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


// ============================================================================
// View Loading
// ============================================================================

/// Fetch data and render content for a route.
async fn load_view_for_route(route: &Route) -> Result<Node, JsValue> {
    match route {
        // -- Lateral views --
        Route::Health(_) => {
            let status = api::get_status().await?;
            let insights = api::get_insights().await.ok();
            let mut children = vec![views::render_status_content(&status)];
            if let Some(ref ins) = insights {
                children.push(views::render_insights_content(ins));
            }
            start_health_poll();
            Ok(div().children(children).into())
        }
        Route::Config(_) => {
            let config_json = api::get_config_json().await?;
            CONFIG_DATA.with(|cell| *cell.borrow_mut() = Some(config_json.clone()));
            Ok(views::render_config_editor(&config_json))
        }
        Route::Deploy(_) => {
            let data = api::get_deploy_status().await?;
            Ok(views::render_deploy_content(&data))
        }
        Route::Inbox(_) => {
            let data = api::get_inbox_overview().await?;
            Ok(views::render_inbox_content(&data))
        }
        Route::History(_) => {
            let data = api::get_edit_history().await?;
            Ok(views::render_edit_history_content(&data))
        }
        Route::ExternalMatches(_) => {
            let data = api::get_external_matches().await?;
            let mut children = vec![views::render_external_matches_content(&data)];
            children.push(views::render_packing_overview(&data));
            Ok(div().children(children).into())
        }
        Route::Transaction(_) => {
            let status = api::get_status().await?;
            let details = api::tx_details().await.ok();
            Ok(views::render_transaction_content(&status, details.as_ref()))
        }
        Route::Search(_) => {
            let data = api::get_corpus_files_with_tags().await?;
            SEARCH_DATA.with(|cell| *cell.borrow_mut() = Some(data.clone()));
            Ok(views::render_search_view(&data))
        }
        Route::Files(_) => {
            let data = api::get_corpus_files_with_tags().await?;
            Ok(views::render_files_view(&data))
        }

        // -- Overlay views --
        Route::PackingBrowser(r) => {
            let data = api::get_query_with(
                "packing-browser-data",
                &format!("category_prefix={}", r.category),
            ).await?;
            Ok(views::render_packing_browser_data(&r.category, &data))
        }
        Route::TagEditor(r) => {
            let inode = r.inodes.first().copied().ok_or_else(|| {
                JsValue::from_str("tag editor requires at least one inode")
            })?;
            let tags = api::get_query_with(
                "corpus-tags",
                &format!("inode={inode}"),
            ).await?;
            let path = inode.to_string();
            Ok(views::render_tag_editor(inode, &path, &tags))
        }

        // Resolution, TransactionReview, KnotBrowser — not yet wired to web
        // renderers. Fall back to health view for now.
        _ => {
            let status = api::get_status().await?;
            let insights = api::get_insights().await.ok();
            let mut children = vec![views::render_status_content(&status)];
            if let Some(ref ins) = insights {
                children.push(views::render_insights_content(ins));
            }
            start_health_poll();
            Ok(div().children(children).into())
        }
    }
}

/// Parse the current URL hash into a Route, load its content, and mount.
async fn load_from_hash() -> Result<(), JsValue> {
    let route = current_route();
    let lateral = lateral_view_for_route(&route);

    // Check if a transaction is active to show the Transaction tab.
    let tx_open = api::get_status()
        .await
        .ok()
        .map_or(false, |s| s.transaction.is_some());

    let content = load_view_for_route(&route).await?;
    mount(&render_app_shell(lateral, tx_open, content, Some("Connected")));
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
            navigate_to(&Route::Health(Default::default()));
            load_from_hash().await?;
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
    stop_poll();
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
    navigate_to(&route_for_lateral(view));
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
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
            Ok(_) => { load_from_hash().await.ok(); }
            Err(e) => web_sys::console::error_1(&format!("tx confirm error: {e:?}").into()),
        }
    });
}

#[wasm_bindgen]
pub fn mm_tx_discard() {
    spawn_local(async {
        match api::tx_discard().await {
            Ok(_) => { load_from_hash().await.ok(); }
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
                        navigate_to(&Route::Health(Default::default()));
                        load_from_hash().await?;
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
    let route = Route::PackingBrowser(route::PackingBrowserRoute {
        category: category.to_string(),
        cursor: None,
    });
    navigate_to(&route);
    spawn_local(async move {
        if let Err(e) = load_from_hash().await {
            web_sys::console::error_1(&format!("packing browse error: {e:?}").into());
        }
    });
}

/// Start polling status + insights for the Health view (~3s interval).
fn start_health_poll() {
    stop_poll();
    let window = web_sys::window().unwrap();
    let cb = wasm_bindgen::closure::Closure::wrap(Box::new(|| {
        spawn_local(async {
            do_health_refresh().await;
        });
    }) as Box<dyn Fn()>);
    let handle = window
        .set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            3000,
        )
        .unwrap_or(-1);
    cb.forget(); // Leak closure — lives for the interval lifetime.
    POLL_HANDLE.with(|cell| *cell.borrow_mut() = Some(handle));
}

fn stop_poll() {
    POLL_HANDLE.with(|cell| {
        if let Some(handle) = cell.borrow_mut().take() {
            web_sys::window().unwrap().clear_interval_with_handle(handle);
        }
    });
}

async fn do_health_refresh() {
    // Stop polling if we've lost auth.
    if !api::has_token() {
        stop_poll();
        return;
    }

    let doc = web_sys::window().unwrap().document().unwrap();

    if let Ok(status) = api::get_status().await {
        let node = views::render_status_content(&status);
        if let Some(el) = doc.get_element_by_id("mm-status-section") {
            el.set_inner_html(&node.to_html());
        }
    }

    if let Ok(insights) = api::get_insights().await {
        let node = views::render_insights_content(&insights);
        if let Some(el) = doc.get_element_by_id("mm-insights-section") {
            el.set_inner_html(&node.to_html());
        }
    }
}

/// Queue a background task (e.g. "SchemaReconciliation") and refresh the view.
#[wasm_bindgen]
pub fn mm_queue_task(task: &str) {
    let task = task.to_string();
    spawn_local(async move {
        match api::queue_task(&task).await {
            Ok(_) => {
                // Refresh current view to update counts.
                load_from_hash().await.ok();
            }
            Err(e) => web_sys::console::error_1(&format!("queue-task error: {e:?}").into()),
        }
    });
}

/// Execute a typed protocol binding and refresh the view.
#[wasm_bindgen]
pub fn mm_execute(binding_json: &str) {
    let json_str = binding_json.to_string();
    spawn_local(async move {
        let binding: serde_json::Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                web_sys::console::error_1(&format!("mm_execute parse error: {e}").into());
                return;
            }
        };
        match api::execute_action(&binding).await {
            Ok(_) => {
                load_from_hash().await.ok();
            }
            Err(e) => web_sys::console::error_1(&format!("mm_execute error: {e:?}").into()),
        }
    });
}

/// Filter search results without full page re-render.
/// Reads cached data from SEARCH_DATA and only replaces the results container.
#[wasm_bindgen]
pub fn mm_search(query: &str) {
    let q = query.to_string();
    SEARCH_DATA.with(|cell| {
        let data = cell.borrow();
        if let Some(ref d) = *data {
            let node = views::render_search_results(d, &q);
            let doc = web_sys::window().unwrap().document().unwrap();
            if let Some(container) = doc.get_element_by_id("mm-search-results") {
                container.set_inner_html(&node.to_html());
            }
        }
    });
}

/// Save edited config — collects form values, patches cached JSON, POSTs to server.
#[wasm_bindgen]
pub fn mm_config_save() {
    spawn_local(async {
        if let Err(e) = do_config_save().await {
            web_sys::console::error_1(&format!("config save error: {e:?}").into());
        }
    });
}

/// Find the mutable slot for a config field by name.
/// Searches root-level keys, then opinions top-level, then opinions sub-blocks.
fn find_config_slot<'a>(
    config: &'a mut serde_json::Value,
    name: &str,
) -> Option<&'a mut serde_json::Value> {
    // Root level.
    if config.get(name).is_some() {
        return config.get_mut(name);
    }
    // Opinions top-level and sub-blocks.
    if let Some(opinions) = config.get_mut("opinions") {
        if opinions.get(name).is_some() {
            return opinions.get_mut(name);
        }
        if let Some(obj) = opinions.as_object_mut() {
            for (_block_key, block_val) in obj.iter_mut() {
                if let Some(block_obj) = block_val.as_object_mut() {
                    if block_obj.contains_key(name) {
                        return block_obj.get_mut(name);
                    }
                }
            }
        }
    }
    None
}

async fn do_config_save() -> Result<(), JsValue> {
    let config_json = CONFIG_DATA.with(|cell| cell.borrow().clone());
    let Some(mut config) = config_json else {
        return Err(JsValue::from_str("no config data cached"));
    };

    // Walk all form inputs and patch changed values into the config JSON.
    let doc = web_sys::window().unwrap().document().unwrap();
    let inputs = doc.query_selector_all("input[id^='cfg-']")
        .map_err(|e| JsValue::from_str(&format!("querySelectorAll: {e:?}")))?;

    for i in 0..inputs.length() {
        let el = inputs.get(i).unwrap();
        let input: web_sys::HtmlInputElement = el.dyn_into()?;
        let name = input.get_attribute("name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let input_type = input.get_attribute("type").unwrap_or_default();

        // Find this field in the config JSON and update it.
        // Fields can be in opinions top-level or in opinions sub-blocks.
        let new_val = if input_type == "checkbox" {
            serde_json::Value::Bool(input.checked())
        } else if input_type == "number" {
            let v = input.value();
            if let Ok(n) = v.parse::<i64>() {
                serde_json::json!(n)
            } else if let Ok(n) = v.parse::<f64>() {
                serde_json::json!(n)
            } else {
                continue;
            }
        } else {
            serde_json::Value::String(input.value())
        };

        // Patch the value into the config, preserving the original type.
        // find_config_slot returns a mutable reference to the slot so we
        // can check what JSON type it held before overwriting.
        let slot = find_config_slot(&mut config, &name);
        if let Some(slot) = slot {
            // If the original value was an array but the form produced a
            // string, split on commas back into an array of strings.
            if slot.is_array() {
                if let serde_json::Value::String(ref s) = new_val {
                    let arr: Vec<serde_json::Value> = s
                        .split(',')
                        .map(|part| serde_json::Value::String(part.trim().to_string()))
                        .filter(|v| v.as_str() != Some(""))
                        .collect();
                    *slot = serde_json::Value::Array(arr);
                } else {
                    *slot = new_val;
                }
            } else {
                *slot = new_val;
            }
        }
    }

    api::save_config(&config).await?;
    // Reload config view to show saved state.
    load_from_hash().await?;
    Ok(())
}

/// Toggle expand/collapse of a directory in the Files view.
#[wasm_bindgen]
pub fn mm_toggle_dir(dir_id: &str) {
    let doc = web_sys::window().unwrap().document().unwrap();
    if let Some(el) = doc.get_element_by_id(dir_id) {
        let current = el.get_attribute("style").unwrap_or_default();
        let hidden = current.contains("display:none") || current.contains("display: none");
        el.set_attribute("style", if hidden { "" } else { "display:none" }).ok();
    }
}
