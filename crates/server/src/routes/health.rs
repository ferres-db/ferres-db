//! # Health Routes — rotas de health check e dashboard (públicas)

use axum::{
    routing::get,
    Router,
};
use axum::response::Redirect;

use crate::handlers::dashboard::get_dashboard;
use crate::handlers::health::health_check;
use crate::state::AppState;

/// Cria as rotas de health check e dashboard (públicas, sem autenticação).
pub fn create_health_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/dashboard") }))
        .route("/health", get(health_check))
        .route("/dashboard", get(get_dashboard))
}

/// Cria a rota de save (protegida, requer API key).
pub fn create_save_routes() -> Router<AppState> {
    use axum::routing::post;
    use crate::handlers::save::save_collections;

    Router::new()
        .route("/api/v1/save", post(save_collections))
}

