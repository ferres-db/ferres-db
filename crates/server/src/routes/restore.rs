//! # Restore Routes — PITR (Point-in-Time Recovery)
//!
//! POST /api/v1/admin/restore — restaura ao timestamp
//! GET /api/v1/admin/restore/points — lista pontos de restauração

use axum::{routing::get, routing::post, Router};

use crate::handlers::restore::{get_restore_points, restore_to_timestamp};
use crate::state::AppState;

pub fn create_restore_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/restore", post(restore_to_timestamp))
        .route("/api/v1/admin/restore/points", get(get_restore_points))
}
