//! fetch() wrapper for the mm-web JSON API.
//!
//! Uses web-sys Request/Response directly. All paths are relative (same origin).
//! Session token persisted in localStorage across page refreshes.

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

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

    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| JsValue::from_str(&format!("JSON parse error: {e}")))?;

    // 401 = token expired/invalid — clear it so refresh shows login.
    if status == 401 {
        clear_token();
    }

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

async fn post(path: &str, body: &serde_json::Value) -> Result<serde_json::Value, JsValue> {
    let body_str = serde_json::to_string(body).map_err(|e| JsValue::from_str(&e.to_string()))?;
    fetch("POST", path, Some(&body_str)).await
}

// ============================================================================
// Typed API helpers
// ============================================================================

/// POST /setup/check → bool (needs_setup)
pub async fn setup_check() -> Result<bool, JsValue> {
    let resp = post("/setup/check", &serde_json::json!({})).await?;
    Ok(resp
        .get("needs_setup")
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
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

/// GET /status → raw JSON value
pub async fn get_status() -> Result<serde_json::Value, JsValue> {
    get("/status").await
}

/// GET /queries/{name} → raw JSON value
pub async fn get_query(name: &str) -> Result<serde_json::Value, JsValue> {
    get(&format!("/queries/{name}")).await
}

/// GET /queries/{name}?key=value → raw JSON value
pub async fn get_query_with(name: &str, params: &str) -> Result<serde_json::Value, JsValue> {
    get(&format!("/queries/{name}?{params}")).await
}

/// GET /tx/details → transaction decision details
pub async fn tx_details() -> Result<serde_json::Value, JsValue> {
    get("/tx/details").await
}

/// POST /tx/confirm → commit transaction
pub async fn tx_confirm() -> Result<serde_json::Value, JsValue> {
    post("/tx/confirm", &serde_json::json!({})).await
}

/// POST /tx/discard → discard transaction
pub async fn tx_discard() -> Result<serde_json::Value, JsValue> {
    post("/tx/discard", &serde_json::json!({})).await
}
