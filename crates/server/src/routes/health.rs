//! # Health Routes — rotas de health check e persistência (save)

use axum::{routing::get, routing::post, Router};

use crate::handlers::health::health_check;
use crate::handlers::save::save_collections;
use crate::state::AppState;

/// Cria as rotas de health check e save (sem rate limit).
pub fn create_health_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/v1/save", post(save_collections))
}

