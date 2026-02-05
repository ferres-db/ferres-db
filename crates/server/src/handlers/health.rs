//! # Health Handlers — handlers para health check

use axum::{response::Json, extract::State};
use serde_json::json;

use crate::state::AppState;

/// Handler para GET /health
///
/// Retorna "OK" indicando que o servidor está funcionando.
pub async fn health_check(
    State(_app_state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(json!({
        "status": "OK"
    }))
}

