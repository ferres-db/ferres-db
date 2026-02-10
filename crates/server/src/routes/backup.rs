//! # Backup Routes — POST /api/v1/admin/backup (upload snapshot to S3)

use axum::{routing::post, Router};

use crate::handlers::backup::backup_to_s3;
use crate::state::AppState;

/// Rotas de backup (admin only; requer API key + role Admin).
pub fn create_backup_routes() -> Router<AppState> {
    Router::new().route("/api/v1/admin/backup", post(backup_to_s3))
}
