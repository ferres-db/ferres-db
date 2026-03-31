//! # Handlers — API Keys (list, create, delete)

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::audit::{self, AuditResult};
use crate::auth::{AuthenticatedUser, RequireEditor};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// GET /api/v1/keys — lista chaves (Editor ou Admin; Viewer não vê lista de keys).
pub async fn list_keys(
    _editor: RequireEditor,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<crate::api_keys::ApiKeyInfo>>> {
    let store = state
        .api_key_store
        .as_ref()
        .ok_or_else(|| ApiError::api_key_store_unavailable("API key store not configured"))?;
    let keys = store.list_keys().map_err(ApiError::from)?;
    Ok(Json(keys))
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    /// Namespaces permitidos para esta chave. Null/omitido/empty = todos.
    #[serde(default)]
    pub allowed_namespaces: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateKeyNamespacesRequest {
    /// Namespaces permitidos. Null/empty = todos.
    pub allowed_namespaces: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
pub struct CreateKeyResponse {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub key_prefix: String,
    pub created_at: i64,
}

/// POST /api/v1/keys — cria uma nova chave (Editor ou Admin).
pub async fn create_key(
    _editor: RequireEditor,
    AuthenticatedUser(user): AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreateKeyResponse>> {
    let store = state
        .api_key_store
        .as_ref()
        .ok_or_else(|| ApiError::api_key_store_unavailable("API key store not configured"))?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::invalid_payload("name is required"));
    }

    let allowed = body.allowed_namespaces.filter(|v| !v.is_empty());
    let (raw_key, id, key_prefix, created_at) =
        store.create_key(name, allowed).map_err(ApiError::from)?;

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "create_api_key",
            &format!("api_key:{name}"),
            serde_json::json!({"key_prefix": &key_prefix}),
            AuditResult::Success,
            None,
            None,
        );
        state.audit_logger.log(&entry);
    }

    Ok(Json(CreateKeyResponse {
        id,
        name: name.to_string(),
        key: raw_key,
        key_prefix,
        created_at,
    }))
}

/// PUT /api/v1/keys/:id — atualiza os namespaces permitidos (Editor ou Admin).
pub async fn update_key_namespaces(
    _editor: RequireEditor,
    AuthenticatedUser(user): AuthenticatedUser,
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<UpdateKeyNamespacesRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .api_key_store
        .as_ref()
        .ok_or_else(|| ApiError::api_key_store_unavailable("API key store not configured"))?;

    let allowed = body.allowed_namespaces.filter(|v| !v.is_empty());
    let allowed_clone = allowed.clone();
    store
        .update_key_namespaces(id, allowed)
        .map_err(ApiError::from)?;

    let entry = audit::audit_entry(
        &user.username,
        "update_api_key_namespaces",
        &format!("api_key:{id}"),
        serde_json::json!({ "allowed_namespaces": allowed_clone }),
        AuditResult::Success,
        None,
        None,
    );
    state.audit_logger.log(&entry);

    Ok(Json(serde_json::json!({ "updated": true, "id": id })))
}

/// DELETE /api/v1/keys/:id — remove uma chave (Editor ou Admin).
pub async fn delete_key(
    _editor: RequireEditor,
    AuthenticatedUser(user): AuthenticatedUser,
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .api_key_store
        .as_ref()
        .ok_or_else(|| ApiError::api_key_store_unavailable("API key store not configured"))?;

    store.delete_key(id).map_err(ApiError::from)?;

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "delete_api_key",
            &format!("api_key:{id}"),
            serde_json::json!({}),
            AuditResult::Success,
            None,
            None,
        );
        state.audit_logger.log(&entry);
    }

    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}
