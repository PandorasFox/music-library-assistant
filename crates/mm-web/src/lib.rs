mod api;
mod auth;
mod error;
mod socket;

use std::path::PathBuf;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

use socket::WitchPool;

pub use error::ApiError;

#[derive(Clone)]
pub struct AppState {
    pool: WitchPool,
}

impl AppState {
    pub async fn new(socket_path: PathBuf) -> anyhow::Result<Self> {
        let pool = WitchPool::new(socket_path);

        // Verify the Witch is reachable before accepting HTTP traffic.
        pool.verify_connectivity().await?;

        Ok(Self { pool })
    }
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
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
        // Commands
        .route("/commands/queue-task", post(api::commands::queue_task))
        .route("/commands/jettison", post(api::commands::jettison))
        .route("/commands/shutdown", post(api::commands::shutdown))
        .layer(cors)
        .with_state(state)
}
