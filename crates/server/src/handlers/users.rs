//! # Handlers — usuários do dashboard (listar, criar, remover, alterar senha, permissões). Apenas Admin.

use axum::extract::{State, Path};
use axum::Json;
use serde::Deserialize;

use crate::auth::{AuthenticatedUser, RequireAdmin};
use crate::audit::{self, AuditResult};
use crate::error::{ApiError, ApiResult};
use crate::permissions::Permission;
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
    /// Permissões granulares (RBAC). Opcional.
    #[serde(default)]
    pub permissions: Option<Vec<Permission>>,
}

/// POST /api/v1/users — cria um novo usuário com permissões opcionais (apenas Admin).
pub async fn create_user(
    _admin: RequireAdmin,
    AuthenticatedUser(admin_user): AuthenticatedUser,
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
        .create_with_permissions(username, &body.password, role, body.permissions)
        .map_err(ApiError::from)?;

    // Audit trail
    let audit_logger = state.audit_logger.clone();
    let admin_name = admin_user.username.clone();
    let created_name = username.to_string();
    tokio::spawn(async move {
        let entry = audit::audit_entry(
            &admin_name, "create_user", &format!("user:{}", created_name),
            serde_json::json!({"role": role.map(|r| r.as_str())}),
            AuditResult::Success, None, None,
        );
        audit_logger.log(&entry).await;
    });

    Ok(Json(user))
}

/// DELETE /api/v1/users/:id — remove um usuário (apenas Admin).
pub async fn delete_user(
    _admin: RequireAdmin,
    AuthenticatedUser(admin_user): AuthenticatedUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;
    store.delete_by_id(id).map_err(ApiError::from)?;

    // Audit trail
    let audit_logger = state.audit_logger.clone();
    let admin_name = admin_user.username.clone();
    tokio::spawn(async move {
        let entry = audit::audit_entry(
            &admin_name, "delete_user", &format!("user:id={}", id),
            serde_json::json!({}),
            AuditResult::Success, None, None,
        );
        audit_logger.log(&entry).await;
    });

    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct UpdatePasswordRequest {
    pub password: String,
}

/// PUT /api/v1/users/:username/password — altera senha de um usuário (apenas Admin).
pub async fn update_user_password(
    _admin: RequireAdmin,
    AuthenticatedUser(admin_user): AuthenticatedUser,
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

    // Audit trail
    let audit_logger = state.audit_logger.clone();
    let admin_name = admin_user.username.clone();
    let target_user = username.trim().to_string();
    tokio::spawn(async move {
        let entry = audit::audit_entry(
            &admin_name, "update_password", &format!("user:{}", target_user),
            serde_json::json!({}),
            AuditResult::Success, None, None,
        );
        audit_logger.log(&entry).await;
    });

    Ok(Json(serde_json::json!({ "updated": true })))
}

/// Request para atualizar permissões de um usuário.
#[derive(Debug, Deserialize)]
pub struct UpdatePermissionsRequest {
    pub permissions: Option<Vec<Permission>>,
}

/// PUT /api/v1/users/:username/permissions — atualiza permissões granulares (apenas Admin).
pub async fn update_user_permissions(
    _admin: RequireAdmin,
    AuthenticatedUser(admin_user): AuthenticatedUser,
    State(state): State<AppState>,
    Path(username): Path<String>,
    Json(body): Json<UpdatePermissionsRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;
    store
        .update_permissions(username.trim(), body.permissions.clone())
        .map_err(ApiError::from)?;

    // Audit trail
    let audit_logger = state.audit_logger.clone();
    let admin_name = admin_user.username.clone();
    let target_user = username.trim().to_string();
    let perms_count = body.permissions.as_ref().map(|p| p.len()).unwrap_or(0);
    tokio::spawn(async move {
        let entry = audit::audit_entry(
            &admin_name, "update_permissions", &format!("user:{}", target_user),
            serde_json::json!({"permissions_count": perms_count}),
            AuditResult::Success, None, None,
        );
        audit_logger.log(&entry).await;
    });

    Ok(Json(serde_json::json!({ "updated": true, "username": username.trim() })))
}
