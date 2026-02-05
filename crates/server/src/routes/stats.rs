//! # Stats Routes — rotas de estatísticas de coleções

use axum::{routing::get, Router};

use crate::handlers::stats::get_collection_stats;
use crate::state::AppState;

/// Cria as rotas de estatísticas.
pub fn create_stats_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/collections/{name}/stats", get(get_collection_stats))
}

