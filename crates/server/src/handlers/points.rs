//! # Points Handlers — handlers para gerenciamento de pontos

use std::time::Instant;

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use ferres_db_core::{MetadataFilter, Point};

use crate::api_err;
use crate::auth::RequireEditor;
use crate::error::{ApiError, ApiResult};
use crate::request_validation;
use crate::state::{AppState, QueryPhase, QueryProfile, QUERY_PROFILES_CAP};

// ─── Request/Response Types ──────────────────────────────────────────────

/// Payload para upsert de pontos.
/// Limites de batch e dimensão são aplicados em [crate::request_validation].
#[derive(Debug, Deserialize, Validate)]
pub struct UpsertPointsRequest {
    #[validate(length(min = 1, message = "points must contain at least 1 item"))]
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
    /// ID da query (para debug: GET /api/v1/debug/query-profile/{query_id}).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
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

/// Parâmetros de query para listagem de pontos.
#[derive(Debug, Deserialize)]
pub struct ListPointsParams {
    /// Número máximo de pontos a retornar (padrão: 100, máximo: 1000)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Número de pontos a pular (para paginação)
    #[serde(default = "default_offset")]
    pub offset: usize,
    /// Filtro de metadata em formato JSON string (ex: {"field": {"eq": "value"}})
    pub filter: Option<String>,
}

fn default_limit() -> usize {
    100
}

fn default_offset() -> usize {
    0
}

/// Resposta de listagem de pontos.
#[derive(Debug, Serialize)]
pub struct ListPointsResponse {
    pub points: Vec<GetPointResponse>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

// ─── Handlers ─────────────────────────────────────────────────────────────

/// Handler para POST /api/v1/collections/{name}/points
///
/// Insere ou atualiza pontos (Editor ou Admin).
pub async fn upsert_points(
    _editor: RequireEditor,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<UpsertPointsRequest>,
) -> ApiResult<Json<UpsertPointsResponse>> {
    // Validação centralizada (limites de batch e dimensão — previne DoS/OOM)
    request_validation::validate_upsert_request(&payload)?;

    // Valida o payload (schema: min 1 point, etc.)
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

    // Obtém a coleção uma vez e mantém um único write lock para validação + inserção + mark_dirty
    // (evita race: outra thread não pode alterar a coleção entre validação e inserção)
    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let (upserted, batch_failed) = {
        let mut collection = api_err!(collection_arc.write(), "failed to acquire write lock")?;

        // Fase 1: validação e construção dos pontos (dentro do mesmo lock)
        let mut points = Vec::new();
        let mut failed = Vec::new();

        for input in payload.points {
            // Valida dimensão do vetor (usa a coleção atual, não dados obsoletos)
            if let Err(e) = collection.validate_dimension(&input.vector) {
                failed.push(FailedPoint {
                    id: input.id.clone(),
                    reason: e.to_string(),
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

        if points.is_empty() {
            return Ok(Json(UpsertPointsResponse {
                upserted: 0,
                failed,
            }));
        }

        // Fase 2: inserção em batch otimizada e mark_dirty (mesmo lock)
        let upserted = match collection.insert_batch(points) {
            Ok(result) => result.inserted,
            Err(err) => {
                // Se o batch falhou, não podemos saber quais pontos falharam individualmente
                // então retornamos 0 inseridos e adicionamos um erro genérico
                failed.push(FailedPoint {
                    id: "batch".to_string(),
                    reason: err.to_string(),
                });
                0
            }
        };
        collection.mark_dirty();
        (upserted, failed)
    };

    Ok(Json(UpsertPointsResponse {
        upserted,
        failed: batch_failed,
    }))
}

/// Handler para DELETE /api/v1/collections/{name}/points
///
/// Remove pontos (Editor ou Admin).
pub async fn delete_points(
    _editor: RequireEditor,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<DeletePointsRequest>,
) -> ApiResult<Json<DeletePointsResponse>> {
    request_validation::validate_delete_batch_size(payload.ids.len())?;

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let deleted = {
        let mut collection = api_err!(collection_arc.write(), "failed to acquire write lock")?;
        collection.delete_points_batch(&payload.ids).map_err(ApiError::from)?
    };

    Ok(Json(DeletePointsResponse { deleted }))
}

/// Handler para POST /api/v1/collections/{name}/search
///
/// Busca por similaridade (Editor ou Admin; Query Tester).
#[tracing::instrument(
    name = "search_points",
    skip(app_state, payload),
    fields(collection = %name, limit = payload.limit)
)]
pub async fn search_points(
    _editor: RequireEditor,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<SearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    request_validation::validate_search_limit(payload.limit)?;
    request_validation::validate_vector_dimension(&payload.vector)?;

    let query_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    // Fase: validação (get collection + validate dimension)
    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let _span = tracing::info_span!("validate_query").entered();
    collection.validate_dimension(&payload.vector)
        .map_err(ApiError::from)?;
    drop(_span);

    let validation_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let search_start = Instant::now();

    // Fase: busca (HNSW)
    let _span = tracing::info_span!("hnsw_search").entered();
    let results = collection.search(&payload.vector, payload.limit)
        .map_err(ApiError::from)?;
    drop(_span);

    let search_ms = search_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let hydrate_start = Instant::now();

    // Fase: hydrate (construir SearchResults + filtro)
    let _span = tracing::info_span!("hydrate_results").entered();
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

    // Aplica filtro de metadata se fornecido (Eq, Ne, In, Gt, Lt, Gte, Lte)
    let mut filtered_results = search_results;
    if let Some(filter_value) = &payload.filter {
        let filter = match MetadataFilter::from_json(filter_value.clone()) {
            Ok(f) => f,
            Err(e) => return Err(ApiError::invalid_payload(e.to_string())),
        };
        if !filter.is_empty() {
            filtered_results = filtered_results
                .into_iter()
                .filter(|result| filter.matches(&result.metadata))
                .collect();
        }
    }

    let results: Vec<SearchResult> = filtered_results
        .into_iter()
        .map(|r| SearchResult {
            id: r.id,
            score: r.score,
            metadata: r.metadata,
        })
        .collect();
    drop(_span);

    let hydrate_ms = hydrate_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let results_count = results.len();
    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let collection_name = name.clone();
    let vector_preview = payload.vector.clone();
    let filter_clone = payload.filter.clone();

    // Monta perfil por fases (percentual sobre total)
    let total = took_ms as f64;
    let phases = vec![
        QueryPhase {
            name: "validation".to_string(),
            duration_ms: validation_ms,
            percentage: if total > 0.0 { (validation_ms as f64 / total) * 100.0 } else { 0.0 },
        },
        QueryPhase {
            name: "search".to_string(),
            duration_ms: search_ms,
            percentage: if total > 0.0 { (search_ms as f64 / total) * 100.0 } else { 0.0 },
        },
        QueryPhase {
            name: "hydrate".to_string(),
            duration_ms: hydrate_ms,
            percentage: if total > 0.0 { (hydrate_ms as f64 / total) * 100.0 } else { 0.0 },
        },
    ];
    let profile = QueryProfile {
        query_id: query_id.clone(),
        total_ms: took_ms,
        phases,
    };

    // Evict one profile if at capacity, then store
    if app_state.query_profiles.len() >= QUERY_PROFILES_CAP {
        if let Some(entry) = app_state.query_profiles.iter().next() {
            let k = entry.key().clone();
            drop(entry);
            app_state.query_profiles.remove(&k);
        }
    }
    app_state.query_profiles.insert(query_id.clone(), profile);

    drop(collection);
    drop(collection_arc);

    let query_logger = app_state.query_logger.clone();

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

    let query_logger_clone = query_logger.clone();
    let collection_name_for_log = collection_name.clone();
    let vector_preview_for_log = vector_preview.clone();
    let filter_for_log = filter_clone.clone();
    let query_id_for_log = query_id.clone();
    tokio::spawn(async move {
        query_logger_clone
            .log_query(
                Some(&query_id_for_log),
                &collection_name_for_log,
                &vector_preview_for_log,
                payload.limit,
                filter_for_log.as_ref(),
                results_count,
                took_ms,
            )
            .await;
    });

    Ok(Json(SearchPointsResponse {
        results,
        took_ms,
        query_id: Some(query_id),
    }))
}

/// Handler para POST /api/v1/collections/{name}/search/hybrid
///
/// Busca híbrida: combina resultados vetoriais e BM25 (keyword) via RRF.
/// Requer que a coleção tenha sido criada com BM25 habilitado.
/// Sub-operações instrumentadas: validate_query, hybrid_search, hydrate_results.
#[tracing::instrument(
    name = "search_hybrid",
    skip(app_state, payload),
    fields(collection = %name, limit = payload.limit, alpha = payload.alpha)
)]
pub async fn search_hybrid(
    _editor: RequireEditor,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<HybridSearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    request_validation::validate_search_limit(payload.limit)?;
    request_validation::validate_vector_dimension(&payload.query_vector)?;
    if !(0.0..=1.0).contains(&payload.alpha) {
        return Err(ApiError::invalid_payload("alpha must be between 0 and 1"));
    }

    let query_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let _span = tracing::info_span!("validate_query").entered();
    collection.validate_dimension(&payload.query_vector)
        .map_err(ApiError::from)?;
    drop(_span);

    let validation_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let search_start = Instant::now();

    let _span = tracing::info_span!("hybrid_search").entered();
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
    drop(_span);

    let search_ms = search_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let hydrate_start = Instant::now();

    let _span = tracing::info_span!("hydrate_results").entered();
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
    drop(_span);

    let hydrate_ms = hydrate_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let collection_name = name.clone();
    let results_count = results.len();

    let total = took_ms as f64;
    let phases = vec![
        QueryPhase {
            name: "validation".to_string(),
            duration_ms: validation_ms,
            percentage: if total > 0.0 { (validation_ms as f64 / total) * 100.0 } else { 0.0 },
        },
        QueryPhase {
            name: "search".to_string(),
            duration_ms: search_ms,
            percentage: if total > 0.0 { (search_ms as f64 / total) * 100.0 } else { 0.0 },
        },
        QueryPhase {
            name: "hydrate".to_string(),
            duration_ms: hydrate_ms,
            percentage: if total > 0.0 { (hydrate_ms as f64 / total) * 100.0 } else { 0.0 },
        },
    ];
    let profile = QueryProfile {
        query_id: query_id.clone(),
        total_ms: took_ms,
        phases,
    };

    if app_state.query_profiles.len() >= QUERY_PROFILES_CAP {
        if let Some(entry) = app_state.query_profiles.iter().next() {
            let k = entry.key().clone();
            drop(entry);
            app_state.query_profiles.remove(&k);
        }
    }
    app_state.query_profiles.insert(query_id.clone(), profile);

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
    let query_id_for_log = query_id.clone();
    tokio::spawn(async move {
        query_logger
            .log_query(
                Some(&query_id_for_log),
                &collection_name_for_log,
                &vector_for_log,
                payload.limit,
                None,
                results_count,
                took_ms,
            )
            .await;
    });

    Ok(Json(SearchPointsResponse {
        results,
        took_ms,
        query_id: Some(query_id),
    }))
}

/// Handler para GET /api/v1/collections/{name}/points
///
/// Retorna pontos de uma coleção com paginação e filtro por metadata.
/// Query params: limit=100, offset=0, filter={"field": {"eq": "value"}}
pub async fn list_points(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<ListPointsParams>,
) -> ApiResult<Json<ListPointsResponse>> {
    // Valida limit (máximo 1000)
    let limit = params.limit.min(1000);
    let offset = params.offset;

    let collection_arc = app_state.collections.get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?;

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    // Obtém todos os pontos
    let mut all_points: Vec<GetPointResponse> = collection.points_owned()
        .iter()
        .map(|point| GetPointResponse {
            id: point.id.clone(),
            vector: point.vector.clone(),
            metadata: point.metadata.clone(),
            created_at: point.created_at,
        })
        .collect();

    // Aplica filtro de metadata se fornecido
    if let Some(filter_str) = &params.filter {
        if !filter_str.is_empty() {
            // Parse do JSON string para serde_json::Value
            let filter_value: serde_json::Value = serde_json::from_str(filter_str)
                .map_err(|e| ApiError::invalid_payload(format!("invalid JSON filter: {}", e)))?;
            
            let filter = MetadataFilter::from_json(filter_value)
                .map_err(|e| ApiError::invalid_payload(format!("invalid metadata filter: {}", e)))?;

            all_points.retain(|point| {
                filter.matches(&point.metadata)
            });
        }
    }

    let total = all_points.len();
    let has_more = offset + limit < total;

    // Aplica paginação
    let points: Vec<GetPointResponse> = all_points
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();

    Ok(Json(ListPointsResponse {
        points,
        total,
        limit,
        offset,
        has_more,
    }))
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

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

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

