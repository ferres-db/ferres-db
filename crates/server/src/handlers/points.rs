//! # Points Handlers — handlers para gerenciamento de pontos

use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use ferres_db_core::{build_search_explanation, MetadataFilter, Point, QueryCostEstimate};

use crate::api_err;
use crate::audit::{self, AuditResult};
use crate::auth::{check_namespace_access, check_user_permission, AuthenticatedUser};
use crate::error::{ApiError, ApiResult};
use crate::permissions::{merge_restriction_filter, Action, PermissionResult};
use crate::request_validation;
use crate::state::{AppState, QueryPhase, QueryProfile, QUERY_PROFILES_CAP};
use crate::time::unix_now;

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
    /// Namespace lógico (multitenancy). Quando presente, o ponto fica isolado nesse namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// TTL em segundos; se presente, o ponto expira após esse tempo (removido pelo worker de vacuum).
    #[serde(default)]
    pub ttl: Option<u64>,
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
    /// Quando presente, remove apenas pontos deste namespace.
    #[serde(default)]
    pub namespace: Option<String>,
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
    /// Restringe resultados a este namespace (multitenancy).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Campo vetorial contra o qual buscar. Omitido ou "default" = vetor principal;
    /// outro nome (ex.: "title_vector", "content_vector") = índice nomeado.
    #[serde(default)]
    pub vector_field: Option<String>,
    /// Orçamento máximo em ms. Se a estimativa de custo exceder, retorna erro 422
    /// com a estimativa detalhada no body (sem executar a busca).
    #[serde(default)]
    pub budget_ms: Option<u64>,
    /// Quando true, re-pontua candidatos com Cross-Encoder (ONNX) e retorna top limit reordenados.
    /// Requer que o servidor tenha sido iniciado com modelo de re-ranking configurado.
    #[serde(default)]
    pub rerank: Option<bool>,
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
    /// Estratégia de fusão: "weighted" (default) ou "rrf".
    #[serde(default)]
    pub fusion: Option<String>,
    /// Constante k para RRF (default: 60). Apenas usado quando fusion = "rrf".
    #[serde(default)]
    pub rrf_k: Option<usize>,
    /// Restringe resultados a este namespace (multitenancy).
    #[serde(default)]
    pub namespace: Option<String>,
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
    /// Tempo gasto em re-ranking (ms). Presente apenas quando rerank=true e o servidor aplicou re-ranking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_ms: Option<u64>,
}

/// Resultado de uma busca.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// Resposta de obtenção de um ponto.
#[derive(Debug, Serialize)]
pub struct GetPointResponse {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub created_at: u64,
    /// IDs dos pontos relacionados (grafo). Presente quando o ponto tem relações.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relations: Option<Vec<String>>,
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
/// Insere ou atualiza pontos (Editor ou Admin, com verificação granular de permissão Write).
pub async fn upsert_points(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<UpsertPointsRequest>,
) -> ApiResult<Json<UpsertPointsResponse>> {
    let start = Instant::now();

    // Verificação de permissão granular (Write na collection)
    let perm_result = check_user_permission(&user, &name, &Action::Write);
    if !perm_result.is_allowed() {
        // Audit: ação negada
        let entry = audit::audit_entry(
            &user.username,
            "upsert",
            &format!("collection:{name}"),
            serde_json::json!({"points_count": payload.points.len(), "denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: write on collection '{name}'"
        )));
    }

    // Namespace allowance (RBAC multitenancy): cada namespace no batch deve ser permitido pela chave
    for point in &payload.points {
        if let Some(ref ns) = point.namespace {
            check_namespace_access(&user, Some(ns.as_str())).map_err(|_| {
                ApiError::forbidden("API key does not have access to this namespace")
            })?;
        }
    }

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
                    .unwrap_or_else(|| format!("invalid {field}"));
                messages.push(msg);
            }
        }
        ApiError::invalid_payload(messages.join(", "))
    })?;

    let points_count = payload.points.len();

    // Captura IDs para broadcast de eventos WebSocket
    let payload_point_ids: Vec<String> = payload.points.iter().map(|p| p.id.clone()).collect();

    // Clone the Arc and drop the DashMap Ref immediately to release the shard lock.
    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let (upserted, batch_failed) = {
        let mut collection = api_err!(collection_arc.write(), "failed to acquire write lock")?;

        let mut points = Vec::new();
        let mut failed = Vec::new();

        for input in payload.points {
            if let Err(e) = collection.validate_dimension(&input.vector) {
                failed.push(FailedPoint {
                    id: input.id.clone(),
                    reason: e.to_string(),
                });
                continue;
            }

            if input.vector.is_empty() {
                failed.push(FailedPoint {
                    id: input.id.clone(),
                    reason: "vector cannot be empty".to_string(),
                });
                continue;
            }

            if let Some(pos) = input.vector.iter().position(|v| !v.is_finite()) {
                failed.push(FailedPoint {
                    id: input.id.clone(),
                    reason: format!("non-finite value at index {pos}"),
                });
                continue;
            }

            match Point::new(input.id.clone(), input.vector, input.metadata) {
                Ok(mut point) => {
                    point.namespace = input.namespace;
                    if let Some(ttl) = input.ttl {
                        let now_secs = unix_now();
                        point.expires_at = Some(now_secs.saturating_add(ttl));
                    }
                    points.push(point);
                }
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

        let upserted = match collection.insert_batch(points) {
            Ok(result) => result.inserted,
            Err(err) => {
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

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

    // Emite evento no broadcast channel para subscribers WebSocket
    if upserted > 0 {
        let now_secs = unix_now();
        app_state.record_ingest(now_secs, upserted as u64);
        // Coleta IDs dos pontos inseridos com sucesso
        // (todos os que não estão em batch_failed)
        let failed_ids: std::collections::HashSet<&str> =
            batch_failed.iter().map(|f| f.id.as_str()).collect();
        let point_ids: Vec<String> = payload_point_ids
            .into_iter()
            .filter(|id| !failed_ids.contains(id.as_str()))
            .collect();
        let event = crate::state::CollectionEvent {
            collection: name.clone(),
            action: "upsert".to_string(),
            point_ids,
            timestamp: now_secs,
        };
        app_state.emit_event(event);
    }

    // Audit trail
    {
        let failed_count = batch_failed.len();
        let entry = audit::audit_entry(
            &user.username,
            "upsert",
            &format!("collection:{name}"),
            serde_json::json!({"points_submitted": points_count, "upserted": upserted, "failed": failed_count}),
            AuditResult::Success,
            None,
            Some(took_ms),
        );
        app_state.audit_logger.log(&entry);
    }

    Ok(Json(UpsertPointsResponse {
        upserted,
        failed: batch_failed,
    }))
}

/// Handler para DELETE /api/v1/collections/{name}/points
///
/// Remove pontos (Editor ou Admin, com verificação granular de permissão Write).
pub async fn delete_points(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<DeletePointsRequest>,
) -> ApiResult<Json<DeletePointsResponse>> {
    let start = Instant::now();

    // Verificação de permissão granular (Write na collection)
    let perm_result = check_user_permission(&user, &name, &Action::Write);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "delete_points",
            &format!("collection:{name}"),
            serde_json::json!({"ids_count": payload.ids.len(), "denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: write on collection '{name}'"
        )));
    }

    check_namespace_access(&user, payload.namespace.as_deref())
        .map_err(|_| ApiError::forbidden("API key does not have access to this namespace"))?;

    request_validation::validate_delete_batch_size(payload.ids.len())?;

    let ids_count = payload.ids.len();
    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let keys: Vec<String> = payload
        .ids
        .iter()
        .map(|id| Point::storage_id_from_parts(payload.namespace.as_deref(), id))
        .collect();
    let deleted_ids = payload.ids.clone();
    let deleted = {
        let mut collection = api_err!(collection_arc.write(), "failed to acquire write lock")?;
        collection
            .delete_points_batch(&keys)
            .map_err(ApiError::from)?
    };

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

    // Emite evento no broadcast channel para subscribers WebSocket
    if deleted > 0 {
        let event = crate::state::CollectionEvent {
            collection: name.clone(),
            action: "delete".to_string(),
            point_ids: deleted_ids,
            timestamp: unix_now(),
        };
        app_state.emit_event(event);

        // Auto-reindex when tombstones exceed 20% threshold
        crate::handlers::reindex::maybe_auto_reindex(&app_state, &name);
    }

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "delete_points",
            &format!("collection:{name}"),
            serde_json::json!({"ids_submitted": ids_count, "deleted": deleted}),
            AuditResult::Success,
            None,
            Some(took_ms),
        );
        app_state.audit_logger.log(&entry);
    }

    Ok(Json(DeletePointsResponse { deleted }))
}

/// Handler para POST /api/v1/collections/{name}/search
///
/// Busca por similaridade (Editor ou Admin; Query Tester).
#[tracing::instrument(
    name = "search_points",
    skip(app_state, payload),
    fields(
        db.collection = %name,
        db.operation = "vector_search",
        db.vector.dimension = tracing::field::Empty,
        db.vector.limit = payload.limit,
        db.results.count = tracing::field::Empty,
        db.duration.search_ms = tracing::field::Empty,
        db.duration.hydrate_ms = tracing::field::Empty,
        db.index.r#type = "hnsw",
        db.index.ef_search = tracing::field::Empty,
    )
)]
pub async fn search_points(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(mut payload): Json<SearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    // Verificação de permissão granular (Read na collection)
    let perm_result = check_user_permission(&user, &name, &Action::Read);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "search",
            &format!("collection:{name}"),
            serde_json::json!({"denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: read on collection '{name}'"
        )));
    }

    check_namespace_access(&user, payload.namespace.as_deref())
        .map_err(|_| ApiError::forbidden("API key does not have access to this namespace"))?;

    // Se a permissão vem com MetadataRestriction, injeta automaticamente no filtro
    if let PermissionResult::AllowedWithRestriction(ref restriction) = perm_result {
        let merged = merge_restriction_filter(payload.filter.as_ref(), restriction);
        payload.filter = Some(merged);
    }

    request_validation::validate_search_limit(payload.limit)?;
    request_validation::validate_vector_dimension(&payload.vector)?;

    let query_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    // Fase: validação. Keep the read-lock scope as short as possible — only
    // extract the data we need, then release immediately. Cost estimation,
    // filter parsing, and reranker checks happen OUTSIDE the lock.
    let (arc_for_blocking, coll_config, coll_len, reranker_dim_match) = {
        let collection_arc = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;

        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

        let _span = tracing::info_span!("validate_query").entered();
        collection
            .validate_dimension(&payload.vector)
            .map_err(ApiError::from)?;
        drop(_span);

        let cfg = collection.config().clone();
        let len = collection.len();
        let reranker_dim_ok = app_state
            .reranker
            .as_ref()
            .map_or(false, |r| r.dimension() == cfg.dimension);

        {
            let span = tracing::Span::current();
            span.record("db.vector.dimension", cfg.dimension);
            span.record("db.index.ef_search", cfg.hnsw.ef_search);
        }

        (
            Arc::clone(collection_arc.value()),
            cfg,
            len,
            reranker_dim_ok,
        )
    };

    // Cost estimation runs outside the collection lock
    if let Some(budget) = payload.budget_ms {
        let (has_filter, filter_conditions_count) = if let Some(filter_value) = &payload.filter {
            match MetadataFilter::from_json(filter_value.clone()) {
                Ok(f) => (!f.is_empty(), f.conditions().len()),
                Err(_) => (false, 0),
            }
        } else {
            (false, 0)
        };

        let (p50, p95) = {
            app_state
                .query_stats
                .get(&name)
                .map(|s| {
                    let (_, p50, p95, _) = s.calculate_percentiles();
                    (p50, p95)
                })
                .unwrap_or((0.0, 0.0))
        };

        let is_quantized = !matches!(
            coll_config.quantization,
            ferres_db_core::QuantizationConfig::None
        );
        let params = ferres_db_core::CostEstimateParams {
            collection_size: coll_len,
            dimension: coll_config.dimension,
            limit: payload.limit,
            ef_search: coll_config.hnsw.ef_search,
            has_filter,
            filter_conditions_count,
            historical_p50: p50,
            historical_p95: p95,
            is_quantized,
        };

        let estimate = ferres_db_core::estimate_search_cost(&params);

        if estimate.estimated_ms > budget as f64 {
            let estimate_json =
                serde_json::to_value(&estimate).unwrap_or_else(|_| serde_json::json!({}));
            return Err(ApiError::budget_exceeded(
                format!(
                    "estimated cost ({:.1}ms) exceeds budget ({}ms)",
                    estimate.estimated_ms, budget
                ),
                estimate_json,
            ));
        }
    }

    // Filter parsing and reranker check also outside the lock
    let mut filter = match &payload.filter {
        Some(fv) => MetadataFilter::from_json(fv.clone())
            .map_err(|e| ApiError::invalid_payload(e.to_string()))?,
        None => MetadataFilter::empty(),
    };
    if let Some(ns) = &payload.namespace {
        filter.namespace = Some(ns.clone());
    }
    let vector_field = payload.vector_field.clone();
    let use_rerank = payload.rerank == Some(true) && reranker_dim_match;

    let validation_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let search_start = Instant::now();

    let vector = payload.vector.clone();
    let limit = payload.limit;
    let reranker = app_state.reranker.clone();

    // Acquire semaphore permit to limit concurrent CPU-intensive searches.
    // This prevents thread pool starvation under high load.
    let _permit = app_state
        .search_semaphore
        .acquire()
        .await
        .map_err(|e| ApiError::internal_error(format!("semaphore acquire: {}", e)))?;

    // Detach from the parent tracing span before entering spawn_blocking.
    // Under high concurrency the #[instrument] span can be closed by the
    // subscriber while the blocking thread still holds a reference, causing
    // "tried to clone a span that already closed" panics in the sharded
    // registry.  Using Span::none() makes the blocking closure span-free,
    // which also reduces stack depth (fewer tracing frames → less stack
    // pressure on the blocking pool).
    let (search_results, rerank_ms) = tokio::task::spawn_blocking(move || {
        let _guard = tracing::Span::none().entered();

        let collection = match arc_for_blocking.read() {
            Ok(c) => c,
            Err(e) => return Err(ApiError::internal_error(format!("read lock: {}", e))),
        };
        let vector_field_ref = vector_field.as_deref();
        let (raw_results, rerank_ms_val) = if use_rerank {
            let rerank_start = Instant::now();
            let res = if filter.is_empty() {
                collection.search_with_rerank(
                    &vector,
                    limit,
                    None,
                    vector_field_ref,
                    reranker.as_deref(),
                )
            } else {
                let predicate = |id: &str| {
                    collection
                        .get(id)
                        .map(|p| filter.matches_point(&p))
                        .unwrap_or(false)
                };
                collection.search_with_rerank(
                    &vector,
                    limit,
                    Some(&predicate),
                    vector_field_ref,
                    reranker.as_deref(),
                )
            };
            let res = res.map_err(ApiError::from)?;
            let rerank_ms = rerank_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
            (res, Some(rerank_ms))
        } else {
            let results = if filter.is_empty() {
                collection
                    .search(&vector, limit, None, vector_field_ref)
                    .map_err(ApiError::from)?
            } else {
                let predicate = |id: &str| {
                    collection
                        .get(id)
                        .map(|p| filter.matches_point(&p))
                        .unwrap_or(false)
                };
                collection
                    .search(&vector, limit, Some(&predicate), vector_field_ref)
                    .map_err(ApiError::from)?
            };
            (results, None)
        };

        let search_results: Vec<ferres_db_core::SearchResult> = raw_results
            .into_iter()
            .filter_map(|(storage_id, score)| {
                let point = collection.get(&storage_id)?;
                Some(ferres_db_core::SearchResult {
                    id: point.id.clone(),
                    score,
                    metadata: point.metadata.clone(),
                    vector: None,
                    namespace: point.namespace.clone(),
                })
            })
            .collect();

        Ok::<_, ApiError>((search_results, rerank_ms_val))
    })
    .await
    .map_err(|_| ApiError::internal_error("search task joined"))??;

    let search_ms = search_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    tracing::Span::current().record("db.duration.search_ms", search_ms);
    let hydrate_start = Instant::now();

    let results: Vec<SearchResult> = search_results
        .into_iter()
        .map(|r| SearchResult {
            id: r.id,
            score: r.score,
            metadata: r.metadata,
            namespace: r.namespace,
        })
        .collect();

    let hydrate_ms = hydrate_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let results_count = results.len();
    {
        let span = tracing::Span::current();
        span.record("db.duration.hydrate_ms", hydrate_ms);
        span.record("db.results.count", results_count);
    }
    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let collection_name = name.clone();
    // Uma única clonagem do vetor/filtro para o spawn do query_logger (evita clonar duas vezes).
    let vector_for_log = payload.vector.clone();
    let filter_for_log = payload.filter.clone();

    // Monta perfil por fases (percentual sobre total)
    let total = took_ms as f64;
    let mut phases = vec![
        QueryPhase {
            name: "validation".to_string(),
            duration_ms: validation_ms,
            percentage: if total > 0.0 {
                (validation_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
        QueryPhase {
            name: "search".to_string(),
            duration_ms: search_ms,
            percentage: if total > 0.0 {
                (search_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
        QueryPhase {
            name: "hydrate".to_string(),
            duration_ms: hydrate_ms,
            percentage: if total > 0.0 {
                (hydrate_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
    ];
    if let Some(ms) = rerank_ms {
        phases.push(QueryPhase {
            name: "rerank".to_string(),
            duration_ms: ms,
            percentage: if total > 0.0 {
                (ms as f64 / total) * 100.0
            } else {
                0.0
            },
        });
    }
    let profile = QueryProfile {
        query_id: query_id.clone(),
        total_ms: took_ms,
        phases,
    };

    // --- Record stats, profiles, audit and query log off the critical path ---
    // Prometheus metrics are lock-free atomics — safe to call inline.
    crate::metrics::QUERIES_TOTAL
        .with_label_values(&[&collection_name])
        .inc();
    crate::metrics::QUERY_DURATION_MS
        .with_label_values(&[&collection_name])
        .observe(took_ms as f64);

    // Everything else (profile eviction, stats, query log, audit) runs in a
    // single spawned task so the HTTP response is returned immediately.
    {
        let app = app_state.clone();
        let query_logger = app_state.query_logger.clone();
        let coll_name = collection_name.clone();
        let qid = query_id.clone();
        let username = user.username.clone();
        let limit_val = payload.limit;
        tokio::spawn(async move {
            // Profile eviction — only iterate when over capacity
            if app.query_profiles.len() >= QUERY_PROFILES_CAP {
                if let Some(entry) = app.query_profiles.iter().next() {
                    let k = entry.key().clone();
                    drop(entry);
                    app.query_profiles.remove(&k);
                }
            }
            app.query_profiles.insert(qid.clone(), profile);

            // Per-collection stats (non-blocking try_write inside)
            app.query_stats
                .entry(coll_name.clone())
                .or_insert_with(crate::state::QueryStats::new)
                .record_query(took_ms);

            // Global stats (non-blocking channel send inside)
            app.global_query_stats.record(&coll_name, took_ms);

            // Query log (async file write)
            query_logger.log_query(
                Some(&qid),
                &coll_name,
                &vector_for_log,
                limit_val,
                filter_for_log.as_ref(),
                results_count,
                took_ms,
            );

            // Audit trail (non-blocking channel send inside)
            let entry = audit::audit_entry(
                &username,
                "search",
                &format!("collection:{}", coll_name),
                serde_json::json!({"query_id": &qid, "limit": limit_val, "results_count": results_count}),
                AuditResult::Success,
                None,
                Some(took_ms),
            );
            app.audit_logger.log(&entry);
        });
    }

    Ok(Json(SearchPointsResponse {
        results,
        took_ms,
        query_id: Some(query_id),
        rerank_ms,
    }))
}

/// Handler para POST /api/v1/collections/{name}/search/hybrid
///
/// Busca híbrida: combina resultados vetoriais e BM25 (keyword) via fusão configurável.
/// Requer que a coleção tenha sido criada com BM25 habilitado.
/// Sub-operações instrumentadas: validate_query, hybrid_search, hydrate_results.
#[tracing::instrument(
    name = "search_hybrid",
    skip(app_state, payload),
    fields(
        db.collection = %name,
        db.operation = "hybrid_search",
        db.vector.dimension = tracing::field::Empty,
        db.vector.limit = payload.limit,
        db.hybrid.alpha = payload.alpha,
        db.hybrid.fusion = tracing::field::Empty,
        db.results.count = tracing::field::Empty,
        db.duration.search_ms = tracing::field::Empty,
        db.duration.hydrate_ms = tracing::field::Empty,
        db.index.r#type = "hnsw",
        db.index.ef_search = tracing::field::Empty,
    )
)]
pub async fn search_hybrid(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<HybridSearchPointsRequest>,
) -> ApiResult<Json<SearchPointsResponse>> {
    // Verificação de permissão granular (Read na collection)
    let perm_result = check_user_permission(&user, &name, &Action::Read);
    if !perm_result.is_allowed() {
        let entry = audit::audit_entry(
            &user.username,
            "search_hybrid",
            &format!("collection:{name}"),
            serde_json::json!({"denied": true}),
            AuditResult::Denied,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
        return Err(ApiError::forbidden(format!(
            "permission denied: read on collection '{name}'"
        )));
    }

    check_namespace_access(&user, payload.namespace.as_deref())
        .map_err(|_| ApiError::forbidden("API key does not have access to this namespace"))?;

    request_validation::validate_search_limit(payload.limit)?;
    request_validation::validate_vector_dimension(&payload.query_vector)?;
    if !(0.0..=1.0).contains(&payload.alpha) {
        return Err(ApiError::invalid_payload("alpha must be between 0 and 1"));
    }

    // Parse fusion strategy
    let fusion_strategy = match payload.fusion.as_deref() {
        None | Some("weighted") => ferres_db_core::FusionStrategy::WeightedScore {
            alpha: payload.alpha,
        },
        Some("rrf") => {
            let k = payload.rrf_k.unwrap_or(ferres_db_core::DEFAULT_RRF_K);
            if k == 0 {
                return Err(ApiError::invalid_payload("rrf_k must be greater than 0"));
            }
            ferres_db_core::FusionStrategy::RRF { k }
        }
        Some(other) => {
            return Err(ApiError::invalid_payload(format!(
                "unknown fusion strategy '{other}': must be 'weighted' or 'rrf'"
            )));
        }
    };
    let fusion_label = match &fusion_strategy {
        ferres_db_core::FusionStrategy::WeightedScore { .. } => "weighted",
        ferres_db_core::FusionStrategy::RRF { .. } => "rrf",
    };
    tracing::Span::current().record("db.hybrid.fusion", fusion_label);

    let query_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    // Quick validation on async thread (read lock held briefly)
    let (coll_dim, coll_ef_search) = {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        collection
            .validate_dimension(&payload.query_vector)
            .map_err(ApiError::from)?;
        (
            collection.config().dimension,
            collection.config().hnsw.ef_search,
        )
    };

    {
        let span = tracing::Span::current();
        span.record("db.vector.dimension", coll_dim);
        span.record("db.index.ef_search", coll_ef_search);
    }

    let validation_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let search_start = Instant::now();

    let query_vector = payload.query_vector.clone();
    let query_text = payload.query_text.clone();
    let hybrid_limit = payload.limit;
    let ns_filter = payload.namespace.clone();
    let arc_for_blocking = collection_arc;

    let (results, search_ms, hydrate_ms) = tokio::task::spawn_blocking(move || {
        let _guard = tracing::Span::none().entered();

        let collection = match arc_for_blocking.read() {
            Ok(c) => c,
            Err(e) => return Err(ApiError::internal_error(format!("read lock: {}", e))),
        };

        let hybrid_results = collection
            .hybrid_search(&query_vector, &query_text, hybrid_limit, &fusion_strategy)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("BM25") || msg.contains("hybrid search") {
                    ApiError::invalid_payload(msg)
                } else {
                    ApiError::from(e)
                }
            })?;

        let search_ms = search_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let hydrate_start = Instant::now();

        let results: Vec<SearchResult> = hybrid_results
            .into_iter()
            .filter_map(|(storage_id, score)| {
                let point = collection.get(&storage_id)?;
                if let Some(ref ns) = ns_filter {
                    if point.namespace.as_deref() != Some(ns.as_str()) {
                        return None;
                    }
                }
                Some(SearchResult {
                    id: point.id.clone(),
                    score,
                    metadata: point.metadata.clone(),
                    namespace: point.namespace.clone(),
                })
            })
            .collect();

        let hydrate_ms = hydrate_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        Ok::<_, ApiError>((results, search_ms, hydrate_ms))
    })
    .await
    .map_err(|_| ApiError::internal_error("hybrid search task joined"))??;

    tracing::Span::current().record("db.duration.search_ms", search_ms);
    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let collection_name = name.clone();
    let results_count = results.len();
    {
        let span = tracing::Span::current();
        span.record("db.duration.hydrate_ms", hydrate_ms);
        span.record("db.results.count", results_count);
    }

    let total = took_ms as f64;
    let phases = vec![
        QueryPhase {
            name: "validation".to_string(),
            duration_ms: validation_ms,
            percentage: if total > 0.0 {
                (validation_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
        QueryPhase {
            name: "search".to_string(),
            duration_ms: search_ms,
            percentage: if total > 0.0 {
                (search_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
        QueryPhase {
            name: "hydrate".to_string(),
            duration_ms: hydrate_ms,
            percentage: if total > 0.0 {
                (hydrate_ms as f64 / total) * 100.0
            } else {
                0.0
            },
        },
    ];
    let profile = QueryProfile {
        query_id: query_id.clone(),
        total_ms: took_ms,
        phases,
    };

    // --- Record stats, profiles, audit and query log off the critical path ---
    crate::metrics::QUERIES_TOTAL
        .with_label_values(&[&collection_name])
        .inc();
    crate::metrics::QUERY_DURATION_MS
        .with_label_values(&[&collection_name])
        .observe(took_ms as f64);

    {
        let app = app_state.clone();
        let query_logger = app_state.query_logger.clone();
        let coll_name = collection_name.clone();
        let qid = query_id.clone();
        let username = user.username.clone();
        let vector_for_log = payload.query_vector.clone();
        let limit_val = payload.limit;
        tokio::spawn(async move {
            if app.query_profiles.len() >= QUERY_PROFILES_CAP {
                if let Some(entry) = app.query_profiles.iter().next() {
                    let k = entry.key().clone();
                    drop(entry);
                    app.query_profiles.remove(&k);
                }
            }
            app.query_profiles.insert(qid.clone(), profile);

            app.query_stats
                .entry(coll_name.clone())
                .or_insert_with(crate::state::QueryStats::new)
                .record_query(took_ms);

            app.global_query_stats.record(&coll_name, took_ms);

            query_logger.log_query(
                Some(&qid),
                &coll_name,
                &vector_for_log,
                limit_val,
                None,
                results_count,
                took_ms,
            );

            let entry = audit::audit_entry(
                &username,
                "search_hybrid",
                &format!("collection:{}", coll_name),
                serde_json::json!({"query_id": &qid, "results_count": results_count}),
                AuditResult::Success,
                None,
                Some(took_ms),
            );
            app.audit_logger.log(&entry);
        });
    }

    Ok(Json(SearchPointsResponse {
        results,
        took_ms,
        query_id: Some(query_id),
        rerank_ms: None,
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

    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    // Obtém todos os pontos
    let mut all_points: Vec<GetPointResponse> = collection
        .points_owned()
        .iter()
        .map(|point| GetPointResponse {
            id: point.id.clone(),
            vector: point.vector.clone(),
            metadata: point.metadata.clone(),
            namespace: point.namespace.clone(),
            created_at: point.created_at,
            relations: point.relations.clone(),
        })
        .collect();

    // Aplica filtro de metadata se fornecido
    if let Some(filter_str) = &params.filter {
        if !filter_str.is_empty() {
            // Parse do JSON string para serde_json::Value
            let filter_value: serde_json::Value = serde_json::from_str(filter_str)
                .map_err(|e| ApiError::invalid_payload(format!("invalid JSON filter: {e}")))?;

            let filter = MetadataFilter::from_json(filter_value)
                .map_err(|e| ApiError::invalid_payload(format!("invalid metadata filter: {e}")))?;

            all_points.retain(|point| filter.matches(&point.metadata));
        }
    }

    let total = all_points.len();
    let has_more = offset + limit < total;

    // Aplica paginação
    let points: Vec<GetPointResponse> = all_points.into_iter().skip(offset).take(limit).collect();

    Ok(Json(ListPointsResponse {
        points,
        total,
        limit,
        offset,
        has_more,
    }))
}

/// Query params para GET /api/v1/collections/{name}/points/{id}
#[derive(Debug, Deserialize)]
pub struct GetPointQuery {
    /// Namespace do ponto (obrigatório se o ponto foi inserido com namespace).
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Handler para GET /api/v1/collections/{name}/points/{id}
///
/// Retorna um ponto específico pelo ID. Use query param `namespace` quando o ponto tiver namespace.
pub async fn get_point(
    State(app_state): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    Query(query): Query<GetPointQuery>,
) -> ApiResult<Json<GetPointResponse>> {
    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;

    let key = Point::storage_id_from_parts(query.namespace.as_deref(), &id);
    let point = collection
        .get(&key)
        .ok_or_else(|| ApiError::point_not_found(&id))?;

    Ok(Json(GetPointResponse {
        id: point.id.clone(),
        vector: point.vector.clone(),
        metadata: point.metadata.clone(),
        namespace: point.namespace.clone(),
        created_at: point.created_at,
        relations: point.relations.clone(),
    }))
}

// ─── Estimate Search Cost ──────────────────────────────────────────────────

/// Payload para estimativa de custo de busca.
#[derive(Debug, Deserialize)]
pub struct EstimateSearchRequest {
    pub limit: usize,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Se true, inclui dados históricos de latência (p50/p95/p99) no response.
    #[serde(default)]
    pub include_history: Option<bool>,
}

/// Resposta da estimativa de custo de busca.
#[derive(Debug, Serialize)]
pub struct EstimateSearchResponse {
    /// Estimativa de custo detalhada.
    #[serde(flatten)]
    pub estimate: QueryCostEstimate,
    /// Dados históricos de latência (presente se include_history=true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub historical_latency: Option<HistoricalLatency>,
}

/// Dados históricos de latência de uma coleção.
#[derive(Debug, Serialize)]
pub struct HistoricalLatency {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub avg_ms: f64,
    pub total_queries: u64,
}

/// Handler para POST /api/v1/collections/{name}/search/estimate
///
/// Estima o custo de uma busca vetorial sem executá-la. Retorna latência
/// estimada, consumo de memória, nós HNSW visitados e recomendações.
#[tracing::instrument(
    name = "estimate_search_cost",
    skip(app_state, payload),
    fields(collection = %name, limit = payload.limit)
)]
pub async fn estimate_search(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<EstimateSearchRequest>,
) -> ApiResult<Json<EstimateSearchResponse>> {
    // Verificação de permissão granular (Read)
    let perm_result = check_user_permission(&user, &name, &Action::Read);
    if !perm_result.is_allowed() {
        return Err(ApiError::forbidden(format!(
            "permission denied: read on collection '{name}'"
        )));
    }

    check_namespace_access(&user, payload.namespace.as_deref())
        .map_err(|_| ApiError::forbidden("API key does not have access to this namespace"))?;

    request_validation::validate_search_limit(payload.limit)?;

    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let (config, num_points) = {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        (collection.config().clone(), collection.len())
    };

    // Parse do filtro (inclui namespace) para contar condições
    let mut filter = match &payload.filter {
        Some(fv) => MetadataFilter::from_json(fv.clone())
            .map_err(|e| ApiError::invalid_payload(e.to_string()))?,
        None => MetadataFilter::empty(),
    };
    if let Some(ns) = &payload.namespace {
        filter.namespace = Some(ns.clone());
    }
    let has_filter = !filter.is_empty();
    let filter_conditions_count = filter.conditions().len();

    // Obtém percentis históricos do QueryStats
    let (avg, p50, p95, p99, total_queries) = {
        app_state
            .query_stats
            .get(&name)
            .map(|s| {
                let num_queries = s.num_queries.load(std::sync::atomic::Ordering::Relaxed);
                let (avg, p50, p95, p99) = s.calculate_percentiles();
                (avg, p50, p95, p99, num_queries)
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0, 0))
    };

    // Calcula a estimativa
    let is_quantized = !matches!(
        config.quantization,
        ferres_db_core::QuantizationConfig::None
    );
    let params = ferres_db_core::CostEstimateParams {
        collection_size: num_points,
        dimension: config.dimension,
        limit: payload.limit,
        ef_search: config.hnsw.ef_search,
        has_filter,
        filter_conditions_count,
        historical_p50: p50,
        historical_p95: p95,
        is_quantized,
    };

    let estimate = ferres_db_core::estimate_search_cost(&params);

    // Inclui histórico se solicitado
    let historical_latency = if payload.include_history.unwrap_or(false) {
        Some(HistoricalLatency {
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            avg_ms: avg,
            total_queries,
        })
    } else {
        None
    };

    Ok(Json(EstimateSearchResponse {
        estimate,
        historical_latency,
    }))
}

// ─── Explain Search ───────────────────────────────────────────────────────

/// Payload para busca com explicação.
#[derive(Debug, Deserialize)]
pub struct ExplainSearchRequest {
    pub vector: Vec<f32>,
    pub limit: usize,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Campo vetorial contra o qual buscar (ex.: "default", "title_vector").
    #[serde(default)]
    pub vector_field: Option<String>,
}

/// Handler para POST /api/v1/collections/{name}/search/explain
///
/// Retorna uma explicação detalhada de cada resultado da busca vetorial,
/// incluindo score breakdown, avaliação de filtros e estatísticas do índice.
///
/// Delega toda a lógica de explain ao core via [`build_search_explanation`].
#[tracing::instrument(
    name = "explain_search",
    skip(app_state, payload),
    fields(collection = %name, limit = payload.limit)
)]
pub async fn explain_search(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<ExplainSearchRequest>,
) -> ApiResult<Json<ferres_db_core::SearchExplanation>> {
    // Verificação de permissão granular (Read)
    let perm_result = check_user_permission(&user, &name, &Action::Read);
    if !perm_result.is_allowed() {
        return Err(ApiError::forbidden(format!(
            "permission denied: read on collection '{name}'"
        )));
    }

    check_namespace_access(&user, payload.namespace.as_deref())
        .map_err(|_| ApiError::forbidden("API key does not have access to this namespace"))?;

    request_validation::validate_search_limit(payload.limit)?;
    request_validation::validate_vector_dimension(&payload.vector)?;

    let start = Instant::now();

    // Parse do filtro e merge de namespace
    let mut filter = match &payload.filter {
        Some(fv) => Some(
            MetadataFilter::from_json(fv.clone())
                .map_err(|e| ApiError::invalid_payload(e.to_string()))?,
        ),
        None => Some(MetadataFilter::empty()),
    };
    if let (Some(ref mut f), Some(ref ns)) = (filter.as_mut(), &payload.namespace) {
        f.namespace = Some(ns.clone());
    }
    let filter = filter.filter(|f| !f.is_empty());

    let collection_arc = {
        let ref_guard = app_state
            .collections
            .get(&name)
            .ok_or_else(|| ApiError::collection_not_found(&name))?;
        Arc::clone(ref_guard.value())
    };

    let vector_field = payload.vector_field.as_deref();
    let explanation = {
        let collection = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        build_search_explanation(
            &collection,
            &payload.vector,
            payload.limit,
            filter,
            vector_field,
        )
        .map_err(ApiError::from)?
    };

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

    // Métricas Prometheus
    crate::metrics::QUERIES_TOTAL
        .with_label_values(&[&name])
        .inc();
    crate::metrics::QUERY_DURATION_MS
        .with_label_values(&[&name])
        .observe(took_ms as f64);

    Ok(Json(explanation))
}
