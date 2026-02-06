//! # Health Routes — rotas de health check (públicas)

use axum::{
    routing::get,
    Router,
};

use crate::handlers::health::health_check;
use crate::state::AppState;

/// Cria as rotas de health check (públicas, sem autenticação).
pub fn create_health_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health_check))
}

/// Cria a rota de save (protegida, requer API key).
pub fn create_save_routes() -> Router<AppState> {
    use axum::routing::post;
    use crate::handlers::save::save_collections;

    Router::new()
        .route("/api/v1/save", post(save_collections))
}

