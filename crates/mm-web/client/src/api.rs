//! fetch() wrapper for the mm-web JSON API.
//!
//! Uses web-sys Request/Response directly. All paths are relative (same origin).
//! Session token persisted in localStorage across page refreshes.
//!
//! Typed functions deserialize into mm-meta structs where possible, so the
//! compiler catches field name mismatches rather than silently rendering nothing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

use mm_meta::domain_query_types::SessionEditDetail;
use mm_meta::views::cluster_deploy::DeployModalData;
use mm_meta::views::{
    DeployStatus, EditHistoryData, ExternalMatchesData, InsightsData,
};
use mm_meta::witch_types::WitchStatus;

const TOKEN_KEY: &str = "mm-session-token";

fn storage() -> web_sys::Storage {
    web_sys::window()
        .unwrap()
        .local_storage()
        .unwrap()
        .unwrap()
}

pub fn set_token(token: &str) {
    storage().set_item(TOKEN_KEY, token).ok();
}

pub fn get_token() -> Option<String> {
    storage().get_item(TOKEN_KEY).ok().flatten()
}

pub fn clear_token() {
    storage().remove_item(TOKEN_KEY).ok();
}

pub fn has_token() -> bool {
    get_token().is_some()
}

// ============================================================================
// Low-level fetch
// ============================================================================

async fn fetch(method: &str, path: &str, body: Option<&str>) -> Result<serde_json::Value, JsValue> {
    let opts = RequestInit::new();
    opts.set_method(method);
    opts.set_mode(RequestMode::SameOrigin);

    let headers = Headers::new()?;
    headers.set("Content-Type", "application/json")?;

    if let Some(token) = get_token() {
        headers.set("Authorization", &format!("Bearer {token}"))?;
    }

    opts.set_headers(&headers);

    if let Some(b) = body {
        opts.set_body(&JsValue::from_str(b));
    }

    let request = Request::new_with_str_and_init(path, &opts)?;
    let window = web_sys::window().unwrap();
    let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let resp: Response = resp_value.dyn_into()?;

    let status = resp.status();
    let text_promise = resp.text()?;
    let text_js = JsFuture::from(text_promise).await?;
    let text = text_js.as_string().unwrap_or_default();

    // 401 = token expired/invalid — clear it and redirect to login immediately.
    if status == 401 {
        clear_token();
        crate::redirect_to_login();
    }

    // Non-2xx with non-JSON body — surface the raw text.
    if status >= 400 {
        // Try to extract a JSON error message, fall back to raw text.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(err) = value.get("error") {
                return Err(JsValue::from_str(
                    err.as_str().unwrap_or("unknown error"),
                ));
            }
        }
        return Err(JsValue::from_str(&format!("HTTP {status}: {text}")));
    }

    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| JsValue::from_str(&format!("JSON parse error: {e}")))?;

    // Check for error field in response.
    if let Some(err) = value.get("error") {
        return Err(JsValue::from_str(
            err.as_str().unwrap_or("unknown error"),
        ));
    }

    Ok(value)
}

async fn get(path: &str) -> Result<serde_json::Value, JsValue> {
    fetch("GET", path, None).await
}

pub async fn post(path: &str, body: &serde_json::Value) -> Result<serde_json::Value, JsValue> {
    let body_str = serde_json::to_string(body).map_err(|e| JsValue::from_str(&e.to_string()))?;
    fetch("POST", path, Some(&body_str)).await
}

/// Deserialize a JSON value into a typed struct.
fn from_json<T: DeserializeOwned>(val: serde_json::Value) -> Result<T, JsValue> {
    serde_json::from_value(val).map_err(|e| JsValue::from_str(&format!("deserialize: {e}")))
}

// ============================================================================
// Unauthenticated endpoints
// ============================================================================

/// Setup status: whether first-time setup is needed, and optional suggested root.
pub struct SetupStatus {
    pub needs_setup: bool,
    pub suggested_root: Option<String>,
}

/// POST /setup/check → SetupStatus
pub async fn setup_check() -> Result<SetupStatus, JsValue> {
    let resp = post("/setup/check", &serde_json::json!({})).await?;
    Ok(SetupStatus {
        needs_setup: resp.get("needs_setup").and_then(|v| v.as_bool()).unwrap_or(false),
        suggested_root: resp.get("suggested_root").and_then(|v| v.as_str()).map(String::from),
    })
}

/// POST /setup/complete → complete first-time setup
pub async fn setup_complete(root: &str, username: &str, password: &str) -> Result<(), JsValue> {
    let mut body = serde_json::json!({ "root": root });
    if !username.is_empty() && !password.is_empty() {
        body["username"] = serde_json::Value::String(username.to_string());
        body["password"] = serde_json::Value::String(password.to_string());
    }
    post("/setup/complete", &body).await?;
    Ok(())
}

/// POST /auth/login → token string (saved to localStorage)
pub async fn login(username: &str, password: &str) -> Result<String, JsValue> {
    let body = serde_json::json!({
        "username": username,
        "password": password,
    });
    let resp = post("/auth/login", &body).await?;
    let token = resp
        .get("token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| JsValue::from_str("no token in response"))?;
    set_token(&token);
    Ok(token)
}

// ============================================================================
// Typed query endpoints
// ============================================================================

/// GET /status → WitchStatus
pub async fn get_status() -> Result<WitchStatus, JsValue> {
    from_json(get("/status").await?)
}

/// GET /build-info → build timestamp string
pub async fn get_build_info() -> Result<String, JsValue> {
    let json: serde_json::Value = from_json(get("/build-info").await?)?;
    Ok(json.get("build").and_then(|v| v.as_str()).unwrap_or("unknown").to_string())
}

/// GET /config → raw JSON (for config editor's generic field renderer)
pub async fn get_config_json() -> Result<serde_json::Value, JsValue> {
    get("/config").await
}

/// GET /config-kdl → raw KDL text of config.kdl
pub async fn get_config_kdl() -> Result<String, JsValue> {
    let val = get("/config-kdl").await?;
    val.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| JsValue::from_str("expected string response from /config-kdl"))
}

/// GET /dir-config?path=... → Option<SourceDir>
pub async fn get_dir_config(path: &str) -> Result<Option<mm_meta::config::SourceDir>, JsValue> {
    let url = format!(
        "/dir-config?path={}",
        js_sys::encode_uri_component(path)
    );
    from_json(get(&url).await?)
}

/// GET /queries/insights → InsightsData
pub async fn get_insights() -> Result<InsightsData, JsValue> {
    from_json(get("/queries/insights").await?)
}

/// GET /queries/deploy-status → DeployStatus
pub async fn get_deploy_status() -> Result<DeployStatus, JsValue> {
    from_json(get("/queries/deploy-status").await?)
}

/// GET /queries/deploy-data → DeployModalData (full tabbed preview data)
pub async fn get_deploy_data() -> Result<DeployModalData, JsValue> {
    from_json(get("/queries/deploy-data").await?)
}

/// GET /queries/edit-history → EditHistoryData
pub async fn get_edit_history() -> Result<EditHistoryData, JsValue> {
    from_json(get("/queries/edit-history").await?)
}

/// GET /queries/session-edit-detail?session_id=X → SessionEditDetail
pub async fn get_session_detail(session_id: &str) -> Result<SessionEditDetail, JsValue> {
    from_json(
        get(&format!(
            "/queries/session-edit-detail?session_id={}",
            js_sys::encode_uri_component(session_id)
        ))
        .await?,
    )
}

/// GET /queries/external-matches → ExternalMatchesData
pub async fn get_external_matches() -> Result<ExternalMatchesData, JsValue> {
    from_json(get("/queries/external-matches").await?)
}

// ============================================================================
// Untyped query endpoints (composite/complex responses)
// ============================================================================

/// GET /queries/{name} → raw JSON (paramless domain queries)
pub async fn get_query(name: &str) -> Result<serde_json::Value, JsValue> {
    get(&format!("/queries/{name}")).await
}

/// GET /queries/{name}?params → raw JSON (for complex/composite responses)
pub async fn get_query_with(name: &str, params: &str) -> Result<serde_json::Value, JsValue> {
    get(&format!("/queries/{name}?{params}")).await
}

/// GET /queries/directory-listing?zone=Corpus[&parent=some/path]
pub async fn get_directory_listing(parent: Option<&str>) -> Result<serde_json::Value, JsValue> {
    let url = match parent {
        Some(p) => format!(
            "/queries/directory-listing?zone=Corpus&parent={}",
            js_sys::encode_uri_component(p)
        ),
        None => "/queries/directory-listing?zone=Corpus".to_string(),
    };
    get(&url).await
}

/// GET /queries/search-corpus?query=X&limit=200
pub async fn search_corpus(query: &str, limit: usize) -> Result<serde_json::Value, JsValue> {
    let url = format!(
        "/queries/search-corpus?query={}&limit={}",
        js_sys::encode_uri_component(query),
        limit,
    );
    get(&url).await
}

// ============================================================================
// Transaction endpoints
// ============================================================================

/// GET /tx/details → typed decision details
pub async fn tx_details() -> Result<Vec<mm_meta::protocol::DecisionDetail>, JsValue> {
    from_json(get("/tx/details").await?)
}

/// POST /tx/start → start a new transaction
pub async fn tx_start(label: &str) -> Result<serde_json::Value, JsValue> {
    post("/tx/start", &serde_json::json!({ "label": label })).await
}

/// POST /tx/add → add a decision to the active transaction
pub async fn tx_add(
    key: &mm_meta::decisions::DecisionKey,
    decision: &mm_meta::decisions::Decision,
) -> Result<serde_json::Value, JsValue> {
    post("/tx/add", &serde_json::json!({ "key": key, "decision": decision })).await
}

/// POST /tx/remove → remove a decision by key
pub async fn tx_remove(key: &mm_meta::decisions::DecisionKey) -> Result<serde_json::Value, JsValue> {
    post("/tx/remove", &serde_json::json!({ "key": key })).await
}

/// POST /tx/confirm → commit transaction.
///
/// Requires a [`ConfirmationWitness`] — only [`super::mm_tx_confirm`] can mint one,
/// ensuring no code path auto-confirms without operator intent.
pub async fn tx_confirm(_witness: ConfirmationWitness) -> Result<serde_json::Value, JsValue> {
    post("/tx/confirm", &serde_json::json!({})).await
}

/// Zero-sized proof that the current call originated from the operator's
/// explicit "Confirm" action in the transaction review UI.
///
/// Constructor is `pub(super)` — only the `mm_tx_confirm` wasm_bindgen export
/// in `lib.rs` can mint one.
pub struct ConfirmationWitness(());

impl ConfirmationWitness {
    pub(super) fn new() -> Self {
        Self(())
    }
}

/// POST /tx/discard → discard transaction
pub async fn tx_discard() -> Result<serde_json::Value, JsValue> {
    post("/tx/discard", &serde_json::json!({})).await
}

// ============================================================================
// Command endpoints
// ============================================================================

/// POST /commands/queue-task → queue a background task
pub async fn queue_task(task: &str) -> Result<serde_json::Value, JsValue> {
    post("/commands/queue-task", &serde_json::json!({ "task": task })).await
}

/// POST /actions/execute → stage a typed protocol binding as a decision
pub async fn stage_action(binding: &serde_json::Value) -> Result<serde_json::Value, JsValue> {
    post("/actions/execute", binding).await
}

// ============================================================================
// WebSocket event stream (with reconnection)
// ============================================================================

/// Backoff delays for reconnection attempts. After exhausting this table,
/// retries continue at the last delay indefinitely — the WS event stream is
/// a convenience push channel and its failure must never destroy the session.
const RECONNECT_DELAYS: &[i32] = &[1_000, 2_000, 4_000, 8_000, 16_000, 30_000];

thread_local! {
    static EVENT_WS: RefCell<Option<web_sys::WebSocket>> = const { RefCell::new(None) };
    /// Stored callback so reconnection can reuse it without re-capturing.
    static WS_CALLBACK: RefCell<Option<Rc<dyn Fn(WitchStatus)>>> = const { RefCell::new(None) };
    /// Consecutive failed reconnection attempts (reset on first successful message).
    static RECONNECT_ATTEMPT: Cell<u32> = const { Cell::new(0) };
}

/// Open a WebSocket to `/ws?token=<token>` for server-pushed events.
///
/// `on_status` is called with each pushed `WitchStatus` JSON string.
/// On close, attempts reconnection with exponential backoff indefinitely.
/// The WS is a push convenience channel — its failure never touches the
/// session token or redirects to login.
pub fn start_event_stream(on_status: impl Fn(WitchStatus) + 'static) {
    stop_event_stream();
    let callback = Rc::new(on_status);
    WS_CALLBACK.with(|cell| *cell.borrow_mut() = Some(callback.clone()));
    RECONNECT_ATTEMPT.with(|cell| cell.set(0));
    connect_ws(callback);
}

/// Internal: establish a WebSocket connection using the stored token.
fn connect_ws(on_status: Rc<dyn Fn(WitchStatus)>) {
    disconnect_ws();

    let token = match get_token() {
        Some(t) => t,
        None => return,
    };

    let window = web_sys::window().unwrap();
    let location = window.location();
    let protocol = location.protocol().unwrap_or_default();
    let host = location.host().unwrap_or_default();
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    let url = format!("{ws_protocol}//{host}/ws?token={token}");

    let ws = match web_sys::WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(_) => {
            // Constructor failed — schedule reconnection if we still have a token.
            schedule_reconnect();
            return;
        }
    };

    // onmessage: parse WitchEvent JSON → extract WitchStatus → call callback.
    // A successful message means the connection is healthy — reset backoff.
    let on_message = {
        let cb = on_status.clone();
        Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
            RECONNECT_ATTEMPT.with(|cell| cell.set(0));

            if let Some(text) = e.data().as_string() {
                if let Ok(event) =
                    serde_json::from_str::<mm_meta::witch_types::WitchEvent>(&text)
                {
                    match event {
                        mm_meta::witch_types::WitchEvent::StatusChanged(status) => {
                            cb(status);
                        }
                    }
                }
            }
        }) as Box<dyn Fn(web_sys::MessageEvent)>)
    };
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    // onclose: attempt reconnection instead of killing the session.
    // The WS is a push convenience channel — its failure never touches the
    // session token or redirects to login. Append a visual "reconnecting"
    // hint to the status bar so the operator can see push events are stale.
    let on_close = Closure::wrap(Box::new(|_: web_sys::CloseEvent| {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("mm-status-bar"))
        {
            // Prepend a reconnecting notice; the next successful WS push
            // re-renders the entire bar, clearing this automatically.
            let current = el.inner_html();
            el.set_inner_html(&format!(
                "<div class=\"mm-status__line mm-status__line--warn\">\
                 event stream disconnected — reconnecting\u{2026}</div>{current}"
            ));
        }
        // If the token was already cleared (e.g. by a 401 on an HTTP fetch),
        // redirect_to_login() was already called — nothing to do here.
        if has_token() {
            schedule_reconnect();
        }
    }) as Box<dyn Fn(web_sys::CloseEvent)>);
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

    // onerror: onclose fires after onerror — reconnection handled there.
    let on_error = Closure::wrap(Box::new(|_: web_sys::ErrorEvent| {}) as Box<dyn Fn(web_sys::ErrorEvent)>);
    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    EVENT_WS.with(|cell| *cell.borrow_mut() = Some(ws));
}

/// Schedule a reconnection attempt after an exponential backoff delay.
/// Retries indefinitely — the WS is a push channel, not an auth mechanism.
/// After exhausting the delay table, continues at the last (max) delay.
fn schedule_reconnect() {
    let attempt = RECONNECT_ATTEMPT.with(|cell| cell.get());

    let delay_idx = (attempt as usize).min(RECONNECT_DELAYS.len() - 1);
    let delay = RECONNECT_DELAYS[delay_idx];
    RECONNECT_ATTEMPT.with(|cell| cell.set(attempt + 1));

    let closure = Closure::once(move || {
        // Re-check token — an HTTP 401 may have cleared it while we were waiting.
        if !has_token() {
            return;
        }
        let callback = WS_CALLBACK.with(|cell| cell.borrow().clone());
        if let Some(cb) = callback {
            connect_ws(cb);
        }
    });
    web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            delay,
        )
        .ok();
    closure.forget();
}

/// Close the current WebSocket without clearing the stored callback.
/// Used internally before reconnection.
fn disconnect_ws() {
    EVENT_WS.with(|cell| {
        if let Some(ws) = cell.borrow_mut().take() {
            ws.set_onclose(None);
            ws.set_onerror(None);
            ws.set_onmessage(None);
            let _ = ws.close();
        }
    });
}

/// Stop the event stream entirely — closes the WebSocket and clears
/// reconnection state. Used on intentional logout / redirect_to_login.
pub fn stop_event_stream() {
    disconnect_ws();
    WS_CALLBACK.with(|cell| cell.borrow_mut().take());
    RECONNECT_ATTEMPT.with(|cell| cell.set(0));
}

