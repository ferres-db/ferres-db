//! # Metrics — métricas Prometheus para observabilidade
//!
//! Expõe métricas Prometheus para monitoramento do servidor.

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge, register_histogram_vec, CounterVec, Gauge, HistogramVec,
    TextEncoder,
};

lazy_static! {
    /// Contador de requisições HTTP por endpoint e método.
    pub static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "http_requests_total",
        "Total number of HTTP requests",
        &["method", "endpoint", "status"]
    ).unwrap();

    /// Histograma de latência de requisições HTTP em milissegundos.
    pub static ref HTTP_REQUEST_DURATION_MS: HistogramVec = register_histogram_vec!(
        "http_request_duration_ms",
        "HTTP request latency in milliseconds",
        &["method", "endpoint"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).unwrap();

    /// Gauge de coleções ativas.
    pub static ref COLLECTIONS_ACTIVE: Gauge = register_gauge!(
        "collections_active",
        "Number of active collections"
    ).unwrap();

    /// Contador de queries por coleção.
    pub static ref QUERIES_TOTAL: CounterVec = register_counter_vec!(
        "queries_total",
        "Total number of search queries",
        &["collection"]
    ).unwrap();

    /// Histograma de latência de queries em milissegundos.
    pub static ref QUERY_DURATION_MS: HistogramVec = register_histogram_vec!(
        "query_duration_ms",
        "Search query latency in milliseconds",
        &["collection"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).unwrap();

    // ─── WebSocket Metrics ──────────────────────────────────────────────

    /// Gauge de conexões WebSocket ativas.
    pub static ref WS_CONNECTIONS_ACTIVE: Gauge = register_gauge!(
        "ws_connections_active",
        "Number of active WebSocket connections"
    ).unwrap();

    /// Contador de mensagens WebSocket recebidas (por tipo).
    pub static ref WS_MESSAGES_RECEIVED_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_received_total",
        "Total WebSocket messages received",
        &["message_type"]
    ).unwrap();

    /// Contador de mensagens WebSocket enviadas (por tipo).
    pub static ref WS_MESSAGES_SENT_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_sent_total",
        "Total WebSocket messages sent",
        &["message_type"]
    ).unwrap();
}

/// Retorna as métricas em formato Prometheus.
pub fn gather_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    encoder.encode_to_string(&metric_families).unwrap()
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
