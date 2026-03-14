//! mm-web-client: WASM entry point for the Music Magic web UI.
//!
//! Builds mm-ui Node trees from API data and mounts them to the DOM.
//! v1 uses full re-render via innerHTML — no diffing.

mod api;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use mm_ui::html::widgets::{render_titlebar, render_status_bar};
use mm_ui::html::{self, div, h3, span, section, Node};
use mm_ui::lateral_view::LateralView;

// ============================================================================
// Entry Point
// ============================================================================

#[wasm_bindgen(start)]
pub fn main() {
    // Set up panic hook for console error messages.
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
    // Check if setup is needed.
    let needs_setup = api::setup_check().await?;
    if needs_setup {
        mount(&render_setup_needed());
        return Ok(());
    }

    // Show login form.
    mount(&render_login(None));
    Ok(())
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
        .child(
            div()
                .class("mm-login__title")
                .text("Music Magic"),
        )
        .child(
            div()
                .class("mm-login__error")
                .text(msg),
        );
    mount(&tree.into());
}

// ============================================================================
// View Renderers
// ============================================================================

fn render_setup_needed() -> Node {
    div()
        .class("mm-login")
        .child(div().class("mm-login__title").text("Music Magic"))
        .child(div().class("mm-login__error").text("First-time setup required. Use the TUI client to complete setup."))
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

/// Render the main app shell with titlebar, content, and status bar.
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

/// Render a JSON object as a titled section with key-value rows.
fn render_json_section(title: &str, json: &serde_json::Value) -> Node {
    let mut items = Vec::new();
    if let Some(obj) = json.as_object() {
        for (key, val) in obj {
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => "null".into(),
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    serde_json::to_string_pretty(val).unwrap_or_default()
                }
                other => other.to_string(),
            };
            items.push(
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(key))
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

/// Render WitchStatus JSON as a simple key-value display.
fn render_status_view(status_json: &serde_json::Value) -> Node {
    let mut items = Vec::new();

    if let Some(obj) = status_json.as_object() {
        for (key, val) in obj {
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => "null".into(),
                other => other.to_string(),
            };
            items.push(
                div()
                    .class("mm-kv")
                    .child(span().class("mm-kv__key").text(key))
                    .child(span().class("mm-kv__val").text(val_str))
                    .into(),
            );
        }
    }

    section()
        .class("mm-section")
        .child(h3().class("mm-section__title").text("Witch Status"))
        .children(items)
        .into()
}

// ============================================================================
// Global callbacks (called from onclick attributes)
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
        Ok(token) => {
            // Store token and load the main view.
            api::set_token(&token);
            load_main_view().await?;
            Ok(())
        }
        Err(e) => {
            mount(&render_login(Some(&format!("{e:?}"))));
            Ok(())
        }
    }
}

async fn load_main_view() -> Result<(), JsValue> {
    let status_json = api::get_status().await?;
    let insights_json = api::get_query("insights").await.ok();

    let mut content_children = vec![render_status_view(&status_json)];
    if let Some(insights) = insights_json {
        content_children.push(render_json_section("Insights", &insights));
    }

    let content = div().children(content_children).into();
    mount(&render_app_shell(
        LateralView::Health,
        false,
        content,
        Some("Connected"),
    ));
    Ok(())
}

// ============================================================================
// Register global JS function for login button
// ============================================================================

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

/// Called once on init to register the global login callback.
#[wasm_bindgen(js_name = "__mm_setup_globals")]
pub fn setup_globals() {
    // The login button uses onclick="window.__mm_login()" which calls mm_login().
}
