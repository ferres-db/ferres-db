//! # Points Routes — rotas de gerenciamento de pontos

use axum::{
    routing::{get, post},
    Router,
};

use crate::handlers::points::{
    delete_points, estimate_search, explain_search, get_point, list_points, search_hybrid,
    search_points, upsert_points,
};
use crate::state::AppState;

/// Cria as rotas de gerenciamento de pontos.
pub fn create_points_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collections/{name}/points",
            get(list_points).post(upsert_points).delete(delete_points),
        )
        .route(
            "/api/v1/collections/{name}/search",
            post(search_points),
        )
        .route(
            "/api/v1/collections/{name}/search/hybrid",
            post(search_hybrid),
        )
        .route(
            "/api/v1/collections/{name}/search/explain",
            post(explain_search),
        )
        .route(
            "/api/v1/collections/{name}/search/estimate",
            post(estimate_search),
        )
        .route(
            "/api/v1/collections/{name}/points/{id}",
            get(get_point),
        )
}

