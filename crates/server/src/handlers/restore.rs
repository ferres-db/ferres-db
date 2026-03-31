//! # Restore Handler — Point-in-Time Recovery (PITR)
//!
//! POST /api/v1/admin/restore: restaura uma ou todas as coleções ao estado num timestamp.
//! GET /api/v1/admin/restore/points: lista pontos de restauração (snapshot + WAL) por coleção.

use axum::response::IntoResponse;
use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;

use crate::audit::{self, AuditResult};
use crate::auth::{AuthenticatedUser, RequireAdmin};
use crate::error::ApiError;
use crate::state::AppState;

/// Body de POST /api/v1/admin/restore
#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    /// Timestamp Unix (segundos) para o qual restaurar.
    pub timestamp: u64,
    /// Se presente, restaura apenas esta coleção; caso contrário, todas que tiverem WAL/snapshot.
    pub collection: Option<String>,
}

/// Resposta de POST /api/v1/admin/restore
#[derive(Serialize)]
pub struct RestoreResponse {
    pub ok: bool,
    pub restored: Vec<String>,
    pub errors: Vec<String>,
}

/// Resposta de GET /api/v1/admin/restore/points
#[derive(Serialize)]
pub struct RestorePointsResponse {
    pub collections: std::collections::HashMap<String, CollectionRestorePoints>,
}

#[derive(Serialize)]
pub struct CollectionRestorePoints {
    pub last_snapshot_timestamp: u64,
    pub wal_timestamps: Vec<u64>,
}

/// POST /api/v1/admin/restore — restaura ao estado no timestamp dado.
pub async fn restore_to_timestamp(
    _admin: RequireAdmin,
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    axum::Json(body): axum::Json<RestoreRequest>,
) -> axum::response::Response {
    let names: Vec<String> = if let Some(ref name) = body.collection {
        if app_state.collections.contains_key(name) {
            vec![name.clone()]
        } else {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "collection_not_found",
                    "message": format!("Collection '{}' not found", name),
                })),
            )
                .into_response();
        }
    } else {
        app_state
            .collections
            .iter()
            .map(|e| e.key().clone())
            .collect()
    };

    let mut restored = Vec::new();
    let mut errors = Vec::new();

    for name in names {
        match app_state.restore_collection_to_timestamp(&name, body.timestamp) {
            Ok(()) => {
                restored.push(name.clone());
                info!(collection = %name, timestamp = body.timestamp, "PITR restore ok");
            }
            Err(e) => {
                errors.push(format!("{}: {}", name, e));
            }
        }
    }

    if restored.is_empty() && !errors.is_empty() {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "restore_failed",
                "message": errors.join("; "),
                "restored": restored,
                "errors": errors,
            })),
        )
            .into_response();
    }

    {
        let entry = audit::audit_entry(
            &user.username,
            "restore_to_timestamp",
            "system:restore",
            json!({
                "timestamp": body.timestamp,
                "collection": body.collection,
                "restored": restored,
                "errors": errors,
            }),
            if errors.is_empty() {
                AuditResult::Success
            } else {
                AuditResult::Partial
            },
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

    (
        axum::http::StatusCode::OK,
        Json(
            serde_json::to_value(RestoreResponse {
                ok: errors.is_empty(),
                restored,
                errors,
            })
            .unwrap_or(json!({ "ok": true, "restored": [], "errors": [] })),
        ),
    )
        .into_response()
}

/// GET /api/v1/admin/restore/points — lista pontos de restauração por coleção.
pub async fn get_restore_points(
    _admin: RequireAdmin,
    State(app_state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let collection = params.get("collection").map(String::as_str);
    match app_state.list_restore_points(collection) {
        Ok(map) => {
            let collections: std::collections::HashMap<String, CollectionRestorePoints> = map
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        CollectionRestorePoints {
                            last_snapshot_timestamp: v.last_snapshot_timestamp,
                            wal_timestamps: v.wal_timestamps,
                        },
                    )
                })
                .collect();
            (
                axum::http::StatusCode::OK,
                Json(
                    serde_json::to_value(RestorePointsResponse { collections })
                        .unwrap_or(json!({ "collections": {} })),
                ),
            )
                .into_response()
        }
        Err(e) => ApiError::internal_error(e.to_string()).into_response(),
    }
}
