//! # Settings routes — GET/PUT /api/v1/admin/settings/cloud

use axum::{routing::get, Router};

use crate::handlers::settings::{get_cloud_settings, put_cloud_settings};
use crate::state::AppState;

pub fn create_settings_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/settings/cloud", get(get_cloud_settings).put(put_cloud_settings))
}
