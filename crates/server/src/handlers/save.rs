//! # Save Handler — persiste coleções no disco
//!
//! Endpoint para forçar gravação das coleções (útil em testes e antes de backup).

use axum::{extract::State, response::Json};
use serde_json::json;

use crate::api_err;
use crate::audit::{self, AuditResult};
use crate::auth::AuthenticatedUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// Handler para POST /api/v1/save
///
/// Persiste todas as coleções no disco. Útil antes de reiniciar o servidor
/// ou em testes e2e que validam persistência após restart.
pub async fn save_collections(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    api_err!(
        app_state.save_all_collections(),
        "failed to save collections"
    )?;

    // Audit trail
    {
        let entry = audit::audit_entry(
            &user.username,
            "save",
            "system:collections",
            json!({}),
            AuditResult::Success,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

    Ok(Json(json!({ "ok": true })))
}
