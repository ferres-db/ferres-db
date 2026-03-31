//! # Handlers — audit trail (consulta de entradas de auditoria). Apenas Admin.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::audit::AuditEntry;
use crate::auth::RequireAdmin;
use crate::error::ApiResult;
use crate::state::AppState;

/// Query params para GET /api/v1/audit.
#[derive(Debug, Deserialize)]
pub struct AuditQueryParams {
    /// Filtrar por user_id.
    pub user: Option<String>,
    /// Filtrar por action (ex: "search", "upsert", "delete").
    pub action: Option<String>,
    /// Filtrar por resource (substring match, ex: "collection:docs").
    pub resource: Option<String>,
    /// Data/hora de início (RFC 3339, ex: "2026-02-01T00:00:00Z").
    pub from: Option<String>,
    /// Data/hora de fim (RFC 3339, ex: "2026-02-08T00:00:00Z").
    pub to: Option<String>,
    /// Número máximo de entradas (default: 100, max: 1000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// GET /api/v1/audit — consulta entradas de auditoria filtradas (apenas Admin).
pub async fn get_audit(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Query(params): Query<AuditQueryParams>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = params.limit.min(1000);

    let from = params
        .from
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let to = params
        .to
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let entries = state
        .audit_logger
        .query(
            params.user.as_deref(),
            params.action.as_deref(),
            params.resource.as_deref(),
            from,
            to,
            limit,
        )
        .await;

    Ok(Json(entries))
}
