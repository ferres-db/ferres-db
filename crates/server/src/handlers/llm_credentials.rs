//! # LLM Credentials handlers (Admin)
//!
//! - `GET /api/v1/admin/llm-credentials` — lista status (configured/source) dos 3 providers.
//!   Nunca retorna a chave em si.
//! - `PUT /api/v1/admin/llm-credentials/{provider}` — define a chave. Body: `{"api_key": "..."}`.
//! - `DELETE /api/v1/admin/llm-credentials/{provider}` — remove a chave do DB. Não afeta env var.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::audit::{self, AuditResult};
use crate::auth::{AuthenticatedUser, RequireAdmin};
use crate::error::ApiError;
use crate::llm_credentials::LlmProvider;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PutLlmCredentialBody {
    pub api_key: String,
}

/// GET /api/v1/admin/llm-credentials
pub async fn list_llm_credentials(
    _admin: RequireAdmin,
    State(app_state): State<AppState>,
) -> Response {
    let store = match &app_state.llm_credentials_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "error": "llm_credentials_not_available",
                    "message": "LLM credentials store is not configured."
                })),
            )
                .into_response();
        }
    };
    match store.list_status() {
        Ok(list) => (StatusCode::OK, Json(json!({ "providers": list }))).into_response(),
        Err(e) => {
            ApiError::internal_error(format!("failed to list LLM credentials: {e}")).into_response()
        }
    }
}

/// PUT /api/v1/admin/llm-credentials/{provider}
pub async fn put_llm_credential(
    _admin: RequireAdmin,
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(provider_str): Path<String>,
    Json(body): Json<PutLlmCredentialBody>,
) -> Response {
    let provider = match LlmProvider::parse(&provider_str) {
        Ok(p) => p,
        Err(_) => {
            return ApiError::invalid_payload(format!(
                "unknown provider '{provider_str}'; expected one of: openai, anthropic, gemini"
            ))
            .into_response();
        }
    };
    if body.api_key.trim().is_empty() {
        return ApiError::invalid_payload("api_key cannot be empty").into_response();
    }
    let store = match &app_state.llm_credentials_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "error": "llm_credentials_not_available",
                    "message": "LLM credentials store is not configured."
                })),
            )
                .into_response();
        }
    };
    if let Err(e) = store.set(provider, body.api_key.trim()) {
        return ApiError::internal_error(format!("failed to save LLM credential: {e}"))
            .into_response();
    }

    let entry = audit::audit_entry(
        &user.username,
        "llm_credentials_set",
        &format!("provider:{}", provider.as_str()),
        json!({ "provider": provider.as_str() }),
        AuditResult::Success,
        None,
        None,
    );
    app_state.audit_logger.log(&entry);

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "provider": provider.as_str() })),
    )
        .into_response()
}

/// DELETE /api/v1/admin/llm-credentials/{provider}
pub async fn delete_llm_credential(
    _admin: RequireAdmin,
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Path(provider_str): Path<String>,
) -> Response {
    let provider = match LlmProvider::parse(&provider_str) {
        Ok(p) => p,
        Err(_) => {
            return ApiError::invalid_payload(format!(
                "unknown provider '{provider_str}'; expected one of: openai, anthropic, gemini"
            ))
            .into_response();
        }
    };
    let store = match &app_state.llm_credentials_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "error": "llm_credentials_not_available",
                    "message": "LLM credentials store is not configured."
                })),
            )
                .into_response();
        }
    };
    let removed = match store.delete(provider) {
        Ok(r) => r,
        Err(e) => {
            return ApiError::internal_error(format!("failed to delete LLM credential: {e}"))
                .into_response();
        }
    };

    let entry = audit::audit_entry(
        &user.username,
        "llm_credentials_delete",
        &format!("provider:{}", provider.as_str()),
        json!({ "provider": provider.as_str(), "removed": removed }),
        if removed {
            AuditResult::Success
        } else {
            AuditResult::Partial
        },
        None,
        None,
    );
    app_state.audit_logger.log(&entry);

    (
        StatusCode::OK,
        Json(json!({ "ok": true, "removed": removed })),
    )
        .into_response()
}
