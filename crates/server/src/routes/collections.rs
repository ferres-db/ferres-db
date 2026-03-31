//! # Collection Routes — rotas de gerenciamento de coleções

use axum::{
    routing::{get, patch, post},
    Router,
};

use crate::handlers::collections::{
    create_collection, delete_collection, get_collection, get_tier_distribution, list_collections,
    patch_collection_retention,
};
use crate::state::AppState;

/// Rotas base de coleções (sem nome no path — sem rate limit por coleção).
pub fn create_base_collection_routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/collections",
        post(create_collection).get(list_collections),
    )
}

/// Rotas de coleções nomeadas (com `{name}` no path — rate limited por coleção).
pub fn create_named_collection_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collections/{name}",
            get(get_collection)
                .patch(patch_collection_retention)
                .delete(delete_collection),
        )
        .route(
            "/api/v1/collections/{name}/tiers",
            get(get_tier_distribution),
        )
}
