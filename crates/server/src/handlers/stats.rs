//! # Stats Handler — handler para estatísticas de coleções e analytics (queries.log)

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};

use crate::api_err;
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
    /// Estimated bytes held by tombstoned points until next reindex (quantized index only).
    pub tombstone_memory_waste_bytes: usize,
}

// ─── Global stats (analytics: lê de queries.log, cache 1h) ───────────────

/// Um bucket de queries por minuto.
#[derive(Debug, Serialize)]
pub struct QueriesPerMinuteBucket {
    pub timestamp: u64,
    pub count: u64,
}

/// Resposta de GET /api/v1/stats/global.
#[derive(Debug, Serialize)]
pub struct GlobalStatsResponse {
    pub total_collections: usize,
    pub total_points: usize,
    pub total_queries_24h: u64,
    pub avg_latency_ms: f64,
    pub queries_per_minute: Vec<QueriesPerMinuteBucket>,
}

// ─── Queries list (GET /api/v1/stats/queries) ─────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct StatsQueriesParams {
    pub collection: Option<String>,
    #[serde(default = "default_queries_limit")]
    pub limit: usize,
    #[serde(default)]
    pub sort: String,
}

fn default_queries_limit() -> usize {
    100
}

/// Uma entrada de query para GET /api/v1/stats/queries e /stats/slow-queries.
#[derive(Debug, Serialize)]
pub struct QueryEntryResponse {
    pub timestamp: String,
    pub collection: String,
    pub limit: usize,
    pub filter: Option<serde_json::Value>,
    pub took_ms: u64,
    pub results_count: usize,
    pub query_id: String,
}

// ─── Slow queries (GET /api/v1/stats/slow-queries) ──────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct SlowQueriesParams {
    #[serde(default = "default_threshold_ms")]
    pub threshold_ms: u64,
    #[serde(default = "default_slow_limit")]
    pub limit: usize,
}

fn default_threshold_ms() -> u64 {
    100
}

fn default_slow_limit() -> usize {
    10
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

    // Obtém número de pontos e waste de tombstones
    let (num_points, tombstone_memory_waste_bytes) = {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        (collection.len(), collection.tombstone_memory_waste())
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
        tombstone_memory_waste_bytes,
    }))
}

/// Handler para GET /api/v1/stats/global
///
/// Agrega dados do estado (total_collections, total_points) e do queries.log
/// (total_queries_24h, avg_latency_ms, queries_per_minute). Cache de 1h no log.
pub async fn get_global_stats(
    State(app_state): State<AppState>,
) -> ApiResult<Json<GlobalStatsResponse>> {
    let total_collections = app_state.collections.len();
    let total_points: usize = app_state
        .collections
        .iter()
        .map(|entry| {
            entry
                .value()
                .read()
                .map(|c| c.len())
                .unwrap_or(0)
        })
        .sum();

    let cache = &app_state.query_log_cache;
    let total_queries_24h = cache.total_queries_24h();
    let avg_latency_ms = cache.avg_latency_24h();
    let queries_per_minute = cache
        .queries_per_minute_24h()
        .into_iter()
        .map(|(timestamp, count)| QueriesPerMinuteBucket { timestamp, count })
        .collect();

    Ok(Json(GlobalStatsResponse {
        total_collections,
        total_points,
        total_queries_24h,
        avg_latency_ms,
        queries_per_minute,
    }))
}

/// Handler para GET /api/v1/stats/queries
///
/// Lista queries das últimas 24h (lê de queries.log, cache 1h).
/// Query params: collection (opcional), limit=100, sort=latency (ordenar por latência, mais lentas primeiro).
pub async fn get_queries(
    State(app_state): State<AppState>,
    Query(params): Query<StatsQueriesParams>,
) -> ApiResult<Json<Vec<QueryEntryResponse>>> {
    let cache = &app_state.query_log_cache;
    let sort_by_latency = params.sort.eq_ignore_ascii_case("latency");
    let collection_filter = params.collection.as_deref();
    let entries = cache.get_queries(collection_filter, params.limit, sort_by_latency);
    let out = entries
        .into_iter()
        .map(|e| QueryEntryResponse {
            timestamp: e.timestamp,
            collection: e.collection,
            limit: e.limit,
            filter: e.filter,
            took_ms: e.took_ms,
            results_count: e.results_count,
            query_id: e.query_id,
        })
        .collect();
    Ok(Json(out))
}

/// Handler para GET /api/v1/stats/slow-queries
///
/// Queries com latência >= threshold_ms (lê de queries.log, cache 1h).
/// Query params: threshold_ms=100, limit=10.
pub async fn get_slow_queries(
    State(app_state): State<AppState>,
    Query(params): Query<SlowQueriesParams>,
) -> ApiResult<Json<Vec<QueryEntryResponse>>> {
    let cache = &app_state.query_log_cache;
    let entries = cache.get_slow_queries(params.threshold_ms, params.limit);
    let out = entries
        .into_iter()
        .map(|e| QueryEntryResponse {
            timestamp: e.timestamp,
            collection: e.collection,
            limit: e.limit,
            filter: e.filter,
            took_ms: e.took_ms,
            results_count: e.results_count,
            query_id: e.query_id,
        })
        .collect();
    Ok(Json(out))
}

// ─── Feedback (GET /api/v1/stats/feedback) ─────────────────────────────────
// Lê feedback.jsonl do diretório de logs (PoC: feedback inline Útil/Não útil).

#[derive(Debug, serde::Deserialize)]
struct FeedbackLogLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    useful: bool,
    #[serde(default)]
    comment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FeedbackEntryResponse {
    pub timestamp: String,
    pub session_id: Option<String>,
    pub useful: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FeedbackStatsResponse {
    pub useful_count: u64,
    pub not_useful_count: u64,
    pub entries: Vec<FeedbackEntryResponse>,
}

/// Handler para GET /api/v1/stats/feedback
///
/// Lê feedback.jsonl (log_dir/feedback.jsonl). Retorna contagens e últimas entradas para gráfico de satisfação.
pub async fn get_feedback(State(app_state): State<AppState>) -> ApiResult<Json<FeedbackStatsResponse>> {
    let feedback_path = app_state.config.storage_path.join("logs").join("feedback.jsonl");
    let content = match std::fs::read_to_string(&feedback_path) {
        Ok(c) => c,
        Err(_) => {
            return Ok(Json(FeedbackStatsResponse {
                useful_count: 0,
                not_useful_count: 0,
                entries: vec![],
            }));
        }
    };
    let mut useful_count: u64 = 0;
    let mut not_useful_count: u64 = 0;
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: FeedbackLogLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if row.useful {
            useful_count += 1;
        } else {
            not_useful_count += 1;
        }
        entries.push(FeedbackEntryResponse {
            timestamp: row.timestamp.unwrap_or_default(),
            session_id: row.session_id,
            useful: row.useful,
            comment: row.comment,
        });
    }
    // Últimas 200 entradas (mais recentes por ordem no arquivo)
    let take = 200;
    let start = entries.len().saturating_sub(take);
    let entries: Vec<_> = entries.drain(start..).collect();
    Ok(Json(FeedbackStatsResponse {
        useful_count,
        not_useful_count,
        entries,
    }))
}

