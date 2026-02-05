//! # Stats Handler — handler para estatísticas de coleções

use axum::{
    extract::{Path, State},
    response::Json,
};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Resposta de estatísticas de uma coleção.
#[derive(Debug, Serialize)]
pub struct CollectionStatsResponse {
    pub num_points: usize,
    pub num_queries: u64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
}

/// Handler para GET /api/v1/collections/{name}/stats
///
/// Retorna estatísticas de uma coleção incluindo número de pontos,
/// número de queries e percentis de latência.
pub async fn get_collection_stats(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<CollectionStatsResponse>> {
    // Verifica se a coleção existe
    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    // Obtém número de pontos
    let num_points = {
        let collection = collection_arc.read().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire read lock: {}", e))
        })?;
        collection.len()
    };

    // Obtém estatísticas de queries
    let (num_queries, avg_latency_ms, p50_latency_ms, p95_latency_ms, p99_latency_ms) = {
        let stats = app_state.query_stats.get(&name)
            .map(|s| {
                let num_queries = s.num_queries.load(std::sync::atomic::Ordering::Relaxed);
                let (avg, p50, p95, p99) = s.calculate_percentiles();
                (num_queries, avg, p50, p95, p99)
            })
            .unwrap_or((0, 0.0, 0.0, 0.0, 0.0));
        stats
    };

    Ok(Json(CollectionStatsResponse {
        num_points,
        num_queries,
        avg_latency_ms,
        p50_latency_ms,
        p95_latency_ms,
        p99_latency_ms,
    }))
}

