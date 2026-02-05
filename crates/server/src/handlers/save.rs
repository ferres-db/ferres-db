//! # Save Handler — persiste coleções no disco
//!
//! Endpoint para forçar gravação das coleções (útil em testes e antes de backup).

use axum::{extract::State, response::Json};
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Handler para POST /api/v1/save
///
/// Persiste todas as coleções no disco. Útil antes de reiniciar o servidor
/// ou em testes e2e que validam persistência após restart.
pub async fn save_collections(State(app_state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    app_state.save_all_collections().map_err(|e| {
        ApiError::internal_error(format!("failed to save collections: {}", e))
    })?;
    Ok(Json(json!({ "ok": true })))
}
