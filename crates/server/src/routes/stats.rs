//! # Stats Routes — rotas de estatísticas de coleções e analytics

use axum::{routing::get, Router};

use crate::handlers::stats::{
    get_analytics, get_collection_stats, get_feedback, get_global_stats, get_queries,
    get_slow_queries,
};
use crate::state::AppState;

/// Rotas de estatísticas por coleção (usadas sob o router com rate limit por coleção).
pub fn create_stats_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/collections/{name}/stats", get(get_collection_stats))
}

/// Rotas de analytics (global, queries, slow-queries; leem queries.log, cache 1h).
pub fn create_global_stats_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/stats/global", get(get_global_stats))
        .route("/api/v1/stats/analytics", get(get_analytics))
        .route("/api/v1/stats/queries", get(get_queries))
        .route("/api/v1/stats/slow-queries", get(get_slow_queries))
        .route("/api/v1/stats/feedback", get(get_feedback))
}

