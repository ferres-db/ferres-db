//! # Metrics Routes — rotas de métricas Prometheus

use axum::{routing::get, Router};

use crate::handlers::metrics::get_metrics;
use crate::state::AppState;

/// Cria as rotas de métricas.
pub fn create_metrics_routes() -> Router<AppState> {
    Router::new().route("/metrics", get(get_metrics))
}
