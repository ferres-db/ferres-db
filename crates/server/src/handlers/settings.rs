//! # Settings handlers — GET/PUT cloud (S3) backup configuration
//!
//! GET /api/v1/admin/settings/cloud — returns region, bucket, endpoint, access_key_id (secret never returned).
//! PUT /api/v1/admin/settings/cloud — updates settings in SQLite.
//! POST /api/v1/admin/settings/test-s3 — validates S3 connection before saving.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::auth::RequireAdmin;
use crate::error::ApiError;
use crate::handlers::backup::{build_s3_client, resolve_s3_settings};
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
        Err(e) => {
            ApiError::internal_error(format!("failed to load cloud settings: {e}")).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PutCloudSettingsBody {
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub endpoint: Option<String>,
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
        endpoint: body.endpoint,
        access_key_id: body.access_key_id,
        secret_access_key: body.secret_access_key,
    };
    if let Err(e) = store.set(&settings) {
        return ApiError::internal_error(format!("failed to save cloud settings: {e}"))
            .into_response();
    }
    (
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

/// POST /api/v1/admin/settings/test-s3 — validates S3 connection (region, bucket, credentials) before saving.
pub async fn test_s3(
    _admin: RequireAdmin,
    State(app_state): State<AppState>,
) -> axum::response::Response {
    let (region_opt, bucket_opt, endpoint_opt, access_key_id, secret_access_key) =
        resolve_s3_settings(&app_state);
    let region = match region_opt.as_deref() {
        Some(r) if !r.is_empty() => r,
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "message": "S3 is not configured. Set region and bucket in Settings first."
                })),
            )
                .into_response();
        }
    };
    let bucket = match bucket_opt.as_deref() {
        Some(b) if !b.is_empty() => b,
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "message": "S3 bucket not configured. Set bucket in Settings first."
                })),
            )
                .into_response();
        }
    };
    let client = build_s3_client(
        region,
        endpoint_opt.as_deref(),
        access_key_id.as_deref(),
        secret_access_key.as_deref(),
    )
    .await;
    match client.head_bucket().bucket(bucket).send().await {
        Ok(_) => (
            axum::http::StatusCode::OK,
            Json(json!({ "ok": true, "message": "S3 connection successful." })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_GATEWAY,
            Json(json!({
                "ok": false,
                "message": format!("S3 connection failed: {e}")
            })),
        )
            .into_response(),
    }
}
