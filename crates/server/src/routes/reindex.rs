//! # Reindex Routes — background index rebuild endpoints

use axum::{
    routing::{get, post},
    Router,
};

use crate::handlers::reindex::{get_reindex_job, list_reindex_jobs, start_reindex};
use crate::state::AppState;

/// Routes for background reindex operations on a named collection.
pub fn create_reindex_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collections/{name}/reindex",
            post(start_reindex).get(list_reindex_jobs),
        )
        .route(
            "/api/v1/collections/{name}/reindex/{job_id}",
            get(get_reindex_job),
        )
}
