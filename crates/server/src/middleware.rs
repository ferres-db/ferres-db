//! # Middleware — middlewares customizados
//!
//! Middlewares para logging estruturado, métricas, rate limiting, etc.
//!
//! Validação de tamanho de payload (batch, dimensão de vetor) é centralizada em
//! [crate::request_validation] e aplicada nos handlers de points antes do processamento.

use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use std::sync::Arc;
use std::time::Instant;
use governor::middleware::NoOpMiddleware;
use tower_governor::{
    governor::GovernorConfigBuilder,
    key_extractor::KeyExtractor,
    GovernorError, GovernorLayer,
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::metrics::{HTTP_REQUESTS_TOTAL, HTTP_REQUEST_DURATION_MS, normalize_endpoint};

/// Middleware de logging estruturado e raiz do distributed tracing para cada requisição.
///
/// Cria o span pai `http_request` para toda a requisição. Handlers (ex.: `search_points`)
/// criam spans filhos (`validate_query`, `hnsw_search`, `hydrate_results`), permitindo
/// correlacionar logs e medir latência por sub-operação. Com OpenTelemetry habilitado,
/// o mesmo contexto propaga trace_id/span_id para backends (Jaeger, etc.).
///
/// Loga em formato JSON estruturado com:
/// - request_id: UUID único para cada requisição (correlação entre logs)
/// - método HTTP, path, collection, operation
/// - status code da resposta, duration_ms
pub async fn request_logger(req: Request, next: Next) -> Response {
    // Gera request_id único para correlação entre logs e traces
    let request_id = Uuid::new_v4().to_string();
    
    // Extrai informações da requisição
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let normalized_endpoint = normalize_endpoint(&path);
    
    // Extrai collection e operation do path
    let (collection, operation) = extract_collection_and_operation(&path);
    
    // Span pai: toda a requisição e sub-operações ficam como filhos (distributed tracing)
    let span = tracing::span!(
        tracing::Level::INFO,
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
        collection = ?collection,
        operation = ?operation
    );
    let _guard = span.enter();

    // OTel: propaga W3C trace context (traceparent/tracestate) para o span atual
    #[cfg(feature = "otel")]
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let parent_cx = extract_otel_context(req.headers());
        span.set_parent(parent_cx);
    }

    let start = Instant::now();

    #[allow(unused_mut)]
    let mut response = next.run(req).await;
    let latency_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let status = response.status();
    let status_str = status.as_u16().to_string();

    // OTel: adiciona trace_id no header de resposta para correlação/debugging
    #[cfg(feature = "otel")]
    {
        if let Some(trace_id) = otel_trace_id(&span) {
            if let Ok(v) = trace_id.parse() {
                response.headers_mut().insert("x-trace-id", v);
            }
        }
    }

    // Registra métricas Prometheus
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[method.as_str(), &normalized_endpoint, &status_str])
        .inc();
    
    HTTP_REQUEST_DURATION_MS
        .with_label_values(&[method.as_str(), &normalized_endpoint])
        .observe(latency_ms as f64);

    // Log estruturado
    if status.is_server_error() {
        warn!(
            request_id = %request_id,
            method = %method,
            path = %path,
            collection = ?collection,
            operation = ?operation,
            status = status.as_u16(),
            duration_ms = latency_ms,
            timestamp = %Utc::now().to_rfc3339(),
            "HTTP request"
        );
    } else {
        info!(
            request_id = %request_id,
            method = %method,
            path = %path,
            collection = ?collection,
            operation = ?operation,
            status = status.as_u16(),
            duration_ms = latency_ms,
            timestamp = %Utc::now().to_rfc3339(),
            "HTTP request"
        );
    }

    response
}

/// Extrai o nome da coleção e a operação do path da URL.
fn extract_collection_and_operation(path: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = path.split('/').collect();
    
    // Procura por "collections" no path
    if let Some(pos) = parts.iter().position(|&p| p == "collections") {
        if pos + 1 < parts.len() {
            let collection_name = parts[pos + 1];
            
            // Determina a operação baseado no path
            let operation = if path.contains("/search") {
                Some("search".to_string())
            } else if path.contains("/points") && parts.len() > pos + 3 {
                if parts[pos + 3] == "points" {
                    Some("get_point".to_string())
                } else {
                    Some("points".to_string())
                }
            } else if path.ends_with(&format!("/collections/{collection_name}")) {
                Some("get_collection".to_string())
            } else {
                None
            };
            
            return (Some(collection_name.to_string()), operation);
        }
    }
    
    // Tenta identificar operações sem coleção
    let operation = if path == "/api/v1/collections" {
        Some("list_collections".to_string())
    } else if path == "/health" {
        Some("health_check".to_string())
    } else if path == "/metrics" {
        Some("metrics".to_string())
    } else {
        None
    };
    
    (None, operation)
}

// ─── OTel Trace Propagation ──────────────────────────────────────────────

/// Extrai o contexto W3C Trace Context dos headers da requisição (traceparent, tracestate).
#[cfg(feature = "otel")]
fn extract_otel_context(headers: &axum::http::HeaderMap) -> opentelemetry::Context {
    use opentelemetry::propagation::Extractor as OtelExtractor;

    struct HeaderExtractor<'a>(&'a axum::http::HeaderMap);

    impl<'a> OtelExtractor for HeaderExtractor<'a> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(key).and_then(|v| v.to_str().ok())
        }
        fn keys(&self) -> Vec<&str> {
            self.0.keys().map(|k| k.as_str()).collect()
        }
    }

    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(headers))
    })
}

/// Obtém o trace_id do span OTel atual para incluir na resposta HTTP.
#[cfg(feature = "otel")]
fn otel_trace_id(span: &tracing::Span) -> Option<String> {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let otel_ctx = span.context();
    let otel_span = otel_ctx.span();
    let sc = otel_span.span_context();
    if sc.is_valid() {
        Some(sc.trace_id().to_string())
    } else {
        None
    }
}

// ─── Rate Limiting por Coleção ────────────────────────────────────────────

/// Extrator de chave baseado no nome da coleção da rota.
///
/// Extrai o nome da coleção do path da URL (formato: /api/v1/collections/{name}/...).
/// Usado pelo rate limiter para aplicar limites independentes por coleção.
#[derive(Clone, Debug)]
pub struct CollectionKeyExtractor;

impl KeyExtractor for CollectionKeyExtractor {
    type Key = String;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        // Extrai o nome da coleção da URL
        // Formato esperado: /api/v1/collections/{name}/...
        // Nunca retorna Err para evitar 500 (ex.: path "/" ou "/api/v1/collections/").
        let path = req.uri().path();
        let parts: Vec<&str> = path.split('/').collect();

        if let Some(pos) = parts.iter().position(|&p| p == "collections") {
            if pos + 1 < parts.len() {
                let collection_name = parts[pos + 1];
                return Ok(if collection_name.is_empty() {
                    "__no_collection__".to_string()
                } else {
                    collection_name.to_string()
                });
            }
        }

        Ok("__no_collection__".to_string())
    }
}

/// Cria o layer de rate limiting por coleção.
/// Baseline: 100 req/s por coleção; burst até 200 req/s.
pub fn create_collection_rate_limit_layer() -> GovernorLayer<CollectionKeyExtractor, NoOpMiddleware> {
    let mut builder = GovernorConfigBuilder::default();
    builder.per_second(100);
    builder.burst_size(200);
    let config = builder.key_extractor(CollectionKeyExtractor).finish().unwrap();

    GovernorLayer { config: Arc::new(config) }
}
