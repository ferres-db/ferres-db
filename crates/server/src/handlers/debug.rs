//! # Debug Handlers — endpoints de diagnóstico (query profile, etc.)

use axum::{
    extract::{Path, State},
    response::Json,
};

use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, QueryProfile};

/// Handler para GET /api/v1/debug/query-profile/{query_id}
///
/// Retorna o perfil de execução de uma query (tempo por fase: validação, busca, hydrate).
/// Retorna 404 se o query_id não existir (perfis são mantidos em memória, capacidade limitada).
pub async fn get_query_profile(
    State(app_state): State<AppState>,
    Path(query_id): Path<String>,
) -> ApiResult<Json<QueryProfile>> {
    let profile = app_state
        .query_profiles
        .get(&query_id)
        .map(|r| r.value().clone())
        .ok_or_else(|| ApiError::CollectionNotFound {
        message: format!("query profile '{}' not found", query_id),
    })?;
    Ok(Json(profile))
}
