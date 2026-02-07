//! # Routes — definição de rotas HTTP
//!
//! Organiza as rotas do servidor em módulos separados por funcionalidade.
//! Rotas protegidas requerem API key via header `Authorization: Bearer <key>`.

mod audit;
mod auth;
mod collections;
mod debug;
mod health;
mod keys;
mod metrics;
mod reindex;
mod users;
mod points;
mod stats;
mod streaming;

use axum::Router;

use crate::auth::require_api_key;
use crate::middleware;
use crate::state::AppState;

/// Cria o router principal com todas as rotas.
///
/// - **Rotas protegidas** (requerem API key): collections, points, stats por coleção, save
/// - **Rotas públicas** (sem autenticação): health, metrics, stats globais
pub fn create_router() -> Router<AppState> {
    // Rotas protegidas com rate limit por coleção + autenticação
    let collection_scoped = Router::new()
        .merge(collections::create_named_collection_routes())
        .merge(points::create_points_routes())
        .merge(stats::create_stats_routes())
        .merge(reindex::create_reindex_routes())
        .layer(middleware::create_collection_rate_limit_layer());

    // Rotas protegidas (requerem API key)
    let protected = Router::new()
        .merge(collections::create_base_collection_routes())
        .merge(health::create_save_routes())
        .merge(keys::create_keys_routes())
        .merge(users::create_users_routes())
        .merge(audit::create_audit_routes())
        .merge(collection_scoped)
        .layer(axum::middleware::from_fn(require_api_key));

    // Rotas públicas (sem autenticação)
    let public = Router::new()
        .merge(health::create_health_routes())
        .merge(metrics::create_metrics_routes())
        .merge(stats::create_global_stats_routes())
        .merge(debug::create_debug_routes())
        .merge(auth::create_auth_routes());

    // WebSocket route (handles its own authentication via query param or header)
    let ws = streaming::create_streaming_routes();

    public.merge(protected).merge(ws)
}
