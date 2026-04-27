//! # Metrics — métricas Prometheus para observabilidade
//!
//! Expõe métricas Prometheus para monitoramento do servidor.

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge, register_gauge_vec, register_histogram_vec, CounterVec,
    Gauge, GaugeVec, HistogramVec, TextEncoder,
};

lazy_static! {
    /// Contador de requisições HTTP por endpoint e método.
    pub static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "http_requests_total",
        "Total number of HTTP requests",
        &["method", "endpoint", "status"]
    ).expect("prometheus metric 'http_requests_total' registration failed");

    /// Histograma de latência de requisições HTTP em milissegundos.
    pub static ref HTTP_REQUEST_DURATION_MS: HistogramVec = register_histogram_vec!(
        "http_request_duration_ms",
        "HTTP request latency in milliseconds",
        &["method", "endpoint"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).expect("prometheus metric 'http_request_duration_ms' registration failed");

    /// Gauge de coleções ativas.
    pub static ref COLLECTIONS_ACTIVE: Gauge = register_gauge!(
        "collections_active",
        "Number of active collections"
    ).expect("prometheus metric 'collections_active' registration failed");

    /// Contador de queries por coleção.
    pub static ref QUERIES_TOTAL: CounterVec = register_counter_vec!(
        "queries_total",
        "Total number of search queries",
        &["collection"]
    ).expect("prometheus metric 'queries_total' registration failed");

    /// Histograma de latência de queries em milissegundos.
    pub static ref QUERY_DURATION_MS: HistogramVec = register_histogram_vec!(
        "query_duration_ms",
        "Search query latency in milliseconds",
        &["collection"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).expect("prometheus metric 'query_duration_ms' registration failed");

    // ─── WebSocket Metrics ──────────────────────────────────────────────

    /// Gauge de conexões WebSocket ativas.
    pub static ref WS_CONNECTIONS_ACTIVE: Gauge = register_gauge!(
        "ws_connections_active",
        "Number of active WebSocket connections"
    ).expect("prometheus metric 'ws_connections_active' registration failed");

    /// Contador de mensagens WebSocket recebidas (por tipo).
    pub static ref WS_MESSAGES_RECEIVED_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_received_total",
        "Total WebSocket messages received",
        &["message_type"]
    ).expect("prometheus metric 'ws_messages_received_total' registration failed");

    /// Contador de mensagens WebSocket enviadas (por tipo).
    pub static ref WS_MESSAGES_SENT_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_sent_total",
        "Total WebSocket messages sent",
        &["message_type"]
    ).expect("prometheus metric 'ws_messages_sent_total' registration failed");

    // ─── LLM Proxy Metrics ─────────────────────────────────────────────

    /// Total de requisições enviadas ao proxy LLM, por provider e status.
    /// `status` valores: "ok", "error", "auth_error", "upstream_error", "timeout".
    pub static ref LLM_PROXY_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_llm_proxy_requests_total",
        "Total LLM proxy requests by provider and status",
        &["provider", "status"]
    ).expect("prometheus metric 'ferresdb_llm_proxy_requests_total' registration failed");

    // ─── WAL Durability Metrics ────────────────────────────────────────────

    /// Total de chamadas sync_data() no WAL, por coleção.
    pub static ref WAL_FSYNC_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_wal_fsync_total",
        "Total number of WAL fsync calls",
        &["collection"]
    ).expect("prometheus metric 'ferresdb_wal_fsync_total' registration failed");

    /// Latência das chamadas sync_data() no WAL, em segundos.
    pub static ref WAL_FSYNC_DURATION: HistogramVec = register_histogram_vec!(
        "ferresdb_wal_fsync_duration_seconds",
        "WAL fsync latency in seconds",
        &["collection"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    ).expect("prometheus metric 'ferresdb_wal_fsync_duration_seconds' registration failed");

    /// Número de operações WAL desde o último fsync.
    pub static ref WAL_PENDING_FSYNC_OPS: GaugeVec = register_gauge_vec!(
        "ferresdb_wal_pending_fsync_ops",
        "Number of WAL ops since last fsync",
        &["collection"]
    ).expect("prometheus metric 'ferresdb_wal_pending_fsync_ops' registration failed");
}

/// Retorna as métricas em formato Prometheus.
pub fn gather_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    encoder
        .encode_to_string(&metric_families)
        .expect("TextEncoder::encode_to_string failed writing to String")
}

/// Extrai o endpoint do path da URL.
///
/// Normaliza paths como `/api/v1/collections/my-collection/points`
/// para `/api/v1/collections/:name/points` para melhor agregação de métricas.
pub fn normalize_endpoint(path: &str) -> String {
    // Remove query parameters
    let path = path.split('?').next().unwrap_or(path);

    // Normaliza paths de coleções nomeadas
    if let Some(pos) = path.find("/collections/") {
        let after_collections = &path[pos + "/collections/".len()..];
        if let Some(next_slash) = after_collections.find('/') {
            let collection_name = &after_collections[..next_slash];
            // Substitui o nome da coleção por :name
            return path.replace(
                &format!("/collections/{collection_name}"),
                "/collections/:name",
            );
        }
    }

    path.to_string()
}
