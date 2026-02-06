//! # Handlers — usuários do dashboard (listar, criar, remover, alterar senha). Apenas Admin.

use axum::extract::{State, Path};
use axum::Json;
use serde::Deserialize;

use crate::auth::RequireAdmin;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::users::{Role, UserInfo};

/// GET /api/v1/users — lista usuários (apenas Admin).
pub async fn list_users(
    _admin: RequireAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<UserInfo>>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;
    let users = store.list().map_err(ApiError::from)?;
    Ok(Json(users))
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// POST /api/v1/users — cria um novo usuário (apenas Admin).
pub async fn create_user(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(body): Json<CreateUserRequest>,
) -> ApiResult<Json<UserInfo>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;

    let username = body.username.trim();
    if username.is_empty() {
        return Err(ApiError::invalid_payload("username is required"));
    }
    if body.password.is_empty() {
        return Err(ApiError::invalid_payload("password is required"));
    }

    let role = body
        .role
        .as_deref()
        .and_then(Role::from_str)
        .or(Some(Role::Viewer));
    let user = store
        .create(username, &body.password, role)
        .map_err(ApiError::from)?;
    Ok(Json(user))
}

/// DELETE /api/v1/users/:id — remove um usuário (apenas Admin).
pub async fn delete_user(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;
    store.delete_by_id(id).map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct UpdatePasswordRequest {
    pub password: String,
}

/// PUT /api/v1/users/:username/password — altera senha de um usuário (apenas Admin).
pub async fn update_user_password(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(username): Path<String>,
    Json(body): Json<UpdatePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;
    store
        .update_password(username.trim(), &body.password)
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "updated": true })))
}
