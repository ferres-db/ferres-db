//! # Middleware — middlewares customizados
//!
//! Middlewares para logging estruturado, métricas, rate limiting, etc.

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

/// Middleware de logging estruturado para todas as requisições HTTP.
///
/// Loga em formato JSON estruturado com:
/// - request_id: UUID único para cada requisição
/// - método HTTP
/// - path da requisição
/// - collection: nome da coleção (se aplicável)
/// - operation: tipo de operação
/// - status code da resposta
/// - duration_ms: latência em milissegundos
pub async fn request_logger(req: Request, next: Next) -> Response {
    // Gera request_id único
    let request_id = Uuid::new_v4().to_string();
    
    // Extrai informações da requisição
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let normalized_endpoint = normalize_endpoint(&path);
    
    // Extrai collection e operation do path
    let (collection, operation) = extract_collection_and_operation(&path);
    
    // Adiciona request_id ao contexto do tracing
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
    
    let start = Instant::now();

    // Adiciona request_id como header na resposta (opcional, para debugging)
    let response = next.run(req).await;
    let latency_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let status = response.status();
    let status_str = status.as_u16().to_string();

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
            } else if path.ends_with(&format!("/collections/{}", collection_name)) {
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
