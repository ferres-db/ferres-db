//! # Points Handlers — handlers para gerenciamento de pontos

use std::time::Instant;

use axum::{
    extract::{Path, State},
    response::Json,
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use ferres_db_core::Point;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Request/Response Types ──────────────────────────────────────────────

/// Payload para upsert de pontos.
#[derive(Debug, Deserialize, Validate)]
pub struct UpsertPointsRequest {
    #[validate(length(min = 1, max = 1000, message = "points must contain between 1 and 1000 items"))]
    pub points: Vec<PointInput>,
}

/// Input de um ponto para upsert.
#[derive(Debug, Deserialize, Serialize)]
pub struct PointInput {
    pub id: String,
    pub vector: Vec<f32>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Resposta de upsert de pontos.
#[derive(Debug, Serialize)]
pub struct UpsertPointsResponse {
    pub upserted: usize,
    pub failed: Vec<FailedPoint>,
}

/// Informação sobre um ponto que falhou no upsert.
#[derive(Debug, Serialize)]
pub struct FailedPoint {
    pub id: String,
    pub reason: String,
}

/// Payload para deleção de pontos.
#[derive(Debug, Deserialize)]
pub struct DeletePointsRequest {
    pub ids: Vec<String>,
}

/// Resposta de deleção de pontos.
#[derive(Debug, Serialize)]
pub struct DeletePointsResponse {
    pub deleted: usize,
}

/// Payload para busca de pontos.
#[derive(Debug, Deserialize)]
pub struct SearchPointsRequest {
    pub vector: Vec<f32>,
    pub limit: usize,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
}

/// Payload para busca híbrida (vetorial + keyword).
#[derive(Debug, Deserialize)]
pub struct HybridSearchPointsRequest {
    pub query_text: String,
    pub query_vector: Vec<f32>,
    pub limit: usize,
    /// Peso da busca vetorial (0..=1). (1 - alpha) é o peso da busca keyword. Padrão: 0.5.
    #[serde(default = "default_alpha")]
    pub alpha: f32,
}

fn default_alpha() -> f32 {
    0.5
}

/// Resposta de busca de pontos.
#[derive(Debug, Serialize)]
pub struct SearchPointsResponse {
    pub results: Vec<SearchResult>,
    pub took_ms: u64,
}

/// Resultado de uma busca.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}

/// Resposta de obtenção de um ponto.
#[derive(Debug, Serialize)]
pub struct GetPointResponse {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub created_at: u64,
}

// ─── Handlers ─────────────────────────────────────────────────────────────

/// Handler para POST /api/v1/collections/{name}/points
///
/// Insere ou atualiza pontos em uma coleção (batch de até 1000 pontos).
pub async fn upsert_points(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<UpsertPointsRequest>,
) -> ApiResult<Json<UpsertPointsResponse>> {
    // Valida o payload
    payload.validate().map_err(|e| {
        let mut messages = Vec::new();
        for (field, errors) in e.field_errors() {
            for error in errors {
                let msg = error
                    .message
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("invalid {}", field));
                messages.push(msg);
            }
        }
        ApiError::invalid_payload(messages.join(", "))
    })?;

    // Obtém a dimensão esperada da coleção
    let expected_dimension = {
        let collection_arc = app_state.collections.get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        
        let collection = collection_arc.read().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire read lock: {}", e))
        })?;
        
        collection.config().dimension
    };

    // Valida dimensão de cada vetor e cria os pontos
    let mut points = Vec::new();
    let mut failed = Vec::new();

    for input in payload.points {
        // Valida dimensão do vetor
        if input.vector.len() != expected_dimension {
            failed.push(FailedPoint {
                id: input.id.clone(),
                reason: format!(
                    "dimension mismatch: expected {}, got {}",
                    expected_dimension,
                    input.vector.len()
                ),
            });
            continue;
        }

        // Valida que o vetor não está vazio
        if input.vector.is_empty() {
            failed.push(FailedPoint {
                id: input.id.clone(),
                reason: "vector cannot be empty".to_string(),
            });
            continue;
        }

        // Valida que não há valores NaN ou infinito
        if let Some(pos) = input.vector.iter().position(|v| !v.is_finite()) {
            failed.push(FailedPoint {
                id: input.id.clone(),
                reason: format!("non-finite value at index {}", pos),
            });
            continue;
        }

        // Cria o ponto
        match Point::new(input.id.clone(), input.vector, input.metadata) {
            Ok(point) => points.push(point),
            Err(e) => {
                failed.push(FailedPoint {
                    id: input.id,
                    reason: e.to_string(),
                });
            }
        }
    }

    // Se não há pontos válidos, retorna apenas os falhos
    if points.is_empty() {
        return Ok(Json(UpsertPointsResponse {
            upserted: 0,
            failed,
        }));
    }

    // Obtém a coleção
    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    // Insere os pontos válidos
    let mut upserted = 0;
    let mut batch_failed = failed;

    {
        let mut collection = collection_arc.write().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire write lock: {}", e))
        })?;

        for point in points {
            let point_id = point.id.clone();
            match collection.insert(point) {
                Ok(_) => upserted += 1,
                Err(err) => {
                    batch_failed.push(FailedPoint {
                        id: point_id,
                        reason: err.to_string(),
                    });
                }
            }
        }
    }

    // Marca como dirty após modificação
    {
        let collection = collection_arc.read().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire read lock: {}", e))
        })?;
        collection.mark_dirty();
    }

    Ok(Json(UpsertPointsResponse {
        upserted,
        failed: batch_failed,
    }))
}

/// Handler para DELETE /api/v1/collections/{name}/points
///
/// Remove pontos de uma coleção pelos IDs.
pub async fn delete_points(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<DeletePointsRequest>,
) -> ApiResult<Json<DeletePointsResponse>> {
    if payload.ids.is_empty() {
        return Err(ApiError::invalid_payload("ids cannot be empty"));
    }

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    // Conta pontos antes da deleção
    let _count_before = {
        let collection = collection_arc.read().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire read lock: {}", e))
        })?;
        collection.len()
    };

    // Remove os pontos
    let mut deleted = 0;
    {
        let mut collection = collection_arc.write().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire write lock: {}", e))
        })?;

        for id in &payload.ids {
            if collection.remove(id).is_ok() {
                deleted += 1;
            }
        }
    }

    // Marca como dirty após modificação
    {
        let collection = collection_arc.read().map_err(|e| {
            ApiError::internal_error(format!("failed to acquire read lock: {}", e))
        })?;
        collection.mark_dirty();
    }

    Ok(Json(DeletePointsResponse { deleted }))
}

/// Handler para POST /api/v1/collections/{name}/search
///
/// Busca pontos similares a um vetor de consulta.
pub async fn search_points(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<SearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    if payload.limit == 0 {
        return Err(ApiError::invalid_payload("limit must be greater than 0"));
    }

    let start = Instant::now();

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = collection_arc.read().map_err(|e| {
        ApiError::internal_error(format!("failed to acquire read lock: {}", e))
    })?;

    // Valida dimensão do query
    collection.validate_dimension(&payload.vector)
        .map_err(|e| ApiError::from(e))?;

    // Realiza a busca
    let results = collection.search(&payload.vector, payload.limit)
        .map_err(|e| ApiError::from(e))?;

    // Constrói SearchResults
    let search_results: Vec<ferres_db_core::SearchResult> = results
        .into_iter()
        .filter_map(|(id, score)| {
            let point = collection.get(&id)?;
            Some(ferres_db_core::SearchResult {
                id,
                score,
                metadata: point.metadata.clone(),
                vector: None,
            })
        })
        .collect();

    // Aplica filtro de metadata se fornecido (v1: apenas equality)
    let mut filtered_results = search_results;
    if let Some(filter) = &payload.filter {
        if let Some(filter_obj) = filter.as_object() {
            filtered_results = filtered_results
                .into_iter()
                .filter(|result| {
                    // Verifica se todos os campos do filtro correspondem (equality)
                    filter_obj.iter().all(|(key, value)| {
                        result.metadata.get(key) == Some(value)
                    })
                })
                .collect();
        }
    }

    // Converte para formato de resposta
    let results: Vec<SearchResult> = filtered_results
        .into_iter()
        .map(|r| SearchResult {
            id: r.id,
            score: r.score,
            metadata: r.metadata,
        })
        .collect();

    let results_count = results.len();
    let took_ms = start.elapsed().as_millis() as u64;
    let collection_name = name.clone();
    let vector_preview = payload.vector.clone();
    let filter_clone = payload.filter.clone();

    // Libera o lock da coleção antes de operações assíncronas
    drop(collection);
    drop(collection_arc);

    // Prepara tudo que precisamos antes do await
    let query_logger = app_state.query_logger.clone();
    
    // Registra estatísticas da query usando entry() para evitar guards
    app_state.query_stats
        .entry(collection_name.clone())
        .or_insert_with(|| crate::state::QueryStats::new())
        .record_query(took_ms);

    app_state.global_query_stats.record(&collection_name, took_ms);
    
    // Registra métricas Prometheus (não precisa de guard)
    crate::metrics::QUERIES_TOTAL
        .with_label_values(&[&collection_name])
        .inc();
    
    crate::metrics::QUERY_DURATION_MS
        .with_label_values(&[&collection_name])
        .observe(took_ms as f64);

    // Loga a query em queries.log em background (fire-and-forget)
    // Isso evita problemas de Send com o handler
    let query_logger_clone = query_logger.clone();
    let collection_name_for_log = collection_name.clone();
    let vector_preview_for_log = vector_preview.clone();
    let filter_for_log = filter_clone.clone();
    tokio::spawn(async move {
        query_logger_clone.log_query(
            &collection_name_for_log,
            &vector_preview_for_log,
            payload.limit,
            filter_for_log.as_ref(),
            results_count,
            took_ms,
        ).await;
    });

    Ok(Json(SearchPointsResponse { results, took_ms }))
}

/// Handler para POST /api/v1/collections/{name}/search/hybrid
///
/// Busca híbrida: combina resultados vetoriais e BM25 (keyword) via RRF.
/// Requer que a coleção tenha sido criada com BM25 habilitado.
pub async fn search_hybrid(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<HybridSearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    if payload.limit == 0 {
        return Err(ApiError::invalid_payload("limit must be greater than 0"));
    }
    if !(0.0..=1.0).contains(&payload.alpha) {
        return Err(ApiError::invalid_payload("alpha must be between 0 and 1"));
    }

    let start = Instant::now();

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = collection_arc.read().map_err(|e| {
        ApiError::internal_error(format!("failed to acquire read lock: {}", e))
    })?;

    collection.validate_dimension(&payload.query_vector)
        .map_err(ApiError::from)?;

    let hybrid_results = collection.hybrid_search(
        &payload.query_vector,
        &payload.query_text,
        payload.limit,
        payload.alpha,
    ).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("BM25") || msg.contains("hybrid search") {
            ApiError::invalid_payload(msg)
        } else {
            ApiError::from(e)
        }
    })?;

    let results: Vec<SearchResult> = hybrid_results
        .into_iter()
        .filter_map(|(id, score)| {
            let point = collection.get(&id)?;
            Some(SearchResult {
                id,
                score,
                metadata: point.metadata.clone(),
            })
        })
        .collect();

    let took_ms = start.elapsed().as_millis() as u64;
    let collection_name = name.clone();

    drop(collection);
    drop(collection_arc);

    app_state.query_stats
        .entry(collection_name.clone())
        .or_insert_with(|| crate::state::QueryStats::new())
        .record_query(took_ms);
    app_state.global_query_stats.record(&collection_name, took_ms);
    crate::metrics::QUERIES_TOTAL
        .with_label_values(&[&collection_name])
        .inc();
    crate::metrics::QUERY_DURATION_MS
        .with_label_values(&[&collection_name])
        .observe(took_ms as f64);

    let query_logger = app_state.query_logger.clone();
    let collection_name_for_log = collection_name.clone();
    let vector_for_log = payload.query_vector.clone();
    let limit_for_log = payload.limit;
    let results_count = results.len();
    tokio::spawn(async move {
        query_logger
            .log_query(
                &collection_name_for_log,
                &vector_for_log,
                limit_for_log,
                None,
                results_count,
                took_ms,
            )
            .await;
    });

    Ok(Json(SearchPointsResponse { results, took_ms }))
}

/// Handler para GET /api/v1/collections/{name}/points/{id}
///
/// Retorna um ponto específico pelo ID.
pub async fn get_point(
    State(app_state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
) -> ApiResult<Json<GetPointResponse>> {
    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = collection_arc.read().map_err(|e| {
        ApiError::internal_error(format!("failed to acquire read lock: {}", e))
    })?;

    // Busca o ponto
    let point = collection.get(&id)
        .ok_or_else(|| ApiError::point_not_found(&id))?;

    Ok(Json(GetPointResponse {
        id: point.id.clone(),
        vector: point.vector.clone(),
        metadata: point.metadata.clone(),
        created_at: point.created_at,
    }))
}

