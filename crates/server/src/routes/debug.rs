//! # Debug Routes — rotas de diagnóstico

use axum::{routing::get, Router};

use crate::handlers::debug::get_query_profile;
use crate::state::AppState;

/// Cria as rotas de debug.
pub fn create_debug_routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/debug/query-profile/{query_id}",
        get(get_query_profile),
    )
}
