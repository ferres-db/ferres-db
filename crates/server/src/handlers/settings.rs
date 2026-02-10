//! # Settings handlers — GET/PUT cloud (S3) backup configuration
//!
//! GET /api/v1/admin/settings/cloud — returns region, bucket, access_key_id (secret never returned).
//! PUT /api/v1/admin/settings/cloud — updates settings in SQLite.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::auth::RequireAdmin;
use crate::error::ApiError;
use crate::state::AppState;

/// GET /api/v1/admin/settings/cloud
pub async fn get_cloud_settings(
    _admin: RequireAdmin,
    State(app_state): State<AppState>,
) -> axum::response::Response {
    let store = match &app_state.cloud_settings_store {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "error": "cloud_settings_not_available",
                    "message": "Cloud settings store is not configured."
                })),
            )
                .into_response();
        }
    };
    match store.get() {
        Ok(settings) => (axum::http::StatusCode::OK, Json(settings)).into_response(),
        Err(e) => ApiError::internal_error(format!("failed to load cloud settings: {e}"))
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutCloudSettingsBody {
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
}

/// PUT /api/v1/admin/settings/cloud
pub async fn put_cloud_settings(
    _admin: RequireAdmin,
    State(app_state): State<AppState>,
    Json(body): Json<PutCloudSettingsBody>,
) -> axum::response::Response {
    let store = match &app_state.cloud_settings_store {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "error": "cloud_settings_not_available",
                    "message": "Cloud settings store is not configured."
                })),
            )
                .into_response();
        }
    };
    let settings = crate::cloud_settings::CloudSettings {
        region: body.region,
        bucket: body.bucket,
        access_key_id: body.access_key_id,
        secret_access_key: body.secret_access_key,
    };
    if let Err(e) = store.set(&settings) {
        return ApiError::internal_error(format!("failed to save cloud settings: {e}"))
            .into_response();
    }
    (axum::http::StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}
