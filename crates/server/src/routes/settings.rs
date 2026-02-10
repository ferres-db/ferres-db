//! # Settings routes — GET/PUT /api/v1/admin/settings/cloud, POST /api/v1/admin/settings/test-s3

use axum::{routing::get, routing::post, Router};

use crate::handlers::settings::{get_cloud_settings, put_cloud_settings, test_s3};
use crate::state::AppState;

pub fn create_settings_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/settings/cloud", get(get_cloud_settings).put(put_cloud_settings))
        .route("/api/v1/admin/settings/test-s3", post(test_s3))
}
