//! # Middleware — middlewares customizados
//!
//! Middlewares para logging estruturado, métricas, rate limiting, etc.
//!
//! Validação de tamanho de payload (batch, dimensão de vetor) é centralizada em
//! [crate::request_validation] e aplicada nos handlers de points antes do processamento.

use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use governor::middleware::NoOpMiddleware;
use std::sync::Arc;
use std::time::Instant;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::KeyExtractor, GovernorError, GovernorLayer,
};
use tracing::{info, warn, Instrument};
use uuid::Uuid;

use crate::error::ApiError;
use crate::metrics::{normalize_endpoint, HTTP_REQUESTS_TOTAL, HTTP_REQUEST_DURATION_MS};
use crate::state::AppState;

/// Bloqueia operações de escrita (POST/PUT/DELETE) quando o servidor está em modo réplica.
/// Retorna 405 Method Not Allowed para rotas de escrita; permite GET e POST em search/auth.
pub async fn replica_write_guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if state.config.replica_of.is_none() {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let path = req.uri().path();
    if method == Method::GET {
        return next.run(req).await;
    }
    if method == Method::POST
        && (path.ends_with("/search")
            || path.ends_with("/search/hybrid")
            || path.ends_with("/search/explain")
            || path.ends_with("/search/estimate")
            || path == "/api/v1/auth/login")
    {
        return next.run(req).await;
    }
    if method == Method::POST || method == Method::PUT || method == Method::DELETE {
        return ApiError::method_not_allowed("Write operations are not allowed on a read replica")
            .into_response();
    }
    next.run(req).await
}

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

    // Span pai: toda a requisição e sub-operações ficam como filhos (distributed tracing).
    //
    // IMPORTANT: We use `.instrument(span)` instead of `span.enter()` because
    // `enter()` is thread-local and MUST NOT be held across `.await` points.
    // In async code, the tokio runtime can poll different tasks on the same
    // thread.  If task A holds `span.enter()` and yields, then task B runs on
    // the same thread and creates its own span, B's span becomes a *child* of
    // A's span — producing the ever-growing nested span chains visible in logs:
    //   http_request{req1}:http_request{req2}:http_request{req3}:...
    // `.instrument()` correctly attaches the span only while the future is
    // being polled, preventing cross-task contamination.
    let span = tracing::span!(
        tracing::Level::INFO,
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
        collection = ?collection,
        operation = ?operation
    );

    // OTel: propaga W3C trace context (traceparent/tracestate) para o span atual
    #[cfg(feature = "otel")]
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let parent_cx = extract_otel_context(req.headers());
        span.set_parent(parent_cx);
    }

    let start = Instant::now();

    // Run the inner handler instrumented with the span (NOT span.enter()).
    #[allow(unused_mut)]
    let mut response = next.run(req).instrument(span.clone()).await;
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
/// Valores vêm da configuração do servidor (config.toml ou env).
pub fn create_collection_rate_limit_layer(
    per_second: u32,
    burst_size: u32,
) -> GovernorLayer<CollectionKeyExtractor, NoOpMiddleware> {
    let mut builder = GovernorConfigBuilder::default();
    builder.per_second(per_second as u64);
    builder.burst_size(burst_size);
    let config = builder
        .key_extractor(CollectionKeyExtractor)
        .finish()
        .unwrap();

    GovernorLayer {
        config: Arc::new(config),
    }
}
