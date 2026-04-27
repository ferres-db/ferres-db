//! # Handlers — login do dashboard (usuário/senha → JWT)

use axum::extract::State;
use axum::Json;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::Deserialize;

use crate::audit::{self, AuditResult};
use crate::auth::{get_jwt_secret, JwtClaims};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::time::unix_now;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(serde::Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub role: String,
}

/// POST /api/v1/auth/login — valida usuário/senha e retorna JWT (rota pública).
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let store = state
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::internal_error("User store not configured"))?;

    let username = body.username.trim();
    let password = body.password.as_str();
    if username.is_empty() {
        return Err(ApiError::invalid_payload("username is required"));
    }

    let valid = store.validate(username, password).map_err(ApiError::from)?;
    if !valid {
        // Audit: failed login attempt
        let entry = audit::audit_entry(
            username,
            "login",
            &format!("user:{username}"),
            serde_json::json!({"reason": "invalid credentials"}),
            AuditResult::Denied,
            None,
            None,
        );
        state.audit_logger.log(&entry);
        return Err(ApiError::invalid_payload("Invalid username or password"));
    }

    let role = store
        .get_role(username)
        .map_err(ApiError::from)?
        .unwrap_or(crate::users::Role::Viewer);

    let secret = get_jwt_secret().ok_or_else(|| ApiError::internal_error("JWT not configured"))?;
    let now = unix_now() as i64;
    let exp = now + 24 * 3600; // 24h
    let claims = JwtClaims {
        sub: username.to_string(),
        role: role.as_str().to_string(),
        exp,
        iat: now,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|_| ApiError::internal_error("Failed to create token"))?;

    // Audit: successful login
    {
        let entry = audit::audit_entry(
            username,
            "login",
            &format!("user:{username}"),
            serde_json::json!({"role": role.as_str()}),
            AuditResult::Success,
            None,
            None,
        );
        state.audit_logger.log(&entry);
    }

    Ok(Json(LoginResponse {
        token,
        username: username.to_string(),
        role: role.as_str().to_string(),
    }))
}
