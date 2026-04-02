mod api;
mod auth;
mod error;
mod socket;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

use mm_meta::witch_types::WitchStatus;
use socket::WitchConnection;

pub use error::ApiError;

const BUILD_TIMESTAMP: &str = env!("MM_BUILD_TIMESTAMP");

// ============================================================================
// Application State
// ============================================================================

/// Local caches that mm-web maintains independently of Witch round-trips.
///
/// - `cached_status`: Updated from WitchEvent broadcasts. Served at GET /status
///   without ever querying the Witch.
/// - `session_tokens`: Populated on successful login. Used to validate WebSocket
///   connections locally — no Witch round-trip for WS auth.
#[derive(Clone)]
pub struct LocalCache {
    pub cached_status: Arc<RwLock<Option<WitchStatus>>>,
    pub session_tokens: Arc<RwLock<HashSet<Vec<u8>>>>,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) conn: WitchConnection,
    static_dir: PathBuf,
    pub(crate) local: LocalCache,
}

impl AppState {
    pub async fn new(socket_path: PathBuf, static_dir: PathBuf) -> anyhow::Result<Self> {
        let conn = WitchConnection::connect(&socket_path).await?;

        // Verify the Witch is reachable before accepting HTTP traffic.
        conn.verify_connectivity().await?;

        let local = LocalCache {
            cached_status: Arc::new(RwLock::new(None)),
            session_tokens: Arc::new(RwLock::new(HashSet::new())),
        };

        // Spawn background task: subscribe to WitchEvents, update cached status.
        let event_rx = conn.subscribe_events();
        let status_cache = local.cached_status.clone();
        tokio::spawn(async move {
            status_cache_loop(event_rx, status_cache).await;
        });

        Ok(Self { conn, static_dir, local })
    }

    /// Register a session token in the local cache (called on successful login).
    pub(crate) fn register_session(&self, token_bytes: &[u8]) {
        let mut tokens = self.local.session_tokens.write().expect("session lock");
        tokens.insert(token_bytes.to_vec());
    }

    /// Check if a session token is in the local cache.
    pub(crate) fn is_valid_session(&self, token_bytes: &[u8]) -> bool {
        let tokens = self.local.session_tokens.read().expect("session lock");
        tokens.contains(token_bytes)
    }

    /// Get the cached WitchStatus, if available.
    pub(crate) fn cached_status(&self) -> Option<WitchStatus> {
        let status = self.local.cached_status.read().expect("status lock");
        status.clone()
    }
}

/// Background loop: receive WitchEvent broadcasts, update cached status.
async fn status_cache_loop(
    mut event_rx: tokio::sync::broadcast::Receiver<mm_meta::witch_types::WitchEvent>,
    cache: Arc<RwLock<Option<WitchStatus>>>,
) {
    loop {
        match event_rx.recv().await {
            Ok(mm_meta::witch_types::WitchEvent::StatusChanged(status)) => {
                let mut cached = cache.write().expect("status lock");
                *cached = Some(status);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Missed some — next recv will be fresh.
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return; // Witch shut down.
            }
        }
    }
}

// ============================================================================
// Router
// ============================================================================

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let static_dir = state.static_dir.clone();

    Router::new()
        // Unauthenticated
        .route("/setup/check", post(api::unauth::setup_check))
        .route("/setup/complete", post(api::unauth::setup_complete))
        .route("/auth/login", post(api::unauth::login))
        .route("/auth/logout", post(api::unauth::logout))
        // Queries
        .route("/status", get(api::queries::status))
        .route("/config", get(api::queries::config))
        .route("/config-kdl", get(api::queries::config_kdl))
        .route("/dir-config", get(api::queries::dir_config))
        .route("/queries/{name}", get(api::queries::domain_query).post(api::queries::domain_query))
        // Transactions
        .route("/tx/start", post(api::transactions::start))
        .route("/tx/add", post(api::transactions::add_decision))
        .route("/tx/remove", post(api::transactions::remove_decision))
        .route("/tx/details", get(api::transactions::details))
        .route("/tx/confirm", post(api::transactions::confirm))
        .route("/tx/discard", post(api::transactions::discard))
        .route("/tx/approve-releases", post(api::transactions::approve_releases))
        .route("/tx/stage-deploy", post(api::transactions::stage_deploy))
        // WebSocket event stream (validated against local session cache)
        .route("/ws", get(api::ws::ws_handler))
        // Build info (unauthenticated — just a timestamp)
        .route("/build-info", get(serve_build_info))
        // Actions (typed protocol bindings)
        .route("/actions/execute", post(api::actions::execute))
        // Commands
        .route("/commands/queue-task", post(api::commands::queue_task))
        .route("/commands/shutdown", post(api::commands::shutdown))
        // Serve React SPA assets from frontend/dist/
        .nest_service(
            "/assets",
            SetResponseHeader::overriding(
                ServeDir::new(static_dir.join("assets")),
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=31536000, immutable"),
            ),
        )
        // SPA fallback: serve index.html for any unmatched path (React Router).
        .fallback(serve_index)
        .layer(cors)
        .with_state(state)
}

async fn serve_build_info() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "build": BUILD_TIMESTAMP }))
}

async fn serve_index(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl IntoResponse {
    let index_path = state.static_dir.join("index.html");
    let contents = match tokio::fs::read_to_string(&index_path).await {
        Ok(c) => c,
        Err(_) => "<h1>index.html not found — run: cd crates/mm-web/frontend && npm run build</h1>".into(),
    };
    ([(header::CACHE_CONTROL, "no-cache")], Html(contents))
}
