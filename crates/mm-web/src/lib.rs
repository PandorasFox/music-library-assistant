mod api;
mod auth;
mod error;
mod socket;

use std::path::PathBuf;

use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

use socket::WitchConnection;

pub use error::ApiError;

const BUILD_TIMESTAMP: &str = env!("MM_BUILD_TIMESTAMP");

#[derive(Clone)]
pub struct AppState {
    conn: WitchConnection,
    static_dir: PathBuf,
}

impl AppState {
    pub async fn new(socket_path: PathBuf, static_dir: PathBuf) -> anyhow::Result<Self> {
        let conn = WitchConnection::connect(&socket_path).await?;

        // Verify the Witch is reachable before accepting HTTP traffic.
        conn.verify_connectivity().await?;

        Ok(Self { conn, static_dir })
    }
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let static_dir = state.static_dir.clone();

    Router::new()
        // HTML shell
        .route("/", get(serve_index))
        // Unauthenticated
        .route("/setup/check", post(api::unauth::setup_check))
        .route("/setup/complete", post(api::unauth::setup_complete))
        .route("/auth/login", post(api::unauth::login))
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
        // Actions (typed protocol bindings)
        .route("/actions/execute", post(api::actions::execute))
        // Commands
        .route("/commands/queue-task", post(api::commands::queue_task))
        .route("/commands/shutdown", post(api::commands::shutdown))
        // Static files (CSS, WASM, JS) — cached aggressively, busted by ?v= in index.html
        .nest_service(
            "/static",
            SetResponseHeader::overriding(
                ServeDir::new(static_dir),
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000"),
            ),
        )
        .layer(cors)
        .with_state(state)
}

async fn serve_index(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl IntoResponse {
    let index_path = state.static_dir.join("index.html");
    let contents = match tokio::fs::read_to_string(&index_path).await {
        Ok(c) => c,
        Err(_) => "<h1>index.html not found</h1>".into(),
    };
    // Inject build-version query params so asset URLs change on each rebuild.
    let contents = contents
        .replace("/static/mm.css", &format!("/static/mm.css?v={BUILD_TIMESTAMP}"))
        .replace(
            "/static/pkg/mm_web_client.js",
            &format!("/static/pkg/mm_web_client.js?v={BUILD_TIMESTAMP}"),
        );
    ([(header::CACHE_CONTROL, "no-cache")], Html(contents))
}
