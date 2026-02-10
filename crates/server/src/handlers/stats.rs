//! # Stats Handler — handler para estatísticas de coleções e analytics (queries.log)

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};

use ferres_db_core::simd_enabled;

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
    /// Number of tombstoned points (deleted but not yet compacted from index).
    pub tombstone_count: usize,
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
    /// Whether SIMD (AVX2/SSE4.1) acceleration is active at runtime for distance kernels.
    pub simd_enabled: bool,
    /// Replication role: "leader" or "replica" (experimental).
    pub role: String,
    /// Whether namespace physical isolation is enabled (points per namespace in separate dirs).
    pub namespace_physical_isolation: bool,
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

// ─── Analytics (GET /api/v1/stats/analytics) ───────────────────────────────

/// Distribuição agregada de pontos por tier (Hot/Warm/Cold).
#[derive(Debug, Serialize)]
pub struct AnalyticsTierDistribution {
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
    pub hot_memory_bytes: usize,
    pub warm_memory_bytes: usize,
    pub cold_memory_bytes: usize,
}

/// Bucket de latência por minuto para gráfico histórico.
#[derive(Debug, Serialize)]
pub struct LatencyPerMinuteBucket {
    pub timestamp: u64,
    pub avg_ms: f64,
    pub p50_ms: f64,
}

/// Métricas de latência de busca (query log, últimas 24h).
#[derive(Debug, Serialize)]
pub struct AnalyticsLatency {
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub latency_per_minute: Vec<LatencyPerMinuteBucket>,
}

/// Agregado de tombstones em todas as coleções.
#[derive(Debug, Serialize)]
pub struct AnalyticsTombstones {
    pub total_count: usize,
    pub total_memory_waste_bytes: usize,
    pub total_points: usize,
}

/// Estado do circuit breaker de storage (0=closed, 1=open, 2=half_open).
#[derive(Debug, Serialize)]
pub struct AnalyticsCircuitBreaker {
    pub state: String,
    pub failure_count: u64,
}

/// Bucket de throughput por minuto (série temporal, últimos 10 min).
#[derive(Debug, Serialize)]
pub struct ThroughputPerMinuteBucket {
    pub timestamp: u64,
    pub points: u64,
}

/// Latência de uma query recente (para gráfico de área).
#[derive(Debug, Serialize)]
pub struct RecentLatencyEntry {
    pub timestamp: u64,
    pub took_ms: u64,
}

/// Agregações de série temporal (últimos 10 min): throughput e latência.
#[derive(Debug, Serialize)]
pub struct TimeSeries10m {
    /// Média de pontos inseridos por segundo (últimos 10 min).
    pub avg_points_per_second: f64,
    /// P95 da latência de busca (ms) nas últimas 10 min.
    pub p95_latency_ms: f64,
    /// Throughput por minuto para gráfico de ingestão.
    pub throughput_per_minute: Vec<ThroughputPerMinuteBucket>,
    /// Últimas consultas (timestamp, took_ms) para gráfico de latência.
    pub recent_latencies: Vec<RecentLatencyEntry>,
}

/// Resposta de GET /api/v1/stats/analytics.
#[derive(Debug, Serialize)]
pub struct AnalyticsResponse {
    pub tier_distribution: AnalyticsTierDistribution,
    pub latency: AnalyticsLatency,
    pub tombstones: AnalyticsTombstones,
    pub circuit_breaker: AnalyticsCircuitBreaker,
    /// Séries temporais dos últimos 10 min (throughput + P95 + dados para gráficos).
    pub time_series_10m: TimeSeries10m,
    /// Cache hit rate % (search_cache do core, agregado em todas as coleções). None se nenhuma busca.
    pub cache_hit_rate_pct: Option<f64>,
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

    // Obtém número de pontos, tombstone count e waste
    let (num_points, tombstone_count, tombstone_memory_waste_bytes) = {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        (
            collection.len(),
            collection.tombstone_count(),
            collection.tombstone_memory_waste(),
        )
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
        tombstone_count,
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

    let role = if app_state.config.replica_of.is_some() {
        "replica"
    } else {
        "leader"
    };

    Ok(Json(GlobalStatsResponse {
        total_collections,
        total_points,
        total_queries_24h,
        avg_latency_ms,
        queries_per_minute,
        simd_enabled: simd_enabled(),
        role: role.to_string(),
        namespace_physical_isolation: app_state.config.namespace_physical_isolation,
    }))
}

/// Handler para GET /api/v1/stats/analytics
///
/// Retorna JSON consolidado: distribuição por tier, latência (avg, P50/P95/P99, histórico por minuto),
/// tombstones agregados e estado do circuit breaker de storage.
pub async fn get_analytics(
    State(app_state): State<AppState>,
) -> ApiResult<Json<AnalyticsResponse>> {
    // Agregação de tier (mesma lógica que get_tier_distribution por coleção)
    let mut hot = 0_usize;
    let warm = 0_usize;
    let cold = 0_usize;
    let mut hot_memory_bytes = 0_usize;
    let warm_memory_bytes = 0_usize;
    let cold_memory_bytes = 0_usize;
    for entry in app_state.collections.iter() {
        let collection = match entry.value().read() {
            Ok(c) => c,
            Err(_) => continue,
        };
        let config = collection.config();
        let num_points = collection.len();
        let dimension = config.dimension;
        let vector_bytes = dimension * 4;
        let metadata_est = 200;
        let hnsw_node_est = 128;
        if !config.tiered_storage.enabled {
            hot += num_points;
            hot_memory_bytes += num_points * (vector_bytes + metadata_est + hnsw_node_est);
        } else {
            hot += num_points;
            hot_memory_bytes += num_points * (vector_bytes + metadata_est + hnsw_node_est);
        }
    }

    // Latência: query_log_cache
    let cache = &app_state.query_log_cache;
    let avg_ms = cache.avg_latency_24h();
    let entries = cache.entries_24h();
    let mut sorted_ms: Vec<u64> = entries.iter().map(|e| e.took_ms).collect();
    sorted_ms.sort();
    let len = sorted_ms.len();
    let p50_ms = if len == 0 {
        0.0
    } else {
        sorted_ms[len * 50 / 100] as f64
    };
    let p95_ms = if len == 0 {
        0.0
    } else {
        sorted_ms[(len * 95 / 100).min(len.saturating_sub(1))] as f64
    };
    let p99_ms = if len == 0 {
        0.0
    } else {
        sorted_ms[(len * 99 / 100).min(len.saturating_sub(1))] as f64
    };
    let latency_per_minute: Vec<LatencyPerMinuteBucket> = cache
        .latency_per_minute_24h()
        .into_iter()
        .map(|(timestamp, avg_ms, p50_ms)| LatencyPerMinuteBucket {
            timestamp,
            avg_ms,
            p50_ms,
        })
        .collect();

    // Tombstones agregados
    let mut total_tombstone_count = 0_usize;
    let mut total_tombstone_memory_waste_bytes = 0_usize;
    let mut total_points = 0_usize;
    for entry in app_state.collections.iter() {
        let collection = match entry.value().read() {
            Ok(c) => c,
            Err(_) => continue,
        };
        total_tombstone_count += collection.tombstone_count();
        total_tombstone_memory_waste_bytes += collection.tombstone_memory_waste();
        total_points += collection.len();
    }

    // Circuit breaker (0=closed, 1=open, 2=half_open, cf. ferres_db_core::storage)
    let cb = &app_state.storage_circuit_breaker;
    let state_u8 = cb.state();
    let state_str = match state_u8 {
        0 => "closed",
        1 => "open",
        2 => "half_open",
        _ => "unknown",
    };
    let failure_count = cb.failure_count();

    // Séries temporais (10 min): throughput e P95 latência (leitura fresca do log para analytics)
    let (avg_points_per_second, throughput_raw) = app_state.time_series_ingest_10m();
    let p95_latency_10m = cache.p95_latency_10m_fresh();
    let entries_10m = cache.entries_10m_fresh();
    let start = entries_10m.len().saturating_sub(100);
    let recent_latencies: Vec<RecentLatencyEntry> = entries_10m
        .get(start..)
        .unwrap_or_default()
        .iter()
        .map(|e| RecentLatencyEntry {
            timestamp: e.timestamp_secs,
            took_ms: e.took_ms,
        })
        .collect();
    let throughput_per_minute: Vec<ThroughputPerMinuteBucket> = throughput_raw
        .into_iter()
        .map(|(timestamp, points)| ThroughputPerMinuteBucket { timestamp, points })
        .collect();

    // Cache hit rate (search_cache agregado em todas as coleções)
    let mut total_hits = 0_u64;
    let mut total_misses = 0_u64;
    for entry in app_state.collections.iter() {
        if let Ok(coll) = entry.value().read() {
            let (h, m) = coll.search_cache_stats();
            total_hits += h;
            total_misses += m;
        }
    }
    let cache_hit_rate_pct = match total_hits + total_misses {
        0 => None,
        total => Some((total_hits as f64 / total as f64) * 100.0),
    };

    Ok(Json(AnalyticsResponse {
        tier_distribution: AnalyticsTierDistribution {
            hot,
            warm,
            cold,
            hot_memory_bytes,
            warm_memory_bytes,
            cold_memory_bytes,
        },
        latency: AnalyticsLatency {
            avg_ms,
            p50_ms,
            p95_ms,
            p99_ms,
            latency_per_minute,
        },
        tombstones: AnalyticsTombstones {
            total_count: total_tombstone_count,
            total_memory_waste_bytes: total_tombstone_memory_waste_bytes,
            total_points,
        },
        circuit_breaker: AnalyticsCircuitBreaker {
            state: state_str.to_string(),
            failure_count,
        },
        time_series_10m: TimeSeries10m {
            avg_points_per_second,
            p95_latency_ms: p95_latency_10m,
            throughput_per_minute,
            recent_latencies,
        },
        cache_hit_rate_pct,
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

