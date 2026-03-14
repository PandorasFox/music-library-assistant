mod api;
mod auth;
mod error;
mod socket;

use std::path::PathBuf;

use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use socket::WitchConnection;

pub use error::ApiError;

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
        .route("/queries/{name}", get(api::queries::domain_query).post(api::queries::domain_query))
        // Transactions
        .route("/tx/start", post(api::transactions::start))
        .route("/tx/add", post(api::transactions::add_decision))
        .route("/tx/remove", post(api::transactions::remove_decision))
        .route("/tx/details", get(api::transactions::details))
        .route("/tx/confirm", post(api::transactions::confirm))
        .route("/tx/discard", post(api::transactions::discard))
        // Actions (typed protocol bindings)
        .route("/actions/execute", post(api::actions::execute))
        // Commands
        .route("/commands/queue-task", post(api::commands::queue_task))
        .route("/commands/save-config", post(api::commands::save_config))
        .route("/commands/jettison", post(api::commands::jettison))
        .route("/commands/shutdown", post(api::commands::shutdown))
        // Static files (CSS, WASM, JS)
        .nest_service("/static", ServeDir::new(static_dir))
        .layer(cors)
        .with_state(state)
}

async fn serve_index(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Html<String> {
    let index_path = state.static_dir.join("index.html");
    match tokio::fs::read_to_string(&index_path).await {
        Ok(contents) => Html(contents),
        Err(_) => Html("<h1>index.html not found</h1>".into()),
    }
}
