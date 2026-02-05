//! # Routes — definição de rotas HTTP
//!
//! Organiza as rotas do servidor em módulos separados por funcionalidade.

mod collections;
mod health;
mod metrics;
mod points;
mod stats;

use axum::Router;

use crate::middleware;
use crate::state::AppState;

/// Cria o router principal com todas as rotas.
///
/// O rate limiting por coleção é aplicado apenas às rotas que operam sobre
/// uma coleção específica (identificada por `:name` no path). Rotas como
/// `/health`, `GET /api/v1/collections` e `POST /api/v1/collections` ficam
/// sem rate limit por coleção.
pub fn create_router() -> Router<AppState> {
    // Rotas que operam sobre uma coleção nomeada — rate limited por coleção
    let collection_scoped = Router::new()
        .merge(collections::create_named_collection_routes())
        .merge(points::create_points_routes())
        .merge(stats::create_stats_routes())
        .layer(middleware::create_collection_rate_limit_layer());

    Router::new()
        .merge(health::create_health_routes())
        .merge(metrics::create_metrics_routes())
        .merge(collections::create_base_collection_routes())
        .merge(stats::create_global_stats_routes())
        .merge(collection_scoped)
}
