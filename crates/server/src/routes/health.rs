//! # Health Routes — rotas de health check

use axum::{routing::get, Router};

use crate::handlers::health::health_check;
use crate::state::AppState;

/// Cria as rotas de health check.
pub fn create_health_routes() -> Router<AppState> {
    Router::new().route("/health", get(health_check))
}

