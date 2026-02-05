//! # Health Routes — rotas de health check, dashboard e persistência (save)

use axum::{
    routing::get,
    routing::post,
    Router,
};
use axum::response::Redirect;

use crate::handlers::dashboard::get_dashboard;
use crate::handlers::health::health_check;
use crate::handlers::save::save_collections;
use crate::state::AppState;

/// Cria as rotas de health check, dashboard e save (sem rate limit).
pub fn create_health_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/dashboard") }))
        .route("/health", get(health_check))
        .route("/dashboard", get(get_dashboard))
        .route("/api/v1/save", post(save_collections))
}

