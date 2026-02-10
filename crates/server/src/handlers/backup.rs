//! # Backup Handler — snapshot binário e upload para S3
//!
//! POST /api/v1/admin/backup: persiste todas as coleções, gera um archive tar.gz
//! do diretório de storage e faz upload para o bucket S3 configurado.

use std::io::Write;
use std::path::Path;

use axum::response::IntoResponse;
use axum::{extract::State, response::Json};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use tracing::info;

use crate::auth::{AuthenticatedUser, RequireAdmin};
use crate::audit::{self, AuditResult};
use crate::error::ApiError;
use crate::state::AppState;

/// Resposta de POST /api/v1/admin/backup
#[derive(serde::Serialize)]
pub struct BackupResponse {
    pub ok: bool,
    pub key: String,
    pub bucket: String,
    pub size_bytes: u64,
    pub region: Option<String>,
}

fn add_dir_to_tar(
    ar: &mut tar::Builder<&mut Vec<u8>>,
    src_path: &Path,
    prefix: &Path,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(src_path)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "logs" {
            continue;
        }
        if path.is_dir() {
            add_dir_to_tar(ar, &path, prefix)?;
            // Tar expects entries for directories when we add files inside; we add files with path so no need to add dir entries
        } else if path.is_file() {
            let rel = path.strip_prefix(prefix).unwrap_or(&path);
            ar.append_path_with_name(&path, rel)?;
        }
    }
    Ok(())
}

/// Cria um archive tar.gz do diretório `src_path` (apenas ficheiros; ignora diretório `logs`).
fn create_tar_gz(src_path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut tar_buf = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut tar_buf);
        if src_path.is_dir() {
            add_dir_to_tar(&mut ar, src_path, src_path)?;
        }
        ar.finish()?;
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_buf)?;
    gz.finish()
}

/// Resolve S3 settings: cloud_settings_store (SQLite) first, then config (env/toml).
fn resolve_s3_settings(app_state: &AppState) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let store = app_state.cloud_settings_store.as_ref();
    let from_store = store.and_then(|s| s.get_with_secret().ok());
    let config = &app_state.config;
    let region = from_store
        .as_ref()
        .and_then(|s| s.region.as_deref())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| config.s3_region.clone());
    let bucket = from_store
        .as_ref()
        .and_then(|s| s.bucket.as_deref())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| config.s3_bucket.clone());
    let access_key_id = from_store
        .as_ref()
        .and_then(|s| s.access_key_id.as_deref())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| config.s3_access_key_id.clone());
    let secret_access_key = from_store
        .as_ref()
        .and_then(|s| s.secret_access_key.as_deref())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| config.s3_secret_access_key.clone());
    (region, bucket, access_key_id, secret_access_key)
}

/// Handler for POST /api/v1/admin/backup
///
/// Requires Admin role. Saves all collections, builds a tar.gz snapshot of storage
/// and uploads to the configured S3 bucket (region, bucket, credentials from Settings or config).
pub async fn backup_to_s3(
    _admin: RequireAdmin,
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
) -> axum::response::Response {
    let (region_opt, bucket_opt, access_key_id, secret_access_key) = resolve_s3_settings(&app_state);
    let region = match region_opt.as_deref() {
        Some(r) if !r.is_empty() => r,
        _ => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "s3_not_configured",
                    "message": "S3 backup is not configured. Set region and bucket in Settings or in config.toml / FERRESDB_S3_REGION, FERRESDB_S3_BUCKET.",
                    "code": 503
                })),
            )
                .into_response();
        }
    };
    let bucket = match bucket_opt.as_deref() {
        Some(b) if !b.is_empty() => b,
        _ => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "s3_not_configured",
                    "message": "S3 bucket not configured. Set bucket in Settings or config.toml / FERRESDB_S3_BUCKET.",
                    "code": 503
                })),
            )
                .into_response();
        }
    };

    if let Err(e) = app_state.save_all_collections() {
        return ApiError::internal_error(format!("failed to save collections before backup: {e}"))
            .into_response();
    }

    let storage_path = app_state.config.storage_path.clone();
    let tar_gz_bytes = match tokio::task::spawn_blocking(move || create_tar_gz(&storage_path)).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            return ApiError::internal_error(format!("failed to create backup archive: {e}"))
                .into_response();
        }
        Err(e) => {
            return ApiError::internal_error(format!("backup archive task failed: {e}"))
                .into_response();
        }
    };

    let key = format!(
        "backups/ferresdb-{}.tar.gz",
        Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
    );
    let size_bytes = tar_gz_bytes.len() as u64;

    let s3_config = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()));
    let s3_config = if let (Some(ak), Some(sk)) = (&access_key_id, &secret_access_key) {
        s3_config.credentials_provider(Credentials::new(
            ak.as_str(),
            sk.as_str(),
            None,
            None,
            "ferresdb-backup",
        ))
    } else {
        s3_config
    };
    let s3_config = s3_config.load().await;
    let client = Client::new(&s3_config);

    let body = ByteStream::from(tar_gz_bytes);
    if let Err(e) = client
        .put_object()
        .bucket(bucket)
        .key(&key)
        .body(body)
        .content_type("application/gzip")
        .send()
        .await
    {
        return (
            axum::http::StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "s3_upload_failed",
                "message": format!("S3 upload failed: {e}"),
                "code": 502
            })),
        )
            .into_response();
    }

    info!(
        bucket = %bucket,
        key = %key,
        size_bytes = size_bytes,
        "backup uploaded to S3"
    );

    {
        let entry = audit::audit_entry(
            &user.username,
            "backup_to_s3",
            "system:backup",
            json!({ "bucket": bucket, "key": key, "size_bytes": size_bytes }),
            AuditResult::Success,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

    let response = BackupResponse {
        ok: true,
        key: key.clone(),
        bucket: bucket.to_string(),
        size_bytes,
        region: Some(region.to_string()),
    };
    (
        axum::http::StatusCode::OK,
        Json(serde_json::to_value(response).unwrap_or(json!({ "ok": true, "key": key, "bucket": bucket, "size_bytes": size_bytes }))),
    )
        .into_response()
}
